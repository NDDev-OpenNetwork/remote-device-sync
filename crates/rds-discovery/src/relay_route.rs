//! Owned relay locator. The URI user-info position carries a public endpoint
//! identity, never a password. Keep parsing shared by discovery and transport.
use crate::{DiscoveryError, EndpointKey, authority::invalid};
use ed25519_dalek::VerifyingKey;
use std::{fmt, net::SocketAddr, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedRelayRoute {
    pub key: EndpointKey,
    pub addr: SocketAddr,
}
impl FromStr for OwnedRelayRoute {
    type Err = DiscoveryError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.len() > crate::MAX_RELAY_URL_BYTES {
            return Err(invalid("owned relay locator exceeds limit"));
        }
        let url = url::Url::parse(raw).map_err(|_| invalid("invalid owned relay locator"))?;
        if url.scheme() != "rds-relay"
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid("invalid owned relay locator fields"));
        }
        let identity = url.username();
        if identity.len() != 64 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid("owned relay requires a 32-byte hex identity"));
        }
        let mut bytes = [0; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&identity[index * 2..index * 2 + 2], 16)
                .map_err(|_| invalid("invalid owned relay identity"))?;
        }
        VerifyingKey::from_bytes(&bytes).map_err(|_| invalid("invalid owned relay identity"))?;
        let ip = match url.host().ok_or_else(|| invalid("missing relay host"))? {
            url::Host::Ipv4(ip) => ip.into(),
            url::Host::Ipv6(ip) => ip.into(),
            // For non-special URI schemes the URL library represents an IPv4
            // literal as an opaque domain. DNS relay names are not this format.
            url::Host::Domain(host) => host
                .parse()
                .map_err(|_| invalid("owned relay requires an IP literal"))?,
        };
        let addr = SocketAddr::new(
            ip,
            url.port()
                .ok_or_else(|| invalid("missing owned relay port"))?,
        );
        if addr.port() == 0 || addr.ip().is_unspecified() || addr.ip().is_multicast() {
            return Err(invalid("invalid owned relay socket"));
        }
        Ok(Self {
            key: EndpointKey(bytes),
            addr,
        })
    }
}
impl fmt::Display for OwnedRelayRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("rds-relay://")?;
        for byte in self.key.0 {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "@{}", self.addr)
    }
}
