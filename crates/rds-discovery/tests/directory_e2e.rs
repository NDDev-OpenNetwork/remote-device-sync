//! Directory service e2e: real TCP sockets, real HTTP codec, real
//! store — the same paths `rds-server` runs in production.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use rds_discovery::client::Client;
use rds_discovery::registry::SignedRegistry;
use rds_discovery::service::{self, Limits, ServiceConfig};
use rds_discovery::{DiscoveryError, EndpointKey, EndpointRecord, MemoryStore, now_unix};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn record_at(k: &SigningKey, revision: u64, issued_at: u64, ttl: u64) -> EndpointRecord {
    EndpointRecord::sign(
        &rds_discovery::Payload {
            version: rds_discovery::RECORD_VERSION,
            revision,
            key: EndpointKey(k.verifying_key().to_bytes()),
            addrs: vec![std::net::SocketAddr::from(([10, 0, 0, 1], 4200))],
            relay_urls: vec![],
            services: vec![rds_discovery::Service::Ping],
            issued_at,
            expires_at: issued_at + ttl,
        },
        k,
    )
    .unwrap()
}

fn record(k: &SigningKey, issued_at: u64, ttl: u64) -> EndpointRecord {
    record_at(k, 1, issued_at, ttl)
}

async fn serve() -> (service::Directory, Client) {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    (dir, client)
}

#[tokio::test]
async fn publish_fetch_roundtrip() {
    let (_dir, client) = serve().await;
    let k = key(11);
    let rec = record(&k, now_unix().unwrap(), 300);
    client.publish(&rec).await.unwrap();
    let fetched = client
        .fetch(&EndpointKey(k.verifying_key().to_bytes()))
        .await
        .unwrap();
    assert_eq!(fetched.payload, rec.payload);
    client.health().await.unwrap();
}

/// C7: `/v1/metrics` carries per-endpoint accounting under anonymized
/// writer labels — never the public key itself.
#[tokio::test]
async fn metrics_scrape_has_anonymized_per_endpoint_counts() {
    let (dir, client) = serve().await;
    let k = key(11);
    let rec = record(&k, now_unix().unwrap(), 300);
    client.publish(&rec).await.unwrap();

    // Raw HTTP GET — Client has no metrics helper.
    let mut sock = TcpStream::connect(dir.addr()).await.unwrap();
    sock.write_all(b"GET /v1/metrics HTTP/1.0\r\n\r\n")
        .await
        .unwrap();
    let mut body = String::new();
    sock.read_to_string(&mut body).await.unwrap();

    assert!(body.contains("rds_directory_puts_ok 1"), "{body}");
    assert!(body.contains("rds_directory_writers_distinct 1"), "{body}");
    let line = body
        .lines()
        .find(|l| l.starts_with("rds_directory_endpoint_puts_total"))
        .expect("per-endpoint counter missing");
    // Label is the 16-hex-char blake3 prefix — and the raw verifying
    // key (base32 or hex) must appear nowhere in the scrape.
    let label = line.split('"').nth(1).unwrap();
    assert_eq!(label.len(), 16, "writer label: {label}");
    let raw_hex: String = k
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(!body.contains(&raw_hex), "raw endpoint key in scrape");
    assert!(!body.contains("10.0.0.1"), "peer address in scrape");
}

#[tokio::test]
async fn stale_replay_and_forgery_rejected() {
    let (_dir, client) = serve().await;
    let k = key(12);
    let now = now_unix().unwrap();
    client.publish(&record_at(&k, 2, now, 300)).await.unwrap();
    // Replaying an older record must not roll the directory back.
    let err = client.publish(&record(&k, now, 300)).await.unwrap_err();
    assert!(
        matches!(err, DiscoveryError::Http { status: 409, .. }),
        "stale replay -> 409, got {err:?}"
    );
    // Forged: valid shape, wrong signer.
    let mut forged = record_at(&k, 3, now, 300);
    forged.signature[0] ^= 1;
    let err = client.publish(&forged).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Http { status: 401, .. }));
}

#[tokio::test]
async fn expired_record_refused() {
    let (_dir, client) = serve().await;
    let k = key(13);
    let past = now_unix().unwrap() - 1000;
    let err = client.publish(&record(&k, past, 60)).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Http { status: 410, .. }));
}

