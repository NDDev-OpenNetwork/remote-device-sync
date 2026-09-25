//! `rds`: operator CLI for remote device access.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use rds_net::{
    EndpointOverrides, EndpointSettings, Ticket, acquire_key, bind_endpoint, default_key_path,
    load_or_create_key,
};

mod managed;

#[derive(Parser)]
#[command(version, about = "Remote device access for the GDS estate")]
struct Cli {
    /// Use a separate endpoint explicitly; its identity must not be in use.
    #[arg(long, global = true, conflicts_with = "control_dir")]
    direct: bool,
    /// Agent control directory; default is beside the default endpoint key.
    #[arg(long, global = true)]
    control_dir: Option<std::path::PathBuf>,
    /// Path to the endpoint secret key.
    #[arg(long, global = true)]
    key_file: Option<std::path::PathBuf>,
    /// Custom relay URL; default is the n0 public relays. Repeatable —
    /// ≥2 relays give automatic client-side failover.
    #[arg(long, global = true, conflicts_with_all = ["owned_relay", "no_relay"])]
    relay: Vec<String>,
    /// Transport backend: `iroh` (default) or `noq` (with the
    /// `transport-noq` feature).
    #[arg(long, global = true)]
    backend: Option<rds_net::Backend>,
    /// Versioned endpoint JSON configuration. Explicit flags override it.
    #[arg(long, global = true)]
    endpoint_config: Option<std::path::PathBuf>,
    /// Key-pinned owned relay: rds-relay://PUBLIC_HEX_KEY@IP:PORT.
    #[arg(long, global = true, conflicts_with_all = ["relay", "no_relay"])]
    owned_relay: Option<String>,
    /// Disable relay and public address lookup services.
    #[arg(long, global = true, conflicts_with_all = ["relay", "owned_relay"])]
    no_relay: bool,
    /// Local UDP bind address; repeatable on the owned backend.
    #[arg(long, global = true)]
    bind_address: Vec<SocketAddr>,
    /// Directory HTTP(S) origin or legacy IP:port. Enables bare-key lookups and GDS
    /// device-name resolution; tickets still work without it.
    #[arg(long, global = true)]
    server: Option<String>,
    /// PEM CA bundle for directory HTTPS; replaces the public root store.
    #[arg(long, global = true, requires = "server")]
    directory_ca: Option<std::path::PathBuf>,
    /// Trusted registry verifying key (base32), provisioned by GDS.
    /// Required for device names; tickets and pinned keys are independent.
    #[arg(long, global = true, requires = "server")]
    registry_key: Option<String>,
    /// Bootstrap registry authority epoch.
    #[arg(long, global = true, default_value = "1")]
    registry_epoch: u64,
    /// Private name-trust state directory; default is beside the endpoint key.
    #[arg(long, global = true, requires = "registry_key")]
    registry_state: Option<std::path::PathBuf>,
    /// Dual-signed authority rotation receipt. Repeat in epoch order.
    #[arg(long, global = true, requires = "registry_key")]
    authority_rotation: Vec<std::path::PathBuf>,
    /// Capability grant file (JSON `Grant` as minted by the estate).
    /// Required when the target agent runs in grant mode.
    #[arg(long, global = true)]
    grant: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage connections through the local agent; no key loading or UDP bind.
    Session(managed::Options),
    /// Create a new private admin scrape credential without endpoint initialization.
    AdminToken {
        #[arg(long)]
        file: std::path::PathBuf,
    },
    /// Print this device's endpoint id.
    Id,
    /// Print this device's dialable ticket.
    Ticket,
    /// Measure round-trip latency to a peer.
    Ping {
        /// Peer ticket or bare endpoint id.
        target: String,
        /// Number of probes.
        #[arg(short = 'c', long, default_value = "4")]
        count: u32,
    },
    /// Print peer metadata (version, services, displays).
    Info { target: String },
    /// Local port forward to the peer's sshd. Then: `ssh -p PORT localhost`.
    Ssh {
        target: String,
        /// Local listen address.
        #[arg(short = 'L', long, default_value = "127.0.0.1:2222")]
        bind: SocketAddr,
        /// Remote sshd address.
        #[arg(long, default_value = "127.0.0.1:22")]
        remote: rds_core::TcpTarget,
        /// Maximum simultaneous local forwarding workers (positive 16-bit).
        #[arg(long, default_value = "64")]
        max_connections: std::num::NonZeroU16,
    },
    /// Forward a local port to an arbitrary peer-side TCP target.
    Forward {
        target: String,
        #[arg(short = 'L', long, default_value = "127.0.0.1:0")]
        bind: SocketAddr,
        /// Remote target host:port.
        #[arg(long)]
        remote: rds_core::TcpTarget,
        /// Maximum simultaneous local forwarding workers (positive 16-bit).
        #[arg(long, default_value = "64")]
        max_connections: std::num::NonZeroU16,
    },
    /// Open a remote desktop session (requires the `desktop` feature).
    Desktop {
        target: String,
        #[arg(long, default_value = "0")]
        display: u32,
        #[arg(long, default_value = "30")]
        max_fps: u32,
    },
    /// Push a file into the peer's sync directory (resumable).
    Send {
        target: String,
        /// Local file to send.
        path: std::path::PathBuf,
    },
    /// Pull a file from the peer's sync directory into `dir`.
    Recv {
        target: String,
        /// Relative path inside the peer's sync directory.
        rel_path: String,
        /// Local directory to receive into.
        #[arg(long, default_value = ".")]
        dir: std::path::PathBuf,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    rds_observe::run_main(rds_observe::Service::Cli, "warn", run(cli)).await
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    if matches!(cli.command, Command::Session(_)) {
        validate_managed(&cli)?;
        anyhow::ensure!(
            !cli.direct && cli.grant.is_none(),
            "session commands use agent configuration; omit --direct and use connect --grant-file for credentials"
        );
        let directory = control_directory(&cli)?;
        if let Command::Session(options) = cli.command {
            return managed::run(options, directory).await;
        }
    }
    if let Command::AdminToken { file } = &cli.command {
        let file = file.clone();
        tokio::task::spawn_blocking(move || rds_observe::admin::Token::create(&file)).await??;
        return Ok(());
    }
    if !cli.direct && !matches!(cli.command, Command::Id) {
        validate_managed(&cli)?;
        let directory = control_directory(&cli)?;
        return managed::run_default(cli.command, directory, cli.grant).await;
    }
    anyhow::ensure!(
        cli.control_dir.is_none(),
        "--control-dir applies to managed commands; use session list --json to inspect the running identity"
    );
    let mut config = cli
        .endpoint_config
        .as_deref()
        .map(EndpointSettings::load)
        .transpose()?
        .unwrap_or_default()
        .apply(EndpointOverrides {
            backend: cli.backend,
            bind_addrs: cli.bind_address,
            relays: cli.relay,
            owned_relay: cli.owned_relay,
            no_relay: cli.no_relay,
        })?
        .into_endpoint()?;

    let key_path = cli
        .key_file
        .clone()
        .or_else(default_key_path)
        .ok_or_else(|| {
            anyhow::anyhow!("no endpoint key: pass --key-file or set HOME/XDG_CONFIG_HOME")
        })?;
    // `id` is a pure function of the key: no socket, no relay contact.
    // (An unresolvable key previously fell back to an ephemeral identity,
    // printing a different id on every run.)
    if let Command::Id = cli.command {
        let key = tokio::task::spawn_blocking(move || load_or_create_key(&key_path)).await??;
        println!("{}", key.public());
        return Ok(());
    }
    let identity = tokio::task::spawn_blocking(move || acquire_key(&key_path)).await??;
    config.secret_key = Some(identity.secret_key().clone());
    let directory = cli
        .server
        .as_deref()
        .map(|origin| -> anyhow::Result<_> {
            let mut client = rds_discovery::client::Client::from_endpoint(origin)?;
            if let Some(path) = &cli.directory_ca {
                client = client.with_ca_pem(&std::fs::read(path)?)?;
            }
            if let Some(key) = cli.registry_key.as_deref() {
                let authority =
                    rds_discovery::authority::Authority::from_base32(key, cli.registry_epoch)?;
                let path = cli
                    .registry_state
                    .clone()
                    .or_else(|| {
                        cli.key_file
                            .clone()
                            .or_else(default_key_path)
                            .map(|key| key.with_extension("registry-state"))
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "name resolution requires --registry-state or a key/config directory"
                        )
                    })?;
                let rotations = rds_discovery::policy::read_rotations(&cli.authority_rotation)?;
                client = client.with_registry_store(authority, path, rotations)?;
            }
            Ok(client)
        })
        .transpose()?;
    let grant = managed::read_grant(cli.grant).await?.map(|grant| *grant);
    let endpoint = bind_endpoint(config).await?;
    let result = async {
        match cli.command {
            Command::Session(_) => {
                unreachable!("local session handled before endpoint initialization")
            }
            Command::AdminToken { .. } => {
                unreachable!("admin token handled before endpoint initialization")
            }
            Command::Id => unreachable!("handled before endpoint bind"),
            Command::Ticket => {
                // Bound the wait: an unreachable relay must not hang the
                // command; the ticket still carries any resolved addresses.
                if tokio::time::timeout(std::time::Duration::from_secs(15), endpoint.online())
                    .await
                    .is_err()
                {
                    eprintln!(
                        "warning: relay unreachable after 15s; ticket may lack a relay address"
                    );
                }
                println!("{}", Ticket::of(&endpoint));
            }
            Command::Ping { target, count } => {
                let conn = dial(
                    &endpoint,
                    resolve(&directory, &target).await?,
                    grant.clone(),
                )
                .await?;
                println!("connected to {}", conn.remote_id());
                for i in 0..count {
                    let rtt = rds_cli::ping(&conn, i as u64).await?;
                    println!("pong seq={i} rtt={:.1}ms", rtt.as_secs_f64() * 1000.0);
                }
                let snapshot = conn.path_stats_snapshot();
                println!("path coverage={:?}", snapshot.coverage);
                for p in snapshot.paths {
                    println!(
                        "path id={} via={} selected={} rtt={:.1}ms sent={} lost={} cwnd={}",
                        p.path_id,
                        if p.via_relay { "relay" } else { "direct" },
                        p.selected,
                        p.rtt.as_secs_f64() * 1000.0,
                        p.sent,
                        p.lost,
                        p.cwnd,
                    );
                }
            }
            Command::Info { target } => {
                let conn = dial(
                    &endpoint,
                    resolve(&directory, &target).await?,
                    grant.clone(),
                )
                .await?;
                let info = rds_cli::info(&conn).await?;
                println!("{info:#?}");
            }
            Command::Ssh {
                target,
                bind,
                remote,
                max_connections,
            } => {
                let conn = Arc::new(
                    dial(
                        &endpoint,
                        resolve(&directory, &target).await?,
                        grant.clone(),
                    )
                    .await?,
                );
                let listener = tokio::net::TcpListener::bind(bind).await?;
                let local = listener.local_addr()?;
                eprintln!(
                    "endpoint {} — run: ssh -p {} <user>@{}",
                    conn.remote_id(),
                    local.port(),
                    local.ip()
                );
                rds_cli::forward_bound_listener(conn, listener, remote, max_connections).await?;
            }
            Command::Forward {
                target,
                bind,
                remote,
                max_connections,
            } => {
                let conn = Arc::new(
                    dial(
                        &endpoint,
                        resolve(&directory, &target).await?,
                        grant.clone(),
                    )
                    .await?,
                );
                let listener = tokio::net::TcpListener::bind(bind).await?;
                rds_cli::forward_bound_listener(conn, listener, remote, max_connections).await?;
            }
            Command::Desktop {
                target,
                display,
                max_fps,
            } => {
                #[cfg(feature = "desktop")]
                {
                    let conn = dial(
                        &endpoint,
                        resolve(&directory, &target).await?,
                        grant.clone(),
                    )
                    .await?;
                    rds_desktop::client::run_desktop_client(conn, display, max_fps).await?;
                }
                #[cfg(not(feature = "desktop"))]
                {
                    let _ = (target, display, max_fps);
                    anyhow::bail!(
                        "rds built without desktop support; enable the `desktop` feature"
                    );
                }
            }
            Command::Send { target, path } => {
                let conn = dial(
                    &endpoint,
                    resolve(&directory, &target).await?,
                    grant.clone(),
                )
                .await?;
                let (send, recv) = rds_cli::open_sync(&conn).await?;
                let stats = rds_sync::engine::send_file(&conn, &path, send, recv).await?;
                println!(
                    "sent {} ({} chunks, {} bytes)",
                    path.display(),
                    stats.total,
                    stats.bytes
                );
            }
            Command::Recv {
                target,
                rel_path,
                dir,
            } => {
                let conn = dial(
                    &endpoint,
                    resolve(&directory, &target).await?,
                    grant.clone(),
                )
                .await?;
                let (send, recv) = rds_cli::open_sync(&conn).await?;
                let (dest, stats) =
                    rds_sync::engine::recv_file(&conn, &rel_path, &dir, send, recv).await?;
                println!(
                    "received {} ({} chunks fetched, {} bytes)",
                    dest.display(),
                    stats.fetched,
                    stats.bytes
                );
            }
        }
        Ok(())
    }
    .await;
    // Dropping the endpoint without close() makes iroh log an
    // "ungraceful abort" error on every command exit.
    endpoint.close().await;
    result
}

