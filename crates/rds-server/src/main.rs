//! `rds-server`: the GDS-facing service host for rds.
//!
//! Composes the server-side pieces that run on the GDS services
//! machine:
//!
//! - packet **relay** for endpoints behind hard NAT (`rds-relay`),
//! - the **discovery directory**: signed `EndpointRecord`s stored on
//!   disk (`rds-discovery`), published by devices and queried by peers.
//!
//! Planned next: the HTTP/QUIC discovery API (PUT/GET records), the
//! device-registry bridge into GDS estate state, presence, and audit
//! emission. Today it binds the relay and loads the record directory.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use rds_discovery::{FileStore, RecordStore};
use tracing::info;

#[derive(Parser)]
#[command(version, about = "RDS services host: relay + discovery directory")]
struct Cli {
    /// Address the relay endpoint binds to.
    #[arg(long, default_value = "0.0.0.0:3340")]
    relay_addr: SocketAddr,
    /// Directory holding signed endpoint records.
    #[arg(long, default_value = "/var/lib/rds/directory")]
    directory: PathBuf,
    /// Restrict relay use to these endpoint ids. Empty = open relay.
    #[arg(long = "allow")]
    allow: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();

    let store = FileStore::new(&cli.directory)?;
    info!(dir = %cli.directory.display(), "endpoint record directory ready");
    let _ = &store as &dyn RecordStore; // keep trait usage honest

    let allow: Vec<iroh::EndpointId> = cli
        .allow
        .iter()
        .map(|s| s.parse())
        .collect::<Result<_, _>>()?;
    let relay = rds_relay::serve(cli.relay_addr, allow).await?;
    info!(addr = %relay.http_addr().expect("relay config enabled"), "relay listening");

    tokio::signal::ctrl_c().await?;
    Ok(())
}