#[tokio::test]
async fn put_rate_limit_enforced() {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            limits: Limits {
                put_min_interval: Duration::from_secs(60),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    let k = key(14);
    let now = now_unix().unwrap();
    client.publish(&record(&k, now, 300)).await.unwrap();
    // A newer, valid record inside the interval still 429s.
    let err = client
        .publish(&record_at(&k, 2, now, 300))
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::RateLimited));
}

#[tokio::test]
async fn global_write_limit_covers_delete_and_registry() {
    // put_per_minute = 1: the first verifying write consumes the
    // window; a write on a different route hits the same bound before
    // any parse or signature work.
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            limits: Limits {
                put_per_minute: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    let k = key(16);
    client
        .publish(&record(&k, now_unix().unwrap(), 300))
        .await
        .unwrap();
    let err = client
        .remove(&rds_discovery::DeleteRequest::new(&k, 2).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, DiscoveryError::RateLimited));
}

#[tokio::test]
async fn signed_delete_removes_record() {
    let (_dir, client) = serve().await;
    let k = key(15);
    let ek = EndpointKey(k.verifying_key().to_bytes());
    client
        .publish(&record(&k, now_unix().unwrap(), 300))
        .await
        .unwrap();
    client
        .remove(&rds_discovery::DeleteRequest::new(&k, 2).unwrap())
        .await
        .unwrap();
    let err = client.fetch(&ek).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Http { status: 404, .. }));
    // Unsigned/garbage delete body is a 400.
    let mut sock = TcpStream::connect(client_addr(&client)).await.unwrap();
    rds_discovery::http::write_request(
        &mut sock,
        "DELETE",
        &format!("/v1/records/{ek}"),
        b"garbage",
    )
    .await
    .unwrap();
    let resp = rds_discovery::http::read_response(&mut sock).await.unwrap();
    assert_eq!(resp.status, 400);
}

fn client_addr(c: &Client) -> std::net::SocketAddr {
    c.addr().unwrap()
}

#[tokio::test]
async fn registry_names_resolve() {
    let reg_key = key(20);
    let device_key = key(21);
    let mut entries = BTreeMap::new();
    entries.insert(
        "amsterdam".to_string(),
        EndpointKey(device_key.verifying_key().to_bytes()),
    );
    let snap = SignedRegistry::publish(&reg_key, 1, 1, entries, Duration::from_secs(3600)).unwrap();
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            registry_key: Some(reg_key.verifying_key()),
            registry: Some(snap),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr()).with_registry_key(reg_key.verifying_key());
    let key = client.resolve_name("amsterdam").await.unwrap();
    assert_eq!(key.0, device_key.verifying_key().to_bytes());
    assert!(
        client.resolve_name("nonexistent").await.is_err(),
        "unknown name 404s"
    );
    assert!(
        client.resolve_name("BAD NAME!!").await.is_err(),
        "invalid name rejected"
    );
}

#[tokio::test]
async fn hostile_input_never_panics() {
    let (_dir, client) = serve().await;
    let addr = client_addr(&client);
    // Garbage request line, oversized head, huge content-length,
    // truncated body — each must yield an error response, not a hang
    // or crash.
    for blob in [
        b"NOT A REQUEST\r\n\r\n".as_slice(),
        b"GET /v1/health HTTP/9.9 extra junk\r\n\r\n".as_slice(),
        b"PUT /v1/records HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n{}".as_slice(),
        b"PUT /v1/records HTTP/1.1\r\nContent-Length: 100\r\n\r\nshort".as_slice(),
        b"PUT /v1/records HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".as_slice(),
    ] {
        let mut sock = TcpStream::connect(addr).await.unwrap();
        sock.write_all(blob).await.unwrap();
        // Half-close: a truncated body must surface as EOF, not a hang.
        sock.shutdown().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
            .await
            .expect("server must answer, not hang")
            .unwrap();
        assert!(n > 0, "empty answer for {blob:?}");
        let head = String::from_utf8_lossy(&buf[..n]);
        assert!(
            head.starts_with("HTTP/1.1 4"),
            "expected 4xx for {blob:?}, got {head}"
        );
    }
    // Service still healthy after the barrage.
    client.health().await.unwrap();
}