fn validate_managed(cli: &Cli) -> anyhow::Result<()> {
    anyhow::ensure!(
        cli.key_file.is_none()
            && cli.relay.is_empty()
            && cli.backend.is_none()
            && cli.endpoint_config.is_none()
            && cli.owned_relay.is_none()
            && !cli.no_relay
            && cli.bind_address.is_empty()
            && cli.server.is_none()
            && cli.directory_ca.is_none()
            && cli.registry_key.is_none()
            && cli.registry_epoch == 1
            && cli.registry_state.is_none()
            && cli.authority_rotation.is_empty(),
        "managed commands use agent configuration; configure the agent or use --direct with a separate identity"
    );
    Ok(())
}

fn control_directory(cli: &Cli) -> anyhow::Result<std::path::PathBuf> {
    match &cli.control_dir {
        Some(path) => Ok(path.clone()),
        None => Ok(rds_client::local::control_dir_for_key(
            &default_key_path().ok_or_else(|| {
                anyhow::anyhow!(
                    "no control directory: pass --control-dir or set HOME/XDG_CONFIG_HOME"
                )
            })?,
        )?),
    }
}

async fn dial(
    endpoint: &rds_net::Endpoint,
    target: rds_net::EndpointAddr,
    grant: Option<rds_core::grant::Grant>,
) -> anyhow::Result<rds_net::Connection> {
    match grant {
        Some(g) => rds_cli::connect_authorized(endpoint, target, &g).await,
        None => rds_cli::connect(endpoint, target).await,
    }
}

async fn resolve(
    directory: &Option<rds_discovery::client::Client>,
    target: &str,
) -> anyhow::Result<rds_net::EndpointAddr> {
    rds_net::resolve_target(directory.clone(), target).await
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn tcp_target_flags_reject_invalid_destinations() {
        for command in ["ssh", "forward"] {
            for target in [":22", "host:0", "host:nope", "::1:22", "[::1]22"] {
                assert!(
                    Cli::try_parse_from(["rds", command, "unused-peer", "--remote", target])
                        .is_err(),
                    "{command}: {target}"
                );
            }
        }
    }
}
