//! `rds-agent`: daemon on a controlled device.

use std::str::FromStr;

use clap::Parser;
use rds_agent::{Agent, AgentPolicy};
use rds_net::EndpointId;
use rds_net::{EndpointConfig, Ticket, bind_endpoint, default_key_path, load_or_create_key};

#[derive(Parser)]
#[command(
    version,
    about = "RDS agent: serve SSH and desktop sessions to allowed peers"
)]
struct Cli {
    /// Path to the endpoint secret key (created if missing).
    #[arg(long)]
    key_file: Option<std::path::PathBuf>,
    /// Custom relay URL; default is the n0 public relays. Repeatable —
    /// ≥2 relays give automatic client-side failover.
    #[arg(long)]
    relay: Vec<String>,
    /// Transport backend: `iroh` (default) or `noq` (with the
    /// `transport-noq` feature).
    #[arg(long, default_value = "iroh")]
    backend: String,
    /// Allowed peer EndpointId. Repeatable.
    #[arg(long = "allow")]
    allow: Vec<String>,
    /// SSH socket the TcpConnect service may reach.
    #[arg(long, default_value = "127.0.0.1:22")]
    ssh: String,
    /// Permit TcpConnect to any host:port (development only).
    #[arg(long)]
    allow_any_tcp: bool,
    /// Discovery directory address; when set the agent publishes its
    /// signed record and keeps it fresh.
    #[arg(long)]
    directory: Option<std::net::SocketAddr>,
    /// Record TTL when `--directory` is set.
    #[arg(long, default_value = "300")]
    record_ttl: u64,
    /// Trusted grant issuer (base32 verifying key). Repeatable. When
    /// set, every connection must present a valid estate-signed grant
    /// before any service stream opens.
    #[arg(long = "issuer")]
    issuers: Vec<String>,
    /// Maximum grant lifetime accepted, in seconds.
    #[arg(long, default_value = "300")]
    grant_ttl: u64,
    /// Verifying key that signs the estate revocation snapshot
    /// (`GET /v1/revocations`). Required for denylist polling when
    /// `--directory` is set.
    #[arg(long)]
    revocations_key: Option<String>,
    /// Revocation poll interval in seconds.
    #[arg(long, default_value = "30")]
    revocations_interval: u64,
    /// Directory the Sync service may read/write under.
    #[arg(long)]
    sync_dir: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();

    let key_path = cli
        .key_file
        .or_else(default_key_path)
        .ok_or_else(|| anyhow::anyhow!("no --key-file and no config dir"))?;
    let secret_key = load_or_create_key(&key_path)?;

    let (ssh_host, ssh_port) = cli
        .ssh
        .split_once(':')
        .map(|(h, p)| (h.to_string(), p.parse::<u16>()))
        .ok_or_else(|| anyhow::anyhow!("--ssh must be host:port"))
        .and_then(|(h, p)| p.map(|p| (h, p)).map_err(Into::into))?;

    let mut policy = AgentPolicy::ssh_only((ssh_host, ssh_port));
    for id in &cli.allow {
        policy.allow.insert(EndpointId::from_str(id)?);
    }
    policy.allow_any_tcp = cli.allow_any_tcp;
    policy.grant_max_ttl = std::time::Duration::from_secs(cli.grant_ttl);
    policy.sync_dir = cli.sync_dir;
    for s in &cli.issuers {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.to_uppercase().as_bytes())
            .map_err(|e| anyhow::anyhow!("--issuer not base32: {e}"))?;
        let raw: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("--issuer is not 32 bytes"))?;
        policy.issuers.insert(raw);
    }

    let backend = match cli.backend.as_str() {
        "iroh" => rds_net::Backend::Iroh,
        #[cfg(feature = "transport-noq")]
        "noq" => rds_net::Backend::Noq,
        other => anyhow::bail!("unknown or unavailable backend {other:?}"),
    };

    let mut config = EndpointConfig {
        secret_key: Some(secret_key.clone()),
        backend,
        ..Default::default()
    };
    config = config.with_relays(&cli.relay)?;

    let endpoint = bind_endpoint(config).await?;
    endpoint.online().await;

    let policy_handle = std::sync::Arc::new(policy.clone());
    let _announce = cli.directory.map(|addr| {
        rds_net::announce(
            endpoint.clone(),
            rds_net::AnnounceConfig {
                key: secret_key.clone(),
                directory: rds_discovery::client::Client::new(addr),
                services: vec![
                    rds_discovery::Service::Ping,
                    rds_discovery::Service::TcpForward,
                ],
                ttl: std::time::Duration::from_secs(cli.record_ttl),
            },
        )
    });

    // Denylist feed: poll the estate-signed revocation snapshot so
    // revoked grants die on live connections too, not just new ones.
    let _revocations = match (cli.directory, &cli.revocations_key) {
        (Some(addr), Some(s)) => {
            let bytes = data_encoding::BASE32_NOPAD
                .decode(s.to_uppercase().as_bytes())
                .map_err(|e| anyhow::anyhow!("--revocations-key not base32: {e}"))?;
            let raw: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("--revocations-key is not 32 bytes"))?;
            let key = ed25519_dalek::VerifyingKey::from_bytes(&raw)
                .map_err(|e| anyhow::anyhow!("--revocations-key invalid: {e}"))?;
            Some(rds_agent::watch_revocations(
                rds_discovery::client::Client::new(addr),
                key,
                policy_handle.clone(),
                std::time::Duration::from_secs(cli.revocations_interval),
            ))
        }
        (Some(_), None) => {
            eprintln!("note: --directory without --revocations-key — no denylist feed");
            None
        }
        (None, _) => None,
    };

    let agent = Agent::new(endpoint, policy);
    println!("endpoint id: {}", agent.id());
    println!("ticket: {}", Ticket::of(&agent.endpoint));
    if agent.policy.allow.is_empty() {
        eprintln!("warning: empty --allow list; every peer will be rejected");
    }
    tokio::select! {
        res = agent.run() => res?,
        _ = shutdown_signal() => {}
    }
    // Close the endpoint so peers get CONNECTION_CLOSE instead of an
    // abrupt socket death (and iroh does not log an ungraceful drop).
    agent.endpoint.close().await;
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
