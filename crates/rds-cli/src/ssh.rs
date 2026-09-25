//! CLI policy and lifecycle; the SSH implementation lives in rds-ssh.
use std::{path::PathBuf, sync::Arc};

use anyhow::Context as _;
use clap::Args;
use rds_client::local::Client;
use rds_core::local::SessionId;
use rds_ssh::{Authentication, Config, Exit, PublicKey, Request, Session};

#[cfg(unix)]
#[path = "ssh/unix.rs"]
mod platform;

#[derive(Args)]
pub struct Options {
    /// Remote account (explicit; never defaults to root).
    #[arg(short = 'l', long)]
    user: String,
    /// Trusted OpenSSH host public key, provisioned out of band for this peer/target.
    #[arg(long)]
    host_key: PathBuf,
    /// Private OpenSSH key. For encrypted keys, use --agent-key instead.
    #[arg(
        short = 'i',
        long,
        required_unless_present = "agent_key",
        conflicts_with = "agent_key"
    )]
    identity: Option<PathBuf>,
    /// Public key to use through SSH_AUTH_SOCK; no agent forwarding.
    #[arg(long)]
    agent_key: Option<PathBuf>,
    /// Remote SSH server address, still constrained by the agent's TCP policy.
    #[arg(long, default_value = "127.0.0.1:22")]
    pub remote: rds_core::TcpTarget,
    /// One exact command string interpreted by the remote account's shell.
    #[arg(long)]
    exec: Option<String>,
    /// Allocate a PTY for --exec (interactive shell allocates one by default).
    #[arg(short = 't', long, conflicts_with = "no_pty")]
    pty: bool,
    /// Disable PTY allocation, including for a shell; keep stdout/stderr separate.
    #[arg(short = 'T', long)]
    no_pty: bool,
}

#[derive(Debug)]
pub struct ExitStatus(pub u8);

impl std::fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "remote command exited with status {}", self.0)
    }
}
impl std::error::Error for ExitStatus {}

pub async fn managed(client: &Client, session: SessionId, options: Options) -> anyhow::Result<()> {
    let prepared = prepare(options).await?;
    // open_tcp uses the pinned session ID, never the mutable selection. Restart
    // cannot alias an old session. No local listen port or localhost host alias.
    let stream = client.open_tcp(session, prepared.remote.clone()).await?;
    run(stream, prepared).await
}

pub async fn direct(conn: &rds_net::Connection, options: Options) -> anyhow::Result<()> {
    let prepared = prepare(options).await?;
    let (host, port) = prepared.remote.clone().into_parts();
    let (send, recv) = rds_client::open_tcp(conn, &host, port).await?;
    run(tokio::io::join(recv, send), prepared).await
}

struct Prepared {
    config: Config,
    remote: rds_core::TcpTarget,
    command: Option<String>,
    pty: bool,
}

async fn prepare(options: Options) -> anyhow::Result<Prepared> {
    anyhow::ensure!(
        !options.user.is_empty()
            && options.user.len() <= 256
            && !options.user.chars().any(char::is_control),
        "invalid SSH user"
    );
    anyhow::ensure!(
        options
            .exec
            .as_ref()
            .is_none_or(|c| !c.is_empty() && c.len() <= 65536 && !c.contains('\0')),
        "invalid SSH command"
    );
    let pty = !options.no_pty && (options.pty || options.exec.is_none());
    if pty {
        platform::require_terminal()?;
    }
    let (host_key, authentication) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let host_key = public_key(&options.host_key)?;
        let authentication = if let Some(path) = options.identity {
            let content = platform::read_key(&path, true)?;
            let key = rds_ssh::decode_secret_key(&content, None).map_err(|_| {
                anyhow::anyhow!("cannot decode SSH private key; use --agent-key for encrypted keys")
            })?;
            Authentication::Key(Arc::new(key))
        } else if let Some(path) = options.agent_key {
            let socket = std::env::var_os("SSH_AUTH_SOCK")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .context("--agent-key requires SSH_AUTH_SOCK")?;
            Authentication::Agent {
                socket,
                key: public_key(&path)?,
            }
        } else {
            anyhow::bail!("an SSH identity is required");
        };
        Ok((host_key, authentication))
    })
    .await??;
    Ok(Prepared {
        config: Config {
            user: options.user,
            host_key,
            authentication,
        },
        remote: options.remote,
        command: options.exec,
        pty,
    })
}

fn public_key(path: &std::path::Path) -> anyhow::Result<PublicKey> {
    let content = platform::read_key(path, false)?;
    PublicKey::from_openssh(content.trim())
        .map_err(|_| anyhow::anyhow!("invalid OpenSSH public key"))
}

async fn run<S>(stream: S, prepared: Prepared) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut signals = platform::Signals::new()?;
    let work = async {
        let session = rds_observe::observe(
            rds_observe::Operation::SshConnect,
            Session::connect(stream, prepared.config),
        )
        .await?;
        let mut terminal = if prepared.pty {
            Some(platform::RawTerminal::new()?)
        } else {
            None
        };
        let request = Request {
            command: prepared.command,
            terminal: terminal.as_ref().map(|t| t.request()).transpose()?,
        };
        let size = request
            .terminal
            .as_ref()
            .map(|t| t.size)
            .unwrap_or(rds_ssh::Size {
                columns: 80,
                rows: 24,
            });
        let (sender, receiver) = tokio::sync::watch::channel(size);
        let _stdio_flags = platform::StdioFlags::save()?;
        let input = platform::Io::new(std::io::stdin())?;
        let output = platform::Io::new(std::io::stdout())?;
        let error = platform::Io::new(std::io::stderr())?;
        let mut window = platform::resize_signal()?;
        if let Some(terminal) = &mut terminal {
            terminal.enter()?;
        }
        let result = {
            let resize = async {
                loop {
                    if window.recv().await.is_none() {
                        std::future::pending::<()>().await;
                    }
                    if let Ok(size) = platform::size() {
                        sender.send_replace(size);
                    }
                }
            };
            tokio::pin!(resize);
            tokio::select! {
                result = rds_observe::observe(rds_observe::Operation::SshSession, session.run(request, input, output, error, receiver)) => result,
                () = &mut resize => unreachable!("resize loop never ends"),
            }
        };
        if let Some(terminal) = &mut terminal {
            terminal.restore()?;
        }
        Ok::<_, anyhow::Error>(exit_code(result?))
    };
    let code = tokio::select! {
        result = work => result?,
        code = signals.cancelled() => code,
    };
    if code != 0 {
        return Err(ExitStatus(code).into());
    }
    Ok(())
}

fn exit_code(exit: Exit) -> u8 {
    match exit {
        Exit::Status(status) => u8::try_from(status).unwrap_or(255),
        Exit::Signal(signal) => match signal {
            rds_ssh::Sig::HUP => 129,
            rds_ssh::Sig::INT => 130,
            rds_ssh::Sig::QUIT => 131,
            rds_ssh::Sig::KILL => 137,
            rds_ssh::Sig::PIPE => 141,
            rds_ssh::Sig::TERM => 143,
            _ => 255,
        },
    }
}
