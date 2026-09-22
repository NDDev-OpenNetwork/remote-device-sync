//! `rds-server`: the GDS-facing service host for rds.
//!
//! Composes the server-side pieces that run on the GDS services
//! machine:
//!
//! - packet **relay** for endpoints behind hard NAT (`rds-relay`),
//! - the **discovery directory**: signed `EndpointRecord`s stored on
//!   disk (`rds-discovery`), served over the directory HTTP API
//!   (`PUT/GET/DELETE /v1/records`, `GET /v1/names/{name}`,
//!   `PUT /v1/registry`, health, metrics),
//! - the **registry bridge**: a estate-signed name→key snapshot the
//!   directory verifies against `--registry-key` before serving.
//!
//! The estate publishes the signed snapshot; this host never sees the
//! private signing key.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use rds_discovery::registry::SignedRegistry;
use rds_discovery::service::{self, ServiceConfig};
use rds_discovery::{FileStore, RecordStore};
use tracing::info;

#[derive(Parser)]
#[command(version, about = "RDS services host: relay + discovery directory")]
struct Cli {
    /// Address the relay endpoint binds to.
    #[arg(long, default_value = "0.0.0.0:3340")]
    relay_addr: SocketAddr,
    /// Address the discovery HTTP API binds to.
    #[arg(long, default_value = "0.0.0.0:3341")]
    http_addr: SocketAddr,
    /// Directory holding signed endpoint records.
    #[arg(long, default_value = "/var/lib/rds/directory")]
    directory: PathBuf,
    /// Restrict relay use to these endpoint ids. Empty = open relay.
    #[arg(long = "allow")]
    allow: Vec<String>,
    /// Base32 verifying key that signs estate registry snapshots.
    /// Required for name resolution and `PUT /v1/registry`.
    #[arg(long)]
    registry_key: Option<String>,
    /// JSON file with the initial estate-signed registry snapshot.
    #[arg(long)]
    registry: Option<PathBuf>,
    /// PEM certificate chain enabling HTTPS relaying. Requires --tls-key.
    /// Unprivileged services cannot bind :443 — use --tls-https-addr ≥1024.
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// PEM private key for --tls-cert.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// HTTPS bind address when relay TLS is enabled. ACME needs :443.
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
    tls_acme_cache: Option<PathBuf>,
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

    let store: Arc<dyn RecordStore> = Arc::new(FileStore::new(&cli.directory)?);
    info!(dir = %cli.directory.display(), "endpoint record directory ready");

    let registry_key = cli
        .registry_key
        .as_deref()
        .map(|s| {
            let bytes = data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .map_err(|e| anyhow::anyhow!("--registry-key not base32: {e}"))?;
            let raw: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("--registry-key is not 32 bytes"))?;
            ed25519_dalek::VerifyingKey::from_bytes(&raw)
                .map_err(|e| anyhow::anyhow!("--registry-key invalid: {e}"))
        })
        .transpose()?;
    let registry = cli
        .registry
        .as_deref()
        .map(|p| -> anyhow::Result<SignedRegistry> {
            Ok(serde_json::from_slice(&std::fs::read(p)?)?)
        })
        .transpose()?;
    if registry.is_some() && registry_key.is_none() {
        anyhow::bail!("--registry given without --registry-key");
    }

    let dir = service::serve(
        cli.http_addr,
        store,
        ServiceConfig {
            registry_key,
            registry,
            ..Default::default()
        },
    )
    .await?;
    info!(addr = %dir.addr(), "discovery directory listening");
    let _dir = dir; // serves until process exit

    let allow: Vec<iroh::EndpointId> = cli
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
    let relay = rds_relay::serve(cli.relay_addr, allow, tls).await?;
    info!(addr = %relay.http_addr().expect("relay config enabled"), "relay listening");
    if let Some(addr) = relay.https_addr() {
        info!(%addr, "relay tls listening");
    }

    shutdown_signal().await;
    // Graceful stop: close listener + client websockets instead of
    // letting attached endpoints hit a silent RST.
    let _ = relay.shutdown().await;
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
