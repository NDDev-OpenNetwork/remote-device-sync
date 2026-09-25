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
    #[command(flatten)]
    admin: rds_observe::admin::Args,
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
    let prepared_admin = cli.admin.bind().await?;
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
    let metrics = server.metrics();
    let mut admin = rds_observe::admin::Server::start(prepared_admin, move || metrics.snapshot());
    if let Some(addr) = admin.addr() {
        tracing::info!(%addr, "admin metrics listening");
    }
    rds_observe::emit(rds_observe::Event::ListenerReady);
    let unexpected = tokio::select! {
        biased;
        _ = server.stopped() => Some("relay"),
        _ = admin.stopped() => Some("admin metrics"),
        _ = shutdown => None,
    };
    let (product, admin_result) = tokio::join!(server.shutdown(), admin.close());
    let result = product.map_err(anyhow::Error::from).and_then(|()| {
        if let Some(service) = unexpected {
            anyhow::bail!("{service} stopped unexpectedly");
        }
        Ok(())
    });
    finish_admin(result, admin_result)
}

fn finish_admin(
    product: anyhow::Result<()>,
    admin: Result<(), rds_observe::admin::Error>,
) -> anyhow::Result<()> {
    match (product, admin) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(error), Err(admin)) => {
            Err(error.context(format!("admin shutdown also failed: {admin}")))
        }
    }
}
