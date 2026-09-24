use super::*;
use crate::{authority::SnapshotStamp, persist::Phase};
use ed25519_dalek::SigningKey;
use std::{collections::BTreeSet, path::PathBuf, time::Duration};

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
        let path =
            std::env::temp_dir().join(format!("rds-policy-crash-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn now() -> Reading {
    Reading {
        boot: [7; 16],
        wall: Duration::from_secs(2_000_000_000),
        continuous: Duration::from_secs(1000),
    }
}
fn authority() -> Authority {
    Authority::new(&SigningKey::from_bytes(&[73; 32]).verifying_key(), 1).unwrap()
}
fn snapshot(revision: u64) -> SignedRevocations {
    let key = SigningKey::from_bytes(&[73; 32]);
    SignedRevocations::sign(
        &RevocationPayload {
            stamp: SnapshotStamp::new(&key.verifying_key(), 1, revision).unwrap(),
            revoked: BTreeSet::from([[revision as u8; 32]]),
            issued_at: now().wall.as_secs(),
            expires_at: now().wall.as_secs() + 60,
        },
        &key,
    )
    .unwrap()
}
fn revision(store: &PolicyStore) -> u64 {
    store.revocations(now()).unwrap().unwrap().1.stamp.revision
}
fn committed(phase: Phase) -> u64 {
    if matches!(phase, Phase::AfterRename | Phase::AfterDirectorySync) {
        2
    } else {
        1
    }
}

#[test]
fn persistence_failure_never_publishes_an_uncertain_commit() {
    for phase in PHASES {
        let tmp = Temp::new();
        let mut store = PolicyStore::open(&tmp.0, authority(), now()).unwrap();
        store.accept_revocations(&snapshot(1), now()).unwrap();
        store.disk.as_mut().unwrap().fault = Some((phase, false));
        assert!(
            store.accept_revocations(&snapshot(2), now()).is_err(),
            "{phase:?}"
        );
        assert!(!store.is_healthy());
        // Old memory is retained for diagnosis but cannot authorize anything.
        assert_eq!(
            store.state.revocations.as_ref().unwrap().payload,
            snapshot(1).payload
        );
        assert!(store.revocations(now()).is_err());
        assert!(store.accept_revocations(&snapshot(3), now()).is_err());
        drop(store);
        let reopened = PolicyStore::open(&tmp.0, authority(), now()).unwrap();
        assert_eq!(revision(&reopened), committed(phase), "{phase:?}");
    }
}

#[test]
fn crash_writer() {
    let Some(path) = std::env::var_os("RDS_TEST_POLICY_CRASH_DIRECTORY") else {
        return;
    };
    let phase: usize = std::env::var("RDS_TEST_POLICY_CRASH_PHASE")
        .unwrap()
        .parse()
        .unwrap();
    let mut store = PolicyStore::open(Path::new(&path), authority(), now()).unwrap();
    assert_eq!(revision(&store), 1);
    store.disk.as_mut().unwrap().fault = Some((PHASES[phase], true));
    store.accept_revocations(&snapshot(2), now()).unwrap();
    panic!("crash checkpoint did not terminate the child");
}

#[test]
fn abrupt_process_exit_recovers_at_each_persistence_boundary() {
    for (index, phase) in PHASES.into_iter().enumerate() {
        let tmp = Temp::new();
        let mut store = PolicyStore::open(&tmp.0, authority(), now()).unwrap();
        store.accept_revocations(&snapshot(1), now()).unwrap();
        drop(store);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "policy::tests::crash_writer"])
            .env("RDS_TEST_POLICY_CRASH_DIRECTORY", &tmp.0)
            .env("RDS_TEST_POLICY_CRASH_PHASE", index.to_string())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "{phase:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut reopened = PolicyStore::open(&tmp.0, authority(), now()).unwrap();
        assert_eq!(revision(&reopened), committed(phase), "{phase:?}");
        if committed(phase) == 2 {
            assert!(matches!(
                reopened.accept_revocations(&snapshot(1), now()),
                Err(DiscoveryError::Stale)
            ));
        }
        reopened.accept_revocations(&snapshot(3), now()).unwrap();
        assert_eq!(revision(&reopened), 3);
    }
}
