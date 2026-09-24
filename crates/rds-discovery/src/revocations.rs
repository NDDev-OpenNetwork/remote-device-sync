//! Signed grant-revocation snapshot — the denylist half of WS4.
//!
//! The estate's registry key (the same trust root that signs the
//! name→key registry) signs a snapshot of revoked grant ids. The
//! directory serves it at `GET /v1/revocations` and accepts updates at
//! `PUT /v1/revocations`; agents poll it on a cadence and feed their
//! policy denylist, so a revoked grant dies both for new connections
//! and — through the agent's denylist watchers — for live sessions.
//!
//! Grant ids are BLAKE3 digests of signed grant payloads (see
//! `rds_core::grant::GrantId`); they are content hashes, not secrets,
//! so the snapshot is safe to serve unauthenticated — integrity comes
//! from the signature, freshness from `issued_at`/`expires_at`.

use std::collections::BTreeSet;

use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::authority::{self, SnapshotStamp, invalid, lifetime};
use crate::{DiscoveryError, now_unix};

const DOMAIN: &[u8] = b"rds/revocations/v1\0";
pub const MAX_REVOCATION_TTL_SECS: u64 = 300;
pub const MAX_REVOKED_GRANTS: usize = 1024;
const MAX_PAYLOAD: usize = 40 * 1024;

/// A revoked grant id (`blake3` of the grant's signed payload).
pub type GrantHash = [u8; 32];

/// Estate-signed revocation snapshot served by the directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRevocations {
    /// Encoded [`RevocationPayload`] bytes.
    pub payload: Vec<u8>,
    /// Ed25519 signature over `payload`, made by the registry key.
    pub signature: Vec<u8>,
}

/// Signed portion of a [`SignedRevocations`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationPayload {
    pub stamp: SnapshotStamp,
    /// Revoked grant ids. BTreeSet keeps the encoding canonical.
    pub revoked: BTreeSet<GrantHash>,
    /// Unix seconds when the estate issued this snapshot.
    pub issued_at: u64,
    /// Unix seconds after which the snapshot stops being served.
    pub expires_at: u64,
}

impl SignedRevocations {
    /// Sign a fresh snapshot: `revoked` valid for `ttl`.
    pub fn publish(
        key: &SigningKey,
        epoch: u64,
        revision: u64,
        revoked: BTreeSet<GrantHash>,
        ttl: std::time::Duration,
    ) -> Result<Self, DiscoveryError> {
        let issued_at = now_unix()?;
        let payload = RevocationPayload {
            stamp: SnapshotStamp::new(&key.verifying_key(), epoch, revision)?,
            revoked,
            issued_at,
            expires_at: issued_at
                .checked_add(ttl.as_secs())
                .ok_or_else(|| invalid("TTL overflow"))?,
        };
        lifetime(
            payload.issued_at,
            payload.expires_at,
            issued_at,
            MAX_REVOCATION_TTL_SECS,
        )?;
        Self::sign(&payload, key)
    }

    pub fn sign(payload: &RevocationPayload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        payload.stamp.verify(&key.verifying_key())?;
        if payload.revoked.len() > MAX_REVOKED_GRANTS {
            return Err(invalid("too many revoked grants"));
        }
        let bytes = postcard::to_stdvec(&payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        if bytes.len() > MAX_PAYLOAD {
            return Err(invalid("revocations payload too large"));
        }
        let signature = authority::sign(DOMAIN, &bytes, key);
        Ok(Self {
            payload: bytes,
            signature,
        })
    }

    /// Verify the signature against the registry `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<RevocationPayload, DiscoveryError> {
        authority::verify(DOMAIN, &self.payload, &self.signature, key, MAX_PAYLOAD)?;
        let payload: RevocationPayload = postcard::from_bytes(&self.payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        payload.stamp.verify(key)?;
        if payload.revoked.len() > MAX_REVOKED_GRANTS {
            return Err(invalid("too many revoked grants"));
        }
        lifetime(
            payload.issued_at,
            payload.expires_at,
            payload.issued_at,
            MAX_REVOCATION_TTL_SECS,
        )?;
        Ok(payload)
    }

    pub fn verify_at(
        &self,
        key: &VerifyingKey,
        now: u64,
    ) -> Result<RevocationPayload, DiscoveryError> {
        let payload = self.verify(key)?;
        lifetime(
            payload.issued_at,
            payload.expires_at,
            now,
            MAX_REVOCATION_TTL_SECS,
        )?;
        Ok(payload)
    }

    /// Verify signature, freshness and monotonic replacement — a new
    /// snapshot must be strictly newer than `current`.
    pub fn verify_fresh(
        &self,
        key: &VerifyingKey,
        current: Option<&RevocationPayload>,
    ) -> Result<RevocationPayload, DiscoveryError> {
        let payload = self.verify_at(key, now_unix()?)?;
        if let Some(cur) = current {
            payload.stamp.newer_than(&cur.stamp)?;
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revocations_roundtrip_and_freshness() {
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let revoked = BTreeSet::from([[1u8; 32], [2u8; 32]]);
        let snap =
            SignedRevocations::publish(&key, 1, 1, revoked, std::time::Duration::from_secs(60))
                .unwrap();
        let payload = snap
            .verify_fresh(&key.verifying_key(), None)
            .expect("fresh snapshot verifies");
        assert!(payload.revoked.contains(&[1u8; 32]));
        // Same-age replacement is stale.
        assert!(matches!(
            snap.verify_fresh(&key.verifying_key(), Some(&payload)),
            Err(DiscoveryError::Stale)
        ));
        // Forged signature rejected.
        let wrong = SigningKey::from_bytes(&[6u8; 32]);
        assert!(matches!(
            snap.verify(&wrong.verifying_key()),
            Err(DiscoveryError::BadSignature)
        ));
    }
}
