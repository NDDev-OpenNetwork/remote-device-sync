//! Native directory HTTPS: identity, confidentiality boundaries and deadlines.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use ed25519_dalek::SigningKey;
use rds_discovery::{
    DiscoveryError, EndpointKey, EndpointRecord, MemoryStore, Service,
    client::Client,
    http::{self, Response},
    registry::SignedRegistry,
    service::{self, Directory, Limits, ServiceConfig},
    tls::server_config_from_pem,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

fn certificate() -> (String, String) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    (cert.pem(), signing_key.serialize_pem())
}

async fn directory(cert: &str, key: &str, limits: Limits) -> Directory {
    service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig {
            tls: Some(server_config_from_pem(cert.as_bytes(), key.as_bytes()).unwrap()),
            limits,
            ..ServiceConfig::open_ephemeral()
        },
    )
    .await
    .unwrap()
}

fn client(dir: &Directory, cert: &str) -> Client {
    Client::from_endpoint(&format!("https://localhost:{}", dir.addr().port()))
        .unwrap()
        .with_ca_pem(cert.as_bytes())
        .unwrap()
}

#[tokio::test]
async fn dns_https_preserves_the_signed_identity_chain() {
    let (cert, key) = certificate();
    let issuer = SigningKey::from_bytes(&[25; 32]);
    let device = SigningKey::from_bytes(&[26; 32]);
    let identity = EndpointKey(device.verifying_key().to_bytes());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig {
            tls: Some(server_config_from_pem(cert.as_bytes(), key.as_bytes()).unwrap()),
            registry_key: Some(issuer.verifying_key()),
            ..ServiceConfig::open_ephemeral()
        },
    )
    .await
    .unwrap();
    let client = client(&dir, &cert);
    client.health().await.unwrap();
    let record = EndpointRecord::publish(
        &device,
        1,
        vec!["127.0.0.1:4321".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        Duration::from_secs(60),
    )
    .unwrap();
    client.publish(&record).await.unwrap();
    client
        .update_registry(
            &SignedRegistry::publish(
                &issuer,
                1,
                1,
                BTreeMap::from([("device-a".into(), identity)]),
                Duration::from_secs(60),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    // A valid HTTPS certificate does not grant authority over device names.
    assert!(client.resolve_name("device-a").await.is_err());
    let wrong = client
        .clone()
        .with_registry_key(SigningKey::from_bytes(&[27; 32]).verifying_key());
    assert!(matches!(
        wrong.resolve_name("device-a").await,
        Err(DiscoveryError::BadSignature)
    ));
    let trusted = client.with_registry_key(issuer.verifying_key());
    let resolved = trusted.resolve_name("device-a").await.unwrap();
    assert_eq!(resolved, identity);
    assert_eq!(
        trusted
            .fetch(&resolved)
            .await
            .unwrap()
            .verify_fresh()
            .unwrap()
            .key,
        identity
    );
}

#[tokio::test]
async fn untrusted_wrong_host_and_expired_certificates_are_refused() {
    let (cert, key) = certificate();
    let dir = directory(&cert, &key, Limits::default()).await;
    client(&dir, &cert).health().await.unwrap();
    let public =
        Client::from_endpoint(&format!("https://localhost:{}", dir.addr().port())).unwrap();
    let wrong_host = Client::from_endpoint(&format!("https://{}", dir.addr()))
        .unwrap()
        .with_ca_pem(cert.as_bytes())
        .unwrap();
    for bad in [public, wrong_host] {
        let error = bad.health().await.unwrap_err();
        assert!(error.to_string().contains("TLS handshake"), "{error}");
    }
    // Never interpret a plaintext HTTP request on the HTTPS listener.
    assert!(Client::new(dir.addr()).health().await.is_err());

    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    params.not_before = rcgen::date_time_ymd(2000, 1, 1);
    params.not_after = rcgen::date_time_ymd(2001, 1, 1);
    let expired = params.self_signed(&key).unwrap().pem();
    let dir = directory(&expired, &key.serialize_pem(), Limits::default()).await;
    let error = client(&dir, &expired).health().await.unwrap_err();
    assert!(error.to_string().contains("TLS handshake"), "{error}");
}

#[test]
fn origins_and_trust_configuration_are_strict() {
    for origin in [
        "https://localhost",
        "http://localhost:3341/",
        "https://[::1]:3341",
        "https://127.0.0.1:3341",
        "127.0.0.1:3341",
        "[::1]:3341",
    ] {
        assert!(Client::from_endpoint(origin).is_ok(), "{origin}");
    }
    for origin in [
        "localhost:3341",
        "ftp://localhost",
        "http:localhost",
        "https:///localhost",
        "https://user@localhost",
        "https://@localhost",
        "https://localhost/a",
        "https://localhost/a/..",
        "https://localhost?token=example",
        "https://localhost#x",
        "https://localhost:0",
        "https://localhost\r\nX-Header: x",
        " https://localhost",
        "https://localhost\\example",
        "https://localhost:65536",
    ] {
        assert!(Client::from_endpoint(origin).is_err(), "{origin}");
    }
    let (cert, key) = certificate();
    assert!(server_config_from_pem(b"", key.as_bytes()).is_err());
    assert!(server_config_from_pem(cert.as_bytes(), b"invalid").is_err());
    let (_, other_key) = certificate();
    assert!(server_config_from_pem(cert.as_bytes(), other_key.as_bytes()).is_err());
    assert!(
        Client::from_endpoint("https://localhost")
            .unwrap()
            .with_ca_pem(b"")
            .is_err()
    );
    assert!(
        Client::from_endpoint("http://localhost")
            .unwrap()
            .with_ca_pem(cert.as_bytes())
            .is_err()
    );
    assert!(
        Client::from_endpoint("https://localhost")
            .unwrap()
            .addr()
            .is_none()
    );
    assert_eq!(
        Client::from_endpoint("https://[::1]:3341").unwrap().addr(),
        Some("[::1]:3341".parse().unwrap())
    );
}

#[tokio::test]
async fn host_header_is_sent_and_cannot_inject_another_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(sock.read_u8().await.unwrap());
        }
        let head = String::from_utf8(head).unwrap();
        assert!(
            head.contains(&format!("\r\nHost: localhost:{port}\r\n")),
            "{head}"
        );
        http::write_response(&mut sock, &Response::text(200, "ok"))
            .await
            .unwrap();
    });
    Client::from_endpoint(&format!("http://localhost:{port}"))
        .unwrap()
        .health()
        .await
        .unwrap();
    task.await.unwrap();
    let mut wire = Vec::new();
    assert!(
        http::write_request_with_host(&mut wire, "localhost\r\nX: bad", "GET", "/", &[])
            .await
            .is_err()
    );
    assert!(wire.is_empty());
}

