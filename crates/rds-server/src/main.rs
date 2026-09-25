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
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use rds_discovery::registry::SignedRegistry;
use rds_discovery::service::{self, ServiceConfig};
use rds_discovery::{FileStore, RecordStore};
use rds_relay::{RelayArgs, RelayBinding};
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
    #[command(flatten)]
    relay: RelayArgs,
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
    /// Base32 verifying key that signs estate registry snapshots.
    /// Required for name resolution and `PUT /v1/registry`.
    #[arg(long)]
    registry_key: Option<String>,
    /// Bootstrap authority epoch; defaults to 1 with --registry-key.
    /// Rotation receipts advance it durably.
    #[arg(long, requires = "registry_key")]
    registry_epoch: Option<NonZeroU64>,
    /// Private policy state directory; default is a sibling of endpoint records.
    #[arg(long, requires = "registry_key")]
    policy_state: Option<PathBuf>,
    /// Dual-signed authority rotation receipt. Repeat in epoch order.
    #[arg(long, requires = "registry_key")]
    authority_rotation: Vec<PathBuf>,
    /// JSON file with the initial estate-signed registry snapshot.
    #[arg(long, requires = "registry_key")]
    registry: Option<PathBuf>,
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
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    rds_observe::run_main(rds_observe::Service::Server, "info", run(cli)).await
}

async fn run(cli: Cli) -> anyhow::Result<()> {
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

    // Complete read-only configuration checks before identity/catalog creation
    // or listener startup. Durable authority checks still belong to PolicyStore.
    let enrolled_publishers = cli.directory_allow.len();
    let enrollment = rds_discovery::Enrollment::new(cli.directory_allow)?;
    let relay = cli.relay.prepare()?;
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
    let authority = registry_key
        .as_ref()
        .map(|key| {
            rds_discovery::authority::Authority::new(
                key,
                cli.registry_epoch.map(NonZeroU64::get).unwrap_or(1),
            )
        })
        .transpose()?;
    let registry = cli
        .registry
        .as_deref()
        .map(|path| -> anyhow::Result<SignedRegistry> {
            Ok(serde_json::from_slice(&read_config_file(
                path,
                rds_discovery::http::MAX_BODY,
                "registry snapshot",
            )?)?)
        })
        .transpose()?;
    let rotations = rds_discovery::policy::read_rotations(&cli.authority_rotation)?;
    let directory_tls = match (&cli.directory_tls_cert, &cli.directory_tls_key) {
        (Some(cert), Some(key)) => Some(rds_discovery::tls::server_config_from_pem(
            &read_config_file(cert, 1024 * 1024, "directory TLS chain")?,
            &read_config_file(key, 64 * 1024, "directory TLS key")?,
        )?),
        (None, None) => None,
        _ => anyhow::bail!("directory TLS requires both certificate and key"),
    };
    let relay = relay.initialize().await?;
    let directory_path = cli.directory.clone();
    let store: Arc<dyn RecordStore> =
        Arc::new(tokio::task::spawn_blocking(move || FileStore::new(&directory_path)).await??);
    info!(dir = %cli.directory.display(), "endpoint record directory ready");
    let policy = if let Some(authority) = authority {
        let path = cli
            .policy_state
            .unwrap_or_else(|| cli.directory.with_extension("policy"));
        Some(
            tokio::task::spawn_blocking(move || -> Result<_, rds_discovery::DiscoveryError> {
                let now = rds_discovery::clock::Reading::now()?;
                let mut store = rds_discovery::policy::PolicyStore::open(&path, authority, now)?;
                for receipt in rotations {
                    store.apply_rotation(&receipt, now)?;
                }
                Ok(store)
            })
            .await??,
        )
    } else {
        None
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
    let mut relay = match relay.bind(cli.relay_addr).await {
        Ok(relay) => relay,
        Err(error) => {
            return check_shutdown(None, Err(error), dir.close().await);
        }
    };
    let shutdown = match rds_relay::shutdown_signal() {
        Ok(shutdown) => shutdown,
        Err(error) => {
            let (relay_result, directory_result) = tokio::join!(relay.shutdown(), dir.close());
            check_shutdown(None, relay_result, directory_result)
                .with_context(|| format!("could not install shutdown handlers: {error}"))?;
            return Err(error.into());
        }
    };
    match relay.binding() {
        RelayBinding::Iroh { http, https } => {
            info!(addr = %http, "relay listening");
            if let Some(addr) = https {
                info!(%addr, "relay tls listening");
            }
        }
        RelayBinding::Noq { addr, id } => {
            info!(%addr, endpoint_id = %id, "owned relay listening");
        }
    }
    rds_observe::emit(rds_observe::Event::ListenerReady);
    let unexpected = tokio::select! {
        biased;
        _ = relay.stopped() => Some("relay"),
        _ = dir.wait_stopped() => Some("directory"),
        _ = shutdown => None,
    };
    let (relay_result, directory_result) = tokio::join!(relay.shutdown(), dir.close());
    check_shutdown(unexpected, relay_result, directory_result)
}

fn check_shutdown(
    unexpected: Option<&str>,
    relay: Result<(), rds_relay::RelayRuntimeError>,
    directory: std::io::Result<()>,
) -> anyhow::Result<()> {
    // Both services have already joined. Preserve both failures if teardown
    // failed in both components, rather than letting the first `?` hide one.
    match (relay, directory) {
        (Err(relay), Err(directory)) => {
            let relay = anyhow::Error::new(relay);
            anyhow::bail!("relay failed: {relay:#}; directory failed: {directory}");
        }
        (Err(error), Ok(())) => return Err(error.into()),
        (Ok(()), Err(error)) => return Err(error.into()),
        (Ok(()), Ok(())) => {}
    }
    if let Some(service) = unexpected {
        anyhow::bail!("{service} stopped unexpectedly");
    }
    Ok(())
}

fn read_config_file(path: &std::path::Path, limit: usize, label: &str) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("could not open {label}"))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= limit, "{label} exceeds {limit} bytes");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpected_clean_service_exit_is_failure_and_dual_errors_are_preserved() {
        assert!(check_shutdown(None, Ok(()), Ok(())).is_ok());
        for service in ["relay", "directory"] {
            let error = check_shutdown(Some(service), Ok(()), Ok(())).unwrap_err();
            assert!(error.to_string().contains(service));
        }
        let relay = rds_relay::RelayRuntimeError::IrohShutdown(
            anyhow::anyhow!("fixture relay cause").context("fixture supervisor error"),
        );
        let error = check_shutdown(
            None,
            Err(relay),
            Err(std::io::Error::other("fixture directory cause")),
        )
        .unwrap_err()
        .to_string();
        for cause in [
            "fixture relay cause",
            "fixture supervisor error",
            "fixture directory cause",
        ] {
            assert!(error.contains(cause), "lost failure: {error}");
        }
    }

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
