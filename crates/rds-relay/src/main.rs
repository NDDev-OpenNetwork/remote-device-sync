//! `rds-relay`: relay server for the GDS estate.
//!
//! Forwards already-encrypted QUIC traffic between endpoints that cannot
//! reach each other directly, and serves as the rendezvous point for
//! hole punching. It cannot read session content.

use std::net::SocketAddr;

use clap::Parser;
use iroh::EndpointId;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let allow: Vec<EndpointId> = cli
        .allow
        .iter()
        .map(|s| s.parse())
        .collect::<Result<_, _>>()?;

    let server = rds_relay::serve(cli.addr, allow).await?;
    println!(
        "relay listening on http://{}",
        server.http_addr().expect("relay config enabled")
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
