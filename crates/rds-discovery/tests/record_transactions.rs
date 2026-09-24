//! R02: deleting a signed endpoint must retain replay protection.
use ed25519_dalek::SigningKey;
use rds_discovery::{DeleteRequest, EndpointRecord, FileStore, MemoryStore, RecordStore, Service};
use std::{path::PathBuf, time::Duration};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rds-record-txn-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn issuer() -> SigningKey {
    SigningKey::from_bytes(&[81; 32])
}
fn record() -> EndpointRecord {
    EndpointRecord::publish(
        &issuer(),
        1,
        vec!["127.0.0.1:4000".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        Duration::from_secs(300),
    )
    .unwrap()
}

#[test]
fn memory_deletion_retains_replay_protection() {
    let store = MemoryStore::default();
    let record = record();
    store.put(&record).unwrap();
    store
        .remove(&DeleteRequest::new(&issuer(), 2).unwrap())
        .unwrap();
    assert!(
        store.put(&record).is_err(),
        "deleted signed record was resurrected"
    );
}

#[test]
fn file_deletion_retains_replay_protection_after_restart() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    let record = record();
    store.put(&record).unwrap();
    store
        .remove(&DeleteRequest::new(&issuer(), 2).unwrap())
        .unwrap();
    drop(store);
    let restarted = FileStore::new(&tmp.0).unwrap();
    assert!(
        restarted.put(&record).is_err(),
        "restart forgot committed deletion"
    );
}

#[test]
fn corrupt_database_cannot_reset_the_stored_history() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    let record = record();
    store.put(&record).unwrap();
    drop(store);
    let path = tmp.0.join("records.redb");
    std::fs::write(&path, b"truncated").unwrap();
    assert!(
        FileStore::new(&tmp.0).is_err(),
        "corrupt history was treated as an absent record"
    );
    assert_eq!(std::fs::read(path).unwrap(), b"truncated");
}

#[test]
fn restoring_an_old_database_cannot_undo_an_acknowledged_delete() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    let record = record();
    store.put(&record).unwrap();
    drop(store);
    let old_database = std::fs::read(tmp.0.join("records.redb")).unwrap();
    let store = FileStore::new(&tmp.0).unwrap();
    store
        .remove(&DeleteRequest::new(&issuer(), 2).unwrap())
        .unwrap();
    drop(store);
    std::fs::write(tmp.0.join("records.redb"), old_database).unwrap();
    assert!(
        FileStore::new(&tmp.0).is_err(),
        "old database bypassed durable generation anchor"
    );
}

#[test]
fn missing_anchor_or_database_never_initializes_empty_history() {
    for file in ["records.redb", "records.anchor"] {
        let tmp = Temp::new();
        let store = FileStore::new(&tmp.0).unwrap();
        store.put(&record()).unwrap();
        store
            .remove(&DeleteRequest::new(&issuer(), 2).unwrap())
            .unwrap();
        drop(store);
        std::fs::remove_file(tmp.0.join(file)).unwrap();
        assert!(FileStore::new(&tmp.0).is_err(), "reset missing {file}");
        assert!(!tmp.0.join(file).exists(), "recreated missing {file}");
    }
}

#[test]
fn legacy_files_require_explicit_migration_and_remain_untouched() {
    let tmp = Temp::new();
    let path = tmp.0.join("legacy.json");
    std::fs::write(&path, b"legacy-record").unwrap();
    assert!(FileStore::new(&tmp.0).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"legacy-record");
    assert!(!tmp.0.join("records.redb").exists());
}

