//! `rds-relay`: relay server for the GDS estate.
//!
//! Forwards already-encrypted QUIC traffic between endpoints that cannot
//! reach each other directly, and serves as the rendezvous point for
//! hole punching. It cannot read session content.

use std::net::SocketAddr;

use clap::Parser;
use rds_relay::{RelayArgs, RelayBinding};

#[derive(Parser)]
#[command(version, about = "RDS relay server (iroh HTTP or owned QUIC)")]
struct Cli {
    /// Relay bind address: HTTP for iroh, UDP for the owned QUIC backend.
    #[arg(long, default_value = "0.0.0.0:3340")]
    addr: SocketAddr,
    #[command(flatten)]
    relay: RelayArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let server = cli
        .relay
        .prepare()?
        .initialize()
        .await?
        .bind(cli.addr)
        .await?;
    let shutdown = match rds_relay::shutdown_signal() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            server.shutdown().await?;
            return Err(error.into());
        }
    };
    match server.binding() {
        RelayBinding::Iroh { http, https } => {
            println!("relay listening on http://{http}");
            if let Some(addr) = https {
                println!("relay tls url: https://{addr}");
            }
        }
        RelayBinding::Noq { addr, id } => {
            println!("owned relay listening on udp://{addr}");
            println!("relay endpoint id: {id}");
        }
    }
    shutdown.await;
    server.shutdown().await?;
    Ok(())
}
