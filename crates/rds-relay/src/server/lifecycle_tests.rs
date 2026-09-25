use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_session_removes_its_registration_and_observed_flow_history() {
    tokio::time::timeout(Duration::from_secs(10), canceled_session_case())
        .await
        .unwrap();
}
async fn canceled_session_case() {
    let config = || EndpointConfig {
        backend: rds_net::Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![proto::RELAY_ALPN.to_vec()],
        ..Default::default()
    };
    let server = rds_noq::bind_endpoint(config()).await.unwrap();
    let client = rds_noq::bind_endpoint(config()).await.unwrap();
    let (local, remote) = tokio::join!(client.connect(server.addr(), proto::RELAY_ALPN), async {
        server.accept().await.unwrap().await
    });
    let local = local.unwrap();
    let state = std::sync::Arc::new(State {
        conns: Mutex::new(HashMap::new()),
        recent: Mutex::new(HashMap::new()),
        allow: None,
        draining: AtomicBool::new(false),
        stats: Stats::default(),
        limits: ServerLimits::default(),
        admission: std::sync::Arc::new(Semaphore::new(256)),
        rejected: AtomicU64::new(0),
    });
    let worker = tokio::spawn(serve_conn(remote.unwrap(), state.clone()));
    let (mut send, mut recv) = local.open_bi().await.unwrap();
    write_control(&mut send, &RelayControl::Register)
        .await
        .unwrap();
    assert!(matches!(
        read_control(&mut recv).await.unwrap(),
        RelayControl::Registered
    ));
    while state.conns.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    // A real forwarded frame proves history exists before cancellation.
    local
        .send_datagram(proto::encode_forward(
            client.id().as_bytes(),
            b"history fixture",
        ))
        .unwrap();
    let frame = local.read_datagram().await.unwrap();
    assert_eq!(proto::decode_frame(&frame).unwrap().1, b"history fixture");
    assert_eq!(state.recent.lock().unwrap().len(), 1);
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(state.conns.lock().unwrap().is_empty());
    assert!(state.recent.lock().unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(1), local.inner().closed())
        .await
        .unwrap();
    tokio::join!(client.close(), server.close());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_drain_callers_both_observe_completed_cleanup() {
    tokio::time::timeout(Duration::from_secs(8), concurrent_drain_case())
        .await
        .unwrap();
}
async fn concurrent_drain_case() {
    let relay = serve(
        EndpointConfig {
            backend: rds_net::Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (mut socket, handle) = rds_noq::relay::RelaySocket::connect(
        relay.endpoint_addr(),
        rds_net::SecretKey::from_bytes(&[134; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    while relay.endpoints() == 0 {
        tokio::task::yield_now().await;
    }
    tokio::join!(relay.drain(), async {
        while !handle.drained() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        relay.drain().await;
        assert_eq!(
            relay.endpoints(),
            0,
            "concurrent drain returned during grace"
        );
        assert_eq!(relay.lifecycle_stats().active_connections, 0);
    });
    assert_eq!(relay.endpoint.active_path_drivers(), 0);
    socket.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_relay_closes_attached_tunnels() {
    let relay = serve(
        EndpointConfig {
            backend: rds_net::Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    // Kept only to clean up the failing-before implementation. Holding
    // an endpoint clone must not disable the service owner's shutdown.
    let cleanup = relay.endpoint.clone();
    let (mut socket, handle) = rds_noq::relay::RelaySocket::connect(
        relay.endpoint_addr(),
        rds_net::SecretKey::from_bytes(&[131; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    assert!(handle.is_available());
    drop(relay);
    let closed = tokio::time::timeout(Duration::from_secs(1), async {
        while handle.is_available() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(socket.close(), cleanup.close());
    })
    .await
    .expect("drop fixture cleanup stalled");
    assert!(
        closed.is_ok(),
        "Relay Drop left its attached tunnel running"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_drain_still_finishes_server_shutdown() {
    let relay = serve(
        EndpointConfig {
            backend: rds_net::Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (mut socket, handle) = rds_noq::relay::RelaySocket::connect(
        relay.endpoint_addr(),
        rds_net::SecretKey::from_bytes(&[132; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while relay.endpoints() != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), relay.drain())
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while !handle.drained() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("drain did not start");
    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        while handle.is_available() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(socket.close(), relay.close());
    })
    .await
    .expect("canceled drain fixture cleanup stalled");
    assert!(
        closed.is_ok(),
        "canceled drain left the server running after grace"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_registration_holds_admission_and_releases_it_on_disconnect() {
    tokio::time::timeout(Duration::from_secs(12), admission_case())
        .await
        .unwrap();
}
async fn admission_case() {
    let config = || EndpointConfig {
        backend: rds_net::Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![proto::RELAY_ALPN.to_vec()],
        ..Default::default()
    };
    let relay = serve_with_limits(
        config(),
        Vec::new(),
        ServerLimits {
            max_connections: NonZeroU16::new(1).unwrap(),
        },
    )
    .await
    .unwrap();
    let first = rds_noq::bind_endpoint(config()).await.unwrap();
    let second = rds_noq::bind_endpoint(config()).await.unwrap();
    let pending = first
        .connect(relay.endpoint_addr(), proto::RELAY_ALPN)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while relay.lifecycle_stats().active_connections != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(relay.endpoints(), 0, "silent peer must not be attached");
    let refused = tokio::time::timeout(
        Duration::from_secs(2),
        second.connect(relay.endpoint_addr(), proto::RELAY_ALPN),
    )
    .await
    .expect("over-budget incoming handshake was parked");
    assert!(refused.is_err());
    assert!(relay.lifecycle_stats().rejected_connections > 0);
    assert_eq!(relay.lifecycle_stats().active_connections, 1);
    pending.close(0u32.into(), b"release fixture slot");
    tokio::time::timeout(Duration::from_secs(1), async {
        while relay.lifecycle_stats().active_connections != 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let successor = second
        .connect(relay.endpoint_addr(), proto::RELAY_ALPN)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(relay.close(), relay.close());
    })
    .await
    .unwrap();
    assert_eq!(relay.lifecycle_stats().active_connections, 0);
    assert_eq!(relay.lifecycle_stats().history_entries, 0);
    assert_eq!(relay.endpoint.active_path_drivers(), 0);
    tokio::time::timeout(Duration::from_secs(1), successor.inner().closed())
        .await
        .unwrap();
    tokio::join!(first.close(), second.close());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_close_preserves_the_runner_for_other_waiters() {
    let relay = serve(
        EndpointConfig {
            backend: rds_net::Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (mut socket, handle) = rds_noq::relay::RelaySocket::connect(
        relay.endpoint_addr(),
        rds_net::SecretKey::from_bytes(&[133; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    let held = relay.accept_task.lock().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(25), relay.close())
            .await
            .is_err()
    );
    drop(held);
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(relay.close(), relay.close());
        socket.close().await;
    })
    .await
    .unwrap();
    assert_eq!(relay.lifecycle_stats().active_connections, 0);
    assert_eq!(relay.endpoint.active_path_drivers(), 0);
    assert_eq!(relay.endpoints(), 0);
    assert!(!handle.is_available());
}
