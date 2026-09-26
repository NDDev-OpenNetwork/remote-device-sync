//! Keyless commands for the running agent's shared endpoint.
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use rds_client::local::Client;
use rds_core::local::{Command, Reply, SessionId};

#[derive(Args)]
pub struct Options {
    #[command(subcommand)]
    command: Action,
}

impl Options {
    pub fn is_ssh(&self) -> bool {
        matches!(self.command, Action::Ssh { .. })
    }
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
    /// Extend a pinned session using a signed grant revision with the same scope.
    Renew {
        #[arg(long)]
        session: SessionId,
        #[arg(long)]
        grant_file: PathBuf,
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
    /// Open SSH on the selected or explicitly pinned managed connection.
    Ssh {
        #[arg(long)]
        session: Option<SessionId>,
        #[command(flatten)]
        options: super::ssh::Options,
    },
    /// Send a file through the selected or explicitly pinned session.
    Send {
        #[arg(long)]
        session: Option<SessionId>,
        path: PathBuf,
    },
    /// Receive a file through the selected or explicitly pinned session.
    Recv {
        #[arg(long)]
        session: Option<SessionId>,
        rel_path: String,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
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

pub async fn run(options: Options, directory: PathBuf) -> anyhow::Result<()> {
    let client = Client::new(directory);
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
            let grant = read_grant(grant_file).await?;
            Command::Connect { target, grant }
        }
        Action::Renew {
            session,
            grant_file,
        } => {
            let grant = read_grant(Some(grant_file))
                .await?
                .ok_or_else(|| anyhow::anyhow!("grant required"))?;
            Command::Renew { session, grant }
        }
        Action::Use { session } => Command::Select { session },
        Action::Disconnect { session } => Command::Disconnect { session },
        Action::Ping { session } => Command::Ping { session, nonce: 1 },
        Action::Info { session } => Command::Info { session },
        Action::Ssh { session, options } => {
            let session = client.selected(session).await?;
            return super::ssh::managed(&client, session, options).await;
        }
        Action::Send { session, path } => {
            let session = client.selected(session).await?;
            return send_file(&client, session, path).await;
        }
        Action::Recv {
            session,
            rel_path,
            dir,
        } => {
            let session = client.selected(session).await?;
            return recv_file(&client, session, rel_path, dir).await;
        }
        Action::Forward {
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
            return forward(&client, session, bind, remote, max_connections).await;
        }
    };
    match client.request(command).await? {
        Reply::Connected(id) => println!("{id}"),
        Reply::Done => {}
        reply => println!("{}", serde_json::to_string_pretty(&reply)?),
    }
    Ok(())
}

pub async fn read_grant(
    path: Option<PathBuf>,
) -> anyhow::Result<Option<Box<rds_core::grant::Grant>>> {
    match path {
        Some(path) => Ok(Some(Box::new(
            tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                use rustix::fs::{Mode, OFlags};
                use std::io::Read;
                let file = std::fs::File::from(rustix::fs::open(
                    &path,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )?);
                anyhow::ensure!(file.metadata()?.is_file(), "grant must be a regular file");
                let mut bytes = Vec::new();
                file.take(65537).read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 65536, "grant file exceeds 64 KiB");
                Ok(serde_json::from_slice(&bytes)?)
            })
            .await??,
        ))),
        None => Ok(None),
    }
}

