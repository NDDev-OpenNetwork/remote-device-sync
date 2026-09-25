//! `rds-relay`: relay server for the GDS estate.
//!
//! Forwards already-encrypted QUIC traffic between endpoints that cannot
//! reach each other directly, and serves as the rendezvous point for
//! hole punching. It cannot read session content.

use std::net::SocketAddr;

use anyhow::Context as _;
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
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    rds_observe::run_main(rds_observe::Service::Relay, "info", run(cli)).await
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let mut server = cli
        .relay
        .prepare()?
        .initialize()
        .await?
        .bind(cli.addr)
        .await?;
    let shutdown = match rds_relay::shutdown_signal() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            server
                .shutdown()
                .await
                .with_context(|| format!("could not install shutdown handlers: {error}"))?;
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
    rds_observe::emit(rds_observe::Event::ListenerReady);
    let unexpected = tokio::select! {
        biased;
        _ = server.stopped() => true,
        _ = shutdown => false,
    };
    server.shutdown().await?;
    anyhow::ensure!(!unexpected, "relay stopped unexpectedly");
    Ok(())
}
