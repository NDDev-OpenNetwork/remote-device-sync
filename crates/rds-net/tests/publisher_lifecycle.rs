//! Real HTTP retries and publisher lifecycle; no public relay or DNS service.
use rds_discovery::{
    DiscoveryError, MemoryStore, Service,
    client::Client,
    http,
    publisher::RecordIssuer,
    service::{self, ServiceConfig},
};
use rds_net::{AnnounceConfig, EndpointConfig, SecretKey, announce, bind_endpoint};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::net::{TcpListener, TcpStream};

async fn endpoint(key: &SecretKey) -> rds_net::Endpoint {
    bind_endpoint(EndpointConfig {
        secret_key: Some(key.clone()),
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap()
}
fn signing(key: &SecretKey) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&key.to_bytes())
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rds-publisher-net-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn lost_success_reply_retries_identical_signed_bytes_through_real_directory() {
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig::open_ephemeral(),
    )
    .await
    .unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(proxy.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut first = None;
        for attempt in 0..2 {
            let (mut down, _) = proxy.accept().await.unwrap();
            let request = http::read_request(&mut down).await.unwrap().unwrap();
            let mut up = TcpStream::connect(dir.addr()).await.unwrap();
            http::write_request(&mut up, &request.method, &request.path, &request.body)
                .await
                .unwrap();
            let response = http::read_response(&mut up).await.unwrap();
            assert_eq!(response.status, 200, "attempt {attempt}");
            if attempt == 0 {
                first = Some(request.body); // Commit succeeded; deliberately lose the ACK.
            } else {
                assert_eq!(first.as_ref().unwrap(), &request.body);
                http::write_response(&mut down, &response).await.unwrap();
            }
        }
    });
    let key = SecretKey::from_bytes(&[88; 32]);
    let ep = endpoint(&key).await;
    let mut task = announce(
        ep.clone(),
        AnnounceConfig {
            issuer: RecordIssuer::memory(signing(&key)),
            directory: client,
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(5), server) => result.unwrap().unwrap(),
        result = task.wait() => panic!("retry loop stopped: {result:?}"),
    }
    drop(task);
    ep.close().await;
}

#[tokio::test]
async fn missing_publisher_history_reaches_supervisor_without_network_publication() {
    let tmp = Temp::new();
    let key = SecretKey::from_bytes(&[89; 32]);
    let ep = endpoint(&key).await;
    let issuer =
        RecordIssuer::open(&tmp.0, signing(&key), rds_discovery::now_unix().unwrap()).unwrap();
    std::fs::remove_file(tmp.0.join("publisher.json")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut task = announce(
        ep.clone(),
        AnnounceConfig {
            issuer,
            directory: Client::new(listener.local_addr().unwrap()),
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), task.wait())
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        error.to_string().contains("publisher state disappeared"),
        "{error}"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    assert!(!tmp.0.join("publisher.json").exists());
    drop(task);
    ep.close().await;
}

#[tokio::test]
async fn stale_publisher_history_is_fatal_instead_of_guessing_server_revision() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        http::read_request(&mut stream).await.unwrap().unwrap();
        http::write_response(
            &mut stream,
            &http::Response::error(409, &DiscoveryError::Stale),
        )
        .await
        .unwrap();
    });
    let key = SecretKey::from_bytes(&[90; 32]);
    let ep = endpoint(&key).await;
    let mut task = announce(
        ep.clone(),
        AnnounceConfig {
            issuer: RecordIssuer::memory(signing(&key)),
            directory: client,
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), task.wait())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, DiscoveryError::Http { status: 409, .. }));
    server.await.unwrap();
    drop(task);
    ep.close().await;
}

#[tokio::test]
async fn expired_server_lease_allocates_one_durable_successor_then_retries_exactly() {
    let tmp = Temp::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut first_revision = None;
        let mut renewed = None;
        for attempt in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = http::read_request(&mut stream).await.unwrap().unwrap();
            let record: rds_discovery::EndpointRecord =
                serde_json::from_slice(&request.body).unwrap();
            let payload = record.verify_fresh().unwrap();
            match attempt {
                0 => {
                    first_revision = Some(payload.revision);
                    http::write_response(
                        &mut stream,
                        &http::Response::error(410, &DiscoveryError::Expired),
                    )
                    .await
                    .unwrap();
                }
                1 => {
                    assert_eq!(payload.revision, first_revision.unwrap() + 1);
                    renewed = Some(record); // Deliberately lose the successor's ACK.
                }
                _ => {
                    assert_eq!(renewed.as_ref().unwrap(), &record);
                    http::write_response(
                        &mut stream,
                        &http::Response::json(200, serde_json::json!({"stored":true})),
                    )
                    .await
                    .unwrap();
                }
            }
        }
        renewed.unwrap()
    });
    let key = SecretKey::from_bytes(&[104; 32]);
    let ep = endpoint(&key).await;
    let issuer =
        RecordIssuer::open(&tmp.0, signing(&key), rds_discovery::now_unix().unwrap()).unwrap();
    let mut task = announce(
        ep.clone(),
        AnnounceConfig {
            issuer,
            directory: client,
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();
    let renewed = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(8), server) => result.unwrap().unwrap(),
        result = task.wait() => panic!("renewal loop stopped: {result:?}"),
    };
    drop(task);
    ep.close().await;
    let payload = renewed.verify().unwrap();
    let mut issuer = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match RecordIssuer::open(&tmp.0, signing(&key), rds_discovery::now_unix().unwrap()) {
                Ok(issuer) => break issuer,
                Err(DiscoveryError::Busy) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("reopen failed: {error}"),
            }
        }
    })
    .await
    .unwrap();
    let draft = rds_discovery::RecordDraft {
        addrs: payload.addrs,
        relay_urls: payload.relay_urls,
        services: payload.services,
        ttl: Duration::from_secs(120),
    };
    assert_eq!(
        issuer
            .record(draft, rds_discovery::now_unix().unwrap())
            .unwrap(),
        renewed
    );
}
