//! `rds-relay`: embedded iroh relay for the GDS estate.
//!
//! The relay forwards already-encrypted QUIC traffic between endpoints that
//! cannot reach each other directly, and serves as the rendezvous point for
//! hole punching. It cannot read session content.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use iroh::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, ClientRequest, ConnectionId, RelayConfig, Server, ServerConfig,
};

#[derive(Parser)]
#[command(version, about = "RDS relay server (iroh relay, HTTP mode)")]
struct Cli {
    /// Address the relay HTTP endpoint binds to.
    #[arg(long, default_value = "0.0.0.0:3340")]
    addr: SocketAddr,
    /// Restrict relay use to these endpoint ids. Empty = open relay.
    #[arg(long = "allow")]
    allow: Vec<String>,
}

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();

    let mut relay_config = RelayConfig::new(cli.addr);
    if !cli.allow.is_empty() {
        let allowed: HashSet<EndpointId> = cli
            .allow
            .iter()
            .map(|s| s.parse())
            .collect::<Result<_, _>>()?;
        relay_config.access = Arc::new(AllowList(allowed));
    }

    let mut config = ServerConfig::default();
    config.relay = Some(relay_config);
    let server = Server::spawn(config).await?;

    println!(
        "relay listening on http://{}",
        server.http_addr().expect("relay config enabled")
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
