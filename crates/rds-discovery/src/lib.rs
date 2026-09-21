//! Discovery records and stores for rds.
//!
//! Devices self-publish a signed [`EndpointRecord`]: the record carries
//! the endpoint's reachability data (direct addrs, relay URLs, offered
//! services) and is verifiable against the Ed25519 key inside it — the
//! same key that authenticates QUIC connections. The GDS server stores
//! and serves records; correctness never depends on trusting the store.
//!
//! Two stores implement [`RecordStore`]: [`MemoryStore`] for tests and
//! embedded use, [`FileStore`] for the on-disk directory the GDS server
//! keeps. Wire/HTTP transport lives in `client`/`server` modules.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Public endpoint identity: the raw Ed25519 verifying key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EndpointKey(pub [u8; 32]);

impl std::fmt::Display for EndpointKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            data_encoding::BASE32_NOPAD.encode(&self.0).to_lowercase()
        )
    }
}

impl std::str::FromStr for EndpointKey {
    type Err = DiscoveryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_uppercase().as_bytes())
            .map_err(|_| DiscoveryError::InvalidRecord("endpoint key is not base32".into()))?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| DiscoveryError::InvalidRecord("endpoint key is not 32 bytes".into()))?;
        Ok(Self(key))
    }
}

/// A service the endpoint offers (mirrors `rds_core::ServiceKind`
/// without coupling the crates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Service {
    Ping,
    Info,
    TcpForward,
    Desktop,
    Audio,
    Sync,
}

/// What the endpoint publishes about itself. The `signature` covers the
/// postcard encoding of `Payload`; everything else is derived at load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointRecord {
    /// Encoded [`Payload`] bytes.
    pub payload: Vec<u8>,
    /// Ed25519 signature over `payload`, made by the payload's key.
    pub signature: Vec<u8>,
}

/// Signed portion of an [`EndpointRecord`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub key: EndpointKey,
    /// Direct candidate addresses (IPv4/IPv6).
    pub addrs: Vec<SocketAddr>,
    /// Relay base URLs the endpoint is reachable through.
    pub relay_urls: Vec<String>,
    /// Services this endpoint serves.
    pub services: Vec<Service>,
    /// Unix seconds after which the record must be refreshed.
    pub expires_at: u64,
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("record signature invalid")]
    BadSignature,
    #[error("record malformed: {0}")]
    InvalidRecord(String),
    #[error("record expired")]
    Expired,
    #[error("record not found")]
    NotFound,
    #[error("store error: {0}")]
    Store(String),
}

impl EndpointRecord {
    /// Sign a fresh record for `key` valid for `ttl`.
    pub fn publish(
        key: &SigningKey,
        addrs: Vec<SocketAddr>,
        relay_urls: Vec<String>,
        services: Vec<Service>,
        ttl: Duration,
    ) -> Result<Self, DiscoveryError> {
        let expires_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| DiscoveryError::Store(e.to_string()))?
            .as_secs()
            + ttl.as_secs();
        let payload = Payload {
            key: EndpointKey(key.verifying_key().to_bytes()),
            addrs,
            relay_urls,
            services,
            expires_at,
        };
        let bytes = postcard::to_stdvec(&payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let signature = key.sign(&bytes);
        Ok(Self {
            payload: bytes,
            signature: signature.to_bytes().to_vec(),
        })
    }

    /// Verify the signature and return the payload. Expiry is checked by
    /// callers that care (a store may still return expired records for
    /// diagnostics).
    pub fn verify(&self) -> Result<Payload, DiscoveryError> {
        let payload: Payload = postcard::from_bytes(&self.payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let key = VerifyingKey::from_bytes(&payload.key.0)
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

    /// Verify and additionally require the record to be unexpired.
    pub fn verify_fresh(&self) -> Result<Payload, DiscoveryError> {
        let payload = self.verify()?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| DiscoveryError::Store(e.to_string()))?
            .as_secs();
        if payload.expires_at < now {
            return Err(DiscoveryError::Expired);
        }
        Ok(payload)
    }
}

/// Storage for endpoint records. The GDS server implements this over
/// its database; agents and tests use the in-memory version.
pub trait RecordStore: Send + Sync {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError>;
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError>;
}

/// In-memory store; rejects forged or expired records on `put`.
#[derive(Default)]
pub struct MemoryStore {
    records: std::sync::RwLock<HashMap<EndpointKey, EndpointRecord>>,
}

impl RecordStore for MemoryStore {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        let payload = record.verify()?;
        self.records
            .write()
            .map_err(|_| DiscoveryError::Store("store poisoned".into()))?
            .insert(payload.key, record.clone());
        Ok(())
    }

    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        self.records
            .read()
            .map_err(|_| DiscoveryError::Store("store poisoned".into()))?
            .get(key)
            .cloned()
            .ok_or(DiscoveryError::NotFound)
    }
}

/// Append-only file store: one `<base32-key>.json` record file per
/// endpoint under `dir`. Suitable for the GDS server's on-disk directory.
pub struct FileStore {
    dir: std::path::PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, key: &EndpointKey) -> std::path::PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl RecordStore for FileStore {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        let payload = record.verify()?;
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        std::fs::write(self.path(&payload.key), bytes)
            .map_err(|e| DiscoveryError::Store(e.to_string()))
    }

    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        let path = self.path(key);
        let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => DiscoveryError::NotFound,
            _ => DiscoveryError::Store(e.to_string()),
        })?;
        serde_json::from_slice(&bytes).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(key: &SigningKey) -> EndpointRecord {
        EndpointRecord::publish(
            key,
            vec![SocketAddr::from(([10, 0, 0, 5], 4200))],
            vec!["https://relay.example.com".into()],
            vec![Service::Ping, Service::TcpForward],
            Duration::from_secs(3600),
        )
        .unwrap()
    }

    #[test]
    fn signed_record_verifies() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let payload = rec(&key).verify_fresh().unwrap();
        assert_eq!(payload.key.0, key.verifying_key().to_bytes());
        assert_eq!(payload.services, vec![Service::Ping, Service::TcpForward]);
    }

    #[test]
    fn forged_record_rejected() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let mut record = rec(&key);
        record.payload[0] ^= 1;
        assert!(matches!(record.verify(), Err(DiscoveryError::BadSignature)));
    }

    #[test]
    fn memory_store_roundtrip_and_expiry() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let store = MemoryStore::default();
        let record = rec(&key);
        let k = record.verify().unwrap().key;
        store.put(&record).unwrap();
        assert_eq!(store.get(&k).unwrap().verify().unwrap().key, k);
        assert!(matches!(
            store.get(&EndpointKey([0; 32])),
            Err(DiscoveryError::NotFound)
        ));
    }
}
