//! Versioned, domain-separated endpoint mutations. Revisions belong to the
//! publisher identity and are allocated durably, independently of wall time.
use crate::{DiscoveryError, EndpointKey, Service, authority, now_unix};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, de::Visitor};
use std::{fmt, marker::PhantomData, net::SocketAddr, time::Duration};

pub const RECORD_VERSION: u16 = 1;
pub const MAX_RECORD_TTL: u64 = 3600;
pub const DELETE_TTL: u64 = 300;
pub const MAX_RECORD_BYTES: usize = 8192;
pub const MAX_DIRECT_ADDRS: usize = 32;
pub const MAX_RELAY_URLS: usize = 8;
pub const MAX_RELAY_URL_BYTES: usize = 512;
const RECORD_DOMAIN: &[u8] = b"rds/endpoint-record/v1\0";
const DELETE_DOMAIN: &[u8] = b"rds/endpoint-delete/v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointRecord {
    /// Untrusted verification hint; the signed payload must contain this key.
    pub key: EndpointKey,
    #[serde(deserialize_with = "limited::<_, u8, MAX_RECORD_BYTES>")]
    pub payload: Vec<u8>,
    #[serde(deserialize_with = "limited::<_, u8, 64>")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub version: u16,
    pub key: EndpointKey,
    pub revision: u64,
    #[serde(deserialize_with = "limited::<_, SocketAddr, MAX_DIRECT_ADDRS>")]
    pub addrs: Vec<SocketAddr>,
    #[serde(deserialize_with = "limited::<_, String, MAX_RELAY_URLS>")]
    pub relay_urls: Vec<String>,
    #[serde(deserialize_with = "limited::<_, Service, 6>")]
    pub services: Vec<Service>,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub key: EndpointKey,
    #[serde(deserialize_with = "limited::<_, u8, 128>")]
    pub payload: Vec<u8>,
    #[serde(deserialize_with = "limited::<_, u8, 64>")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePayload {
    pub version: u16,
    pub key: EndpointKey,
    pub revision: u64,
    pub issued_at: u64,
    pub expires_at: u64,
}

fn invalid(message: &str) -> DiscoveryError {
    authority::invalid(message)
}

