//! Signed device-name registry — the public half of the GDS bridge.
//!
//! The private estate owns the mapping from device names to
//! [`EndpointKey`]s and signs it with the estate registry key. The
//! public module ships only the snapshot shape and verification: the
//! directory serves `name → key` lookups exclusively from a snapshot
//! whose signature chains to a configured verifying key. Nothing about
//! names is trustable without that signature — this keeps the estate's
//! inventory authority intact even though the directory itself is
//! public infrastructure.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{DiscoveryError, EndpointKey, now_unix};

/// Estate-signed name→key snapshot served by the directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRegistry {
    /// Encoded [`RegistryPayload`] bytes.
    pub payload: Vec<u8>,
    /// Ed25519 signature over `payload`, made by the registry key.
    pub signature: Vec<u8>,
}

/// Signed portion of a [`SignedRegistry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPayload {
    /// Name → endpoint key. Names are validated by the estate; the
    /// directory treats them as opaque labels (`[a-z0-9-]{1,63}`).
    pub entries: BTreeMap<String, EndpointKey>,
    /// Unix seconds when the estate issued this snapshot.
    pub issued_at: u64,
    /// Unix seconds after which the snapshot stops being served.
    pub expires_at: u64,
}

impl SignedRegistry {
    /// Sign a fresh snapshot: `entries` valid for `ttl`.
    pub fn publish(
        key: &SigningKey,
        entries: BTreeMap<String, EndpointKey>,
        ttl: std::time::Duration,
    ) -> Result<Self, DiscoveryError> {
        let issued_at = now_unix()?;
        Self::sign(
            &RegistryPayload {
                entries,
                issued_at,
                expires_at: issued_at + ttl.as_secs(),
            },
            key,
        )
    }

    /// Serialize and sign an already-built [`RegistryPayload`].
    pub fn sign(payload: &RegistryPayload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        let bytes = postcard::to_stdvec(payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let signature = key.sign(&bytes);
        Ok(Self {
            payload: bytes,
            signature: signature.to_bytes().to_vec(),
        })
    }

    /// Verify the signature against the configured registry `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<RegistryPayload, DiscoveryError> {
        let payload: RegistryPayload = postcard::from_bytes(&self.payload)
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

    /// Verify signature, freshness and monotonic replacement: a new
    /// snapshot must be strictly newer than `current`.
    pub fn verify_fresh(
        &self,
        key: &VerifyingKey,
        current: Option<&RegistryPayload>,
    ) -> Result<RegistryPayload, DiscoveryError> {
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

    fn entries() -> BTreeMap<String, EndpointKey> {
        BTreeMap::from([
            ("amsterdam".to_string(), EndpointKey([1; 32])),
            ("gds-services".to_string(), EndpointKey([2; 32])),
        ])
    }

    #[test]
    fn registry_roundtrip_and_name_lookup() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let snap =
            SignedRegistry::publish(&key, entries(), std::time::Duration::from_secs(600)).unwrap();
        let payload = snap
            .verify_fresh(&key.verifying_key(), None)
            .expect("fresh snapshot verifies");
        assert_eq!(payload.entries["amsterdam"], EndpointKey([1; 32]));
    }

    #[test]
    fn registry_forged_and_stale_rejected() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let wrong = SigningKey::from_bytes(&[4u8; 32]);
        let snap =
            SignedRegistry::publish(&key, entries(), std::time::Duration::from_secs(600)).unwrap();
        assert!(matches!(
            snap.verify(&wrong.verifying_key()),
            Err(DiscoveryError::BadSignature)
        ));
        let payload = snap.verify(&key.verifying_key()).unwrap();
        // Same-issued_at replacement is stale.
        assert!(matches!(
            snap.verify_fresh(&key.verifying_key(), Some(&payload)),
            Err(DiscoveryError::Stale)
        ));
    }
}
