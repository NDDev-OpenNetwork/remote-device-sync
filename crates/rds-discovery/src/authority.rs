//! Shared authority, revision and lifetime rules for signed policy streams.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{DiscoveryError, EndpointKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authority {
    pub key: EndpointKey,
    pub epoch: u64,
}

impl Authority {
    pub fn from_base32(value: &str, epoch: u64) -> Result<Self, DiscoveryError> {
        let key: EndpointKey = value.parse()?;
        let verifying = VerifyingKey::from_bytes(&key.0).map_err(|e| invalid(&e.to_string()))?;
        Self::new(&verifying, epoch)
    }
    pub fn new(key: &VerifyingKey, epoch: u64) -> Result<Self, DiscoveryError> {
        if epoch == 0 {
            return Err(invalid("authority epoch must be positive"));
        }
        Ok(Self {
            key: EndpointKey(key.to_bytes()),
            epoch,
        })
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey, DiscoveryError> {
        if self.epoch == 0 {
            return Err(invalid("authority epoch must be positive"));
        }
        VerifyingKey::from_bytes(&self.key.0).map_err(|e| invalid(&e.to_string()))
    }
}

/// Revisions are assigned durably by the issuer, independently for each
/// policy stream. A clock timestamp is never a revision allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotStamp {
    pub authority: Authority,
    pub revision: u64,
}

impl SnapshotStamp {
    pub fn new(key: &VerifyingKey, epoch: u64, revision: u64) -> Result<Self, DiscoveryError> {
        let stamp = Self {
            authority: Authority::new(key, epoch)?,
            revision,
        };
        stamp.verify(key)?;
        Ok(stamp)
    }

    pub fn verify(&self, key: &VerifyingKey) -> Result<(), DiscoveryError> {
        if self.revision == 0 || self.authority.epoch == 0 {
            return Err(invalid("snapshot epoch and revision must be positive"));
        }
        if self.authority.key.0 != key.to_bytes() {
            return Err(DiscoveryError::BadSignature);
        }
        Ok(())
    }

    /// Crossing authority epochs requires an authenticated rotation, never an
    /// arbitrary larger number in a snapshot response.
    pub fn newer_than(&self, previous: &Self) -> Result<(), DiscoveryError> {
        if self.authority != previous.authority || self.revision <= previous.revision {
            return Err(DiscoveryError::Stale);
        }
        Ok(())
    }
}

pub(crate) fn invalid(message: &str) -> DiscoveryError {
    DiscoveryError::InvalidRecord(message.into())
}

pub(crate) fn lifetime(
    issued: u64,
    expires: u64,
    now: u64,
    max_ttl: u64,
) -> Result<(), DiscoveryError> {
    if issued > now || expires <= issued || expires - issued > max_ttl {
        return Err(invalid("invalid policy validity interval"));
    }
    if now >= expires {
        return Err(DiscoveryError::Expired);
    }
    Ok(())
}

pub(crate) fn sign(domain: &[u8], payload: &[u8], key: &SigningKey) -> Vec<u8> {
    key.sign(&[domain, payload].concat()).to_bytes().to_vec()
}

pub(crate) fn verify(
    domain: &[u8],
    payload: &[u8],
    signature: &[u8],
    key: &VerifyingKey,
    limit: usize,
) -> Result<(), DiscoveryError> {
    if payload.len() > limit {
        return Err(invalid("signed payload exceeds limit"));
    }
    let bytes = signature
        .try_into()
        .map_err(|_| invalid("signature is not 64 bytes"))?;
    key.verify_strict(&[domain, payload].concat(), &Signature::from_bytes(&bytes))
        .map_err(|_| DiscoveryError::BadSignature)
}

const ROTATION_DOMAIN: &[u8] = b"rds/authority-rotation/v1\0";
const MAX_ROTATION_TTL: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRotation {
    pub payload: Vec<u8>,
    pub previous_signature: Vec<u8>,
    pub next_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationPayload {
    pub previous: Authority,
    pub next: Authority,
    pub issued_at: u64,
    pub expires_at: u64,
}

impl SignedRotation {
    /// Both authorities approve the same transition. Provision the receipt
    /// through configuration; an untrusted new key alone cannot rotate trust.
    pub fn sign(
        payload: &RotationPayload,
        previous: &SigningKey,
        next: &SigningKey,
    ) -> Result<Self, DiscoveryError> {
        let bytes = postcard::to_stdvec(payload).map_err(|e| invalid(&e.to_string()))?;
        let signed = Self {
            previous_signature: sign(ROTATION_DOMAIN, &bytes, previous),
            next_signature: sign(ROTATION_DOMAIN, &bytes, next),
            payload: bytes,
        };
        signed.verify_at(payload.previous, payload.issued_at)?;
        Ok(signed)
    }

    pub fn verify_at(
        &self,
        current: Authority,
        now: u64,
    ) -> Result<RotationPayload, DiscoveryError> {
        verify(
            ROTATION_DOMAIN,
            &self.payload,
            &self.previous_signature,
            &current.verifying_key()?,
            1024,
        )?;
        let payload: RotationPayload =
            postcard::from_bytes(&self.payload).map_err(|e| invalid(&e.to_string()))?;
        if payload.previous != current
            || current.epoch.checked_add(1) != Some(payload.next.epoch)
            || payload.next.key == current.key
        {
            return Err(invalid("invalid authority transition"));
        }
        verify(
            ROTATION_DOMAIN,
            &self.payload,
            &self.next_signature,
            &payload.next.verifying_key()?,
            1024,
        )?;
        lifetime(payload.issued_at, payload.expires_at, now, MAX_ROTATION_TTL)?;
        Ok(payload)
    }

    /// A transition committed while valid remains authoritative after its
    /// delivery window expires. Used only when revalidating protected state.
    pub(crate) fn verify_committed(
        &self,
        current: Authority,
    ) -> Result<RotationPayload, DiscoveryError> {
        verify(
            ROTATION_DOMAIN,
            &self.payload,
            &self.previous_signature,
            &current.verifying_key()?,
            1024,
        )?;
        let payload: RotationPayload =
            postcard::from_bytes(&self.payload).map_err(|e| invalid(&e.to_string()))?;
        self.verify_at(current, payload.issued_at)
    }
}