/// Ordinary connectivity commands share agent sessions. No key load, endpoint
/// bind or direct fallback is permitted when the manager cannot be reached.
pub async fn run_default(
    command: super::Command,
    directory: PathBuf,
    grant: Option<PathBuf>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let client = Client::new(directory);
    let target = match &command {
        super::Command::Ticket => {
            anyhow::ensure!(grant.is_none(), "ticket does not accept a grant");
            if let Reply::Ticket(ticket) = client
                .request(Command::Ticket)
                .await
                .context("cannot reach local agent; start rds-agent or set --control-dir")?
            {
                println!("{ticket}");
                return Ok(());
            }
            anyhow::bail!("unexpected local ticket response");
        }
        super::Command::Ping { target, .. }
        | super::Command::Info { target }
        | super::Command::Ssh { target, .. }
        | super::Command::Send { target, .. }
        | super::Command::Recv { target, .. } => target,
        super::Command::Forward { target, bind, .. } => {
            anyhow::ensure!(
                bind.ip().is_loopback(),
                "managed forwarding requires a loopback listen address"
            );
            target
        }
        super::Command::Desktop { .. } => {
            anyhow::bail!(
                "desktop does not yet have a manager API; use --direct with a separate --key-file"
            );
        }
        _ => anyhow::bail!("unsupported managed command"),
    };
    let grant = read_grant(grant).await?;
    let reply = client
        .request(Command::Connect {
            target: target.clone(),
            grant,
        })
        .await
        .context(
            "managed connection failed; ensure rds-agent is running and configured for this peer",
        )?;
    let Reply::Connected(session) = reply else {
        anyhow::bail!("unexpected local connection response");
    };
    match command {
        super::Command::Ping { count, .. } => {
            for nonce in 0..count {
                let reply = client
                    .request(Command::Ping {
                        session: Some(session),
                        nonce: nonce.into(),
                    })
                    .await?;
                let Reply::Pong {
                    session: returned,
                    micros,
                } = reply
                else {
                    anyhow::bail!("unexpected local ping response");
                };
                anyhow::ensure!(returned == session, "local session mismatch");
                println!("pong seq={nonce} rtt={:.1}ms", micros as f64 / 1000.0);
            }
        }
        super::Command::Info { .. } => {
            let reply = client
                .request(Command::Info {
                    session: Some(session),
                })
                .await?;
            let Reply::Info {
                session: returned,
                info,
            } = reply
            else {
                anyhow::bail!("unexpected local info response");
            };
            anyhow::ensure!(returned == session, "local session mismatch");
            println!("{info:#?}");
        }
        super::Command::Ssh { options, .. } => {
            super::ssh::managed(&client, session, options).await?
        }
        super::Command::Send { path, .. } => send_file(&client, session, path).await?,
        super::Command::Recv { rel_path, dir, .. } => {
            recv_file(&client, session, rel_path, dir).await?
        }
        super::Command::Forward {
            bind,
            remote,
            max_connections,
            ..
        } => {
            forward(&client, session, bind, remote, max_connections).await?;
        }
        _ => anyhow::bail!("unsupported managed command"),
    }
    Ok(())
}

async fn send_file(client: &Client, session: SessionId, path: PathBuf) -> anyhow::Result<()> {
    let stats = tokio::select! {
        result = client.send_file(session, &path) => result?,
        _ = tokio::signal::ctrl_c() => anyhow::bail!("transfer canceled; a started commit may still complete; reconcile before retrying"),
    };
    println!(
        "sent: {} bytes, {}/{} chunks",
        stats.bytes, stats.fetched, stats.total
    );
    Ok(())
}

async fn recv_file(
    client: &Client,
    session: SessionId,
    rel_path: String,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let stats = tokio::select! {
        result = client.recv_file(session, &rel_path, &dir) => result?,
        _ = tokio::signal::ctrl_c() => anyhow::bail!("transfer canceled; a started commit may still complete; reconcile before retrying"),
    };
    println!(
        "received: {} bytes, {}/{} chunks",
        stats.bytes, stats.fetched, stats.total
    );
    Ok(())
}

async fn forward(
    client: &Client,
    session: SessionId,
    bind: SocketAddr,
    remote: rds_core::TcpTarget,
    max_connections: std::num::NonZeroU16,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!(
        "session {session}: listening on {} -> {remote}",
        listener.local_addr()?
    );
    tokio::select! {
        result = client.forward(session, listener, remote, max_connections) => result.map_err(Into::into),
        result = tokio::signal::ctrl_c() => result.map_err(Into::into),
    }
}