#[tokio::test]
async fn registry_put_requires_estate_signature() {
    let reg_key = key(30);
    let wrong_key = key(31);
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            registry_key: Some(reg_key.verifying_key()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr()).with_registry_key(reg_key.verifying_key());

    // A snapshot signed by a non-estate key is refused.
    let forged = SignedRegistry::publish(
        &wrong_key,
        1,
        1,
        BTreeMap::from([("evil".into(), EndpointKey([9; 32]))]),
        Duration::from_secs(600),
    )
    .unwrap();
    let err = client.update_registry(&forged).await.unwrap_err();
    assert!(
        matches!(err, DiscoveryError::Http { status: 401, .. }),
        "forged registry -> 401, got {err:?}"
    );
    // Names stay unresolvable — nothing was stored.
    assert!(client.resolve_name("evil").await.is_err());

    // The estate key's snapshot is accepted…
    let snap = SignedRegistry::publish(
        &reg_key,
        1,
        1,
        BTreeMap::from([("amsterdam".into(), EndpointKey([2; 32]))]),
        Duration::from_secs(600),
    )
    .unwrap();
    client.update_registry(&snap).await.unwrap();
    assert_eq!(
        client.resolve_name("amsterdam").await.unwrap(),
        EndpointKey([2; 32])
    );
    // An exact retry is idempotent and does not refresh the lease.
    client.update_registry(&snap).await.unwrap();
}

#[tokio::test]
async fn revocations_roundtrip_and_authz() {
    // WS4 denylist channel: the estate signs a snapshot of revoked
    // grant ids; the directory stores it verbatim and serves it.
    let reg_key = key(40);
    let wrong_key = key(41);
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            registry_key: Some(reg_key.verifying_key()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());

    // Nothing published yet → 404 → None.
    assert!(client.fetch_revocations().await.unwrap().is_none());

    // Forged signature refused.
    let forged = rds_discovery::revocations::SignedRevocations::publish(
        &wrong_key,
        1,
        1,
        BTreeSet::from([[9u8; 32]]),
        Duration::from_secs(60),
    )
    .unwrap();
    let err = client.update_revocations(&forged).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Http { status: 401, .. }));

    // Estate-signed snapshot accepted and served verbatim.
    let snap = rds_discovery::revocations::SignedRevocations::publish(
        &reg_key,
        1,
        1,
        BTreeSet::from([[1u8; 32], [2u8; 32]]),
        Duration::from_secs(60),
    )
    .unwrap();
    client.update_revocations(&snap).await.unwrap();
    let served = client
        .fetch_revocations()
        .await
        .unwrap()
        .expect("snapshot served");
    let payload = served.verify(&reg_key.verifying_key()).unwrap();
    assert!(payload.revoked.contains(&[1u8; 32]));

    // An exact retry is idempotent and does not refresh the lease.
    client.update_revocations(&snap).await.unwrap();
}

#[tokio::test]
async fn policy_survives_directory_restart_and_expiry_is_checked_on_get() {
    use rds_discovery::{
        authority::Authority, clock::Reading, policy::PolicyStore, revocations::SignedRevocations,
    };
    let path = std::env::temp_dir().join(format!(
        "rds-directory-policy-{:032x}",
        rand::random::<u128>()
    ));
    std::fs::create_dir(&path).unwrap();
    let issuer = key(44);
    let authority = Authority::new(&issuer.verifying_key(), 1).unwrap();
    let old = SignedRegistry::publish(
        &issuer,
        1,
        1,
        BTreeMap::from([("device-a".into(), EndpointKey([1; 32]))]),
        Duration::from_secs(120),
    )
    .unwrap();
    let new = SignedRegistry::publish(
        &issuer,
        1,
        2,
        BTreeMap::from([("device-a".into(), EndpointKey([2; 32]))]),
        Duration::from_secs(120),
    )
    .unwrap();
    let policy = PolicyStore::open(&path, authority, Reading::now().unwrap()).unwrap();
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig {
            registry: Some(old.clone()),
            policy: Some(policy),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(directory.addr());
    client.update_registry(&new).await.unwrap();
    let revocations = SignedRevocations::publish(
        &issuer,
        1,
        2,
        BTreeSet::from([[7; 32]]),
        Duration::from_secs(4),
    )
    .unwrap();
    let expires = revocations
        .verify(&issuer.verifying_key())
        .unwrap()
        .expires_at;
    client.update_revocations(&revocations).await.unwrap();
    drop(directory);
    let policy = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match PolicyStore::open(&path, authority, Reading::now().unwrap()) {
                Ok(store) => break store,
                Err(DiscoveryError::Busy) => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(error) => panic!("restart failed: {error}"),
            }
        }
    })
    .await
    .unwrap();
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig {
            registry: Some(old),
            policy: Some(policy),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(directory.addr()).with_registry_key(issuer.verifying_key());
    assert_eq!(
        client.resolve_name("device-a").await.unwrap(),
        EndpointKey([2; 32])
    );
    assert_eq!(
        client.fetch_revocations().await.unwrap().unwrap().payload,
        revocations.payload
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while now_unix().unwrap() < expires {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        client.fetch_revocations().await,
        Err(DiscoveryError::Http { status: 410, .. })
    ));
    drop(directory);
    std::fs::remove_dir_all(path).unwrap();
}

