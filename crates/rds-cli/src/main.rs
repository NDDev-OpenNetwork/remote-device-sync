//! `rds`: operator CLI for remote device access.

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use rds_net::{EndpointConfig, Ticket, bind_endpoint, default_key_path, load_or_create_key};

#[derive(Parser)]
#[command(version, about = "Remote device access for the GDS estate")]
struct Cli {
    /// Path to the endpoint secret key.
    #[arg(long, global = true)]
    key_file: Option<std::path::PathBuf>,
    /// Custom relay URL; default is the n0 public relays. Repeatable —
    /// ≥2 relays give automatic client-side failover.
    #[arg(long, global = true)]
    relay: Vec<String>,
    /// Transport backend: `iroh` (default) or `noq` (with the
    /// `transport-noq` feature).
    #[arg(long, global = true, default_value = "iroh")]
    backend: String,
    /// Discovery directory address. Enables bare-key lookups and GDS
    /// device-name resolution; tickets still work without it.
    #[arg(long, global = true)]
    server: Option<SocketAddr>,
    /// Trusted registry verifying key (base32), provisioned by GDS.
    /// Required for device names; tickets and pinned keys are independent.
    #[arg(long, global = true, requires = "server")]
    registry_key: Option<String>,
    /// Capability grant file (JSON `Grant` as minted by the estate).
    /// Required when the target agent runs in grant mode.
    #[arg(long, global = true)]
    grant: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
        remote: String,
    },
    /// Forward a local port to an arbitrary peer-side TCP target.
    Forward {
        target: String,
        #[arg(short = 'L', long, default_value = "127.0.0.1:0")]
        bind: SocketAddr,
        /// Remote target host:port.
        #[arg(long)]
        remote: String,
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
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let cli = Cli::parse();

    let key = match &cli.key_file {
        Some(path) => Some(load_or_create_key(path)?),
        None => default_key_path()
            .map(|p| load_or_create_key(&p))
            .transpose()?,
    };
    // `id` is a pure function of the key: no socket, no relay contact.
    // (An unresolvable key previously fell back to an ephemeral identity,
    // printing a different id on every run.)
    if let Command::Id = cli.command {
        let key = key.ok_or_else(|| {
            anyhow::anyhow!("no endpoint key: pass --key-file or set HOME/XDG_CONFIG_HOME")
        })?;
        println!("{}", key.public());
        return Ok(());
    }
    let backend = match cli.backend.as_str() {
        "iroh" => rds_net::Backend::Iroh,
        #[cfg(feature = "transport-noq")]
        "noq" => rds_net::Backend::Noq,
        other => anyhow::bail!("unknown or unavailable backend {other:?}"),
    };
    let mut config = EndpointConfig {
        secret_key: key,
        backend,
        ..Default::default()
    };
    config = config.with_relays(&cli.relay)?;
    let directory = cli
        .server
        .map(|addr| {
            let client = rds_discovery::client::Client::new(addr);
            match cli.registry_key.as_deref() {
                Some(key) => client.with_registry_key_base32(key),
                None => Ok(client),
            }
        })
        .transpose()?;
    let endpoint = bind_endpoint(config).await?;
    let grant = cli
        .grant
        .as_deref()
        .map(|p| -> anyhow::Result<rds_core::grant::Grant> {
            Ok(serde_json::from_slice(&std::fs::read(p)?)?)
        })
        .transpose()?;
    match cli.command {
        Command::Id => unreachable!("handled before endpoint bind"),
        Command::Ticket => {
            // Bound the wait: an unreachable relay must not hang the
            // command; the ticket still carries any resolved addresses.
            if tokio::time::timeout(std::time::Duration::from_secs(15), endpoint.online())
                .await
                .is_err()
            {
                eprintln!("warning: relay unreachable after 15s; ticket may lack a relay address");
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
            // Path evidence for ops/reports: which transport path the
            // connection actually selected, with its smoothed RTT.
            for p in conn.path_stats() {
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
        } => {
            let conn = Arc::new(
                dial(
                    &endpoint,
                    resolve(&directory, &target).await?,
                    grant.clone(),
                )
                .await?,
            );
            let (host, port) = parse_host_port(&remote)?;
            eprintln!(
                "endpoint {} — run: ssh -p {} <user>@{}",
                conn.remote_id(),
                bind.port(),
                bind.ip()
            );
            rds_cli::forward_listener(conn, bind, host, port).await?;
        }
        Command::Forward {
            target,
            bind,
            remote,
        } => {
            let conn = Arc::new(
                dial(
                    &endpoint,
                    resolve(&directory, &target).await?,
                    grant.clone(),
                )
                .await?,
            );
            let (host, port) = parse_host_port(&remote)?;
            rds_cli::forward_listener(conn, bind, host, port).await?;
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
                anyhow::bail!("rds built without desktop support; enable the `desktop` feature");
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
    // Dropping the endpoint without close() makes iroh log an
    // "ungraceful abort" error on every command exit.
    endpoint.close().await;
    Ok(())
}

fn parse_host_port(s: &str) -> anyhow::Result<(String, u16)> {
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected host:port, got {s}"))?;
    Ok((host.to_string(), port.parse()?))
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
