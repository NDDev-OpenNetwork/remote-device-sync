//! Owned endpoint identity and addressing types.
//!
//! These are the wire-stable types peers exchange (tickets, discovery
//! records, relay advertisements). They are deliberately owned here —
//! not re-exports of a transport backend's types — so a backend swap
//! never rewrites the signed/serialized forms. The serde, Display and
//! FromStr shapes intentionally match iroh-base 1.2 exactly: an
//! [`EndpointAddr`] postcard-encodes to the same bytes as
//! `iroh::EndpointAddr`, and [`EndpointId`] renders/parses as the same
//! lowercase-hex (or legacy base32) string.

use std::collections::BTreeSet;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize, de, ser};

/// Length in bytes of an endpoint's public identity.
pub const PUBLIC_KEY_LENGTH: usize = 32;

/// The public identity of an endpoint: an ed25519 verifying key.
///
/// This is the device's stable name on the network. Display and
/// parsing use lowercase hex; base32 (RFC 4648, no padding) input is
/// also accepted for compatibility with historical key strings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EndpointId([u8; 32]);

impl EndpointId {
    /// The raw 32 bytes of the verifying key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Wrap raw key bytes, rejecting malformed ed25519 points.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, KeyParseError> {
        VerifyingKey::from_bytes(bytes).map_err(|_| KeyParseError::InvalidKeyData)?;
        Ok(Self(*bytes))
    }

    /// Wrap an already-validated verifying key.
    pub fn from_verifying_key(key: VerifyingKey) -> Self {
        Self(key.to_bytes())
    }

    /// The verifying key, for signature checks against this identity.
    pub fn verifying_key(&self) -> VerifyingKey {
        // Infallible: every stored value passed validation at construction.
        VerifyingKey::from_bytes(&self.0).expect("validated at construction")
    }

    /// First five bytes as hex, for compact log/tag rendering.
    pub fn fmt_short(&self) -> impl fmt::Display + Copy + 'static {
        #[derive(Clone, Copy)]
        struct Short([u8; 5]);
        impl fmt::Display for Short {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                hex_write(&self.0, f)
            }
        }
        Short(self.0[0..5].try_into().expect("slice has five bytes"))
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex_write(&self.0, f)
    }
}

impl fmt::Debug for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EndpointId({self})")
    }
}

impl Serialize for EndpointId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            self.0.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for EndpointId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s = <&str>::deserialize(deserializer)?;
            Self::from_str(s).map_err(de::Error::custom)
        } else {
            let data: [u8; 32] = Deserialize::deserialize(deserializer)?;
            Self::from_bytes(&data).map_err(de::Error::custom)
        }
    }
}

impl TryFrom<&[u8]> for EndpointId {
    type Error = KeyParseError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let arr: [u8; 32] = bytes.try_into().map_err(|_| KeyParseError::InvalidLength)?;
        Self::from_bytes(&arr)
    }
}

impl From<EndpointId> for [u8; 32] {
    fn from(id: EndpointId) -> Self {
        id.0
    }
}

/// Borrow as raw key bytes so `[u8; 32]`-keyed lookups work without
/// re-wrapping (e.g. datagram dispatch maps).
impl std::borrow::Borrow<[u8; 32]> for EndpointId {
    fn borrow(&self) -> &[u8; 32] {
        &self.0
    }
}

impl FromStr for EndpointId {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(&decode_key_string(s)?)
    }
}

/// The secret half of an endpoint identity.
///
/// Serializes like the wrapped ed25519 signing key (32 raw bytes in
/// binary formats) and zeroizes on drop via `ed25519-dalek/zeroize`.
#[derive(Clone)]
pub struct SecretKey(SigningKey);

impl SecretKey {
    /// The public identity of this key.
    pub fn public(&self) -> EndpointId {
        EndpointId(self.0.verifying_key().to_bytes())
    }

    /// Generate a fresh random key.
    pub fn generate() -> Self {
        Self::from_bytes(&rand::random())
    }

    /// Wrap raw secret bytes. Every 32-byte string is a valid key.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(bytes))
    }

    /// The raw 32 bytes of the secret part.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Sign `msg`, returning an ed25519 signature.
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.0.sign(msg)
    }

    /// The wrapped signing key, for backends that need it directly.
    pub fn signing_key(&self) -> &SigningKey {
        &self.0
    }
}

