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

use crate::authority::{self, SnapshotStamp};
use crate::{DiscoveryError, EndpointKey, now_unix};

const NAME_DOMAIN: &[u8] = b"rds/name-binding/v2\0";
const REGISTRY_DOMAIN: &[u8] = b"rds/registry/v2\0";
const MAX_BINDING_BYTES: usize = 256;
/// Upper bound on one registry's verification work and memory.
pub const MAX_REGISTRY_NAMES: usize = 256;
/// Names must be refreshed at least daily. Durable revision/rotation policy
/// is a separate concern from a signature's cryptographic validity.
pub const MAX_NAME_TTL_SECS: u64 = 24 * 60 * 60;

/// Proof disclosed for exactly one requested name, without the rest of the
/// inventory. The issuer creates it; the directory never has a signing key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedNameBinding {
    pub payload: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameBindingPayload {
    pub stamp: SnapshotStamp,
    /// Digest of the signed registry payload: all disclosed proofs from one
    /// revision must refer to the same snapshot without revealing its entries.
    pub registry_digest: [u8; 32],
    pub version: u16,
    pub name: String,
    pub key: EndpointKey,
    pub issued_at: u64,
    pub expires_at: u64,
}

impl SignedNameBinding {
    pub fn sign(payload: &NameBindingPayload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        payload.stamp.verify(&key.verifying_key())?;
        if payload.version != 2 || !valid_name(&payload.name) {
            return Err(invalid("invalid name binding version or name"));
        }
        let payload = postcard::to_stdvec(payload).map_err(|e| invalid(&e.to_string()))?;
        if payload.len() > MAX_BINDING_BYTES {
            return Err(invalid("name binding too large"));
        }
        let signature = key.sign(&domain_message(&payload)).to_bytes().to_vec();
        Ok(Self { payload, signature })
    }

    pub fn verify(
        &self,
        key: &VerifyingKey,
        name: &str,
        now: u64,
    ) -> Result<NameBindingPayload, DiscoveryError> {
        if !valid_name(name) || self.payload.len() > MAX_BINDING_BYTES {
            return Err(invalid("invalid name or binding size"));
        }
        let signature = signature(&self.signature)?;
        key.verify_strict(&domain_message(&self.payload), &signature)
            .map_err(|_| DiscoveryError::BadSignature)?;
        let payload: NameBindingPayload =
            postcard::from_bytes(&self.payload).map_err(|e| invalid(&e.to_string()))?;
        payload.stamp.verify(key)?;
        if payload.version != 2 || payload.name != name {
            return Err(invalid("name binding mismatch"));
        }
        check_lifetime(payload.issued_at, payload.expires_at, now)?;
        Ok(payload)
    }

