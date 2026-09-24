//! Announce loop e2e: a real endpoint publishing into a real
//! directory service — the D2 contract end to end.

use std::sync::Arc;
use std::time::Duration;

use rds_discovery::client::Client;
use rds_discovery::service::{self, ServiceConfig};
use rds_discovery::{EndpointKey, MemoryStore, Service};
use rds_net::{AnnounceConfig, EndpointConfig, announce, bind_endpoint};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn announce_publishes_and_keeps_record_live() {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        store.clone(),
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());

    let key = rds_net::SecretKey::from_bytes(&[7u8; 32]);
    let endpoint = bind_endpoint(EndpointConfig {
        secret_key: Some(key.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    let _announce = announce(
        endpoint.clone(),
        AnnounceConfig {
            issuer: rds_discovery::publisher::RecordIssuer::memory(
                ed25519_dalek::SigningKey::from_bytes(&key.to_bytes()),
            ),
            directory: client.clone(),
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();

    // The announce task publishes asynchronously; poll until visible.
    let ek = EndpointKey(*endpoint.id().as_bytes());
    let mut payload = None;
    for _ in 0..50 {
        if let Ok(rec) = client.fetch(&ek).await
            && let Ok(p) = rec.verify_fresh()
        {
            payload = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let payload = payload.expect("record appears in the directory");
    assert_eq!(payload.services, vec![Service::Ping]);
    assert!(
        !payload.addrs.is_empty() || !payload.relay_urls.is_empty(),
        "record advertises reachability"
    );
}

/// The announce loop republishes promptly when the advertised address
/// set changes: starting announce before `online()` yields a record
/// without a relay candidate; once the home relay attaches, the next
/// poll must publish a record carrying it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn announce_republishes_when_addrs_change() {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        store,
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());

    let mut relay_config = iroh_relay::server::ServerConfig::default();
    relay_config.relay = Some(iroh_relay::server::RelayConfig::new(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let relay = iroh_relay::server::Server::spawn(relay_config)
        .await
        .unwrap();
    let relay_url = format!("http://{}", relay.http_addr().unwrap());

    // Announce starts while the endpoint is still offline: the first
    // record has direct addrs only.
    let key = rds_net::SecretKey::from_bytes(&[9u8; 32]);
    let endpoint = bind_endpoint(
        EndpointConfig {
            secret_key: Some(key.clone()),
            ..Default::default()
        }
        .with_relay(&relay_url)
        .unwrap(),
    )
    .await
    .unwrap();
    let _announce = announce(
        endpoint.clone(),
        AnnounceConfig {
            issuer: rds_discovery::publisher::RecordIssuer::memory(
                ed25519_dalek::SigningKey::from_bytes(&key.to_bytes()),
            ),
            directory: client.clone(),
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();
    let ek = EndpointKey(*endpoint.id().as_bytes());

    let mut first = None;
    for _ in 0..50 {
        if let Ok(rec) = client.fetch(&ek).await
            && let Ok(p) = rec.verify_fresh()
        {
            first = Some(p);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let first = first.expect("initial record publishes");
    assert!(
        first.relay_urls.is_empty(),
        "offline publish should carry no relay candidate yet"
    );

    // The home relay attaches; the next poll must republish with it.
    endpoint.online().await;
    let mut updated = false;
    for _ in 0..100 {
        if let Ok(rec) = client.fetch(&ek).await
            && let Ok(p) = rec.verify_fresh()
            && !p.relay_urls.is_empty()
        {
            updated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        updated,
        "announce must republish when the advertised addrs change"
    );
}

/// Resolve fallbacks: tickets never need a directory, names require
/// one, and a dead directory fails clean — never a hang, never a
/// wrong peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_fallbacks_behave() {
    // A ticket resolves with no directory at all.
    let endpoint = bind_endpoint(EndpointConfig::default()).await.unwrap();
    let ticket = rds_net::Ticket::of(&endpoint).to_string();
    let addr = rds_net::resolve_target(None, &ticket).await.unwrap();
    assert_eq!(addr.id, endpoint.id());

    // A bare key with no directory yields the key-only addr — the
    // caller decides if that's dialable.
    let bare = format!("{}", endpoint.id());
    let addr = rds_net::resolve_target(None, &bare).await.unwrap();
    assert_eq!(addr.id, endpoint.id());

    // A name with no directory is a clean, immediate error.
    let err = rds_net::resolve_target(None, "amsterdam")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("--server"),
        "name without directory should ask for --server, got {err}"
    );

    // A dead directory fails fast instead of hanging the resolve.
    let dead = Client::with_timeout("127.0.0.1:1".parse().unwrap(), Duration::from_millis(300));
    let t0 = std::time::Instant::now();
    let err = rds_net::resolve_target(Some(dead), &bare)
        .await
        .unwrap_err();
    assert!(
        t0.elapsed() < Duration::from_secs(3),
        "dead directory took {:?} to fail: {err}",
        t0.elapsed()
    );
}

/// A record that ages past `expires_at` while sitting in the store
/// must be refused at resolve time — resolvers verify freshness
/// themselves, they don't trust the directory's word.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_refuses_aged_out_record() {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        store,
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());

    // One hand-signed record with a 1s TTL — deterministic, no
    // background task that could republish past the boundary.
    let signing = ed25519_dalek::SigningKey::from_bytes(&[10u8; 32]);
    let ek = EndpointKey(signing.verifying_key().to_bytes());
    let now = rds_discovery::now_unix().unwrap();
    let rec = rds_discovery::EndpointRecord::sign(
        &rds_discovery::Payload {
            version: rds_discovery::RECORD_VERSION,
            revision: 1,
            key: ek,
            addrs: vec!["10.0.0.9:4200".parse().unwrap()],
            relay_urls: vec![],
            services: vec![Service::Ping],
            issued_at: now,
            expires_at: now + 1,
        },
        &signing,
    )
    .unwrap();
    client.publish(&rec).await.unwrap();

    let id = rds_net::EndpointId::from_bytes(&ek.0).unwrap();
    let bare = format!("{id}");
    rds_net::resolve_target(Some(client.clone()), &bare)
        .await
        .expect("fresh record resolves");

    // Past expiry the same stored record must be refused. issued_at
    // and now are whole seconds; wait beyond the exact expiry boundary.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let err = rds_net::resolve_target(Some(client.clone()), &bare)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<rds_discovery::DiscoveryError>(),
            Some(rds_discovery::DiscoveryError::Http { status: 410, .. })
        ),
        "directory must refuse aged record, got {err:#}"
    );

    // A hostile directory can still replay expired signed bytes. The resolver
    // must enforce validity independently, even when HTTP reports success.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hostile = Client::new(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        rds_discovery::http::read_request(&mut stream)
            .await
            .unwrap()
            .unwrap();
        rds_discovery::http::write_response(
            &mut stream,
            &rds_discovery::http::Response::json(200, rec),
        )
        .await
        .unwrap();
    });
    let err = rds_net::resolve_target(Some(hostile), &bare)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<rds_discovery::DiscoveryError>(),
            Some(rds_discovery::DiscoveryError::Expired)
        ),
        "{err:#}"
    );
    server.await.unwrap();
}

/// Resolve path e2e: announce → resolve_target by bare key → connect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_then_connect_by_bare_key() {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
        store,
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());

    // Local iroh relay shared by both endpoints — no internet
    // dependency in tests.
    let mut relay_config = iroh_relay::server::ServerConfig::default();
    relay_config.relay = Some(iroh_relay::server::RelayConfig::new(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let relay = iroh_relay::server::Server::spawn(relay_config)
        .await
        .unwrap();
    let relay_url = format!("http://{}", relay.http_addr().unwrap());

    // "Agent" endpoint.
    let key = rds_net::SecretKey::from_bytes(&[8u8; 32]);
    let agent = bind_endpoint(
        EndpointConfig {
            secret_key: Some(key.clone()),
            ..Default::default()
        }
        .with_relay(&relay_url)
        .unwrap(),
    )
    .await
    .unwrap();
    // Wait for the home relay so the first publish carries a dialable
    // relay candidate, same as production agents.
    agent.online().await;
    let _announce = announce(
        agent.clone(),
        AnnounceConfig {
            issuer: rds_discovery::publisher::RecordIssuer::memory(
                ed25519_dalek::SigningKey::from_bytes(&key.to_bytes()),
            ),
            directory: client.clone(),
            services: vec![Service::Ping],
            ttl: Duration::from_secs(120),
        },
    )
    .unwrap();

    // "CLI" endpoint resolves the agent's bare key via the directory.
    let cli_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    cli_ep.online().await;
    let target = format!("{}", agent.id());
    let mut addr = None;
    for _ in 0..50 {
        if let Ok(a) = rds_net::resolve_target(Some(client.clone()), &target).await {
            addr = Some(a);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let addr = addr.expect("bare key resolves through directory");
    assert_eq!(addr.id, agent.id());
    assert!(!addr.addrs.is_empty());

    // And the resolved addr actually dials. Awaiting the `Incoming`
    // drives the server-side handshake, so it must run concurrently
    // with the client connect.
    let accept = tokio::spawn({
        let agent = agent.clone();
        async move { agent.accept().await.expect("agent accepts").await }
    });
    let conn = cli_ep
        .connect(addr, rds_core::ALPN)
        .await
        .expect("connect through resolved record");
    assert_eq!(conn.remote_id(), agent.id());
    let conn_b = accept.await.unwrap().expect("agent handshake");
    assert_eq!(conn_b.remote_id(), cli_ep.id());
}
