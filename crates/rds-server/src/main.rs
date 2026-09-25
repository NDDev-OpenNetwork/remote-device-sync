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

use clap::{Parser, Subcommand};
use rds_discovery::registry::SignedRegistry;
use rds_discovery::service::{self, ServiceConfig};
use rds_discovery::{FileStore, RecordStore};
use tracing::info;

#[derive(Parser)]
#[command(
    version,
    about = "RDS services host: relay + discovery directory",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Address the relay endpoint binds to.
    #[arg(long, default_value = "0.0.0.0:3340")]
    relay_addr: SocketAddr,
    /// Address the discovery HTTP(S) API binds to.
    #[arg(long, default_value = "0.0.0.0:3341")]
    http_addr: SocketAddr,
    /// PEM chain enabling native HTTPS on --http-addr (separate from relay TLS).
    #[arg(long, requires = "directory_tls_key")]
    directory_tls_cert: Option<PathBuf>,
    /// PEM private key matching --directory-tls-cert.
    #[arg(long, requires = "directory_tls_cert")]
    directory_tls_key: Option<PathBuf>,
    /// Directory holding signed endpoint records.
    #[arg(long, default_value = "/var/lib/rds/directory")]
    directory: PathBuf,
    /// Enrolled directory publisher (base32 endpoint key); repeat per device.
    /// Empty denies record publish/fetch/delete. Independent of relay --allow.
    #[arg(long = "directory-allow")]
    directory_allow: Vec<rds_discovery::EndpointKey>,
    /// Restrict relay use to these endpoint ids. Empty = open relay.
    #[arg(long = "allow")]
    allow: Vec<String>,
    /// Base32 verifying key that signs estate registry snapshots.
    /// Required for name resolution and `PUT /v1/registry`.
    #[arg(long)]
    registry_key: Option<String>,
    /// Bootstrap authority epoch (rotation receipts advance it durably).
    #[arg(long, default_value = "1")]
    registry_epoch: u64,
    /// Private policy state directory; default is a sibling of endpoint records.
    #[arg(long, requires = "registry_key")]
    policy_state: Option<PathBuf>,
    /// Dual-signed authority rotation receipt. Repeat in epoch order.
    #[arg(long, requires = "registry_key")]
    authority_rotation: Vec<PathBuf>,
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

#[derive(Subcommand)]
enum Command {
    /// Offline format-2 import; preserve the source and create a NEW sibling.
    /// Imported addresses require a newer signed publication before use.
    MigrateV2 {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    if let Some(Command::MigrateV2 {
        source,
        destination,
    }) = cli.command
    {
        let receipt =
            tokio::task::spawn_blocking(move || rds_discovery::migrate_v2(&source, &destination))
                .await??;
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }

    let enrolled_publishers = cli.directory_allow.len();
    let enrollment = rds_discovery::Enrollment::new(cli.directory_allow)?;

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

    let policy = if let Some(key) = &registry_key {
        let authority = rds_discovery::authority::Authority::new(key, cli.registry_epoch)?;
        let path = cli
            .policy_state
            .unwrap_or_else(|| cli.directory.with_extension("policy"));
        let receipts = cli.authority_rotation;
        Some(
            tokio::task::spawn_blocking(move || -> Result<_, rds_discovery::DiscoveryError> {
                let now = rds_discovery::clock::Reading::now()?;
                let mut store = rds_discovery::policy::PolicyStore::open(&path, authority, now)?;
                for receipt in rds_discovery::policy::read_rotations(&receipts)? {
                    store.apply_rotation(&receipt, now)?;
                }
                Ok(store)
            })
            .await??,
        )
    } else {
        None
    };

    let directory_tls = match (&cli.directory_tls_cert, &cli.directory_tls_key) {
        (Some(cert), Some(key)) => Some(rds_discovery::tls::server_config_from_pem(
            &std::fs::read(cert)?,
            &std::fs::read(key)?,
        )?),
        (None, None) => None,
        _ => anyhow::bail!("directory TLS requires both certificate and key"),
    };
    let https = directory_tls.is_some();
    let dir = service::serve(
        cli.http_addr,
        store,
        ServiceConfig {
            enrollment,
            tls: directory_tls,
            policy,
            registry_key,
            registry,
            ..Default::default()
        },
    )
    .await?;
    info!(addr = %dir.addr(), https, enrolled_publishers, "discovery directory listening");

    let relay = match rds_relay::serve(cli.relay_addr, allow, tls).await {
        Ok(relay) => relay,
        Err(error) => {
            // A partially started service still owns listener and disk work.
            dir.close().await?;
            return Err(error);
        }
    };
    // Readiness must not race signal-handler installation.
    let shutdown = match shutdown_signal() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            let _ = tokio::join!(relay.shutdown(), dir.close());
            return Err(error.into());
        }
    };
    info!(addr = %relay.http_addr().expect("relay config enabled"), "relay listening");
    if let Some(addr) = relay.https_addr() {
        info!(%addr, "relay tls listening");
    }

    shutdown.await;
    // Start both shutdowns before awaiting either; directory storage work
    // remains owned until it finishes, even when relay shutdown fails.
    let (relay_result, directory_result) = tokio::join!(relay.shutdown(), dir.close());
    directory_result?;
    relay_result?;
    Ok(())
}

/// Install SIGINT/SIGTERM handlers before publishing final readiness.
#[cfg(unix)]
fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = term.recv() => {}
        }
    })
}

#[cfg(not(unix))]
fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_migration_requires_both_paths_and_refuses_service_flags() {
        assert!(
            Cli::try_parse_from(["rds-server"])
                .unwrap()
                .command
                .is_none()
        );
        let cli = Cli::try_parse_from([
            "rds-server",
            "migrate-v2",
            "--source",
            "/state/old",
            "--destination",
            "/state/new",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::MigrateV2 { .. })));
        assert!(
            Cli::try_parse_from(["rds-server", "migrate-v2", "--source", "/state/old"]).is_err()
        );
        assert!(
            Cli::try_parse_from([
                "rds-server",
                "--http-addr",
                "127.0.0.1:9000",
                "migrate-v2",
                "--source",
                "/state/old",
                "--destination",
                "/state/new",
            ])
            .is_err()
        );
    }
}