impl From<SigningKey> for SecretKey {
    fn from(key: SigningKey) -> Self {
        Self(key)
    }
}

impl From<SecretKey> for SigningKey {
    fn from(key: SecretKey) -> Self {
        key.0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKey").finish_non_exhaustive()
    }
}

impl Serialize for SecretKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        Ok(Self(SigningKey::deserialize(deserializer)?))
    }
}

impl FromStr for SecretKey {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_bytes(&decode_key_string(s)?))
    }
}

/// The relay/home-server URL of an endpoint address.
///
/// Wraps a normalized absolute URL; Display/FromStr/serde behave
/// exactly like `url::Url`. `Arc` keeps `TransportAddr` (and so
/// `EndpointAddr`) small — one variant carries a pointer, not an
/// inline `Url`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RelayUrl(std::sync::Arc<url::Url>);

impl RelayUrl {
    /// The parsed URL.
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }
}

impl std::ops::Deref for RelayUrl {
    type Target = url::Url;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromStr for RelayUrl {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(std::sync::Arc::new(url::Url::parse(s)?)))
    }
}

impl fmt::Display for RelayUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<url::Url> for RelayUrl {
    fn from(url: url::Url) -> Self {
        Self(std::sync::Arc::new(url))
    }
}

impl From<RelayUrl> for url::Url {
    fn from(url: RelayUrl) -> Self {
        (*url.0).clone()
    }
}

/// One transport-level way to reach an endpoint.
///
/// `Relay` sorts before `Ip` — the ordering feeds both `BTreeSet`
/// iteration and the postcard variant index, and must not change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TransportAddr {
    /// Reachable through a relay at the given URL.
    Relay(RelayUrl),
    /// Reachable directly at a socket address.
    Ip(SocketAddr),
}

impl TransportAddr {
    /// True for the relay form.
    pub fn is_relay(&self) -> bool {
        matches!(self, Self::Relay(_))
    }

    /// True for the direct-IP form.
    pub fn is_ip(&self) -> bool {
        matches!(self, Self::Ip(_))
    }
}

impl fmt::Display for TransportAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relay(url) => write!(f, "relay:{url}"),
            Self::Ip(addr) => write!(f, "ip:{addr}"),
        }
    }
}

/// Everything needed to dial an endpoint: its identity plus the
/// transport addresses it can be reached at.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointAddr {
    /// The endpoint's public identity.
    pub id: EndpointId,
    /// The transport addresses it is reachable at.
    pub addrs: BTreeSet<TransportAddr>,
}

impl EndpointAddr {
    /// An address carrying only the identity — dialable when
    /// discovery can fill in the transport addresses.
    pub fn new(id: EndpointId) -> Self {
        Self {
            id,
            addrs: Default::default(),
        }
    }

    /// Build from its parts.
    pub fn from_parts(id: EndpointId, addrs: impl IntoIterator<Item = TransportAddr>) -> Self {
        Self {
            id,
            addrs: addrs.into_iter().collect(),
        }
    }

    /// Adds a relay address.
    pub fn with_relay_url(mut self, relay_url: RelayUrl) -> Self {
        self.addrs.insert(TransportAddr::Relay(relay_url));
        self
    }

    /// Adds a direct socket address.
    pub fn with_ip_addr(mut self, addr: SocketAddr) -> Self {
        self.addrs.insert(TransportAddr::Ip(addr));
        self
    }

    /// Adds several addresses.
    pub fn with_addrs(mut self, addrs: impl IntoIterator<Item = TransportAddr>) -> Self {
        self.addrs.extend(addrs);
        self
    }

    /// True when no transport addresses are present.
    pub fn is_empty(&self) -> bool {
        self.addrs.is_empty()
    }

    /// Iterator over the direct socket addresses.
    pub fn ip_addrs(&self) -> impl Iterator<Item = &SocketAddr> {
        self.addrs.iter().filter_map(|addr| match addr {
            TransportAddr::Ip(addr) => Some(addr),
            _ => None,
        })
    }

