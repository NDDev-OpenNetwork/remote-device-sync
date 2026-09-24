use super::tests::{PHASES, Temp};
use super::*;
use ed25519_dalek::SigningKey;

pub(super) fn time(wall: u64, continuous: u64) -> Reading {
    Reading {
        boot: [1; 16],
        wall: Duration::from_secs(wall),
        continuous: Duration::from_secs(continuous),
    }
}
pub(super) fn record(seed: u8, revision: u64, issued: u64) -> EndpointRecord {
    let signer = SigningKey::from_bytes(&[seed; 32]);
    EndpointRecord::sign(
        &crate::Payload {
            version: crate::RECORD_VERSION,
            key: EndpointKey(signer.verifying_key().to_bytes()),
            revision,
            issued_at: issued,
            expires_at: issued + 300,
            addrs: vec!["127.0.0.1:4000".parse().unwrap()],
            relay_urls: vec![],
            services: vec![crate::Service::Ping],
        },
        &signer,
    )
    .unwrap()
}
fn put(store: &FileStore, record: &EndpointRecord, now: Reading) -> Result<(), DiscoveryError> {
    store
        .access(record.key, Some(Entry::Record(record.clone())), Some(now))
        .map(|_| ())
}

#[test]
fn process_restart_and_suspend_cannot_rearm_a_record_lease() {
    let tmp = Temp::new();
    let record = record(91, 1, 1000);
    let store = FileStore::new(&tmp.0).unwrap();
    put(&store, &record, time(1000, 10)).unwrap();
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    // Wall time lags after sleep, but the original continuous deadline passed.
    assert!(matches!(
        store.access(record.key, None, Some(time(1250, 310))),
        Err(DiscoveryError::Expired)
    ));
    assert_eq!(store.len(), 0);
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    assert!(matches!(
        put(&store, &record, time(1250, 311)),
        Err(DiscoveryError::Expired)
    ));
    let successor = self::record(91, 2, 1250);
    put(&store, &successor, time(1250, 311)).unwrap();
    assert_eq!(
        store
            .access(record.key, None, Some(time(1250, 312)))
            .unwrap()
            .unwrap(),
        successor
    );
}

#[test]
fn observed_expiry_persists_a_clock_floor_and_never_reanimates_content() {
    let tmp = Temp::new();
    let record = record(92, 1, 1000);
    let store = FileStore::new(&tmp.0).unwrap();
    put(&store, &record, time(1000, 10)).unwrap();
    assert!(matches!(
        store.access(record.key, None, Some(time(1300, 11))),
        Err(DiscoveryError::Expired)
    ));
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    assert!(matches!(
        store.access(record.key, None, Some(time(1299, 12))),
        Err(DiscoveryError::Store(_))
    ));
    assert!(store.collect_at(Some(time(1299, 12))).is_err());
    assert!(put(&store, &self::record(92, 2, 1299), time(1299, 12)).is_err());
    assert!(matches!(
        store.access(record.key, None, Some(time(1300, 13))),
        Err(DiscoveryError::Expired)
    ));
    // Same revision with newly signed validity still conflicts with the floor.
    assert!(matches!(
        put(&store, &self::record(92, 1, 1300), time(1300, 13)),
        Err(DiscoveryError::Stale)
    ));
    put(&store, &self::record(92, 2, 1300), time(1300, 13)).unwrap();
}

#[test]
fn runtime_clock_rollback_is_refused_without_poisoning_the_store() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    let record = record(93, 1, 1000);
    put(&store, &record, time(1000, 10)).unwrap();
    assert!(
        store
            .access(record.key, None, Some(time(1100, 110)))
            .is_ok()
    );
    assert!(
        store
            .access(record.key, None, Some(time(1099, 111)))
            .is_err()
    );
    assert!(
        store
            .access(record.key, None, Some(time(1100, 112)))
            .is_ok()
    );
}

#[test]
fn reboot_requires_a_successor_revision_and_retry_does_not_rearm() {
    let tmp = Temp::new();
    let record = record(94, 1, 1000);
    let store = FileStore::new(&tmp.0).unwrap();
    put(&store, &record, time(1000, 10)).unwrap();
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    let mut reboot = time(1100, 1);
    reboot.boot = [2; 16];
    assert!(matches!(
        put(&store, &record, reboot),
        Err(DiscoveryError::Expired)
    ));
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    assert!(matches!(
        store.access(record.key, None, Some(time(1100, 110))),
        Err(DiscoveryError::Expired)
    ));
    put(&store, &self::record(94, 2, 1100), reboot).unwrap();
}

#[test]
fn collection_is_bounded_compacts_content_and_retains_all_identity_floors() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    for seed in 0..=GC_BATCH as u8 {
        put(&store, &record(seed, 1, 1000), time(1000, 10)).unwrap();
    }
    assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), GC_BATCH);
    assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), 1);
    assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), 0);
    assert_eq!(store.len(), 0);
    {
        let inner = store.inner.lock().unwrap();
        assert_eq!(inner.metadata.identities, (GC_BATCH + 1) as u64);
        let read = inner.db.begin_read().unwrap();
        let table = read.open_table(RECORDS).unwrap();
        for item in table.iter().unwrap() {
            let (_, bytes) = item.unwrap();
            assert!(
                bytes.value().len() < 64,
                "expired payload was not reclaimed"
            );
        }
    }
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    for seed in 0..=GC_BATCH as u8 {
        assert!(matches!(
            put(&store, &record(seed, 1, 1300), time(1300, 310)),
            Err(DiscoveryError::Stale)
        ));
    }
}

