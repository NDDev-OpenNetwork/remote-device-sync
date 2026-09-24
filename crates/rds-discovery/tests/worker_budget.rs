//! Timed-out HTTP requests must not release running disk-worker capacity.
use rds_discovery::{
    DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, RecordStore,
    client::Client,
    http,
    service::{self, Limits, ServiceConfig},
};
use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, net::TcpStream};

#[derive(Default)]
struct BlockingStore {
    released: Mutex<bool>,
    wake: Condvar,
    started: tokio::sync::Notify,
}
impl BlockingStore {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
impl RecordStore for BlockingStore {
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
    fn collect_expired(&self) -> Result<usize, DiscoveryError> {
        Ok(0)
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
        self.started.notify_one();
        let (_guard, timeout) = self
            .wake
            .wait_timeout_while(
                self.released.lock().unwrap(),
                Duration::from_secs(10),
                |released| !*released,
            )
            .unwrap();
        assert!(!timeout.timed_out(), "test failed to release worker");
        Err(DiscoveryError::NotFound)
    }
}
struct Release(Arc<BlockingStore>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test]
async fn request_timeout_retains_worker_permit_until_disk_job_ends() {
    let store = Arc::new(BlockingStore::default());
    let release = Release(store.clone());
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
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
    .unwrap();
    let client = Client::new(directory.addr());
    let request = tokio::spawn({
        let client = client.clone();
        async move { client.fetch(&EndpointKey([3; 32])).await }
    });
    tokio::time::timeout(Duration::from_secs(2), store.started.notified())
        .await
        .unwrap();
    assert!(request.await.unwrap().is_err());
    assert!(matches!(
        client.health().await,
        Err(DiscoveryError::RateLimited)
    ));
    let mut stream = TcpStream::connect(directory.addr()).await.unwrap();
    http::write_request(&mut stream, "HEAD", "/v1/health", &[])
        .await
        .unwrap();
    let mut wire = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut wire))
        .await
        .unwrap()
        .unwrap();
    assert!(wire.ends_with(b"\r\n\r\n"), "HEAD overload returned a body");
    let response = http::read_response(&mut std::io::Cursor::new(wire))
        .await
        .unwrap();
    assert_eq!(response.status, 429);
    assert!(response.body.is_empty());
    drop(release);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match client.health().await {
                Ok(()) => break,
                Err(DiscoveryError::RateLimited) => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("health failed: {error}"),
            }
        }
    })
    .await
    .unwrap();
}
