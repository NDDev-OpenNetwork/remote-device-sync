use ed25519_dalek::{Signer, SigningKey};
use rds_discovery::{
    DiscoveryError, EndpointKey,
    authority::{Authority, RotationPayload, SignedRotation, SnapshotStamp},
    clock::{Lease, Reading},
    policy::PolicyStore,
    registry::{RegistryPayload, SignedRegistry},
    revocations::{RevocationPayload, SignedRevocations},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rds-policy-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn root() -> Authority {
    Authority::new(&key(61).verifying_key(), 1).unwrap()
}
fn time(wall: u64, continuous: u64) -> Reading {
    Reading {
        boot: [7; 16],
        wall: Duration::from_secs(wall),
        continuous: Duration::from_secs(continuous),
    }
}
fn start() -> Reading {
    time(2_000_000_000, 1000)
}
fn rev(
    key: &SigningKey,
    epoch: u64,
    revision: u64,
    ids: BTreeSet<[u8; 32]>,
    now: Reading,
) -> SignedRevocations {
    SignedRevocations::sign(
        &RevocationPayload {
            stamp: SnapshotStamp::new(&key.verifying_key(), epoch, revision).unwrap(),
            revoked: ids,
            issued_at: now.wall.as_secs(),
            expires_at: now.wall.as_secs() + 60,
        },
        key,
    )
    .unwrap()
}
fn registry(revision: u64, byte: u8, now: Reading) -> SignedRegistry {
    SignedRegistry::sign(
        &RegistryPayload {
            stamp: SnapshotStamp::new(&key(61).verifying_key(), 1, revision).unwrap(),
            entries: BTreeMap::from([
                ("device-a".into(), EndpointKey([byte; 32])),
                ("device-b".into(), EndpointKey([byte; 32])),
            ]),
            issued_at: now.wall.as_secs(),
            expires_at: now.wall.as_secs() + 120,
        },
        &key(61),
    )
    .unwrap()
}

#[test]
fn revisions_survive_restart_and_do_not_depend_on_wall_timestamp() {
    let tmp = Temp::new();
    let now = start();
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    let old = rev(&key(61), 1, 1, BTreeSet::new(), now);
    let new = rev(&key(61), 1, 2, BTreeSet::from([[3; 32]]), now);
    assert!(store.accept_revocations(&old, now).unwrap());
    assert!(store.accept_revocations(&new, now).unwrap());
    assert!(!store.accept_revocations(&new, now).unwrap());
    drop(store);
    let mut store = PolicyStore::open(&tmp.0, root(), time(now.wall.as_secs() + 10, 1010)).unwrap();
    assert!(matches!(
        store.accept_revocations(&old, now),
        Err(DiscoveryError::Stale)
    ));
    let conflict = rev(&key(61), 1, 2, BTreeSet::new(), now);
    assert!(matches!(
        store.accept_revocations(&conflict, now),
        Err(DiscoveryError::Stale)
    ));
    assert!(
        store
            .revocations(now)
            .unwrap()
            .unwrap()
            .1
            .revoked
            .contains(&[3; 32])
    );
    let unknown_epoch = rev(&key(61), 2, 3, BTreeSet::new(), now);
    assert!(store.accept_revocations(&unknown_epoch, now).is_err());
}

#[test]
fn replay_and_restart_never_rearm_the_original_deadline() {
    let tmp = Temp::new();
    let now = start();
    let snapshot = rev(&key(61), 1, 8, BTreeSet::new(), now);
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    store.accept_revocations(&snapshot, now).unwrap();
    drop(store);
    // Wall time advanced only 20 seconds, but 61 seconds including sleep elapsed.
    let later = time(now.wall.as_secs() + 20, 1061);
    let mut store = PolicyStore::open(&tmp.0, root(), later).unwrap();
    assert!(!store.accept_revocations(&snapshot, later).unwrap());
    assert!(matches!(
        store.revocations(later),
        Err(DiscoveryError::Expired)
    ));
    let renewed = rev(&key(61), 1, 9, BTreeSet::new(), later);
    store.accept_revocations(&renewed, later).unwrap();
    assert!(store.revocations(later).unwrap().is_some());
}

#[test]
fn reboot_requires_a_new_revision_and_clock_rollback_is_not_recovery() {
    let tmp = Temp::new();
    let now = start();
    let snapshot = rev(&key(61), 1, 1, BTreeSet::from([[3; 32]]), now);
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    store.accept_revocations(&snapshot, now).unwrap();
    drop(store);
    assert!(PolicyStore::open(&tmp.0, root(), time(now.wall.as_secs() - 1, 1010)).is_err());
    let rebooted = Reading {
        boot: [8; 16],
        ..time(now.wall.as_secs() + 1, 1)
    };
    let mut store = PolicyStore::open(&tmp.0, root(), rebooted).unwrap();
    assert!(!store.accept_revocations(&snapshot, rebooted).unwrap());
    assert!(store.revocations(rebooted).is_err());
    store
        .accept_revocations(
            &rev(&key(61), 1, 2, BTreeSet::from([[3; 32]]), rebooted),
            rebooted,
        )
        .unwrap();
    assert!(
        store
            .revocations(rebooted)
            .unwrap()
            .unwrap()
            .1
            .revoked
            .contains(&[3; 32])
    );
}

#[test]
fn lease_checks_wall_steps_suspend_and_exact_expiry() {
    let now = start();
    let lease = Lease::new(now.wall.as_secs(), now.wall.as_secs() + 60, now).unwrap();
    assert!(lease.valid_at(now));
    for reading in [
        time(now.wall.as_secs() - 1, 1001),
        time(now.wall.as_secs() + 60, 1001),
        time(now.wall.as_secs() + 1, 1060),
        time(now.wall.as_secs() + 1, 999),
        Reading {
            boot: [8; 16],
            ..now
        },
    ] {
        assert!(!lease.valid_at(reading), "{reading:?}");
    }
    assert!(Reading::now().unwrap().continuous > Duration::ZERO);
}

#[test]
fn authenticated_rotation_is_durable_and_cannot_reopen_old_authority() {
    let tmp = Temp::new();
    let now = start();
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    let old = rev(&key(61), 1, 900, BTreeSet::from([[3; 32]]), now);
    store.accept_revocations(&old, now).unwrap();
    let next = Authority::new(&key(62).verifying_key(), 2).unwrap();
    let receipt = SignedRotation::sign(
        &RotationPayload {
            previous: root(),
            next,
            issued_at: now.wall.as_secs(),
            expires_at: now.wall.as_secs() + 60,
        },
        &key(61),
        &key(62),
    )
    .unwrap();
    let mut forged = receipt.clone();
    forged.next_signature[0] ^= 1;
    assert!(store.apply_rotation(&forged, now).is_err());
    assert_eq!(store.authority(), root());
    store.apply_rotation(&receipt, now).unwrap();
    assert!(store.revocations(now).unwrap().is_none());
    assert!(store.accept_revocations(&old, now).is_err());
    store
        .accept_revocations(&rev(&key(62), 2, 1, BTreeSet::from([[3; 32]]), now), now)
        .unwrap();
    drop(store);
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    assert_eq!(store.authority(), next);
    store
        .apply_rotation(&receipt, time(now.wall.as_secs() + 61, 1061))
        .unwrap();
    assert!(store.accept_revocations(&old, now).is_err());
    drop(store);
    assert!(
        PolicyStore::open(&tmp.0, next, now).is_err(),
        "changing the bootstrap cannot silently reset trust"
    );
}

#[test]
fn name_revision_floor_is_global_and_durable_without_inventory_disclosure() {
    let tmp = Temp::new();
    let now = start();
    let new = registry(10, 3, now);
    let old = registry(9, 2, now);
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    store
        .accept_name(&new.bindings["device-a"], "device-a", now)
        .unwrap();
    drop(store);
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    assert!(matches!(
        store.accept_name(&old.bindings["device-b"], "device-b", now),
        Err(DiscoveryError::Stale)
    ));
    let mut conflict = new.bindings["device-b"]
        .verify(&key(61).verifying_key(), "device-b", now.wall.as_secs())
        .unwrap();
    conflict.expires_at += 1;
    let conflict = rds_discovery::registry::SignedNameBinding::sign(&conflict, &key(61)).unwrap();
    assert!(matches!(
        store.accept_name(&conflict, "device-b", now),
        Err(DiscoveryError::Stale)
    ));
    assert!(
        store
            .accept_name(&new.bindings["device-b"], "device-b", now)
            .is_ok()
    );
    let conflict = registry(10, 4, now);
    assert!(matches!(
        store.accept_name(&conflict.bindings["device-a"], "device-a", now),
        Err(DiscoveryError::Stale)
    ));
    assert!(
        store
            .accept_name(&new.bindings["device-a"], "device-b", now)
            .is_err()
    );
    let rebooted = Reading {
        boot: [8; 16],
        ..now
    };
    assert!(
        store
            .accept_name(&new.bindings["device-a"], "device-a", rebooted)
            .is_err()
    );
}

#[test]
fn old_bootstrap_cannot_replace_committed_registry() {
    let tmp = Temp::new();
    let now = start();
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    store.accept_registry(&registry(2, 3, now), now).unwrap();
    drop(store);
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    store.bootstrap_registry(&registry(1, 2, now), now).unwrap();
    let proof = store.name("device-a", now).unwrap();
    assert_eq!(
        proof
            .verify(&key(61).verifying_key(), "device-a", now.wall.as_secs())
            .unwrap()
            .key,
        EndpointKey([3; 32])
    );
    assert!(matches!(
        store.name("device-a", time(now.wall.as_secs() + 120, 1120)),
        Err(DiscoveryError::Expired)
    ));
}

#[test]
fn missing_corrupt_linked_and_busy_state_never_fall_back_to_empty() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let tmp = Temp::new();
    let now = start();
    let mut store = PolicyStore::open(&tmp.0, root(), now).unwrap();
    assert!(matches!(
        PolicyStore::open(&tmp.0, root(), now),
        Err(DiscoveryError::Busy)
    ));
    store
        .accept_revocations(&rev(&key(61), 1, 1, BTreeSet::new(), now), now)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&tmp.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(tmp.0.join("policy.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let committed = std::fs::read(tmp.0.join("policy.json")).unwrap();
    let other = Temp::new();
    let external = other.0.join("external");
    std::fs::write(&external, b"untouched").unwrap();
    std::fs::remove_file(tmp.0.join("policy.json")).unwrap();
    symlink(&external, tmp.0.join("policy.json")).unwrap();
    assert!(
        store
            .accept_revocations(&rev(&key(61), 1, 2, BTreeSet::new(), now), now)
            .is_err()
    );
    assert!(!store.is_healthy());
    assert!(store.revocations(now).is_err());
    assert_eq!(std::fs::read(&external).unwrap(), b"untouched");
    drop(store);
    assert!(PolicyStore::open(&tmp.0, root(), now).is_err());
    std::fs::remove_file(tmp.0.join("policy.json")).unwrap();
    assert!(
        PolicyStore::open(&tmp.0, root(), now).is_err(),
        "missing committed file reset trust"
    );
    std::fs::write(tmp.0.join("policy.json"), b"{}").unwrap();
    assert!(PolicyStore::open(&tmp.0, root(), now).is_err());
    std::fs::write(tmp.0.join("policy.json"), &committed).unwrap();
    let mut damaged = committed.clone();
    damaged[0] ^= 1;
    std::fs::write(tmp.0.join("policy.json"), damaged).unwrap();
    assert!(PolicyStore::open(&tmp.0, root(), now).is_err());
    std::fs::write(tmp.0.join("policy.json"), &committed).unwrap();
    std::fs::hard_link(tmp.0.join("policy.json"), tmp.0.join("alias")).unwrap();
    assert!(PolicyStore::open(&tmp.0, root(), now).is_err());
}

#[test]
fn revocation_domain_size_lifetime_and_authority_are_verified_before_use() {
    let now = start();
    let key = key(61);
    let snapshot = rev(&key, 1, 1, BTreeSet::new(), now);
    let mut raw_domain = snapshot.clone();
    raw_domain.signature = key.sign(&snapshot.payload).to_bytes().to_vec();
    assert!(matches!(
        raw_domain.verify(&key.verifying_key()),
        Err(DiscoveryError::BadSignature)
    ));
    let mut huge = snapshot.clone();
    huge.payload.resize(40 * 1024 + 1, 0);
    assert!(huge.verify(&key.verifying_key()).is_err());
    let mut bad = snapshot.verify(&key.verifying_key()).unwrap();
    bad.expires_at = bad.issued_at + 301;
    assert!(
        SignedRevocations::sign(&bad, &key)
            .unwrap()
            .verify_at(&key.verifying_key(), now.wall.as_secs())
            .is_err()
    );
    bad.expires_at = bad.issued_at + 60;
    bad.stamp.revision = 0;
    assert!(SignedRevocations::sign(&bad, &key).is_err());
    assert!(
        snapshot
            .verify_at(&key.verifying_key(), now.wall.as_secs() + 60)
            .is_err()
    );
}