#[test]
fn deletion_floor_survives_content_collection_and_restart() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    let signer = SigningKey::from_bytes(&[95; 32]);
    let record = record(95, 1, 1000);
    put(&store, &record, time(1000, 10)).unwrap();
    let tomb = DeleteRequest::sign(
        &crate::DeletePayload {
            version: crate::RECORD_VERSION,
            key: record.key,
            revision: 2,
            issued_at: 1001,
            expires_at: 1100,
        },
        &signer,
    )
    .unwrap();
    store
        .access(record.key, Some(Entry::Deleted(tomb)), Some(time(1001, 11)))
        .unwrap();
    assert_eq!(store.collect_at(Some(time(1100, 110))).unwrap(), 1);
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    assert!(matches!(
        put(&store, &record, time(1100, 110)),
        Err(DiscoveryError::Stale)
    ));
    assert!(matches!(
        store.access(record.key, None, Some(time(1100, 110))),
        Err(DiscoveryError::NotFound)
    ));
    put(&store, &self::record(95, 3, 1100), time(1100, 110)).unwrap();
}

#[test]
fn failed_retirement_closes_reads_until_recovery_and_never_discards_history() {
    for phase in PHASES {
        let tmp = Temp::new();
        let store = FileStore::new(&tmp.0).unwrap();
        let record = record(96, 1, 1000);
        put(&store, &record, time(1000, 10)).unwrap();
        store.inner.lock().unwrap().fault = Some((phase, false));
        assert!(matches!(
            store.access(record.key, None, Some(time(1300, 310))),
            Err(DiscoveryError::Store(_))
        ));
        assert!(
            store
                .access(record.key, None, Some(time(1300, 310)))
                .is_err()
        );
        drop(store);
        let store = FileStore::new(&tmp.0).unwrap();
        assert_eq!(store.inner.lock().unwrap().metadata.identities, 1);
        assert!(matches!(
            store.access(record.key, None, Some(time(1300, 310))),
            Err(DiscoveryError::Expired)
        ));
        assert!(matches!(
            put(&store, &self::record(96, 1, 1300), time(1300, 310)),
            Err(DiscoveryError::Stale)
        ));
    }
}

#[test]
fn crash_collector() {
    let Some(path) = std::env::var_os("RDS_TEST_GC_CRASH_DIRECTORY") else {
        return;
    };
    let index: usize = std::env::var("RDS_TEST_GC_CRASH_PHASE")
        .unwrap()
        .parse()
        .unwrap();
    let store = FileStore::new(std::path::PathBuf::from(path)).unwrap();
    store.inner.lock().unwrap().fault = Some((PHASES[index], true));
    store.collect_at(Some(time(1300, 310))).unwrap();
    panic!("crash checkpoint not reached");
}

#[test]
fn abrupt_collection_exit_recovers_one_atomic_floor_generation() {
    for (index, phase) in PHASES.into_iter().enumerate() {
        let tmp = Temp::new();
        let store = FileStore::new(&tmp.0).unwrap();
        put(&store, &record(97, 1, 1000), time(1000, 10)).unwrap();
        drop(store);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "records::expiry_tests::crash_collector"])
            .env("RDS_TEST_GC_CRASH_DIRECTORY", &tmp.0)
            .env("RDS_TEST_GC_CRASH_PHASE", index.to_string())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "{phase:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let store = FileStore::new(&tmp.0).unwrap();
        assert_eq!(store.len(), usize::from(phase == Phase::BeforeDatabase));
        assert_eq!(store.inner.lock().unwrap().metadata.identities, 1);
        store.collect_at(Some(time(1300, 310))).unwrap();
        assert!(matches!(
            put(&store, &record(97, 1, 1300), time(1300, 310)),
            Err(DiscoveryError::Stale)
        ));
    }
}

#[test]
fn stored_decoders_reject_trailing_bytes() {
    let record = record(98, 1, 1000);
    let stored = Stored::Active {
        entry: Entry::Record(record.clone()),
        lease: crate::clock::Lease::new(1000, 1300, time(1000, 10)).unwrap(),
    };
    let mut bytes = postcard::to_stdvec(&stored).unwrap();
    bytes.push(0);
    assert!(decode(&bytes, &record.key).is_err());
    let mut metadata = postcard::to_stdvec(&Metadata {
        format: 3,
        database: [3; 16],
        generation: 1,
        live: 0,
        identities: 0,
        wall_floor: Duration::ZERO,
    })
    .unwrap();
    metadata.push(0);
    assert!(exact::<Metadata>(&metadata).is_err());
}

#[test]
fn concurrent_renewal_and_collection_never_retire_the_successor() {
    let tmp = Temp::new();
    let store = FileStore::new(&tmp.0).unwrap();
    for seed in 110..118 {
        put(&store, &record(seed, 1, 1000), time(1000, 10)).unwrap();
    }
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            store.collect_at(Some(time(1300, 310))).unwrap();
        });
        scope.spawn(|| {
            barrier.wait();
            for seed in 110..118 {
                put(&store, &record(seed, 2, 1300), time(1300, 310)).unwrap();
            }
        });
    });
    assert_eq!(store.len(), 8);
    drop(store);
    let store = FileStore::new(&tmp.0).unwrap();
    for seed in 110..118 {
        let expected = record(seed, 2, 1300);
        assert_eq!(
            store
                .access(expected.key, None, Some(time(1300, 310)))
                .unwrap()
                .unwrap(),
            expected
        );
    }
}
