use super::*;
use crate::persist::Phase;
use std::path::PathBuf;
const PHASES: [Phase; 5] = [
    Phase::BeforeWrite,
    Phase::AfterWrite,
    Phase::AfterFileSync,
    Phase::AfterRename,
    Phase::AfterDirectorySync,
];
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rds-publisher-fault-{:032x}",
            rand::random::<u128>()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key() -> SigningKey {
    SigningKey::from_bytes(&[86; 32])
}
fn draft(port: u16) -> RecordDraft {
    RecordDraft {
        addrs: vec![([127, 0, 0, 1], port).into()],
        relay_urls: vec![],
        services: vec![Service::Ping],
        ttl: Duration::from_secs(300),
    }
}

#[test]
fn failed_durable_allocation_never_returns_bytes_and_poison_requires_reopen() {
    for phase in PHASES {
        let tmp = Temp::new();
        let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
        issuer.record(draft(4000), 1000).unwrap();
        issuer.disk.as_mut().unwrap().fault = Some((phase, false));
        assert!(issuer.record(draft(4001), 1000).is_err(), "{phase:?}");
        assert!(
            issuer.record(draft(4001), 1000).is_err(),
            "failed issuer reopened itself"
        );
        drop(issuer);
        let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
        let record = issuer.record(draft(4001), 1000).unwrap();
        assert_eq!(record.verify().unwrap().revision, 2, "{phase:?}");
        drop(issuer);
        let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
        assert_eq!(issuer.record(draft(4001), 1000).unwrap(), record);
    }
}

#[test]
fn disk_full_recovery_keeps_ownership_and_reconciles_exact_commit_boundaries() {
    for phase in PHASES {
        let tmp = Temp::new();
        let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
        issuer.record(draft(4000), 1000).unwrap();
        issuer.disk.as_mut().unwrap().storage_full = Some(phase);
        assert!(matches!(
            issuer.record(draft(4001), 1000),
            Err(DiscoveryError::StorageUnavailable(_))
        ));
        assert!(issuer.record(draft(4001), 1001).is_err());
        assert!(matches!(
            RecordIssuer::open(&tmp.0, key(), 1001),
            Err(DiscoveryError::Busy)
        ));
        assert!(matches!(
            issuer.recover_storage_full(1001),
            Err(DiscoveryError::StorageUnavailable(_))
        ));
        assert!(issuer.recover_storage_full(1000).is_err());
        issuer.disk.as_mut().unwrap().storage_full = None;
        issuer.recover_storage_full(1001).unwrap();
        let record = issuer.record(draft(4001), 1001).unwrap();
        assert_eq!(record.verify().unwrap().revision, 2, "{phase:?}");
        assert_eq!(issuer.record(draft(4001), 1002).unwrap(), record);
        drop(issuer);
        let mut reopened = RecordIssuer::open(&tmp.0, key(), 1002).unwrap();
        assert_eq!(reopened.record(draft(4001), 1002).unwrap(), record);
    }
}

#[test]
fn disk_full_recovery_never_accepts_unrelated_or_corrupt_local_history() {
    let tmp = Temp::new();
    let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
    issuer.record(draft(4000), 1000).unwrap();
    issuer.disk.as_mut().unwrap().storage_full = Some(Phase::BeforeWrite);
    assert!(matches!(
        issuer.record(draft(4001), 1000),
        Err(DiscoveryError::StorageUnavailable(_))
    ));
    issuer.disk.as_mut().unwrap().storage_full = None;
    let mut unrelated = RecordIssuer::memory(key());
    unrelated.record(draft(5000), 1000).unwrap();
    unrelated.record(draft(5001), 1000).unwrap();
    issuer
        .disk
        .as_ref()
        .unwrap()
        .write(&encode(&unrelated.state).unwrap())
        .unwrap();
    assert!(issuer.recover_storage_full(1001).is_err());
    assert!(issuer.record(draft(4001), 1001).is_err());
    issuer.disk.as_ref().unwrap().write(b"corrupted").unwrap();
    assert!(issuer.recover_storage_full(1002).is_err());
    assert!(issuer.record(draft(4001), 1002).is_err());
}

#[test]
fn crash_writer() {
    let Some(path) = std::env::var_os("RDS_TEST_PUBLISHER_CRASH_DIRECTORY") else {
        return;
    };
    let index: usize = std::env::var("RDS_TEST_PUBLISHER_CRASH_PHASE")
        .unwrap()
        .parse()
        .unwrap();
    let mut issuer = RecordIssuer::open(&PathBuf::from(path), key(), 1000).unwrap();
    issuer.disk.as_mut().unwrap().fault = Some((PHASES[index], true));
    let _ = issuer.record(draft(4001), 1000);
    panic!("crash checkpoint not reached");
}

#[test]
fn abrupt_exit_preserves_acknowledged_counter_and_exact_retry() {
    for (index, phase) in PHASES.into_iter().enumerate() {
        let tmp = Temp::new();
        let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
        issuer.record(draft(4000), 1000).unwrap();
        drop(issuer);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "publisher::tests::crash_writer"])
            .env("RDS_TEST_PUBLISHER_CRASH_DIRECTORY", &tmp.0)
            .env("RDS_TEST_PUBLISHER_CRASH_PHASE", index.to_string())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(86), "{phase:?}");
        let mut issuer = RecordIssuer::open(&tmp.0, key(), 1000).unwrap();
        let record = issuer.record(draft(4001), 1000).unwrap();
        assert_eq!(record.verify().unwrap().revision, 2, "{phase:?}");
        assert_eq!(issuer.record(draft(4001), 1001).unwrap(), record);
    }
}
