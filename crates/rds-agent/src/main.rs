//! `rds-agent`: daemon on a controlled device.

use std::str::FromStr;

use clap::Parser;
use iroh::EndpointId;
use rds_agent::{Agent, AgentPolicy};
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
    /// Custom relay URL; default is the n0 public relays.
    #[arg(long)]
    relay: Option<String>,
    /// Allowed peer EndpointId. Repeatable.
    #[arg(long = "allow")]
    allow: Vec<String>,
    /// SSH socket the TcpConnect service may reach.
    #[arg(long, default_value = "127.0.0.1:22")]
    ssh: String,
    /// Permit TcpConnect to any host:port (development only).
    #[arg(long)]
    allow_any_tcp: bool,
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

    let mut config = EndpointConfig {
        secret_key: Some(secret_key),
        ..Default::default()
    };
    if let Some(url) = &cli.relay {
        config = config.with_relay(url)?;
    }

    let endpoint = bind_endpoint(config).await?;
    endpoint.online().await;

    let agent = Agent::new(endpoint, policy);
    println!("endpoint id: {}", agent.id());
    println!("ticket: {}", Ticket::of(&agent.endpoint));
    if agent.policy.allow.is_empty() {
        eprintln!("warning: empty --allow list; every peer will be rejected");
    }
    agent.run().await
}
