//! Keyless commands for the running agent's shared endpoint.
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use rds_client::local::Client;
use rds_core::local::{Command, Reply, SessionId};

#[derive(Args)]
pub struct Options {
    /// Private control directory configured on the local rds-agent.
    #[arg(long)]
    control_dir: PathBuf,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// List live connections, pending dials and the selected device.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Keep a connection open using the agent identity; print its session ID.
    Connect {
        /// Pinned ticket/key, or a name when the agent has directory name trust.
        target: String,
        /// Grant for the local agent's identity (JSON, at most 64 KiB).
        #[arg(long)]
        grant_file: Option<PathBuf>,
    },
    /// Select a device for subsequent commands; existing streams stay pinned.
    Use { session: SessionId },
    /// Close a connection and all its streams, or cancel a pending dial.
    Disconnect { session: SessionId },
    /// Ping the selected or explicitly specified session.
    Ping {
        #[arg(long)]
        session: Option<SessionId>,
    },
    /// Read capabilities of the selected or explicitly specified session.
    Info {
        #[arg(long)]
        session: Option<SessionId>,
    },
    /// Forward a local port to sshd through an existing managed connection.
    Ssh {
        #[arg(long)]
        session: Option<SessionId>,
        #[arg(short = 'L', long, default_value = "127.0.0.1:2222")]
        bind: SocketAddr,
        #[arg(long, default_value = "127.0.0.1:22")]
        remote: rds_core::TcpTarget,
        #[arg(long, default_value = "64")]
        max_connections: std::num::NonZeroU16,
    },
    /// Forward to a peer-side TCP target through an existing connection.
    Forward {
        #[arg(long)]
        session: Option<SessionId>,
        #[arg(short = 'L', long, default_value = "127.0.0.1:0")]
        bind: SocketAddr,
        #[arg(long)]
        remote: rds_core::TcpTarget,
        #[arg(long, default_value = "64")]
        max_connections: std::num::NonZeroU16,
    },
}

pub async fn run(options: Options) -> anyhow::Result<()> {
    let client = Client::new(options.control_dir);
    let command = match options.command {
        Action::List { json } => {
            let snapshot = client.snapshot().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!("  SESSION                           STATE       PEER");
                for session in snapshot.sessions {
                    let selected = if snapshot.selected == Some(session.id) {
                        '*'
                    } else {
                        ' '
                    };
                    let status = match session.status {
                        rds_core::local::Status::Connecting => "connecting",
                        rds_core::local::Status::Connected => "connected",
                    };
                    println!("{selected} {}  {status:10}  {}", session.id, session.peer);
                }
            }
            return Ok(());
        }
        Action::Connect { target, grant_file } => {
            let grant = match grant_file {
                Some(path) => Some(Box::new(
                    tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                        use std::io::Read;
                        let mut bytes = Vec::new();
                        std::fs::File::open(path)?
                            .take(65537)
                            .read_to_end(&mut bytes)?;
                        anyhow::ensure!(bytes.len() <= 65536, "grant file exceeds 64 KiB");
                        Ok(serde_json::from_slice::<rds_core::grant::Grant>(&bytes)?)
                    })
                    .await??,
                )),
                None => None,
            };
            Command::Connect { target, grant }
        }
        Action::Use { session } => Command::Select { session },
        Action::Disconnect { session } => Command::Disconnect { session },
        Action::Ping { session } => Command::Ping { session, nonce: 1 },
        Action::Info { session } => Command::Info { session },
        Action::Ssh {
            session,
            bind,
            remote,
            max_connections,
        }
        | Action::Forward {
            session,
            bind,
            remote,
            max_connections,
        } => {
            anyhow::ensure!(
                bind.ip().is_loopback(),
                "managed forwarding requires a loopback listen address"
            );
            let session = client.selected(session).await?;
            let listener = tokio::net::TcpListener::bind(bind).await?;
            eprintln!(
                "session {session}: listening on {} -> {remote}",
                listener.local_addr()?
            );
            return tokio::select! {
                result = client.forward(session, listener, remote, max_connections) => result.map_err(Into::into),
                result = tokio::signal::ctrl_c() => result.map_err(Into::into),
            };
        }
    };
    match client.request(command).await? {
        Reply::Connected(id) => println!("{id}"),
        Reply::Done => {}
        reply => println!("{}", serde_json::to_string_pretty(&reply)?),
    }
    Ok(())
}
