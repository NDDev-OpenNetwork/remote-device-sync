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
    /// PEM certificate chain enabling HTTPS relaying. Requires --tls-key.
    /// Unprivileged services cannot bind :443 — use --tls-https-addr ≥1024.
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<std::path::PathBuf>,
    /// PEM private key for --tls-cert.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<std::path::PathBuf>,
    /// HTTPS bind address when TLS is enabled. ACME validation needs :443.
    #[arg(long, default_value = "0.0.0.0:3443")]
    tls_https_addr: SocketAddr,
    /// Let's Encrypt domain via in-process ACME (TLS-ALPN-01, needs :443
    /// reachable). Repeatable. Mutually exclusive with --tls-cert.
    #[arg(long, conflicts_with = "tls_cert")]
    tls_acme_domain: Vec<String>,
    /// ACME contact (repeatable); emails need a `mailto:` prefix.
    #[arg(long, requires = "tls_acme_domain")]
    tls_acme_contact: Vec<String>,
    /// Directory caching issued ACME certificates across restarts.
    #[arg(long, requires = "tls_acme_domain")]
    tls_acme_cache: Option<std::path::PathBuf>,
    /// Use the Let's Encrypt staging directory (untrusted certs; testing).
    #[arg(long)]
    tls_acme_staging: bool,
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

    let tls = rds_relay::tls_from_flags(
        cli.tls_https_addr,
        cli.tls_cert,
        cli.tls_key,
        cli.tls_acme_domain,
        cli.tls_acme_contact,
        cli.tls_acme_cache,
        cli.tls_acme_staging,
    )?;
    let server = rds_relay::serve(cli.addr, allow, tls).await?;
    println!(
        "relay listening on http://{}",
        server.http_addr().expect("relay config enabled")
    );
    if let Some(addr) = server.https_addr() {
        println!("relay tls url: https://{addr}");
    }
    shutdown_signal().await;
    // Graceful stop: close listener + client websockets instead of
    // letting attached endpoints hit a silent RST.
    let _ = server.shutdown().await;
    Ok(())
}

/// SIGINT on every platform, SIGTERM on unix (systemd stop).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