    pub(crate) fn verify_committed(
        &self,
        key: &VerifyingKey,
    ) -> Result<NameBindingPayload, DiscoveryError> {
        authority::verify(
            NAME_DOMAIN,
            &self.payload,
            &self.signature,
            key,
            MAX_BINDING_BYTES,
        )?;
        let payload: NameBindingPayload =
            postcard::from_bytes(&self.payload).map_err(|e| invalid(&e.to_string()))?;
        self.verify(key, &payload.name, payload.issued_at)
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(crate) fn check_lifetime(issued: u64, expires: u64, now: u64) -> Result<(), DiscoveryError> {
    authority::lifetime(issued, expires, now, MAX_NAME_TTL_SECS)
}

fn invalid(message: &str) -> DiscoveryError {
    DiscoveryError::InvalidRecord(message.into())
}
fn signature(bytes: &[u8]) -> Result<Signature, DiscoveryError> {
    let bytes = bytes
        .try_into()
        .map_err(|_| invalid("signature is not 64 bytes"))?;
    Ok(Signature::from_bytes(&bytes))
}
fn domain_message(payload: &[u8]) -> Vec<u8> {
    [NAME_DOMAIN, payload].concat()
}

/// Estate-signed name→key snapshot served by the directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRegistry {
    /// Encoded [`RegistryPayload`] bytes.
    pub payload: Vec<u8>,
    /// Ed25519 signature over `payload`, made by the registry key.
    pub signature: Vec<u8>,
    /// Individually verifiable proofs; required for every entry. Legacy
    /// snapshots deserialize but fail verification until re-signed.
    #[serde(default)]
    pub bindings: BTreeMap<String, SignedNameBinding>,
}

/// Signed portion of a [`SignedRegistry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryPayload {
    pub stamp: SnapshotStamp,
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
        epoch: u64,
        revision: u64,
        entries: BTreeMap<String, EndpointKey>,
        ttl: std::time::Duration,
    ) -> Result<Self, DiscoveryError> {
        let issued_at = now_unix()?;
        Self::sign(
            &RegistryPayload {
                stamp: SnapshotStamp::new(&key.verifying_key(), epoch, revision)?,
                entries,
                issued_at,
                expires_at: issued_at
                    .checked_add(ttl.as_secs())
                    .ok_or_else(|| invalid("TTL overflow"))?,
            },
            key,
        )
    }

    /// Serialize and sign an already-built [`RegistryPayload`].
    pub fn sign(payload: &RegistryPayload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        payload.stamp.verify(&key.verifying_key())?;
        if payload.entries.len() > MAX_REGISTRY_NAMES {
            return Err(invalid("too many registry names"));
        }
        let bytes = postcard::to_stdvec(payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let registry_digest = *blake3::hash(&bytes).as_bytes();
        let bindings = payload
            .entries
            .iter()
            .map(|(name, endpoint)| {
                Ok((
                    name.clone(),
                    SignedNameBinding::sign(
                        &NameBindingPayload {
                            stamp: payload.stamp,
                            registry_digest,
                            version: 2,
                            name: name.clone(),
                            key: *endpoint,
                            issued_at: payload.issued_at,
                            expires_at: payload.expires_at,
                        },
                        key,
                    )?,
                ))
            })
            .collect::<Result<_, DiscoveryError>>()?;
        let signature = authority::sign(REGISTRY_DOMAIN, &bytes, key);
        let signed = Self {
            payload: bytes,
            signature,
            bindings,
        };
        if serde_json::to_vec(&signed)
            .map_err(|e| invalid(&e.to_string()))?
            .len()
            > crate::http::MAX_BODY
        {
            return Err(invalid("registry exceeds wire body limit"));
        }
        Ok(signed)
    }

    /// Verify the signature against the configured registry `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<RegistryPayload, DiscoveryError> {
        if self.payload.len() > crate::http::MAX_BODY || self.bindings.len() > MAX_REGISTRY_NAMES {
            return Err(invalid("registry too large"));
        }
        authority::verify(
            REGISTRY_DOMAIN,
            &self.payload,
            &self.signature,
            key,
            crate::http::MAX_BODY,
        )?;
        let payload: RegistryPayload = postcard::from_bytes(&self.payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        payload.stamp.verify(key)?;
        check_lifetime(payload.issued_at, payload.expires_at, payload.issued_at)?;
        if payload.entries.len() != self.bindings.len() {
            return Err(invalid("missing name proofs: re-sign registry"));
        }
        let registry_digest = *blake3::hash(&self.payload).as_bytes();
        for (name, endpoint) in &payload.entries {
            let proof = self
                .bindings
                .get(name)
                .ok_or_else(|| invalid("missing name proof"))?;
            // Check the interval at issuance; wall-clock freshness is checked
            // by verify_fresh and again on each GET, never only on PUT.
            let binding = proof.verify(key, name, payload.issued_at)?;
            if binding.key != *endpoint
                || binding.stamp != payload.stamp
                || binding.registry_digest != registry_digest
                || binding.issued_at != payload.issued_at
                || binding.expires_at != payload.expires_at
            {
                return Err(invalid("registry and name proof disagree"));
            }
        }
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
        check_lifetime(payload.issued_at, payload.expires_at, now_unix()?)?;
        if let Some(cur) = current {
            payload.stamp.newer_than(&cur.stamp)?;
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> BTreeMap<String, EndpointKey> {
        BTreeMap::from([
            ("device-alpha".to_string(), EndpointKey([1; 32])),
            ("device-beta".to_string(), EndpointKey([2; 32])),
        ])
    }

    #[test]
    fn registry_roundtrip_and_name_lookup() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let snap =
            SignedRegistry::publish(&key, 1, 1, entries(), std::time::Duration::from_secs(600))
                .unwrap();
        let payload = snap
            .verify_fresh(&key.verifying_key(), None)
            .expect("fresh snapshot verifies");
        assert_eq!(payload.entries["device-alpha"], EndpointKey([1; 32]));
    }

    #[test]
    fn registry_forged_and_stale_rejected() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let wrong = SigningKey::from_bytes(&[4u8; 32]);
        let snap =
            SignedRegistry::publish(&key, 1, 1, entries(), std::time::Duration::from_secs(600))
                .unwrap();
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
