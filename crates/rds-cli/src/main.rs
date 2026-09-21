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
    /// Custom relay URL; default is the n0 public relays.
    #[arg(long, global = true)]
    relay: Option<String>,
    /// Transport backend: `iroh` (default) or `noq` (with the
    /// `transport-noq` feature).
    #[arg(long, global = true, default_value = "iroh")]
    backend: String,
    /// Discovery directory address. Enables bare-key lookups and GDS
    /// device-name resolution; tickets still work without it.
    #[arg(long, global = true)]
    server: Option<SocketAddr>,
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
    if let Some(url) = &cli.relay {
        config = config.with_relay(url)?;
    }
    let endpoint = bind_endpoint(config).await?;
    let directory = cli.server.map(rds_discovery::client::Client::new);

    match cli.command {
        Command::Id => println!("{}", endpoint.id()),
        Command::Ticket => {
            endpoint.online().await;
            println!("{}", Ticket::of(&endpoint));
        }
        Command::Ping { target, count } => {
            let conn = rds_cli::connect(&endpoint, resolve(&directory, &target).await?).await?;
            println!("connected to {}", conn.remote_id());
            for i in 0..count {
                let rtt = rds_cli::ping(&conn, i as u64).await?;
                println!("pong seq={i} rtt={:.1}ms", rtt.as_secs_f64() * 1000.0);
            }
        }
        Command::Info { target } => {
            let conn = rds_cli::connect(&endpoint, resolve(&directory, &target).await?).await?;
            let info = rds_cli::info(&conn).await?;
            println!("{info:#?}");
        }
        Command::Ssh {
            target,
            bind,
            remote,
        } => {
            let conn =
                Arc::new(rds_cli::connect(&endpoint, resolve(&directory, &target).await?).await?);
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
            let conn =
                Arc::new(rds_cli::connect(&endpoint, resolve(&directory, &target).await?).await?);
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
                let conn = rds_cli::connect(&endpoint, resolve(&directory, &target).await?).await?;
                rds_desktop::client::run_desktop_client(conn, display, max_fps).await?;
            }
            #[cfg(not(feature = "desktop"))]
            {
                let _ = (target, display, max_fps);
                anyhow::bail!("rds built without desktop support; enable the `desktop` feature");
            }
        }
    }
    Ok(())
}

fn parse_host_port(s: &str) -> anyhow::Result<(String, u16)> {
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected host:port, got {s}"))?;
    Ok((host.to_string(), port.parse()?))
}

async fn resolve(
    directory: &Option<rds_discovery::client::Client>,
    target: &str,
) -> anyhow::Result<rds_net::EndpointAddr> {
    rds_net::resolve_target(directory.clone(), target).await
}
