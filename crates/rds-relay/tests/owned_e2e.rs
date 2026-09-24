//! End-to-end owned relay: two `noq` endpoints attached to one
//! `rds-relay` server, QUIC handshake and datagrams flowing entirely
//! through the tunnel when only the relay candidate is dialable.
#![cfg(feature = "owned-relay")]

use iroh::{EndpointAddr, SecretKey, TransportAddr};
use rds_net::{Backend, EndpointConfig};

const ALPN: &[u8] = b"rds/0";

#[tokio::test]
async fn configured_owned_relay_preserves_a_fixed_primary_bind_port() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key(81)),
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let address = relay.endpoint_addr();
    let route = rds_net::backends::noq::relay::relay_url_for(&address).unwrap();
    let reservation = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let bind = reservation.local_addr().unwrap();
    drop(reservation);
    let file = format!(
        r#"{{"schema_version":1,"backend":"noq","bind_addrs":["{bind}"],"relay":{{"mode":"owned","route":"{route}"}}}}"#
    );
    let config = rds_net::EndpointSettings::from_json(file.as_bytes())
        .unwrap()
        .into_endpoint()
        .unwrap();
    let endpoint = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rds_net::bind_endpoint(config),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(endpoint.addr().addrs.contains(&TransportAddr::Ip(bind)));
    assert_eq!(relay.endpoints(), 1);
    endpoint.close().await;
    relay.close().await;
}

fn key(seed: u8) -> SecretKey {
    SecretKey::from_bytes(&[seed; 32])
}

async fn endpoint(seed: u8, relay_addr: EndpointAddr) -> rds_net::Endpoint {
    rds_net::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        secret_key: Some(key(seed)),
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![ALPN.to_vec()],
        relay_endpoint: Some(relay_addr),
        ..Default::default()
    })
    .await
    .expect("bind endpoint")
}

/// `addr` reduced to its relay candidate — the direct-path dialer has
/// nothing to try, so every packet must traverse the tunnel.
fn relay_only(mut addr: EndpointAddr) -> EndpointAddr {
    addr.addrs.retain(|a| matches!(a, TransportAddr::Relay(_)));
    assert_eq!(addr.addrs.len(), 1, "endpoint must advertise its relay");
    addr
}

#[tokio::test]
async fn silent_direct_candidate_does_not_block_attached_relay() {
    attached_relay_after_silent_direct(false).await;
}

#[tokio::test]
async fn dual_stack_mux_preserves_attached_relay_routing() {
    attached_relay_after_silent_direct(true).await;
}

async fn attached_relay_after_silent_direct(dual_stack: bool) {
    use std::time::Duration;
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key(85)),
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let a = if dual_stack {
        rds_net::bind_endpoint(EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key(86)),
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap(), "[::1]:0".parse().unwrap()],
            relay_endpoint: Some(relay.endpoint_addr()),
            ..Default::default()
        })
        .await
        .unwrap()
    } else {
        endpoint(86, relay.endpoint_addr()).await
    };
    let b = endpoint(87, relay.endpoint_addr()).await;
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut target = relay_only(b.addr());
    target
        .addrs
        .insert(TransportAddr::Ip(silent.local_addr().unwrap()));
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let (outgoing, incoming) = tokio::join!(a.connect(target, ALPN), async {
            b.accept().await.unwrap().await
        });
        let outgoing = outgoing.unwrap();
        let incoming = incoming.unwrap();
        assert_eq!(outgoing.remote_id(), b.id());
        outgoing
            .send_datagram(b"relay fallback".to_vec().into())
            .unwrap();
        assert_eq!(
            &incoming.read_datagram().await.unwrap()[..],
            b"relay fallback"
        );
        incoming
            .send_datagram(b"return route".to_vec().into())
            .unwrap();
        assert_eq!(
            &outgoing.read_datagram().await.unwrap()[..],
            b"return route"
        );
    })
    .await;
    let forwarded = relay.stats().0;
    a.close().await;
    b.close().await;
    relay.close().await;
    assert!(
        result.is_ok(),
        "silent direct candidate blocked the attached relay"
    );
    assert!(forwarded > 0);
}

