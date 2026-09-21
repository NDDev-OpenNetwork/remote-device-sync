//! Relay server library for rds.
//!
//! Two generations live here:
//!
//! - [`iroh_server`] — the shipping relay: embedded `iroh-relay` with an
//!   endpoint allowlist. Forwards already-encrypted QUIC traffic; cannot
//!   read session content.
//! - [`proto`] — the owned relay protocol under design: datagram
//!   forwarding keyed by `EndpointId`, intended to run inside the
//!   `rds-server` composition on the GDS host.

pub mod proto;
#[cfg(feature = "owned-relay")]
pub mod server;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use iroh::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, ClientRequest, ConnectionId, RelayConfig, Server, ServerConfig,
};

/// Admit only endpoint ids on the relay allowlist.
#[derive(Debug)]
struct AllowList(HashSet<EndpointId>);

impl AccessControl for AllowList {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        if self.0.contains(&request.endpoint_id()) {
            Access::Allow
        } else {
            Access::Deny {
                reason: Some("endpoint id not on relay allowlist".into()),
            }
        }
    }

    fn on_disconnect(&self, _endpoint_id: EndpointId, _connection_id: ConnectionId) {}
}

/// Spawn the iroh relay on `addr`, restricted to `allow` when non-empty.
/// Returns the bound server; dropping it stops the relay.
pub async fn serve(addr: SocketAddr, allow: Vec<EndpointId>) -> anyhow::Result<Server> {
    let mut relay_config = RelayConfig::new(addr);
    if !allow.is_empty() {
        relay_config.access = Arc::new(AllowList(allow.into_iter().collect()));
    }
    let mut config = ServerConfig::default();
    config.relay = Some(relay_config);
    Ok(Server::spawn(config).await?)
}