#[tokio::test]
async fn stalled_tls_has_one_deadline_and_never_retries_plaintext() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::from_endpoint(&format!("https://{}", listener.local_addr().unwrap()))
        .unwrap()
        .with_request_timeout(Duration::from_millis(250));
    let task = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // TLS record type, not an HTTP method. Then deliberately stall.
        assert_eq!(sock.read_u8().await.unwrap(), 22);
        assert!(
            timeout(Duration::from_millis(750), listener.accept())
                .await
                .is_err(),
            "unexpected retry connection"
        );
    });
    let error = timeout(Duration::from_secs(2), client.health())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("request timed out"), "{error}");
    task.await.unwrap();
}

#[tokio::test]
async fn handshake_timeout_releases_capacity_and_drop_closes_pending_requests() {
    let (cert, key) = certificate();
    let dir = directory(
        &cert,
        &key,
        Limits {
            conn_timeout: Duration::from_millis(200),
            max_conns: 1,
            ..Default::default()
        },
    )
    .await;
    let mut slow = TcpStream::connect(dir.addr()).await.unwrap();
    slow.write_all(&[22, 3, 3]).await.unwrap();
    let mut byte = [0];
    let end = timeout(Duration::from_secs(2), slow.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(end, Ok(0) | Err(_)),
        "stalled handshake stayed open"
    );
    client(&dir, &cert).health().await.unwrap();

    // The directory owns accepted tasks, including HTTP requests stalled mid-head.
    let plain = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig::open_ephemeral(),
    )
    .await
    .unwrap();
    let mut pending = TcpStream::connect(plain.addr()).await.unwrap();
    pending
        .write_all(b"GET /v1/health HTTP/1.1\r\n")
        .await
        .unwrap();
    // A completed sibling exchange establishes that the accept task has run.
    Client::new(plain.addr()).health().await.unwrap();
    drop(plain);
    let end = timeout(Duration::from_secs(2), pending.read(&mut byte))
        .await
        .unwrap();
    assert!(
        matches!(end, Ok(0) | Err(_)),
        "request escaped service ownership"
    );
}
