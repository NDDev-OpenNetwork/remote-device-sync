//! Service-owned collection: bounded jobs, reclamation and teardown.
use rds_discovery::{
    DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, MemoryStore, RecordStore, Service,
    client::Client,
    service::{self, Limits, ServiceConfig},
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[tokio::test]
async fn directory_reclaims_expired_content_without_a_lookup_request() {
    let store = Arc::new(MemoryStore::default());
    let signer = ed25519_dalek::SigningKey::from_bytes(&[103; 32]);
    let record = EndpointRecord::publish(
        &signer,
        1,
        vec!["127.0.0.1:4000".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        Duration::from_secs(2),
    )
    .unwrap();
    store.put(&record).unwrap();
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        while !store.is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        store.get(&record.key),
        Err(DiscoveryError::Expired)
    ));
    let replay = EndpointRecord::publish(
        &signer,
        1,
        vec!["127.0.0.1:4000".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        Duration::from_secs(300),
    )
    .unwrap();
    assert!(matches!(store.put(&replay), Err(DiscoveryError::Stale)));
    Client::new(directory.addr()).health().await.unwrap();
}

#[derive(Default)]
struct SlowCollection {
    calls: AtomicUsize,
    released: Mutex<bool>,
    wake: Condvar,
    started: tokio::sync::Notify,
    finished: tokio::sync::Notify,
}
impl RecordStore for SlowCollection {
    fn put(&self, _: &EndpointRecord) -> Result<(), DiscoveryError> {
        unreachable!()
    }
    fn remove(&self, _: &DeleteRequest) -> Result<(), DiscoveryError> {
        unreachable!()
    }
    fn get(&self, _: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        Err(DiscoveryError::NotFound)
    }
    fn len(&self) -> usize {
        0
    }
    fn collect_expired(&self) -> Result<usize, DiscoveryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        let (_guard, timeout) = self
            .wake
            .wait_timeout_while(
                self.released.lock().unwrap(),
                Duration::from_secs(10),
                |released| !*released,
            )
            .unwrap();
        assert!(!timeout.timed_out(), "test did not release collector");
        self.finished.notify_one();
        Ok(0)
    }
}
struct Release(Arc<SlowCollection>);
impl Drop for Release {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.wake.notify_all();
    }
}
#[tokio::test]
async fn collection_has_one_owned_job_and_does_not_spend_request_worker_capacity() {
    let store = Arc::new(SlowCollection::default());
    let release = Release(store.clone());
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
        ServiceConfig {
            limits: Limits {
                max_workers: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), store.started.notified())
        .await
        .unwrap();
    Client::new(directory.addr()).health().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    drop(directory);
    tokio::task::yield_now().await;
    drop(release);
    tokio::time::timeout(Duration::from_secs(2), store.finished.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
}
