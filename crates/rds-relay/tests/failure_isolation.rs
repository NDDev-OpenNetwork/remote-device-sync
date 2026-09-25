//! A failed relay link must not tear down a working direct connection.
#![cfg(feature = "owned-relay")]
use noq::{PathStatus, Runtime};
use rds_net::backends::noq::{
    self as owned,
    relay::{RelayHandle, RelaySocket, synthetic_for},
};
use rds_net::{Backend, EndpointConfig, SecretKey};
use std::{sync::Arc, time::Duration};

// Hide relay advertisements to make the test, rather than QNT, create the
// additional relay path with an explicit validation watcher. This is the socket injection seam,
// with real UDP, real relay registration and the same endpoint key throughout.
async fn fixture_endpoint(
    seed: u8,
    relay: rds_net::EndpointAddr,
) -> (owned::Endpoint, RelayHandle) {
    let key = SecretKey::from_bytes(&[seed; 32]);
    let (socket, handle) = RelaySocket::connect(relay, key.clone(), "127.0.0.1:0".parse().unwrap())
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
            ..Default::default()
        },
        Box::new(mux),
        locals,
        runtime,
        None,
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
    let (a, ah) = fixture_endpoint(115, relay.endpoint_addr()).await;
    let (b, bh) = fixture_endpoint(116, relay.endpoint_addr()).await;
    let _ah_route = ah.register_peer(b.id()).unwrap();
    let _bh_route = bh.register_peer(a.id()).unwrap();
    let (ca, cb) = tokio::time::timeout(Duration::from_secs(3), async {
        let (a, b) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
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
    // This explicit open installed the validation watcher; no QNT advertisement
    // in this fixture could have created it without that watcher.
    let path = tokio::time::timeout(
        Duration::from_secs(3),
        ca.inner().open_path_ensure(remote, PathStatus::Backup),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(path.remote_address().unwrap(), remote);
    assert!(relay.stats().0 > 0);
    assert!(ah.is_available() && bh.is_available());
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
    let traffic = tokio::time::timeout(Duration::from_secs(3), async {
        for seq in 0u8..25 {
            // A dead path may be abandoned normally; the direct connection must survive.
            let _ = path.ping();
            ca.send_datagram(vec![seq].into())?;
            let body = cb.read_datagram().await?;
            anyhow::ensure!(body.as_ref() == [seq].as_slice(), "direct datagram changed");
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
    let (a, ah) = fixture_endpoint(117, relay.endpoint_addr()).await;
    let (b, bh) = fixture_endpoint(118, relay.endpoint_addr()).await;
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
