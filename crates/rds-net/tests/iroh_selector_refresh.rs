//! Real actor refresh against isolated loopback endpoints; no deployed devices.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use iroh::endpoint::transports::{PathSelection, PathSelectionContext, PathSelector};

#[derive(Debug)]
struct CountingSelector {
    calls: Arc<AtomicU64>,
    interval: Option<Duration>,
    stall: Arc<AtomicBool>,
    stalled: Arc<AtomicBool>,
}

impl PathSelector for CountingSelector {
    fn refresh_interval(&self) -> Option<Duration> {
        self.interval
    }
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.stall.swap(false, Ordering::Relaxed) {
            // Intentionally block only this isolated test actor to miss ticks.
            std::thread::sleep(Duration::from_secs(1));
            self.stalled.store(true, Ordering::Release);
        }
        let mut choice = PathSelection::none();
        if let Some(path) = ctx.paths().next() {
            choice.set(&path);
        }
        choice
    }
}

async fn exercise(interval: Option<Duration>, stall_probe: bool) {
    let calls = Arc::new(AtomicU64::new(0));
    let stall = Arc::new(AtomicBool::new(false));
    let stalled = Arc::new(AtomicBool::new(false));
    let a = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .path_selector(Arc::new(CountingSelector {
            calls: calls.clone(),
            interval,
            stall: stall.clone(),
            stalled: stalled.clone(),
        }))
        .alpns(vec![rds_core::ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let b = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .alpns(vec![rds_core::ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let (client, server) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    let (client, server) = (client.unwrap(), server.unwrap());
    // Let initial path events settle, then inspect the actual live actor.
    tokio::time::sleep(Duration::from_millis(350)).await;
    let initial = calls.load(Ordering::Relaxed);
    assert!(initial > 0);
    assert_eq!(client.paths().iter().count(), 1);
    if interval.is_some() {
        tokio::time::timeout(Duration::from_secs(2), async {
            while calls.load(Ordering::Relaxed) < initial + 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("opt-in refresh never called the selector again");
    } else {
        tokio::time::sleep(Duration::from_millis(750)).await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            initial,
            "default selector unexpectedly became periodic"
        );
    }
    if stall_probe {
        let before = calls.load(Ordering::Relaxed);
        stall.store(true, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !stalled.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("isolated actor did not execute the delayed selector");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            calls.load(Ordering::Relaxed) - before < 4,
            "missed refresh ticks burst instead of sampling only current state"
        );
    }
    // Ordinary bytes still flow while refresh runs, without new connections.
    let (mut send, mut recv) = client.open_bi().await.unwrap();
    send.write_all(b"refresh").await.unwrap();
    let (mut reply, mut request) = server.accept_bi().await.unwrap();
    let mut body = [0; 7];
    request.read_exact(&mut body).await.unwrap();
    assert_eq!(&body, b"refresh");
    reply.write_all(b"ok").await.unwrap();
    let mut ack = [0; 2];
    recv.read_exact(&mut ack).await.unwrap();
    assert_eq!(&ack, b"ok");
    tokio::join!(a.close(), b.close());
    let after_close = calls.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        calls.load(Ordering::Relaxed),
        after_close,
        "closed endpoint retained refresh work"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_refresh_runs_without_topology_change_and_stops_with_endpoint() {
    exercise(Some(Duration::from_millis(250)), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_custom_selector_remains_topology_only() {
    exercise(None, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missed_refresh_ticks_do_not_burst_after_actor_stall() {
    exercise(Some(Duration::from_millis(250)), true).await;
}
