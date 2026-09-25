//! Endpoint lifecycle for rds: iroh endpoint construction, persistent
//! identity, ticket encoding, connection acceptance.
//!
//! The network identity of an rds peer is the iroh `EndpointId`, an Ed25519
//! public key. QUIC-TLS authentication is derived from it, so a connection
//! is always pinned to a key rather than an IP address.

use std::str::FromStr;

use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, TransportAddr};

use crate::EndpointConfig;

/// Bind an rds endpoint: configured ALPNs, identity and relay mode.
///
/// With a custom relay the endpoint uses `presets::Minimal` — no n0 address
/// lookup — so a private deployment does not publish to third-party DNS.
/// Without one, `presets::N0` gives the public relays plus DNS/Pkarr lookup.
pub async fn bind_endpoint(config: EndpointConfig) -> anyhow::Result<Endpoint> {
    config.validate_for(crate::Backend::Iroh)?;
    let mut builder = match (config.relays.is_empty(), config.discovery) {
        (false, _) => Endpoint::builder(iroh::endpoint::presets::Minimal).relay_mode(
            RelayMode::Custom(RelayMap::from_iter(config.relays.clone())),
        ),
        (true, true) => Endpoint::builder(iroh::endpoint::presets::N0),
        // No relay, no lookup: Minimal binds a plain QUIC socket.
        (true, false) => Endpoint::builder(iroh::endpoint::presets::Minimal),
    };
    if let Some(key) = config.secret_key {
        builder = builder.secret_key(key);
    }
    // Tuning on top of iroh's multipath-aware defaults:
    // - BBRv3: paced, bufferbloat-resistant — the low-latency choice for
    //   interactive desktop + bulk sync over real WAN paths (upstream
    //   default is loss-based Cubic).
    // - 4 MiB stream receive window: upstream tunes for ~100 Mbps x
    //   100 ms; a larger per-stream window keeps a big keyframe or sync
    //   chunk stream from stalling on high-BDP links.
    // - 32 MiB connection send window keeps several bulk streams busy.
    let mut transport = iroh::endpoint::QuicTransportConfig::builder()
        .congestion_controller_factory(std::sync::Arc::new(
            noq_proto::congestion::Bbr3Config::default(),
        ))
        .stream_receive_window(noq_proto::VarInt::from_u32(4 * 1024 * 1024))
        .send_window(32 * 1024 * 1024);
    if let Some(max_paths) = config.max_multipath_paths {
        transport = transport.max_concurrent_multipath_paths(max_paths);
    }
    builder = builder.transport_config(transport.build());
    // iroh manages its own sockets; a single bind address is all it
    // accepts. Multi-interface binding is a `noq`-backend capability.
    if let Some(addr) = config.bind_addrs.first() {
        builder = builder.bind_addr(*addr)?;
    }
    let endpoint = builder.alpns(config.alpns).bind().await?;
    Ok(endpoint)
}

// Compatibility path; persistent identity is shared by every backend.
pub use crate::identity::{default_key_path, load_or_create_key};

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
    use crate::SecretKey;

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