/// Do not trust a sequence length hint, even in a signed payload. Reject before
/// reserving attacker-selected capacity, then stop at the explicit element cap.
fn limited<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    struct Bounded<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for Bounded<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "at most {N} elements")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|size| size > N) {
                return Err(serde::de::Error::custom("sequence exceeds limit"));
            }
            let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(N));
            while let Some(value) = seq.next_element()? {
                if values.len() == N {
                    return Err(serde::de::Error::custom("sequence exceeds limit"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, N>(PhantomData))
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, DiscoveryError> {
    let (value, rest) = postcard::take_from_bytes(bytes).map_err(|e| invalid(&e.to_string()))?;
    if !rest.is_empty() {
        return Err(invalid("trailing signed record bytes"));
    }
    Ok(value)
}

fn metadata(
    version: u16,
    revision: u64,
    issued: u64,
    expires: u64,
    ttl: u64,
) -> Result<(), DiscoveryError> {
    if version != RECORD_VERSION || revision == 0 {
        return Err(invalid("unsupported record version or zero revision"));
    }
    if expires <= issued || expires - issued > ttl {
        return Err(invalid("invalid record validity interval"));
    }
    Ok(())
}

impl Payload {
    fn validate(&self) -> Result<(), DiscoveryError> {
        metadata(
            self.version,
            self.revision,
            self.issued_at,
            self.expires_at,
            MAX_RECORD_TTL,
        )?;
        if self.addrs.len() > MAX_DIRECT_ADDRS
            || self.relay_urls.len() > MAX_RELAY_URLS
            || self.services.len() > 6
        {
            return Err(invalid("record collection exceeds limit"));
        }
        for (i, addr) in self.addrs.iter().enumerate() {
            if addr.port() == 0
                || addr.ip().is_unspecified()
                || addr.ip().is_multicast()
                || self.addrs[..i].contains(addr)
            {
                return Err(invalid("invalid or repeated direct address"));
            }
        }
        for (i, raw) in self.relay_urls.iter().enumerate() {
            if raw.len() > MAX_RELAY_URL_BYTES || self.relay_urls[..i].contains(raw) {
                return Err(invalid("oversized or repeated relay URL"));
            }
            let url = url::Url::parse(raw).map_err(|_| invalid("invalid relay URL"))?;
            if url.scheme() == "rds-relay" {
                raw.parse::<crate::OwnedRelayRoute>()?;
                continue;
            }
            if !matches!(url.scheme(), "http" | "https")
                || url.host().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
                || url.port_or_known_default() == Some(0)
            {
                return Err(invalid(
                    "relay must be an HTTP(S) origin without credentials",
                ));
            }
        }
        for (i, service) in self.services.iter().enumerate() {
            if self.services[..i].contains(service) {
                return Err(invalid("repeated service"));
            }
        }
        Ok(())
    }
}

impl EndpointRecord {
    /// The caller owns durable revision allocation; use RecordIssuer in agents.
    pub fn publish(
        key: &SigningKey,
        revision: u64,
        addrs: Vec<SocketAddr>,
        relay_urls: Vec<String>,
        services: Vec<Service>,
        ttl: Duration,
    ) -> Result<Self, DiscoveryError> {
        if ttl.subsec_nanos() != 0 {
            return Err(invalid("record TTL requires whole seconds"));
        }
        let now = now_unix()?;
        Self::sign(
            &Payload {
                version: RECORD_VERSION,
                key: EndpointKey(key.verifying_key().to_bytes()),
                revision,
                addrs,
                relay_urls,
                services,
                issued_at: now,
                expires_at: now
                    .checked_add(ttl.as_secs())
                    .ok_or_else(|| invalid("TTL overflow"))?,
            },
            key,
        )
    }

    pub fn sign(payload: &Payload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        payload.validate()?;
        if payload.key.0 != key.verifying_key().to_bytes() {
            return Err(DiscoveryError::BadSignature);
        }
        let bytes = postcard::to_stdvec(payload).map_err(|e| invalid(&e.to_string()))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(invalid("record exceeds wire limit"));
        }
        Ok(Self {
            key: payload.key,
            signature: authority::sign(RECORD_DOMAIN, &bytes, key),
            payload: bytes,
        })
    }

    /// Verify bounded signature bytes before decoding variable-length fields.
    /// Does not check current freshness, so expired signed history stays usable.
    pub fn verify(&self) -> Result<Payload, DiscoveryError> {
        let key =
            VerifyingKey::from_bytes(&self.key.0).map_err(|_| DiscoveryError::BadSignature)?;
        authority::verify(
            RECORD_DOMAIN,
            &self.payload,
            &self.signature,
            &key,
            MAX_RECORD_BYTES,
        )?;
        let payload: Payload = decode(&self.payload)?;
        if payload.key != self.key {
            return Err(DiscoveryError::BadSignature);
        }
        payload.validate()?;
        Ok(payload)
    }

    pub fn verify_fresh_at(&self, now: u64) -> Result<Payload, DiscoveryError> {
        let payload = self.verify()?;
        authority::lifetime(payload.issued_at, payload.expires_at, now, MAX_RECORD_TTL)?;
        Ok(payload)
    }
    pub fn verify_fresh(&self) -> Result<Payload, DiscoveryError> {
        self.verify_fresh_at(now_unix()?)
    }
}

impl DeleteRequest {
    pub fn new(key: &SigningKey, revision: u64) -> Result<Self, DiscoveryError> {
        let now = now_unix()?;
        Self::sign(
            &DeletePayload {
                version: RECORD_VERSION,
                key: EndpointKey(key.verifying_key().to_bytes()),
                revision,
                issued_at: now,
                expires_at: now
                    .checked_add(DELETE_TTL)
                    .ok_or_else(|| invalid("TTL overflow"))?,
            },
            key,
        )
    }
    pub fn sign(payload: &DeletePayload, key: &SigningKey) -> Result<Self, DiscoveryError> {
        metadata(
            payload.version,
            payload.revision,
            payload.issued_at,
            payload.expires_at,
            DELETE_TTL,
        )?;
        if payload.key.0 != key.verifying_key().to_bytes() {
            return Err(DiscoveryError::BadSignature);
        }
        let bytes = postcard::to_stdvec(payload).map_err(|e| invalid(&e.to_string()))?;
        Ok(Self {
            key: payload.key,
            signature: authority::sign(DELETE_DOMAIN, &bytes, key),
            payload: bytes,
        })
    }
    pub fn verify(&self) -> Result<DeletePayload, DiscoveryError> {
        let key =
            VerifyingKey::from_bytes(&self.key.0).map_err(|_| DiscoveryError::BadSignature)?;
        authority::verify(DELETE_DOMAIN, &self.payload, &self.signature, &key, 128)?;
        let payload: DeletePayload = decode(&self.payload)?;
        if payload.key != self.key {
            return Err(DiscoveryError::BadSignature);
        }
        metadata(
            payload.version,
            payload.revision,
            payload.issued_at,
            payload.expires_at,
            DELETE_TTL,
        )?;
        Ok(payload)
    }
    pub fn verify_fresh_at(&self, now: u64) -> Result<DeletePayload, DiscoveryError> {
        let payload = self.verify()?;
        authority::lifetime(payload.issued_at, payload.expires_at, now, DELETE_TTL)?;
        Ok(payload)
    }
    pub fn verify_fresh(&self) -> Result<DeletePayload, DiscoveryError> {
        self.verify_fresh_at(now_unix()?)
    }
}
