use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observers_do_not_stop_the_relay_or_lose_normal_and_failed_outcomes() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for abort in [false, true] {
            let relay = serve(
                EndpointConfig {
                    backend: rds_net::Backend::Noq,
                    discovery: false,
                    bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                    ..Default::default()
                },
                vec![],
            )
            .await
            .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(25), relay.wait_stopped())
                    .await
                    .is_err()
            );
            let (mut socket, handle) = rds_noq::relay::RelaySocket::connect(
                relay.endpoint_addr(),
                rds_net::SecretKey::from_bytes(&[158; 32]),
                "127.0.0.1:0".parse().unwrap(),
            )
            .await
            .unwrap();
            assert!(handle.is_available());
            if abort {
                relay
                    .accept_task
                    .lock()
                    .await
                    .task
                    .as_ref()
                    .unwrap()
                    .abort();
                let observed = relay.wait_stopped().await.unwrap_err();
                assert!(observed.0.is_cancelled());
                let again = relay.wait_stopped().await.unwrap_err();
                let closed = relay.close().await.unwrap_err();
                assert!(std::sync::Arc::ptr_eq(&observed.0, &again.0));
                assert!(std::sync::Arc::ptr_eq(&observed.0, &closed.0));
            } else {
                // Observation may hold the runner mutex, but close must still
                // deliver its stop request before waiting for that mutex.
                let (observed, closed) = tokio::join!(relay.wait_stopped(), relay.close());
                observed.unwrap();
                closed.unwrap();
            }
            socket.close().await;
            assert!(!handle.is_available());
            assert_eq!(relay.lifecycle_stats().active_connections, 0);
            assert!(relay.connections.lock().await.is_empty());
        }
    })
    .await
    .unwrap();
}

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
        _identity: None,
        conns: Mutex::new(HashMap::new()),
        recent: Mutex::new(HashMap::new()),
        allow: None,
        draining: AtomicBool::new(false),
        stats: Stats::default(),
        limits: ServerLimits::default(),
        admission: std::sync::Arc::new(Semaphore::new(256)),
        rejected: AtomicU64::new(0),
    });
    let metrics = RelayMetrics(std::sync::Arc::downgrade(&state));
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
    let observed = metrics.snapshot();
    assert_eq!(observed["rds_relay_forwarded_datagrams_total"], 1);
    assert_eq!(
        observed["rds_relay_forwarded_bytes_total"],
        b"history fixture".len() as u64
    );
    assert_eq!(observed["rds_relay_endpoints"], 1);
    assert_eq!(observed["rds_relay_history_edges"], 1);
    {
        let _busy = state.recent.lock().unwrap();
        let observed = metrics.snapshot();
        assert_eq!(observed["rds_relay_history_known"], 0);
        assert!(!observed.contains_key("rds_relay_history_edges"));
    }
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(state.conns.lock().unwrap().is_empty());
    assert!(state.recent.lock().unwrap().is_empty());
    assert_eq!(metrics.snapshot()["rds_relay_endpoints"], 0);
    drop(state);
    assert_eq!(metrics.snapshot()["rds_relay_metrics_available"], 0);
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
    let (drained, ()) = tokio::join!(relay.drain(), async {
        while !handle.drained() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        relay.drain().await.unwrap();
        assert_eq!(
            relay.endpoints(),
            0,
            "concurrent drain returned during grace"
        );
        assert_eq!(relay.lifecycle_stats().active_connections, 0);
    });
    drained.unwrap();
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
        let (_, closed_1) = tokio::join!(socket.close(), relay.close());
        closed_1.unwrap();
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
        let (closed_0, closed_1) = tokio::join!(relay.close(), relay.close());
        closed_0.unwrap();
        closed_1.unwrap();
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
        let (closed_0, closed_1) = tokio::join!(relay.close(), relay.close());
        closed_0.unwrap();
        closed_1.unwrap();
        socket.close().await;
    })
    .await
    .unwrap();
    assert_eq!(relay.lifecycle_stats().active_connections, 0);
    assert_eq!(relay.endpoint.active_path_drivers(), 0);
    assert_eq!(relay.endpoints(), 0);
    assert!(!handle.is_available());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_runner_keeps_children_and_failure_across_canceled_close() {
    tokio::time::timeout(Duration::from_secs(12), failed_runner_case())
        .await
        .unwrap();
}
async fn failed_runner_case() {
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
    let relay = std::sync::Arc::new(relay);
    let (mut socket, handle) = rds_noq::relay::RelaySocket::connect(
        relay.endpoint_addr(),
        rds_net::SecretKey::from_bytes(&[138; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    while relay.endpoints() == 0 {
        tokio::task::yield_now().await;
    }
    {
        let runner = relay.accept_task.lock().await;
        runner.task.as_ref().unwrap().abort();
        while !runner.task.as_ref().unwrap().is_finished() {
            tokio::task::yield_now().await;
        }
    }
    // The actual accept runner is gone. Its registered connection handles
    // must remain available for fallback, along with a controlled async job.
    let mut children = relay.connections.lock().await;
    assert!(!children.is_empty());
    let owner = std::sync::Arc::new(());
    let weak = std::sync::Arc::downgrade(&owner);
    let (release, waiting) = tokio::sync::oneshot::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    children.spawn(async move {
        let _owner = owner;
        let _ = entered.send(());
        let _ = waiting.await;
    });
    ready.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), relay.close())
            .await
            .is_err()
    );
    {
        let runner = relay.accept_task.lock().await;
        assert!(
            runner.task.is_none(),
            "completed runner handle was retained for repoll"
        );
        assert!(runner.outcome.as_ref().unwrap().is_err());
    }
    drop(children);
    let closing = tokio::spawn({
        let relay = relay.clone();
        async move { relay.close().await }
    });
    // The failed runner is finished. Only this close waiter can now hold the
    // shared child-set lock, so cancellation occurs during its actual join.
    tokio::time::timeout(Duration::from_secs(2), async {
        while relay.connections.try_lock().is_ok() {
            assert!(
                !closing.is_finished(),
                "close skipped the pending child join"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    closing.abort();
    assert!(closing.await.unwrap_err().is_cancelled());
    assert!(
        weak.upgrade().is_some(),
        "fallback detached the controlled child"
    );
    release.send(()).unwrap();
    let (one, two) = tokio::join!(relay.close(), relay.close());
    let one = one.unwrap_err();
    let two = two.unwrap_err();
    assert!(one.0.is_cancelled());
    assert!(std::sync::Arc::ptr_eq(&one.0, &two.0));
    assert!(relay.close().await.is_err());
    assert!(relay.connections.lock().await.is_empty());
    assert!(weak.upgrade().is_none());
    assert_eq!(relay.lifecycle_stats().active_connections, 0);
    assert_eq!(relay.lifecycle_stats().history_entries, 0);
    assert_eq!(relay.endpoint.active_path_drivers(), 0);
    assert_eq!(relay.endpoints(), 0);
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.is_available() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    socket.close().await;
}