#[tokio::test]
async fn relay_forwards_handshake_and_datagrams() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key(0)),
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .expect("relay up");

    let a = endpoint(1, relay.endpoint_addr()).await;
    let b = endpoint(2, relay.endpoint_addr()).await;
    assert_eq!(relay.endpoints(), 2, "both endpoints attached");

    // Exercise the directory boundary too: owned relay locators have their own
    // scheme and public identity, and must survive signed announce/resolve.
    let directory = rds_discovery::service::serve(
        "127.0.0.1:0".parse().unwrap(),
        std::sync::Arc::new(rds_discovery::MemoryStore::default()),
        rds_discovery::service::ServiceConfig::open_ephemeral(),
    )
    .await
    .unwrap();
    let client = rds_discovery::client::Client::new(directory.addr());
    let _announce = rds_net::announce(
        b.clone(),
        rds_net::AnnounceConfig {
            issuer: rds_discovery::RecordIssuer::memory(ed25519_dalek::SigningKey::from_bytes(
                &key(2).to_bytes(),
            )),
            directory: client.clone(),
            services: vec![rds_discovery::Service::Ping],
            ttl: std::time::Duration::from_secs(120),
        },
    )
    .unwrap();
    let resolved = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(target) =
                rds_net::resolve_target(Some(client.clone()), &b.id().to_string()).await
            {
                break target;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("owned relay record must publish and resolve");
    let target = relay_only(resolved);
    // Accept must be polled while connect is in flight: the server's
    // endpoint only drives the handshake once the incoming is taken.
    let accept_b = tokio::spawn({
        let b = b.clone();
        async move { b.accept().await.expect("incoming").await }
    });
    let conn_a = a.connect(target, ALPN).await.expect("connect over relay");
    assert_eq!(conn_a.remote_id(), b.id());

    let conn_b = accept_b.await.unwrap().expect("accept");
    assert_eq!(conn_b.remote_id(), a.id());

    // Datagrams both ways — payload is opaque outer-QUIC to the relay.
    conn_a.send_datagram(b"hello".to_vec().into()).unwrap();
    let got = conn_b.read_datagram().await.unwrap();
    assert_eq!(&got[..], b"hello");
    conn_b.send_datagram(b"world".to_vec().into()).unwrap();
    let got = conn_a.read_datagram().await.unwrap();
    assert_eq!(&got[..], b"world");

    // Streams ride the same tunnel.
    let (mut send, mut recv) = conn_a.open_bi().await.unwrap();
    send.write_all(b"stream-data").await.unwrap();
    send.finish().unwrap();
    let (mut bs, mut br) = conn_b.accept_bi().await.unwrap();
    let mut buf = vec![0u8; 11];
    br.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"stream-data");
    bs.write_all(b"ack").await.unwrap();
    bs.finish().unwrap();
    let ack = recv.read_to_end(usize::MAX).await.unwrap();
    assert_eq!(&ack, b"ack");

    let (forwarded, dropped, bytes) = relay.stats();
    assert!(forwarded > 0, "relay must have forwarded datagrams");
    assert!(bytes > 0);
    assert_eq!(dropped, 0);

    a.close().await;
    b.close().await;
    relay.close().await;
}

#[tokio::test]
async fn reattach_replaces_stale_slot() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key(0)),
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .expect("relay up");

    // Two endpoints carrying the same key = one endpoint re-attaching;
    // the stale slot is evicted, not accumulated.
    let _a = endpoint(1, relay.endpoint_addr()).await;
    let _a2 = endpoint(1, relay.endpoint_addr()).await;
    assert_eq!(relay.endpoints(), 1, "re-attach must replace the slot");

    relay.close().await;
}

#[tokio::test]
async fn drain_evicts_and_refuses_new_attachments() {
    let relay_addr;
    let relay;
    {
        let r = rds_relay::server::serve(
            EndpointConfig {
                backend: Backend::Noq,
                secret_key: Some(key(0)),
                bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                ..Default::default()
            },
            Vec::new(),
        )
        .await
        .expect("relay up");
        relay_addr = r.endpoint_addr();
        relay = r;
    }

    let _a = endpoint(1, relay_addr.clone()).await;
    assert_eq!(relay.endpoints(), 1);

    relay.drain().await;
    assert_eq!(relay.endpoints(), 0, "drain closes all slots");

    // New attachments after drain cannot complete registration.
    let res = rds_net::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        secret_key: Some(key(2)),
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![ALPN.to_vec()],
        relay_endpoint: Some(relay_addr),
        ..Default::default()
    })
    .await;
    assert!(res.is_err(), "attach to a drained relay must fail");
}
