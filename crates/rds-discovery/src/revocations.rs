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

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{DiscoveryError, now_unix};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevocationPayload {
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
        revoked: BTreeSet<GrantHash>,
        ttl: std::time::Duration,
    ) -> Result<Self, DiscoveryError> {
        let issued_at = now_unix()?;
        let payload = RevocationPayload {
            revoked,
            issued_at,
            expires_at: issued_at + ttl.as_secs(),
        };
        let bytes = postcard::to_stdvec(&payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let signature = key.sign(&bytes);
        Ok(Self {
            payload: bytes,
            signature: signature.to_bytes().to_vec(),
        })
    }

    /// Verify the signature against the registry `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<RevocationPayload, DiscoveryError> {
        let payload: RevocationPayload = postcard::from_bytes(&self.payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let sig_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| DiscoveryError::InvalidRecord("signature is not 64 bytes".into()))?;
        key.verify_strict(&self.payload, &Signature::from_bytes(&sig_bytes))
            .map_err(|_| DiscoveryError::BadSignature)?;
        Ok(payload)
    }

    /// Verify signature, freshness and monotonic replacement — a new
    /// snapshot must be strictly newer than `current`.
    pub fn verify_fresh(
        &self,
        key: &VerifyingKey,
        current: Option<&RevocationPayload>,
    ) -> Result<RevocationPayload, DiscoveryError> {
        let payload = self.verify(key)?;
        if payload.expires_at < now_unix()? {
            return Err(DiscoveryError::Expired);
        }
        if let Some(cur) = current
            && payload.issued_at <= cur.issued_at
        {
            return Err(DiscoveryError::Stale);
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
            SignedRevocations::publish(&key, revoked, std::time::Duration::from_secs(60)).unwrap();
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
