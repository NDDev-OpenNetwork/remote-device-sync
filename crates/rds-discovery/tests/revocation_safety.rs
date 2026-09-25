use std::{collections::BTreeSet, time::Duration};

use ed25519_dalek::SigningKey;
use rds_discovery::{
    now_unix,
    revocations::{RevocationPayload, SignedRevocations},
};

#[test]
fn future_revocation_snapshot_cannot_claim_freshness() {
    let key = SigningKey::from_bytes(&[61; 32]);
    let now = now_unix().unwrap();
    let payload = RevocationPayload {
        stamp: rds_discovery::authority::SnapshotStamp::new(&key.verifying_key(), 1, 1).unwrap(),
        revoked: BTreeSet::new(),
        issued_at: now + 3600,
        expires_at: now + 3660,
    };
    let signed = SignedRevocations::sign(&payload, &key).unwrap();
    assert!(signed.verify_fresh(&key.verifying_key(), None).is_err());
}

#[test]
fn revocation_snapshot_cannot_delegate_unbounded_offline_access() {
    let key = SigningKey::from_bytes(&[61; 32]);
    let signed =
        SignedRevocations::publish(&key, 1, 1, BTreeSet::new(), Duration::from_secs(86_400));
    assert!(
        signed.is_err()
            || signed
                .unwrap()
                .verify_fresh(&key.verifying_key(), None)
                .is_err()
    );
}
