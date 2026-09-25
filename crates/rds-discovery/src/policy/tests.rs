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
        let metrics = store.metrics();
        assert_eq!(
            metrics.snapshot_at(Some(now()))["rds_directory_revocations_revision"],
            1
        );
        store.disk.as_mut().unwrap().fault = Some((phase, false));
        assert!(
            store.accept_revocations(&snapshot(2), now()).is_err(),
            "{phase:?}"
        );
        assert!(!store.is_healthy());
        let failed = metrics.snapshot_at(Some(now()));
        assert_eq!(failed["rds_directory_policy_healthy"], 0);
        assert!(!failed.contains_key("rds_directory_revocations_revision"));
        assert!(!failed.contains_key("rds_directory_revocations_fresh"));
        // Old memory is retained for diagnosis but cannot authorize anything.
        assert_eq!(
            store.state.revocations.as_ref().unwrap().payload,
            snapshot(1).payload
        );
        assert!(store.revocations(now()).is_err());
        assert!(store.accept_revocations(&snapshot(3), now()).is_err());
        drop(store);
        assert_eq!(
            metrics.snapshot_at(Some(now()))["rds_directory_policy_known"],
            0
        );
        let reopened = PolicyStore::open(&tmp.0, authority(), now()).unwrap();
        assert_eq!(revision(&reopened), committed(phase), "{phase:?}");
        let restored = reopened.metrics().snapshot_at(Some(now()));
        assert_eq!(restored["rds_directory_policy_healthy"], 1);
        assert_eq!(restored["rds_directory_policy_durable"], 1);
        assert_eq!(
            restored["rds_directory_revocations_revision"],
            committed(phase)
        );
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

#[test]
fn policy_observations_preserve_lease_clock_and_revision_semantics() {
    let mut store = PolicyStore::memory(authority()).unwrap();
    let metrics = store.metrics();
    let start = now();
    assert_eq!(
        metrics.snapshot_at(Some(start))["rds_directory_revocations_present"],
        0
    );
    store.accept_revocations(&snapshot(7), start).unwrap();
    let mut later = start;
    later.wall += Duration::from_secs(10);
    later.continuous += Duration::from_secs(40);
    assert!(!store.accept_revocations(&snapshot(7), later).unwrap());
    let observed = metrics.snapshot_at(Some(later));
    assert_eq!(observed["rds_directory_revocations_revision"], 7);
    assert_eq!(observed["rds_directory_revocations_entries"], 1);
    assert_eq!(observed["rds_directory_revocations_fresh"], 1);
    assert_eq!(
        observed["rds_directory_revocations_lease_remaining_seconds"],
        20
    );
    assert_eq!(observed["rds_directory_policy_durable"], 0);
    assert!(store.accept_revocations(&snapshot(6), later).is_err());
    assert_eq!(metrics.snapshot_at(Some(later)), observed);
    let mut rejected = snapshot(8);
    rejected.signature[0] ^= 1;
    assert!(store.accept_revocations(&rejected, later).is_err());
    assert_eq!(metrics.snapshot_at(Some(later)), observed);
    assert!(
        store
            .accept_revocations_admitted(&snapshot(8), later, || Err(DiscoveryError::RateLimited))
            .is_err()
    );
    assert_eq!(metrics.snapshot_at(Some(later)), observed);
    for invalid in [
        Reading {
            boot: [8; 16],
            ..later
        },
        Reading {
            continuous: start.continuous + Duration::from_secs(60),
            ..later
        },
        Reading {
            wall: start.wall + Duration::from_secs(60),
            ..later
        },
        Reading {
            wall: start.wall - Duration::from_secs(1),
            ..later
        },
        Reading {
            continuous: start.continuous - Duration::from_secs(1),
            ..later
        },
    ] {
        let observed = metrics.snapshot_at(Some(invalid));
        assert_eq!(observed["rds_directory_revocations_fresh"], 0);
        assert_eq!(
            observed["rds_directory_revocations_lease_remaining_seconds"],
            0
        );
        assert_eq!(observed["rds_directory_revocations_revision"], 7);
    }
    let unknown = metrics.snapshot_at(None);
    assert_eq!(unknown["rds_directory_policy_clock_known"], 0);
    assert!(!unknown.contains_key("rds_directory_revocations_fresh"));
    store.observed.begin();
    assert_eq!(
        metrics.snapshot_at(Some(later)),
        BTreeMap::from([("rds_directory_policy_known", 0)])
    );
    store.observed.finish(|_| {});
    drop(store);
    assert_eq!(
        metrics.snapshot_at(Some(later))["rds_directory_policy_known"],
        0
    );
}

#[test]
fn policy_observations_cover_registry_name_cache_rotation_and_global_clock_floor() {
    use crate::{
        EndpointKey,
        authority::{RotationPayload, SignedRotation},
        registry::RegistryPayload,
    };
    let signer = SigningKey::from_bytes(&[73; 32]);
    let tmp = Temp::new();
    let mut store = PolicyStore::open(&tmp.0, authority(), now()).unwrap();
    let metrics = store.metrics();
    let start = now();
    store.accept_revocations(&snapshot(1), start).unwrap();
    let mut later = start;
    later.wall += Duration::from_secs(10);
    later.continuous += Duration::from_secs(10);
    let registry = SignedRegistry::sign(
        &RegistryPayload {
            stamp: SnapshotStamp::new(&signer.verifying_key(), 1, 9).unwrap(),
            entries: BTreeMap::from([
                ("synthetic-a".into(), EndpointKey([1; 32])),
                ("synthetic-b".into(), EndpointKey([2; 32])),
            ]),
            issued_at: later.wall.as_secs(),
            expires_at: later.wall.as_secs() + 60,
        },
        &signer,
    )
    .unwrap();
    store.accept_registry(&registry, later).unwrap();
    for name in ["synthetic-a", "synthetic-b"] {
        store
            .accept_name(&registry.bindings[name], name, later)
            .unwrap();
    }
    let observed = metrics.snapshot_at(Some(later));
    for kind in ["registry", "name_cache"] {
        assert_eq!(
            observed[format!("rds_directory_{kind}_revision").as_str()],
            9
        );
        assert_eq!(
            observed[format!("rds_directory_{kind}_entries").as_str()],
            2
        );
    }
    // An older stream's lease alone remains valid, but the newer global policy
    // clock floor closes its admission and must also close reported freshness.
    assert_eq!(
        metrics.snapshot_at(Some(start))["rds_directory_revocations_fresh"],
        0
    );
    drop(store);
    assert_eq!(
        metrics.snapshot_at(Some(later))["rds_directory_policy_known"],
        0
    );
    let mut store = PolicyStore::open(&tmp.0, authority(), later).unwrap();
    let metrics = store.metrics();
    assert_eq!(metrics.snapshot_at(Some(later)), observed);
    let next = SigningKey::from_bytes(&[74; 32]);
    let receipt = SignedRotation::sign(
        &RotationPayload {
            previous: authority(),
            next: Authority::new(&next.verifying_key(), 2).unwrap(),
            issued_at: later.wall.as_secs(),
            expires_at: later.wall.as_secs() + 60,
        },
        &signer,
        &next,
    )
    .unwrap();
    store.apply_rotation(&receipt, later).unwrap();
    let rotated = metrics.snapshot_at(Some(later));
    assert_eq!(rotated["rds_directory_policy_epoch"], 2);
    for kind in ["registry", "name_cache", "revocations"] {
        assert_eq!(rotated[format!("rds_directory_{kind}_present").as_str()], 0);
        assert!(!rotated.contains_key(format!("rds_directory_{kind}_revision").as_str()));
    }
}
