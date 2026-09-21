//! Endpoint lifecycle for rds: iroh endpoint construction, persistent
//! identity, ticket encoding, connection acceptance.
//!
//! The network identity of an rds peer is the iroh `EndpointId`, an Ed25519
//! public key. QUIC-TLS authentication is derived from it, so a connection
//! is always pinned to a key rather than an IP address.

use std::path::Path;
use std::str::FromStr;

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
};

use crate::EndpointConfig;

/// Bind an rds endpoint: configured ALPNs, identity and relay mode.
///
/// With a custom relay the endpoint uses `presets::Minimal` — no n0 address
/// lookup — so a private deployment does not publish to third-party DNS.
/// Without one, `presets::N0` gives the public relays plus DNS/Pkarr lookup.
pub async fn bind_endpoint(config: EndpointConfig) -> anyhow::Result<Endpoint> {
    let mut builder = match &config.relay {
        Some(url) => Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(RelayMode::Custom(RelayMap::from_iter([url.clone()]))),
        None => Endpoint::builder(iroh::endpoint::presets::N0),
    };
    if let Some(key) = config.secret_key {
        builder = builder.secret_key(key);
    }
    // iroh manages its own sockets; a single bind address is all it
    // accepts. Multi-interface binding is a `noq`-backend capability.
    if let Some(addr) = config.bind_addrs.first() {
        builder = builder.bind_addr(*addr)?;
    }
    let endpoint = builder.alpns(config.alpns).bind().await?;
    Ok(endpoint)
}

/// Load a secret key from `path`, or generate and persist a fresh one.
///
/// The file holds the raw 32-byte Ed25519 seed, mode 0600.
pub fn load_or_create_key(path: &Path) -> anyhow::Result<SecretKey> {
    if let Ok(bytes) = std::fs::read(path) {
        let seed: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("key file {path:?} is not 32 bytes"))?;
        return Ok(SecretKey::from_bytes(&seed));
    }
    let key = SecretKey::generate();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_private(path, &key.to_bytes())?;
    Ok(key)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(unix)]
use std::io::Write as _;

/// Default key file location for this OS user.
pub fn default_key_path() -> Option<std::path::PathBuf> {
    dirs_config_dir().map(|d| d.join("remote-device-sync").join("endpoint.key"))
}

fn dirs_config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
}

/// Serialized `EndpointAddr` for copy-paste dialing: `rds1<base32(postcard)>`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Ticket(pub EndpointAddr);

impl Ticket {
    /// Current address of `endpoint`, including its home relay once online.
    pub fn of(endpoint: &crate::Endpoint) -> Self {
        Self(endpoint.addr())
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.0.id
    }
}

impl std::fmt::Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = postcard::to_stdvec(&self.0).map_err(|_| std::fmt::Error)?;
        write!(
            f,
            "rds1{}",
            data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
        )
    }
}

impl FromStr for Ticket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let body = s
            .strip_prefix("rds1")
            .ok_or_else(|| anyhow::anyhow!("ticket must start with 'rds1'"))?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.to_uppercase().as_bytes())
            .map_err(|_| anyhow::anyhow!("ticket is not valid base32"))?;
        let addr = postcard::from_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("ticket payload invalid: {e}"))?;
        Ok(Self(addr))
    }
}

/// Bare `EndpointId` or full ticket → dialable address.
///
/// An `EndpointId` alone relies on configured address lookup (n0 DNS by
/// default); a ticket carries relay and direct addresses explicitly.
pub fn parse_target(target: &str) -> anyhow::Result<EndpointAddr> {
    match Ticket::from_str(target) {
        Ok(ticket) => Ok(ticket.0),
        Err(_) => Ok(EndpointAddr {
            id: EndpointId::from_str(target)?,
            addrs: Default::default(),
        }),
    }
}

/// Relay URL advertised by an address, if any.
pub fn relay_url_of(addr: &EndpointAddr) -> Option<RelayUrl> {
    addr.addrs.iter().find_map(|a| match a {
        TransportAddr::Relay(url) => Some(url.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_roundtrip() {
        let mut addrs = std::collections::BTreeSet::new();
        addrs.insert(TransportAddr::Ip(std::net::SocketAddr::from((
            [10, 0, 0, 7],
            12345,
        ))));
        addrs.insert(TransportAddr::Relay(
            RelayUrl::from_str("https://relay.example.com").unwrap(),
        ));
        let addr = EndpointAddr {
            id: SecretKey::generate().public(),
            addrs,
        };
        let text = Ticket(addr.clone()).to_string();
        assert!(text.starts_with("rds1"));
        let parsed = Ticket::from_str(&text).unwrap();
        assert_eq!(parsed.0, addr);
    }

    #[test]
    fn bare_endpoint_id_parses() {
        let id = SecretKey::generate().public();
        let addr = parse_target(&id.to_string()).unwrap();
        assert_eq!(addr.id, id);
        assert!(addr.addrs.is_empty());
    }
}
