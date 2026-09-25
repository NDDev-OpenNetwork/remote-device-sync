//! Standard SSH over caller-supplied authenticated streams. No endpoint, TCP
//! listener, shell process, trust-on-first-use or automatic command replay.
use std::{borrow::Cow, sync::Arc, time::Duration};

use russh::{ChannelMsg, client};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

pub use russh::keys::{Algorithm, PrivateKey, PublicKey, decode_secret_key};
pub use russh::{Pty, Sig};

const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const CHUNK: usize = 16 * 1024;
const WINDOW: u32 = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SSH setup timed out")]
    Timeout,
    #[error("SSH host key does not match the explicitly provisioned key")]
    HostKey,
    #[error("SSH authentication rejected")]
    Authentication,
    #[error("SSH supports Ed25519 and ECDSA keys; invalid key or user")]
    Configuration,
    #[error("SSH server rejected {0}")]
    Rejected(&'static str),
    #[error("SSH channel ended without a valid exit status")]
    MissingStatus,
    #[error("unexpected SSH channel response")]
    Protocol,
    #[error("SSH protocol failed")]
    Ssh(#[source] russh::Error),
    #[error("SSH agent authentication failed")]
    Agent,
    #[error("SSH stream I/O failed")]
    Io(#[from] std::io::Error),
}

impl From<russh::Error> for Error {
    fn from(error: russh::Error) -> Self {
        Self::Ssh(error)
    }
}

/// The agent is used only to sign with this explicitly selected public key.
/// It is never forwarded to a remote machine or enumerated for fallback keys.
pub enum Authentication {
    Key(Arc<PrivateKey>),
    Agent {
        socket: std::path::PathBuf,
        key: PublicKey,
    },
}

pub struct Config {
    pub user: String,
    /// Out-of-band trusted OpenSSH public key, scoped by the caller to the
    /// pinned RDS peer and TCP target. Certificates are not silently accepted.
    pub host_key: PublicKey,
    pub authentication: Authentication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub columns: u32,
    pub rows: u32,
}

pub struct Terminal {
    pub term: String,
    pub size: Size,
    /// RFC 4254 modes sampled before the local terminal enters raw mode.
    pub modes: Vec<(Pty, u32)>,
}

pub struct Request {
    /// None requests the remote account's shell. Some sends one exact SSH exec
    /// string: argument quoting/shell semantics belong to the caller/server.
    pub command: Option<String>,
    pub terminal: Option<Terminal>,
}

#[derive(Debug)]
pub enum Exit {
    Status(u32),
    Signal(Sig),
}

struct Verifier(PublicKey);

impl client::Handler for Verifier {
    type Error = Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Error> {
        if key.certificate().is_some() || key.public_key().key_data() != self.0.key_data() {
            return Err(Error::HostKey);
        }
        Ok(true)
    }

    // Upstream defaults accept several server-initiated channel types. This
    // adapter has no forwarding/subsystem service: any such opening is fatal.
    async fn server_channel_open_session(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    async fn server_channel_open_agent_forward(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    async fn server_channel_open_direct_tcpip(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
    async fn server_channel_open_x11(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: u32,
        _: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Error> {
        Err(Error::Protocol)
    }
}

fn supported(key: &PublicKey) -> bool {
    matches!(
        key.algorithm(),
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. }
    )
}

fn protocol_config() -> client::Config {
    client::Config {
        window_size: WINDOW,
        maximum_packet_size: 32 * 1024,
        channel_buffer_size: 8,
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        // An idle *responsive* shell remains alive. This also bounds a stalled
        // protocol write; 0.63.3 fixed enforcement during those writes.
        inactivity_timeout: Some(Duration::from_secs(90)),
        preferred: russh::Preferred {
            kex: Cow::Borrowed(&[
                russh::kex::MLKEM768X25519_SHA256,
                russh::kex::CURVE25519,
                russh::kex::CURVE25519_PRE_RFC_8731,
                russh::kex::EXTENSION_SUPPORT_AS_CLIENT,
                russh::kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
            ]),
            key: Cow::Owned(
                russh::Preferred::DEFAULT
                    .key
                    .iter()
                    .filter(|algorithm| {
                        matches!(algorithm, Algorithm::Ed25519 | Algorithm::Ecdsa { .. })
                    })
                    .cloned()
                    .collect(),
            ),
            cipher: Cow::Borrowed(&[russh::cipher::CHACHA20_POLY1305, russh::cipher::AES_256_GCM]),
            ..russh::Preferred::DEFAULT
        },
        ..client::Config::default()
    }
}

/// Owns the only task that can touch the supplied transport. Upstream spawns
/// its protocol runner during KEX, before returning a handle, and dropping that
/// handle does not abort the runner. A bounded duplex bridge makes cancellation
/// during *any* setup/body stage close the actual RDS stream independently.
struct Transport(tokio::task::JoinHandle<()>);

impl Transport {
    fn new<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        mut stream: S,
    ) -> (Self, tokio::io::DuplexStream) {
        // russh polls reads between packet flushes. A tiny duplex capacity can
        // deadlock two peers flushing simultaneously, even with application
        // upload/download polled concurrently. Allow an advertised receive
        // window plus packet/queued-channel headroom; the regression fixture
        // uses only a 4 KiB underlying stream and echoes more than one window.
        let (local, mut remote) = tokio::io::duplex(2 * WINDOW as usize);
        let task = tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
        });
        (Self(task), local)
    }

    async fn close(&mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct Session {
    handle: client::Handle<Verifier>,
    transport: Transport,
}

impl Session {
    pub async fn connect<S>(stream: S, config: Config) -> Result<Self, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let auth_key = match &config.authentication {
            Authentication::Key(key) => key.public_key(),
            Authentication::Agent { key, .. } => key,
        };
        if config.user.is_empty()
            || config.user.len() > 256
            || config.user.chars().any(char::is_control)
            || !supported(&config.host_key)
            || !supported(auth_key)
        {
            return Err(Error::Configuration);
        }
        let (transport, stream) = Transport::new(stream);
        tokio::time::timeout(SETUP_TIMEOUT, async move {
            let mut session = Self {
                handle: client::connect_stream(
                    Arc::new(protocol_config()),
                    stream,
                    Verifier(config.host_key),
                )
                .await?,
                transport,
            };
            let result = match config.authentication {
                Authentication::Key(key) => {
                    session
                        .handle
                        .authenticate_publickey(
                            config.user,
                            russh::keys::PrivateKeyWithHashAlg::new(key, None),
                        )
                        .await?
                }
                Authentication::Agent { socket, key } => {
                    let mut agent = russh::keys::agent::client::AgentClient::connect_uds(socket)
                        .await
                        .map_err(|_| Error::Agent)?;
                    session
                        .handle
                        .authenticate_publickey_with(config.user, key, None, &mut agent)
                        .await
                        .map_err(|_| Error::Agent)?
                }
            };
            if !result.success() {
                return Err(Error::Authentication);
            }
            Ok(session)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Output is streamed with backpressure, including output following
    /// exit-status. EOF alone is not success; wait for channel close. Resizing
    /// is coalesced through a watch channel. No stdin worker survives this call.
    pub async fn run<R, W, E>(
        self,
        request: Request,
        input: R,
        mut output: W,
        mut error: E,
        mut resize: watch::Receiver<Size>,
    ) -> Result<Exit, Error>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
        E: AsyncWrite + Unpin,
    {
        let result = async {
            if request
                .command
                .as_ref()
                .is_some_and(|c| c.is_empty() || c.len() > 65536 || c.contains('\0'))
            {
                return Err(Error::Configuration);
            }
            let pty = request.terminal.is_some();
            let channel = tokio::time::timeout(SETUP_TIMEOUT, async {
                let mut channel = self.handle.channel_open_session().await?;
                if let Some(terminal) = request.terminal {
                    if terminal.term.is_empty()
                        || terminal.term.len() > 128
                        || terminal.term.chars().any(char::is_control)
                        || terminal.size.columns == 0
                        || terminal.size.rows == 0
                        || terminal.modes.len() > 128
                    {
                        return Err(Error::Configuration);
                    }
                    channel
                        .request_pty(
                            true,
                            &terminal.term,
                            terminal.size.columns,
                            terminal.size.rows,
                            0,
                            0,
                            &terminal.modes,
                        )
                        .await?;
                    acknowledgement(&mut channel, "PTY allocation").await?;
                }
                match request.command {
                    Some(command) => channel.exec(true, command).await?,
                    None => channel.request_shell(true).await?,
                }
                acknowledgement(&mut channel, "shell/exec request").await?;
                Ok::<_, Error>(channel)
            })
            .await
            .map_err(|_| Error::Timeout)??;
            let (mut reader, writer) = channel.split();
            let upload = async {
                let mut input = input;
                let mut buf = [0; CHUNK];
                let mut resize_open = pty;
                let mut input_closed = false;
                loop {
                    tokio::select! {
                        result = input.read(&mut buf), if !input_closed => {
                            let count = result?;
                            if count == 0 {
                                writer.eof().await?;
                                if !pty { return Ok::<_, Error>(()); }
                                input_closed = true;
                                continue;
                            }
                            writer.data(&buf[..count]).await?;
                        }
                        result = resize.changed(), if resize_open => {
                            if result.is_err() { resize_open = false; continue; }
                            let size = *resize.borrow_and_update();
                            if size.columns > 0 && size.rows > 0 {
                                writer.window_change(size.columns, size.rows, 0, 0).await?;
                            }
                        }
                        else => std::future::pending::<()>().await,
                    }
                }
            };
            let download = async {
                let mut exit = None;
                let mut eof = false;
                let mut closing = None;
                loop {
                    let message = match closing {
                        Some(deadline) => tokio::time::timeout_at(deadline, reader.wait())
                            .await
                            .map_err(|_| Error::Timeout)?,
                        None => reader.wait().await,
                    };
                    match message {
                        Some(ChannelMsg::Data { data }) if !eof => output.write_all(&data).await?,
                        Some(ChannelMsg::ExtendedData { data, ext: 1 }) if !eof => {
                            error.write_all(&data).await?
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) if exit.is_none() => {
                            exit = Some(Exit::Status(exit_status));
                            closing = Some(tokio::time::Instant::now() + SETUP_TIMEOUT);
                        }
                        Some(ChannelMsg::ExitSignal { signal_name, .. }) if exit.is_none() => {
                            exit = Some(Exit::Signal(signal_name));
                            closing = Some(tokio::time::Instant::now() + SETUP_TIMEOUT);
                        }
                        Some(ChannelMsg::Eof) => {
                            eof = true;
                            closing.get_or_insert(tokio::time::Instant::now() + SETUP_TIMEOUT);
                        }
                        Some(ChannelMsg::Close) => {
                            output.flush().await?;
                            error.flush().await?;
                            return exit.ok_or(Error::MissingStatus);
                        }
                        None => return Err(Error::MissingStatus),
                        Some(ChannelMsg::WindowAdjusted { .. } | ChannelMsg::XonXoff { .. }) => {}
                        _ => return Err(Error::Protocol),
                    }
                }
            };
            tokio::pin!(upload, download);
            tokio::select! {
                result = &mut download => result,
                result = &mut upload => {
                    match result {
                        Ok(()) => download.await,
                        // The remote command may exit before consuming stdin.
                        // A closed upload channel must not erase its status or
                        // truncate output already queued for the read half.
                        Err(send_error @ Error::Ssh(_)) => {
                            match tokio::time::timeout(SETUP_TIMEOUT, download).await {
                                Ok(Ok(exit)) => Ok(exit),
                                _ => Err(send_error),
                            }
                        }
                        Err(error) => Err(error),
                    }
                }
            }
        }
        .await;
        self.close().await;
        result
    }

    pub async fn close(mut self) {
        let _ = tokio::time::timeout(
            CLOSE_TIMEOUT,
            self.handle
                .disconnect(russh::Disconnect::ByApplication, "", ""),
        )
        .await;
        // Give the queued disconnect a chance to flush, but never let remote
        // acknowledgement or a stopped writer retain our transport indefinitely.
        let finished = tokio::time::timeout(CLOSE_TIMEOUT, &mut self.handle)
            .await
            .is_ok();
        self.transport.close().await;
        if !finished {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, &mut self.handle).await;
        }
    }
}

async fn acknowledgement(
    channel: &mut russh::Channel<client::Msg>,
    what: &'static str,
) -> Result<(), Error> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => return Err(Error::Rejected(what)),
            Some(ChannelMsg::WindowAdjusted { .. }) => {}
            _ => return Err(Error::Protocol),
        }
    }
}