#[test]
fn private_single_writer_state_refuses_links_and_preserves_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let tmp = Temp::new();
    let outside = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    assert!(FileStore::new(&tmp.0).is_err());
    assert_eq!(
        std::fs::metadata(&tmp.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(tmp.0.join("records.redb"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(store);
    let target = outside.0.join("sentinel");
    std::fs::write(&target, b"preserve").unwrap();
    std::fs::remove_file(tmp.0.join("records.redb")).unwrap();
    symlink(&target, tmp.0.join("records.redb")).unwrap();
    assert!(FileStore::new(&tmp.0).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
    std::fs::remove_file(tmp.0.join("records.redb")).unwrap();
    std::fs::hard_link(&target, tmp.0.join("records.redb")).unwrap();
    assert!(FileStore::new(&tmp.0).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
}

#[test]
fn concurrent_puts_and_delete_have_one_monotonic_result() {
    use std::sync::{Arc, Barrier};
    let tmp = Temp::new();
    for store in [
        Arc::new(MemoryStore::default()) as Arc<dyn RecordStore>,
        Arc::new(FileStore::new(&tmp.0).unwrap()),
    ] {
        let initial = record();
        let mut payload = initial.verify().unwrap();
        let key = payload.key;
        store.put(&initial).unwrap();
        let tomb = DeleteRequest::new(&issuer(), 2).unwrap();
        let mut versions = Vec::new();
        for revision in 1..=8 {
            payload.revision = 2 + revision;
            payload.expires_at = payload.issued_at + 300;
            versions.push(EndpointRecord::sign(&payload, &issuer()).unwrap());
        }
        let latest = versions.last().unwrap().payload.clone();
        let barrier = Arc::new(Barrier::new(9));
        std::thread::scope(|scope| {
            for record in versions {
                let store = store.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let _ = store.put(&record);
                });
            }
            let store = store.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                let _ = store.remove(&tomb);
            });
        });
        assert_eq!(store.get(&key).unwrap().payload, latest);
        assert!(store.put(&initial).is_err());
        assert_eq!(store.len(), 1);
    }
}

#[test]
fn admission_is_atomic_and_retries_never_spend_another_budget() {
    use rds_discovery::DiscoveryError;
    let tmp = Temp::new();
    for store in [
        Box::new(MemoryStore::default()) as Box<dyn RecordStore>,
        Box::new(FileStore::new(&tmp.0).unwrap()),
    ] {
        let initial = record();
        let key = initial.key;
        assert!(matches!(
            store.put_admitted(&initial, &mut |known| {
                assert!(!known);
                Err(DiscoveryError::RateLimited)
            }),
            Err(DiscoveryError::RateLimited)
        ));
        assert!(matches!(store.get(&key), Err(DiscoveryError::NotFound)));
        store
            .put_admitted(&initial, &mut |known| {
                assert!(!known);
                Ok(())
            })
            .unwrap();
        store
            .put_admitted(&initial, &mut |_| panic!("exact retry was charged"))
            .unwrap();
        let mut payload = initial.verify().unwrap();
        payload.revision = 2;
        let next = EndpointRecord::sign(&payload, &issuer()).unwrap();
        assert!(matches!(
            store.put_admitted(&next, &mut |known| {
                assert!(known);
                Err(DiscoveryError::RateLimited)
            }),
            Err(DiscoveryError::RateLimited)
        ));
        assert_eq!(store.get(&key).unwrap(), initial);
        store
            .put_admitted(&next, &mut |known| {
                assert!(known);
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            store.put_admitted(&initial, &mut |_| panic!("stale record was charged")),
            Err(DiscoveryError::Stale)
        ));
        let tomb = DeleteRequest::new(&issuer(), 3).unwrap();
        assert!(matches!(
            store.remove_admitted(&tomb, &mut |known| {
                assert!(known);
                Err(DiscoveryError::RateLimited)
            }),
            Err(DiscoveryError::RateLimited)
        ));
        assert_eq!(store.get(&key).unwrap(), next);
        store
            .remove_admitted(&tomb, &mut |known| {
                assert!(known);
                Ok(())
            })
            .unwrap();
        store
            .remove_admitted(&tomb, &mut |_| panic!("exact deletion retry was charged"))
            .unwrap();
        payload.revision = 4;
        let next = EndpointRecord::sign(&payload, &issuer()).unwrap();
        store
            .put_admitted(&next, &mut |known| {
                assert!(known, "deletion lost known identity status");
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn concurrent_exact_mutations_charge_once_in_both_stores() {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    let tmp = Temp::new();
    for store in [
        Arc::new(MemoryStore::default()) as Arc<dyn RecordStore>,
        Arc::new(FileStore::new(&tmp.0).unwrap()),
    ] {
        let record = record();
        let calls = AtomicUsize::new(0);
        let barrier = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    store
                        .put_admitted(&record, &mut |known| {
                            assert!(!known);
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .unwrap();
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
