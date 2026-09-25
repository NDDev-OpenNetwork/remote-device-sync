//! Observation must neither own transport I/O nor delay closure until a tick.
use std::time::Duration;

use rds_net::{Backend, Connection, Endpoint, EndpointConfig, bind_endpoint, metrics::Registry};

async fn pair(backend: Backend) -> (Endpoint, Endpoint, Connection, Connection) {
    let config = || EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let a = bind_endpoint(config()).await.unwrap();
    let b = bind_endpoint(config()).await.unwrap();
    let (client, server) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    (a, b, client.unwrap(), server.unwrap())
}

fn assert_released(registry: &Registry) {
    let snapshot = registry.snapshot();
    for name in [
        "rds_net_active_connections",
        "rds_net_live_paths",
        "rds_net_policy_observed_connections",
        "rds_net_degraded_path_observers",
        "rds_net_selected_path_known",
        "rds_net_rtt_us",
        "rds_net_cwnd_bytes",
    ] {
        assert_eq!(snapshot[name], 0, "retained {name}");
    }
}

async fn exercise(backend: Backend) {
    let (a, b, client, server) = pair(backend).await;
    let registry = a.metrics();
    let mut sampler = registry.sampler(client.clone());
    sampler.sample();
    assert_eq!(registry.snapshot()["rds_net_active_connections"], 1);
    drop(client);
    let closed = tokio::time::timeout(Duration::from_secs(2), server.wait_closed()).await;
    // Cleanup before assertions also makes the old-code failure deterministic.
    drop(sampler);
    tokio::join!(a.close(), b.close());
    assert!(closed.is_ok(), "unpolled sampler retained connection I/O");
    assert_released(&registry);

    let (a, b, client, server) = pair(backend).await;
    let registry = a.metrics();
    let sampler = registry.sampler(client.clone());
    let mut running = tokio::spawn(sampler.run(Duration::from_secs(3600)));
    // Wait for the first observation, proving the task has started its wait.
    tokio::time::timeout(Duration::from_secs(2), async {
        while registry.snapshot()["rds_net_live_paths"] == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.close(0u32.into(), b"close while facade remains alive");
    let stopped = tokio::time::timeout(Duration::from_secs(2), &mut running).await;
    if stopped.is_err() {
        running.abort();
        let _ = running.await;
    }
    tokio::join!(a.close(), b.close());
    assert!(
        stopped.is_ok(),
        "sampler waited for its one-hour tick after closure"
    );
    stopped.unwrap().unwrap();
    assert_released(&registry);
    drop((client, server));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_sampler_observes_without_owning_connection() {
    exercise(Backend::Iroh).await;
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_sampler_observes_without_owning_connection() {
    exercise(Backend::Noq).await;
}

async fn streams_and_cancellation(backend: Backend) {
    let (a, b, client, server) = pair(backend).await;
    let registry = a.metrics();
    let (mut send, mut recv) = client.open_bi().await.unwrap();
    send.write_all(b"before").await.unwrap();
    let (mut reply, mut request) = server.accept_bi().await.unwrap();
    let mut prefix = [0; 6];
    request.read_exact(&mut prefix).await.unwrap();
    assert_eq!(&prefix, b"before");
    let sampler = registry.sampler(client.clone());
    let mut running = tokio::spawn(sampler.run(Duration::from_secs(3600)));
    drop(client);
    // The real stream is an I/O owner even after every facade is gone.
    send.write_all(b"after").await.unwrap();
    send.finish().unwrap();
    assert_eq!(request.read_to_end(5).await.unwrap(), b"after");
    reply.write_all(b"ack").await.unwrap();
    reply.finish().unwrap();
    assert_eq!(recv.read_to_end(3).await.unwrap(), b"ack");
    assert!(
        !running.is_finished(),
        "sampler ignored surviving stream I/O"
    );
    drop((send, recv));
    let result = tokio::time::timeout(Duration::from_secs(2), &mut running).await;
    if result.is_err() {
        running.abort();
        let _ = running.await;
    }
    tokio::join!(a.close(), b.close());
    result
        .expect("sampler retained the last stream's connection")
        .unwrap();
    assert_released(&registry);

    let (a, b, client, server) = pair(backend).await;
    let registry = a.metrics();
    let mut sampler = registry.sampler(client.clone());
    sampler.sample();
    let running = tokio::spawn(sampler.run(Duration::from_secs(3600)));
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert_released(&registry);
    assert!(
        !client.is_closed(),
        "observer cancellation closed owned I/O"
    );
    client
        .send_datagram(b"after cancellation".to_vec().into())
        .unwrap();
    assert_eq!(
        &tokio::time::timeout(Duration::from_secs(2), server.read_datagram())
            .await
            .unwrap()
            .unwrap()[..],
        b"after cancellation"
    );
    // A late run after close/drop must not wait for its first hour-long tick.
    let late = registry.sampler(client.clone());
    client.close(0u32.into(), b"late observer");
    assert!(client.path_stats().is_empty());
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), late.run(Duration::from_secs(3600)))
        .await
        .unwrap();
    assert_released(&registry);
    tokio::join!(a.close(), b.close());
}

async fn shared_gauge_ownership(backend: Backend) {
    let (a, b, first, server) = pair(backend).await;
    let (second, other_server) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
        b.accept().await.unwrap().await
    });
    let (second, other_server) = (second.unwrap(), other_server.unwrap());
    let registry = a.metrics();
    let mut one = registry.sampler(first.clone());
    let mut two = registry.sampler(second.clone());
    tokio::time::timeout(Duration::from_secs(2), async {
        for sampler in [&mut one, &mut two] {
            loop {
                sampler.sample();
                if registry.snapshot()["rds_net_selected_path_known"] == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    })
    .await
    .unwrap();
    drop(one);
    assert_eq!(registry.snapshot()["rds_net_active_connections"], 1);
    assert_eq!(
        registry.snapshot()["rds_net_selected_path_known"],
        1,
        "older sampler invalidated another connection's sample"
    );
    drop(two);
    assert_released(&registry);
    assert!(!first.is_closed() && !second.is_closed());
    tokio::join!(a.close(), b.close());
    drop((first, second, server, other_server));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_streams_own_io_and_sampler_cancellation_releases_gauges() {
    streams_and_cancellation(Backend::Iroh).await;
    shared_gauge_ownership(Backend::Iroh).await;
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_streams_own_io_and_sampler_cancellation_releases_gauges() {
    streams_and_cancellation(Backend::Noq).await;
    shared_gauge_ownership(Backend::Noq).await;
}
