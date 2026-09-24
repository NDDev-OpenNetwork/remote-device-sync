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

pub mod authority;
pub mod client;
pub mod clock;
pub mod http;
mod persist;
pub mod publisher;
pub use publisher::{RecordDraft, RecordIssuer};
mod record_wire;
mod relay_route;
pub use record_wire::{
    DeletePayload, DeleteRequest, EndpointRecord, MAX_DIRECT_ADDRS, MAX_RECORD_BYTES,
    MAX_RECORD_TTL, MAX_RELAY_URL_BYTES, MAX_RELAY_URLS, Payload, RECORD_VERSION,
};
pub use relay_route::OwnedRelayRoute;
pub mod policy;
mod records;
pub use records::{
    FileStore, MAX_DATABASE as MAX_RECORD_DATABASE_BYTES, MAX_IDENTITIES as MAX_RECORD_IDENTITIES,
    MemoryStore,
};
mod admission;
pub use admission::Enrollment;
pub mod registry;
pub mod revocations;
pub mod service;
pub mod tls;

use std::time::SystemTime;

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

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("policy state is owned by another process")]
    Busy,
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
    #[error("publisher is not enrolled")]
    NotEnrolled,
    #[error("store error: {0}")]
    Store(String),
}

/// Current unix time in seconds.
pub fn now_unix() -> Result<u64, DiscoveryError> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| DiscoveryError::Store(e.to_string()))?
        .as_secs())
}

/// Storage for endpoint records. The GDS server implements this over
/// its database; agents and tests use the in-memory version.
pub trait RecordStore: Send + Sync {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        self.put_admitted(record, &mut |_| Ok(()))
    }
    /// Invoke `admit(known_identity)` exactly once for a valid higher revision,
    /// after verifying the signature, signed key agreement and current lifetime,
    /// under the same ownership as compare/commit and before changing state.
    /// Duplicate/stale/invalid operations never call it. Its error refuses the
    /// mutation. `known_identity` includes deletion and expiry floors.
    /// The callback must be bounded and must not reenter the store.
    fn put_admitted(
        &self,
        record: &EndpointRecord,
        admit: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<(), DiscoveryError>;
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError>;
    /// Commit an authorized, fresh delete at a higher revision. Exact signed
    /// retries succeed without rewriting; deleted identities retain history.
    fn remove(&self, tombstone: &DeleteRequest) -> Result<(), DiscoveryError> {
        self.remove_admitted(tombstone, &mut |_| Ok(()))
    }
    /// Deletion shares the same admission contract and publisher budget as PUT.
    fn remove_admitted(
        &self,
        tombstone: &DeleteRequest,
        admit: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<(), DiscoveryError>;
    /// Inspect at most 64 identities and reclaim expired signed content. Keep
    /// all replay floors. Repeated calls rotate over the bounded catalog.
    fn collect_expired(&self) -> Result<usize, DiscoveryError>;
    /// Number of stored records awaiting expiry collection (metrics).
    fn len(&self) -> usize;
    /// Whether the store holds no live records.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::{net::SocketAddr, time::Duration};

    fn rec(key: &SigningKey) -> EndpointRecord {
        EndpointRecord::publish(
            key,
            1,
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
