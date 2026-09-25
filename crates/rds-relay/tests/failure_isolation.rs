//! A failed relay link must not tear down a working direct connection.
#![cfg(feature = "owned-relay")]
use noq::{PathStatus, Runtime};
use rds_net::backends::noq::{
    self as owned,
    relay::{RelayHandle, RelaySocket, synthetic_for},
};
use rds_net::{Backend, EndpointConfig, SecretKey};
use std::{sync::Arc, time::Duration};

// The managed variant wires tunnel health and QNT exactly as an attached
// endpoint. The raw variant hides registration for the missing-mapping case.
async fn fixture_endpoint(
    seed: u8,
    relay: rds_net::EndpointAddr,
    managed: bool,
) -> (owned::Endpoint, RelayHandle) {
    let key = SecretKey::from_bytes(&[seed; 32]);
    let (socket, handle) =
        RelaySocket::connect(relay.clone(), key.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
    let runtime = Arc::new(noq::TokioRuntime);
    let direct = runtime
        .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
        .unwrap();
    let mux = owned::socket::Mux::new(vec![direct, Box::new(socket)]).unwrap();
    let locals = mux.local_addrs();
    let endpoint = owned::bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key),
            discovery: false,
            relay_endpoint: managed.then_some(relay),
            ..Default::default()
        },
        Box::new(mux),
        locals,
        runtime,
        managed.then(|| handle.clone()),
    )
    .await
    .unwrap();
    (endpoint, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_relay_path_does_not_close_a_working_direct_connection() {
    tokio::time::timeout(Duration::from_secs(20), failure_case())
        .await
        .expect("failure-isolation fixture exceeded total deadline");
}

async fn failure_case() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (a, ah) = fixture_endpoint(115, relay.endpoint_addr(), true).await;
    let (b, bh) = fixture_endpoint(116, relay.endpoint_addr(), true).await;
    for endpoint in [&a, &b] {
        assert!(endpoint.addr().addrs.iter().all(|address| match address {
            rds_net::TransportAddr::Ip(ip) => !owned::relay::is_synthetic(*ip),
            _ => true,
        }));
    }
    let _ah_route = ah.register_peer(b.id()).unwrap();
    let _bh_route = bh.register_peer(a.id()).unwrap();
    let (ca, cb) = tokio::time::timeout(Duration::from_secs(3), async {
        let mut direct = b.addr();
        direct
            .addrs
            .retain(|address| matches!(address, rds_net::TransportAddr::Ip(_)));
        let (a, b) = tokio::join!(a.connect(direct, rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        (a.unwrap(), b.unwrap())
    })
    .await
    .unwrap();
    assert_eq!(ca.remote_address(), Some(b.local_addr()));
    ca.send_datagram(b"direct before failure".to_vec().into())
        .unwrap();
    assert_eq!(
        &tokio::time::timeout(Duration::from_secs(2), cb.read_datagram())
            .await
            .unwrap()
            .unwrap()[..],
        b"direct before failure"
    );
    let remote = synthetic_for(&b.id());
    let id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let attempt = ca.inner().open_path_ensure(remote, PathStatus::Backup);
            if let Some(id) = attempt.path_id() {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(id, noq::PathId::ZERO);
    // QNT may already have opened this path. Awaiting an existing OpenPath is
    // not validation proof; actual STREAM frames on it below are required.
    let path = tokio::time::timeout(
        Duration::from_secs(3),
        ca.inner().open_path_ensure(remote, PathStatus::Backup),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(path.remote_address().unwrap(), remote);
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut send, _recv) = ca.open_bi().await.unwrap();
        let direct = ca.inner().path(noq::PathId::ZERO).unwrap();
        direct.set_status(PathStatus::Backup).unwrap();
        path.set_status(PathStatus::Available).unwrap();
        send.write_all(b"v").await.unwrap();
        let (_return_send, mut receive) = cb.accept_bi().await.unwrap();
        let mut byte = [0; 1];
        receive.read_exact(&mut byte).await.unwrap();
        while path.stats().frame_tx.stream == 0 {
            direct.set_status(PathStatus::Backup).unwrap();
            path.set_status(PathStatus::Available).unwrap();
            send.write_all(b"v").await.unwrap();
            receive.read_exact(&mut byte).await.unwrap();
            tokio::task::yield_now().await;
        }
        assert_eq!(&byte, b"v");
    })
    .await
    .expect("no application stream frame crossed the relay path");
    assert!(relay.stats().0 > 0);
    assert!(ah.is_available() && bh.is_available());
    let mut relay_only = b.addr();
    relay_only
        .addrs
        .retain(|address| matches!(address, rds_net::TransportAddr::Relay(_)));
    assert!(!relay_only.addrs.is_empty());
    tokio::time::timeout(Duration::from_secs(3), relay.close())
        .await
        .expect("relay close stalled");
    tokio::time::timeout(Duration::from_secs(2), async {
        while ah.is_available() || bh.is_available() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("relay closure was not observed");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let direct_ready = [&ca, &cb].iter().all(|conn| {
                conn.inner().path(noq::PathId::ZERO).unwrap().status().ok()
                    == Some(PathStatus::Available)
            });
            if direct_ready && path.status().is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("failed tunnel remained eligible over a validated direct path");
    for endpoint in [&a, &b] {
        assert!(
            !endpoint
                .addr()
                .addrs
                .iter()
                .any(|address| matches!(address, rds_net::TransportAddr::Relay(_)))
        );
    }
    assert!(
        tokio::time::timeout(
            Duration::from_millis(250),
            a.connect(relay_only, rds_core::ALPN)
        )
        .await
        .expect("known failed relay entered a new handshake wait")
        .is_err()
    );
    let mut progress = (0u8, 0u8, 0u8);
    let traffic = tokio::time::timeout(Duration::from_secs(3), async {
        for seq in 0u8..25 {
            // A dead path may be abandoned normally; the direct connection must survive.
            let _ = path.ping();
            ca.send_datagram(vec![seq].into())?;
            progress.0 += 1;
            let body = cb.read_datagram().await?;
            progress.1 += 1;
            anyhow::ensure!(body.as_ref() == [seq].as_slice(), "direct datagram changed");
            cb.send_datagram(body)?;
            anyhow::ensure!(
                ca.read_datagram().await?.as_ref() == [seq].as_slice(),
                "direct reply changed"
            );
            progress.2 += 1;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&traffic, Ok(Ok(()))) {
        eprintln!("traffic progress sent/received/replied: {progress:?}");
        for (label, conn) in [("a", &ca), ("b", &cb)] {
            eprintln!("{label} closed: {:?}", conn.inner().close_reason());
            for raw in 0u32..8 {
                if let Some(path) = conn.inner().path(noq::PathId::from(raw)) {
                    eprintln!(
                        "{label} path {raw}: remote={:?} status={:?} stats={:?}",
                        path.remote_address(),
                        path.status(),
                        path.stats()
                    );
                }
            }
        }
    }
    let cleanup = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(a.close(), b.close());
    })
    .await;
    assert!(
        matches!(&traffic, Ok(Ok(()))),
        "relay link poisoned direct traffic: {traffic:?}"
    );
    assert!(cleanup.is_ok(), "endpoint cleanup stalled after relay loss");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_relay_peer_mapping_preserves_direct_io_and_can_be_registered_later() {
    tokio::time::timeout(Duration::from_secs(20), missing_mapping_case())
        .await
        .expect("missing-mapping fixture exceeded total deadline");
}

async fn missing_mapping_case() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (a, ah) = fixture_endpoint(117, relay.endpoint_addr(), false).await;
    let (b, bh) = fixture_endpoint(118, relay.endpoint_addr(), false).await;
    // Only the return route is known. No relay advertisement or received
    // relay frame can teach A about B before the explicit registration below.
    let _bh_route = bh.register_peer(a.id()).unwrap();
    let (ca, cb) = tokio::time::timeout(Duration::from_secs(3), async {
        let (a, b) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        (a.unwrap(), b.unwrap())
    })
    .await
    .unwrap();
    let remote = synthetic_for(&b.id());
    let id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let open = ca.inner().open_path_ensure(remote, PathStatus::Backup);
            if let Some(id) = open.path_id() {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let path = ca.inner().path(id).unwrap();
    assert_eq!(path.remote_address().unwrap(), remote);
    let traffic = tokio::time::timeout(Duration::from_secs(2), async {
        for seq in 0u8..3 {
            path.ping()?;
            ca.send_datagram(vec![seq].into())?;
            let body = cb.read_datagram().await?;
            anyhow::ensure!(body.as_ref() == [seq].as_slice(), "direct bytes changed");
            cb.send_datagram(body)?;
            anyhow::ensure!(
                ca.read_datagram().await?.as_ref() == [seq].as_slice(),
                "direct reply changed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if !matches!(&traffic, Ok(Ok(()))) {
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(a.close(), b.close(), relay.close());
        })
        .await;
        panic!("unknown relay destination poisoned direct traffic: {traffic:?}");
    }
    assert!(ah.is_available() && bh.is_available());
    assert_eq!(
        relay.stats().0,
        0,
        "unknown relay route unexpectedly forwarded traffic"
    );
    let _ah_route = ah.register_peer(b.id()).unwrap();
    let validated = tokio::time::timeout(
        Duration::from_secs(3),
        ca.inner().open_path_ensure(remote, PathStatus::Backup),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(validated.id(), id);
    assert!(
        relay.stats().0 > 0,
        "later registration did not enable relay traffic"
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(a.close(), b.close(), relay.close());
    })
    .await
    .expect("fixture cleanup stalled");
}