    /// Iterator over the relay URLs.
    pub fn relay_urls(&self) -> impl Iterator<Item = &RelayUrl> {
        self.addrs.iter().filter_map(|addr| match addr {
            TransportAddr::Relay(url) => Some(url),
            _ => None,
        })
    }
}

impl From<EndpointId> for EndpointAddr {
    fn from(id: EndpointId) -> Self {
        EndpointAddr::new(id)
    }
}

/// Why an endpoint key string failed to parse.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyParseError {
    /// The bytes are not a valid ed25519 verifying key.
    #[error("invalid key data")]
    InvalidKeyData,
    /// The input had the wrong length.
    #[error("invalid key length")]
    InvalidLength,
    /// Hex decoding failed.
    #[error("failed to decode hex string")]
    FailedToDecodeHex,
    /// Base32 decoding failed.
    #[error("failed to decode base32 string")]
    FailedToDecodeBase32,
}

fn hex_write(bytes: &[u8], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 64];
    for (i, b) in bytes.iter().enumerate() {
        buf[i * 2] = HEX[(b >> 4) as usize];
        buf[i * 2 + 1] = HEX[(b & 0x0f) as usize];
    }
    f.write_str(std::str::from_utf8(&buf[..bytes.len() * 2]).expect("hex output is ascii"))
}

/// Decode a key string: lowercase hex when exactly 64 chars, else
/// base32 (RFC 4648, case-insensitive, no padding). Same dual
/// acceptance the backend format used historically.
fn decode_key_string(s: &str) -> Result<[u8; 32], KeyParseError> {
    if s.len() == PUBLIC_KEY_LENGTH * 2 {
        let mut bytes = [0u8; 32];
        for (i, &[hi, lo]) in s.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let hi = hex_val(hi).ok_or(KeyParseError::FailedToDecodeHex)?;
            let lo = hex_val(lo).ok_or(KeyParseError::FailedToDecodeHex)?;
            bytes[i] = (hi << 4) | lo;
        }
        return Ok(bytes);
    }
    data_encoding::BASE32_NOPAD
        .decode(s.to_ascii_uppercase().as_bytes())
        .map_err(|_| KeyParseError::FailedToDecodeBase32)?
        .try_into()
        .map_err(|_| KeyParseError::InvalidLength)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_id_hex_roundtrips() {
        let key = SecretKey::generate();
        let id = key.public();
        let s = id.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(EndpointId::from_str(&s).unwrap(), id);
        assert_eq!(id.fmt_short().to_string(), &s[..10]);
    }

    #[test]
    fn endpoint_id_rejects_bad_inputs() {
        assert!(matches!(
            EndpointId::try_from(&[1u8; 4][..]),
            Err(KeyParseError::InvalidLength)
        ));
        assert!(EndpointId::from_str("zz").is_err());
        assert!(matches!(
            EndpointId::from_str(&"g".repeat(64)),
            Err(KeyParseError::FailedToDecodeHex)
        ));
    }

    #[test]
    fn endpoint_addr_postcard_layout_is_stable() {
        let id = SecretKey::from_bytes(&[7; 32]).public();
        let addr = EndpointAddr::new(id)
            .with_relay_url(RelayUrl::from_str("https://relay.example.test").unwrap())
            .with_ip_addr("127.0.0.1:4433".parse().unwrap());
        let bytes = postcard::to_stdvec(&addr).unwrap();
        let back: EndpointAddr = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, addr);
        // Layout probe: 32-byte id, varint set len, Relay(0)+str, Ip(1)+socket.
        assert_eq!(&bytes[..32], id.as_bytes());
        assert_eq!(bytes[32], 2); // two addrs
    }

    #[test]
    fn transport_addr_relay_sorts_before_ip() {
        let mut set = BTreeSet::new();
        set.insert(TransportAddr::Ip("127.0.0.1:1".parse().unwrap()));
        set.insert(TransportAddr::Relay(
            RelayUrl::from_str("https://r.example").unwrap(),
        ));
        assert!(matches!(set.first(), Some(TransportAddr::Relay(_))));
    }

    #[test]
    fn relay_url_normalizes_like_url() {
        let url = RelayUrl::from_str("https://example.com").unwrap();
        assert_eq!(url.to_string(), "https://example.com/");
    }
}
