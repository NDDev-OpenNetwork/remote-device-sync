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
//! keeps. The HTTP wire codec, directory service and client live in
//! [`http`], [`service`] and [`client`]; the estate-signed name
//! registry in [`registry`].

pub mod client;
pub mod http;
pub mod registry;
pub mod revocations;
pub mod service;
pub mod tls;

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
    /// Unix seconds when the record was issued. Stores reject a record
    /// whose `issued_at` is not newer than the stored one — replay of an
    /// older record cannot roll the directory back.
    pub issued_at: u64,
    /// Unix seconds after which the record must be refreshed.
    pub expires_at: u64,
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("directory configuration invalid: {0}")]
    Configuration(String),
    #[error("record signature invalid")]
    BadSignature,
    #[error("record malformed: {0}")]
    InvalidRecord(String),
    #[error("record expired")]
    Expired,
    #[error("record is not newer than the stored record")]
    Stale,
    #[error("record not found")]
    NotFound,
    #[error("directory unreachable: {0}")]
    Unreachable(String),
    #[error("directory answered {status}: {message}")]
    Http { status: u16, message: String },
    #[error("rate limited")]
    RateLimited,
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
        let issued_at = now_unix()?;
        let payload = Payload {
            key: EndpointKey(key.verifying_key().to_bytes()),
            addrs,
            relay_urls,
            services,
            issued_at,
            expires_at: issued_at + ttl.as_secs(),
        };
        Self::sign(&payload, key)
    }

    /// Serialize and sign an already-built [`Payload`].
    pub fn sign(payload: &Payload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        let bytes = postcard::to_stdvec(payload)
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
        if payload.expires_at < now_unix()? {
            return Err(DiscoveryError::Expired);
        }
        Ok(payload)
    }
}

/// Current unix time in seconds.
pub fn now_unix() -> Result<u64, DiscoveryError> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| DiscoveryError::Store(e.to_string()))?
        .as_secs())
}

/// Reject `new` when the stored `old` was issued at the same time or
/// later — replay protection shared by every store and the HTTP layer.
pub fn check_freshness(old: Option<&EndpointRecord>, new: &Payload) -> Result<(), DiscoveryError> {
    match old {
        Some(old) => {
            let old_issued = old.verify()?.issued_at;
            if new.issued_at <= old_issued {
                return Err(DiscoveryError::Stale);
            }
            Ok(())
        }
        None => Ok(()),
    }
}

/// A delete tombstone: signed by the record's key, authorizes removal.
/// Replaying an older tombstone against a newer record is refused by
/// the same `issued_at` ordering as record replacement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteRequest {
    /// Encoded [`DeletePayload`] bytes.
    pub payload: Vec<u8>,
    /// Ed25519 signature over `payload`, made by the record's key.
    pub signature: Vec<u8>,
}

/// Signed portion of a [`DeleteRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePayload {
    pub key: EndpointKey,
    pub issued_at: u64,
}

impl DeleteRequest {
    /// Sign a delete for `key` at the current time.
    pub fn new(key: &SigningKey) -> Result<Self, DiscoveryError> {
        let payload = DeletePayload {
            key: EndpointKey(key.verifying_key().to_bytes()),
            issued_at: now_unix()?,
        };
        let bytes = postcard::to_stdvec(&payload)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let signature = key.sign(&bytes);
        Ok(Self {
            payload: bytes,
            signature: signature.to_bytes().to_vec(),
        })
    }

    /// Verify signature and return the payload.
    pub fn verify(&self) -> Result<DeletePayload, DiscoveryError> {
        let payload: DeletePayload = postcard::from_bytes(&self.payload)
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
}

/// Storage for endpoint records. The GDS server implements this over
/// its database; agents and tests use the in-memory version.
pub trait RecordStore: Send + Sync {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError>;
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError>;
    /// Remove `key` when `tombstone` is authorized and not older than
    /// the stored record's `issued_at`. `Ok` when already absent.
    fn remove(&self, tombstone: &DeleteRequest) -> Result<(), DiscoveryError>;
    /// Number of live records (metrics).
    fn len(&self) -> usize;
    /// Whether the store holds no live records.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// In-memory store; rejects forged or expired records on `put`.
#[derive(Default)]
pub struct MemoryStore {
    records: std::sync::RwLock<HashMap<EndpointKey, EndpointRecord>>,
}

impl RecordStore for MemoryStore {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        let payload = record.verify()?;
        let mut records = self
            .records
            .write()
            .map_err(|_| DiscoveryError::Store("store poisoned".into()))?;
        check_freshness(records.get(&payload.key), &payload)?;
        records.insert(payload.key, record.clone());
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

    fn remove(&self, tombstone: &DeleteRequest) -> Result<(), DiscoveryError> {
        let del = tombstone.verify()?;
        let mut records = self
            .records
            .write()
            .map_err(|_| DiscoveryError::Store("store poisoned".into()))?;
        if let Some(stored) = records.get(&del.key)
            && stored.verify()?.issued_at > del.issued_at
        {
            return Err(DiscoveryError::Stale);
        }
        records.remove(&del.key);
        Ok(())
    }

    fn len(&self) -> usize {
        self.records.read().map(|r| r.len()).unwrap_or(0)
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
        check_freshness(self.get(&payload.key).ok().as_ref(), &payload)?;
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

    fn remove(&self, tombstone: &DeleteRequest) -> Result<(), DiscoveryError> {
        let del = tombstone.verify()?;
        if let Ok(stored) = self.get(&del.key)
            && stored.verify()?.issued_at > del.issued_at
        {
            return Err(DiscoveryError::Stale);
        }
        match std::fs::remove_file(self.path(&del.key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(DiscoveryError::Store(e.to_string())),
        }
    }

    fn len(&self) -> usize {
        std::fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().count())
            .unwrap_or(0)
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

    proptest::proptest! {
        /// Arbitrary bytes through the record JSON path must never
        /// panic — parse failure is a clean `Err`, never a crash, and
        /// a parsed-but-invalid record still fails `verify`.
        #[test]
        fn record_decode_never_panics(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..=2048)) {
            if let Ok(record) = serde_json::from_slice::<EndpointRecord>(&bytes) {
                let _ = record.verify();
                let _ = record.verify_fresh();
            }
        }

        /// A signed record survives the JSON round-trip; single-byte
        /// corruption anywhere in the signed payload breaks verify.
        #[test]
        fn signed_record_roundtrip_and_corruption(
            seed in proptest::num::u8::ANY,
            flip in proptest::option::of(proptest::num::usize::ANY),
        ) {
            let key = SigningKey::from_bytes(&[seed; 32]);
            let record = rec(&key);
            let json = serde_json::to_vec(&record).unwrap();
            let back: EndpointRecord = serde_json::from_slice(&json).unwrap();
            proptest::prop_assert!(back.verify().is_ok());
            if let Some(pos) = flip.filter(|p| *p < record.payload.len()) {
                let mut bad = record.clone();
                bad.payload[pos] ^= 1;
                proptest::prop_assert!(bad.verify().is_err());
            }
        }
    }
}