/// Two racing valid PUTs must never leave the older snapshot stored:
/// `verify_fresh`'s monotonic check has to run against the same
/// snapshot the write lock replaces.
#[tokio::test]
async fn concurrent_registry_puts_cannot_regress() {
    use rds_discovery::registry::RegistryPayload;

    let reg_key = key(50);
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            registry_key: Some(reg_key.verifying_key()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Arc::new(Client::new(dir.addr()).with_registry_key(reg_key.verifying_key()));

    let now = now_unix().unwrap();
    let n = 8u64;
    let mut tasks = Vec::new();
    for i in 0..n {
        // Same-second snapshots are ordered by revision; the winner is n-1.
        let snap = SignedRegistry::sign(
            &RegistryPayload {
                stamp: rds_discovery::authority::SnapshotStamp::new(
                    &reg_key.verifying_key(),
                    1,
                    i + 1,
                )
                .unwrap(),
                entries: BTreeMap::from([("device".into(), EndpointKey([i as u8; 32]))]),
                issued_at: now,
                expires_at: now + 3600,
            },
            &reg_key,
        )
        .unwrap();
        let client = client.clone();
        tasks.push(tokio::spawn(
            async move { client.update_registry(&snap).await },
        ));
    }
    let mut accepted = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            accepted += 1;
        }
    }
    assert!(accepted >= 1, "every registry PUT failed");
    // However the PUTs interleaved, the stored snapshot is the newest.
    assert_eq!(
        client.resolve_name("device").await.unwrap(),
        EndpointKey([(n - 1) as u8; 32]),
        "registry regressed under concurrent PUTs"
    );
}

#[tokio::test]
async fn registry_put_refused_without_configured_key() {
    // No estate key configured: the name API is off and PUTs are 401.
    let (_dir, client) = serve().await;
    let snap = SignedRegistry::publish(
        &key(32),
        1,
        1,
        BTreeMap::from([("amsterdam".into(), EndpointKey([2; 32]))]),
        Duration::from_secs(600),
    )
    .unwrap();
    let err = client.update_registry(&snap).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Http { status: 401, .. }));
}

#[tokio::test]
async fn directory_outage_fails_clean_and_fast() {
    // A listener that accepts and never answers — the client's own
    // timeout bounds the wait; resolve must not hang.
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = blackhole.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((sock, _)) = blackhole.accept().await {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(sock);
            });
        }
    });
    let client = Client::with_timeout(addr, Duration::from_millis(500));
    let t0 = std::time::Instant::now();
    let err = client.health().await.unwrap_err();
    assert!(
        matches!(err, DiscoveryError::Unreachable(_)),
        "blackhole -> Unreachable, got {err:?}"
    );
    assert!(
        t0.elapsed() < Duration::from_secs(3),
        "outage took {:?} to surface",
        t0.elapsed()
    );

    // A refused connection fails immediately.
    let refused = Client::new("127.0.0.1:1".parse().unwrap());
    let err = refused.health().await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Unreachable(_)));
}

#[tokio::test]
async fn refresh_keeps_record_live() {
    let (_dir, client) = serve().await;
    let k = key(16);
    let ek = EndpointKey(k.verifying_key().to_bytes());
    // New revisions can share a second; no fictitious future clock is needed.
    for revision in 1..=3 {
        client
            .publish(&record_at(&k, revision, now_unix().unwrap(), 300))
            .await
            .unwrap();
        let fetched = client.fetch(&ek).await.unwrap();
        assert!(fetched.verify_fresh().is_ok());
        assert_eq!(fetched.verify().unwrap().revision, revision);
    }
}
