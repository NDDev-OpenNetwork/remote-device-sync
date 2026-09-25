//! Lifecycle tests use real HTTP sockets and explicitly released blocking work.
use rds_discovery::{
    DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, RecordStore,
    client::Client,
    http,
    service::{self, Directory, Limits, ServiceConfig},
    tls::server_config_from_pem,
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[derive(Default)]
struct Gate {
    released: Mutex<bool>,
    wake: Condvar,
    started: tokio::sync::Notify,
    calls: AtomicUsize,
}
impl Gate {
    fn enter(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        let (_guard, timeout) = self
            .wake
            .wait_timeout_while(
                self.released.lock().unwrap(),
                Duration::from_secs(10),
                |r| !*r,
            )
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "fixture did not release blocking work"
        );
    }
    fn release(&self) {
        *self.released.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.wake.notify_all();
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
#[derive(Default)]
struct Store {
    request: Option<Arc<Gate>>,
    gc: Option<Arc<Gate>>,
    panic_next: AtomicBool,
}
impl RecordStore for Store {
    fn put_admitted(
        &self,
        _: &EndpointRecord,
        _: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<(), DiscoveryError> {
        unreachable!()
    }
    fn remove_admitted(
        &self,
        _: &DeleteRequest,
        _: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<(), DiscoveryError> {
        unreachable!()
    }
    fn put(&self, _: &EndpointRecord) -> Result<(), DiscoveryError> {
        unreachable!()
    }
    fn remove(&self, _: &DeleteRequest) -> Result<(), DiscoveryError> {
        unreachable!()
    }
    fn len(&self) -> usize {
        0
    }
    fn get(&self, _: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        assert!(
            !self.panic_next.swap(false, Ordering::SeqCst),
            "fixture worker panic"
        );
        if let Some(gate) = &self.request {
            gate.enter();
        }
        Err(DiscoveryError::NotFound)
    }
    fn collect_expired(&self) -> Result<usize, DiscoveryError> {
        if let Some(gate) = &self.gc {
            gate.enter();
        }
        Ok(0)
    }
}
async fn serve(store: Arc<Store>) -> Directory {
    service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            limits: Limits {
                max_workers: 1,
                conn_timeout: Duration::from_millis(150),
                ..Default::default()
            },
            ..ServiceConfig::open_ephemeral()
        },
    )
    .await
    .unwrap()
}
async fn started(gate: &Gate) {
    tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
        .await
        .unwrap();
}
async fn finish(directory: &Directory) {
    tokio::time::timeout(Duration::from_secs(2), directory.close())
        .await
        .unwrap()
        .unwrap();
}
async fn listener_closed(addr: std::net::SocketAddr) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while TcpStream::connect(addr).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn canceled_close_keeps_timed_out_job_owned_and_other_waiters_join_it() {
    let gate = Arc::new(Gate::default());
    let release = Release(gate.clone());
    let store = Arc::new(Store {
        request: Some(gate.clone()),
        ..Default::default()
    });
    let weak = Arc::downgrade(&store);
    let directory = Arc::new(serve(store).await);
    let client = Client::new(directory.addr());
    let request = tokio::spawn(async move { client.fetch(&EndpointKey([3; 32])).await });
    started(&gate).await;
    assert!(request.await.unwrap().is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(75), directory.close())
            .await
            .is_err()
    );
    listener_closed(directory.addr()).await;
    assert!(weak.upgrade().is_some(), "started job lost its store");
    let one = tokio::spawn({
        let directory = directory.clone();
        async move { directory.close().await }
    });
    let two = tokio::spawn({
        let directory = directory.clone();
        async move { directory.close().await }
    });
    drop(release);
    for waiter in [one, two] {
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    finish(&directory).await;
    assert!(
        weak.upgrade().is_none(),
        "closed directory retained store state"
    );
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    let _rebound = tokio::net::TcpListener::bind(directory.addr())
        .await
        .unwrap();
}

#[tokio::test]
async fn drop_stops_accepting_and_releases_store_after_started_job_finishes() {
    let gate = Arc::new(Gate::default());
    let release = Release(gate.clone());
    let store = Arc::new(Store {
        request: Some(gate.clone()),
        ..Default::default()
    });
    let weak = Arc::downgrade(&store);
    let directory = serve(store).await;
    let addr = directory.addr();
    let client = Client::new(addr);
    let request = tokio::spawn(async move { client.fetch(&EndpointKey([4; 32])).await });
    started(&gate).await;
    drop(directory);
    listener_closed(addr).await;
    assert!(request.await.unwrap().is_err());
    assert!(weak.upgrade().is_some());
    drop(release);
    tokio::time::timeout(Duration::from_secs(2), async {
        while weak.upgrade().is_some() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn close_joins_started_maintenance_and_schedules_no_successor() {
    let gate = Arc::new(Gate::default());
    let release = Release(gate.clone());
    let store = Arc::new(Store {
        gc: Some(gate.clone()),
        ..Default::default()
    });
    let weak = Arc::downgrade(&store);
    let directory = serve(store).await;
    started(&gate).await;
    // This custom backend supplies no observer. Scrapes must not call len()
    // or invent an empty catalog, including while maintenance is stalled.
    let metrics = directory.metrics().snapshot();
    assert_eq!(metrics["rds_directory_records_supported"], 0);
    assert_eq!(metrics["rds_directory_records_known"], 0);
    assert!(!metrics.contains_key("rds_directory_records_stored"));
    Client::new(directory.addr()).health().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(75), directory.close())
            .await
            .is_err()
    );
    listener_closed(directory.addr()).await;
    assert!(weak.upgrade().is_some());
    drop(release);
    finish(&directory).await;
    assert!(weak.upgrade().is_none());
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn close_cancels_silent_http_and_tls_without_waiting_connection_deadline() {
    for tls in [false, true] {
        let tls_config = tls.then(|| {
            let rcgen::CertifiedKey { cert, signing_key } =
                rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            server_config_from_pem(
                cert.pem().as_bytes(),
                signing_key.serialize_pem().as_bytes(),
            )
            .unwrap()
        });
        let directory = service::serve(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(Store::default()),
            ServiceConfig {
                tls: tls_config,
                limits: Limits {
                    conn_timeout: Duration::from_secs(30),
                    ..Default::default()
                },
                ..ServiceConfig::open_ephemeral()
            },
        )
        .await
        .unwrap();
        let mut sock = TcpStream::connect(directory.addr()).await.unwrap();
        if !tls {
            sock.write_all(b"GET /v1/health HTTP/1.1\r\n")
                .await
                .unwrap();
        }
        tokio::task::yield_now().await;
        finish(&directory).await;
        let mut wire = Vec::new();
        // Reset is also a valid close for unread/partial input.
        let _ = tokio::time::timeout(Duration::from_secs(1), sock.read_to_end(&mut wire))
            .await
            .unwrap();
        assert!(wire.is_empty());
    }
}

#[tokio::test]
async fn panicked_storage_worker_returns_error_and_releases_capacity() {
    let directory = serve(Arc::new(Store {
        panic_next: AtomicBool::new(true),
        ..Default::default()
    }))
    .await;
    let mut stream = TcpStream::connect(directory.addr()).await.unwrap();
    http::write_request(
        &mut stream,
        "GET",
        &format!("/v1/records/{}", EndpointKey([5; 32])),
        &[],
    )
    .await
    .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), http::read_response(&mut stream))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 500);
    let client = Client::new(directory.addr());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match client.health().await {
                Ok(()) => break,
                Err(DiscoveryError::RateLimited) => tokio::task::yield_now().await,
                Err(error) => panic!("health after worker panic: {error}"),
            }
        }
    })
    .await
    .unwrap();
    finish(&directory).await;
}
