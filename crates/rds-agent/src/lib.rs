//! The agent: accepts authenticated rds connections and serves streams.
//!
//! Authorization is two-layered:
//!
//! - **Membership** — the peer's QUIC-authenticated `EndpointId` must be
//!   in `policy.allow`, checked as soon as the handshake completes,
//!   before any service stream is read.
//! - **Capability** — when `policy.issuers` is non-empty the peer must
//!   additionally present a [`Grant`](rds_core::grant::Grant) signed by
//!   a trusted issuer as the first stream on the connection. Until a
//!   grant verifies, every service stream is refused; afterwards each
//!   stream is checked against the grant's service scope and
//!   constraints (TCP ports, displays, bitrate ceiling).
//!
//! Revocation: `policy.denylist` holds revoked grant ids — a live
//! connection whose grant lands on the denylist is closed, and a grant
//! that expires mid-session closes its connection at `expires_at`. The
//! same grant cannot authorize two concurrent connections (replay
//! guard in `active_grants`).
//!
//! TCP forwarding is restricted to an explicit set of `(host, port)`
//! targets; the default set is exactly the configured SSH socket.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rds_core::grant::{GrantId, VerifiedGrant};
use rds_core::{
    AgentInfo, HelloAck, PROTOCOL_VERSION, ServiceKind, StreamHello, read_frame, write_frame,
};
use rds_net::{Connection, Endpoint, EndpointId};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tracing::{Instrument, debug, info, info_span, warn};

mod authz;
mod revocations;
use authz::{ConnAuthz, ConnectionLifetime, authorize};
pub use revocations::{RevocationFeed, RevocationPolicy, watch_revocations};

/// Monotonic session ids for structured tracing — every connection's
/// `rds.conn` span carries one, so `session_id` filters a whole
/// session's events (streams, grants, sync, desktop) in the log.
static SESSION_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_session_id() -> u64 {
    SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A peer that opens a stream but never writes its `StreamHello` would
/// otherwise park a task per stream for the connection's lifetime —
/// bounded here so silent streams cost seconds, not the session.
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);

/// Mutex acquisition that survives a poisoned lock: every mutex here
/// guards plain data (a state word, an `Option`, a `HashSet`) whose
/// invariants a panic cannot corrupt, so refusing service forever
/// after one panicked holder would be the worse failure.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runtime policy for the agent.
#[derive(Clone)]
pub struct AgentPolicy {
    /// Peers allowed to open any stream at all.
    pub allow: HashSet<EndpointId>,
    /// TCP targets the `TcpConnect` service may splice to.
    /// The default is the configured SSH socket.
    pub tcp_targets: HashSet<(String, u16)>,
    /// Accept any TCP target. Development escape hatch.
    pub allow_any_tcp: bool,
    /// Trusted grant issuers (Ed25519 verifying keys). Empty = grants
    /// not required: the allowlist alone authorizes (legacy mode).
    /// Non-empty = every connection must present a valid grant on its
    /// first stream before any service stream is served.
    pub issuers: HashSet<[u8; 32]>,
    /// Maximum grant lifetime accepted at verify time.
    pub grant_max_ttl: Duration,
    /// Revoked grant ids and freshness; updated by [`watch_revocations`]
    /// from the estate's signed snapshot or by explicit local policy.
    /// The `watch` channel notifies live connections so a revoked grant
    /// drops its session, not just future ones.
    pub denylist: watch::Sender<Arc<RevocationPolicy>>,
    /// Grant ids currently bound to a live connection — the replay
    /// guard: the same grant cannot run two concurrent sessions.
    pub active_grants: Arc<Mutex<HashSet<GrantId>>>,
    /// Directory the `Sync` service may read/write under (WS6). `None`
    /// disables sync entirely.
    pub sync_dir: Option<PathBuf>,
}

impl std::fmt::Debug for AgentPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentPolicy")
            .field("allow", &self.allow)
            .field("tcp_targets", &self.tcp_targets)
            .field("allow_any_tcp", &self.allow_any_tcp)
            .field("issuers", &self.issuers.len())
            .field("grant_max_ttl", &self.grant_max_ttl)
            .field("sync_dir", &self.sync_dir)
            .finish_non_exhaustive()
    }
}

impl AgentPolicy {
    pub fn ssh_only(ssh: (String, u16)) -> Self {
        Self {
            allow: HashSet::new(),
            tcp_targets: HashSet::from([ssh]),
            allow_any_tcp: false,
            issuers: HashSet::new(),
            grant_max_ttl: Duration::from_secs(300),
            denylist: watch::channel(Arc::new(RevocationPolicy::default())).0,
            active_grants: Arc::new(Mutex::new(HashSet::new())),
            sync_dir: None,
        }
    }

    pub fn permits_tcp(&self, host: &str, port: u16) -> bool {
        self.allow_any_tcp
            || self
                .tcp_targets
                .iter()
                .any(|(h, p)| *p == port && h.eq_ignore_ascii_case(host))
    }

    /// Whether this connection's peer must present a grant.
    fn grants_required(&self) -> bool {
        !self.issuers.is_empty()
    }

    /// Revoke a grant id — pushes onto the denylist and notifies every
    /// live connection watcher. The value is retained without subscribers.
    pub fn revoke(&self, id: GrantId) {
        self.denylist.send_modify(|set| {
            Arc::make_mut(&mut Arc::make_mut(set).ids).insert(id);
        });
    }

    /// Read the current denylist snapshot.
    pub fn denied(&self) -> Arc<HashSet<GrantId>> {
        self.denylist.borrow().ids.clone()
    }

    /// Replace revoked ids without changing freshness. Managed feeds commit
    /// and publish their own complete snapshots; this does not renew a lease.
    /// Live connections re-check on the notification.
    pub fn replace_denylist(&self, ids: HashSet<GrantId>) {
        self.denylist.send_modify(|value| {
            Arc::make_mut(value).ids = Arc::new(ids);
        });
    }
}

/// A bound agent: endpoint plus policy, ready to `run`.
pub struct Agent {
    pub endpoint: Endpoint,
    pub policy: Arc<AgentPolicy>,
    desktop: bool,
}

impl Agent {
    pub fn new(endpoint: Endpoint, policy: AgentPolicy) -> Self {
        Self {
            endpoint,
            policy: Arc::new(policy),
            desktop: cfg!(feature = "desktop"),
        }
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Accept connections until the endpoint closes.
    pub async fn run(&self) -> anyhow::Result<()> {
        info!(id = %self.endpoint.id(), "agent listening");
        while let Some(incoming) = self.endpoint.accept().await {
            let policy = self.policy.clone();
            let desktop = self.desktop;
            let metrics = self.endpoint.metrics();
            tokio::spawn(async move {
                match incoming.await {
                    Ok(conn) => {
                        let span = info_span!(
                            "rds.conn",
                            peer = %conn.remote_id(),
                            session_id = next_session_id(),
                        );
                        let res = serve_connection(conn, policy, desktop, metrics)
                            .instrument(span)
                            .await;
                        if let Err(e) = res {
                            debug!("connection ended: {e}");
                        }
                    }
                    Err(e) => debug!("incoming handshake failed: {e}"),
                }
            });
        }
        Ok(())
    }

    /// Serve a single already-established connection.
    pub async fn serve(&self, conn: Connection) -> anyhow::Result<()> {
        let span = info_span!(
            "rds.conn",
            peer = %conn.remote_id(),
            session_id = next_session_id(),
        );
        serve_connection(
            conn,
            self.policy.clone(),
            self.desktop,
            self.endpoint.metrics(),
        )
        .instrument(span)
        .await
    }
}

async fn serve_connection(
    conn: Connection,
    policy: Arc<AgentPolicy>,
    desktop: bool,
    metrics: rds_net::metrics::Registry,
) -> anyhow::Result<()> {
    let peer = conn.remote_id();
    if !policy.allow.contains(&peer) {
        warn!(%peer, "rejected: endpoint id not in allowlist");
        conn.close(1u32.into(), b"not allowed");
        anyhow::bail!("peer {peer} not in allowlist");
    }
    info!(%peer, "peer connected");
    // Fold this connection's per-path transport counters into the
    // endpoint registry for the connection's lifetime.
    tokio::spawn(metrics.sampler(conn.clone()).run(Duration::from_secs(1)));
    let authz = Arc::new(ConnAuthz::new(policy.grants_required()));
    let _lifetime = ConnectionLifetime {
        conn: conn.clone(),
        authz: authz.clone(),
    };
    loop {
        let (send, recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                debug!(%peer, "connection closed: {e}");
                return Ok(());
            }
        };
        let policy = policy.clone();
        let conn = conn.clone();
        let authz = authz.clone();
        let span = tracing::Span::current();
        tokio::spawn(
            async move {
                if let Err(e) = serve_stream(conn, send, recv, policy, authz, desktop).await {
                    debug!("stream ended: {e}");
                }
            }
            .instrument(span),
        );
    }
}

async fn serve_stream(
    conn: Connection,
    mut send: rds_net::SendStream,
    mut recv: rds_net::RecvStream,
    policy: Arc<AgentPolicy>,
    authz: Arc<ConnAuthz>,
    desktop: bool,
) -> anyhow::Result<()> {
    let hello: StreamHello = match tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut recv)).await
    {
        Ok(h) => h?,
        Err(_) => anyhow::bail!("stream hello timed out"),
    };
    if let StreamHello::Authz(grant) = hello {
        return authorize(&conn, send, grant, &policy, &authz).await;
    }
    let grant = match authz.scope(&policy) {
        Ok(g) => g,
        Err(why) => {
            if why.terminal() {
                conn.close(2u32.into(), why.message().as_bytes());
            }
            let why = why.message();
            write_frame(
                &mut send,
                &HelloAck::Error {
                    message: why.into(),
                },
            )
            .await?;
            anyhow::bail!("stream refused: {why}");
        }
    };
    let scope_err = grant.as_ref().and_then(|g| scope_check(g, &hello).err());
    if let Some(why) = scope_err {
        write_frame(&mut send, &HelloAck::Error { message: why }).await?;
        anyhow::bail!("stream outside grant scope");
    }
    let span = info_span!("rds.stream", service = ?service_kind(&hello));
    async move {
        match hello {
            StreamHello::Ping { nonce } => {
                write_frame(&mut send, &HelloAck::Ok).await?;
                send.write_all(&nonce.to_be_bytes()).await?;
                send.finish()?;
            }
            StreamHello::Info => {
                let info = AgentInfo {
                    protocol: PROTOCOL_VERSION,
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    hostname: hostname(),
                    services: {
                        let mut s = vec![ServiceKind::Ping, ServiceKind::Info, ServiceKind::Tcp];
                        if desktop {
                            s.push(ServiceKind::Desktop);
                        }
                        if policy.sync_dir.is_some() {
                            s.push(ServiceKind::Sync);
                        }
                        s
                    },
                    desktop: desktop_caps(desktop),
                };
                write_frame(&mut send, &HelloAck::Info(info)).await?;
                send.finish()?;
            }
            StreamHello::TcpConnect { host, port } => {
                if !policy.permits_tcp(&host, port) {
                    write_frame(
                        &mut send,
                        &HelloAck::Error {
                            message: format!("tcp target {host}:{port} not permitted"),
                        },
                    )
                    .await?;
                    anyhow::bail!("tcp target {host}:{port} rejected");
                }
                match TcpStream::connect((host.as_str(), port)).await {
                    Ok(mut tcp) => {
                        write_frame(&mut send, &HelloAck::Ok).await?;
                        let mut quic = tokio::io::join(recv, send);
                        tokio::io::copy_bidirectional(&mut tcp, &mut quic).await?;
                    }
                    Err(e) => {
                        write_frame(
                            &mut send,
                            &HelloAck::Error {
                                message: format!("connect {host}:{port} failed: {e}"),
                            },
                        )
                        .await?;
                    }
                }
            }
            StreamHello::Desktop(hello) => {
                if desktop {
                    #[cfg(feature = "desktop")]
                    match rds_desktop::capabilities() {
                        Ok(caps) => {
                            write_frame(&mut send, &HelloAck::Desktop(caps)).await?;
                            let max_bps = grant.as_ref().and_then(|g| g.max_bps());
                            rds_desktop::serve_desktop_with(
                                conn,
                                send,
                                recv,
                                hello,
                                rds_desktop::SessionConfig {
                                    bitrate_ceiling: max_bps,
                                    ..Default::default()
                                },
                            )
                            .await?;
                        }
                        Err(e) => {
                            write_frame(
                                &mut send,
                                &HelloAck::Error {
                                    message: format!("desktop unavailable: {e}"),
                                },
                            )
                            .await?;
                        }
                    }
                    #[cfg(not(feature = "desktop"))]
                    {
                        // `desktop` is `cfg!(feature = "desktop")` so this
                        // is unreachable today — but a request path must
                        // refuse, never panic, if that coupling breaks.
                        write_frame(
                            &mut send,
                            &HelloAck::Error {
                                message: "desktop service not compiled in".into(),
                            },
                        )
                        .await?;
                        anyhow::bail!("desktop requested but not compiled in");
                    }
                } else {
                    let _ = hello;
                    write_frame(
                        &mut send,
                        &HelloAck::Error {
                            message: "agent built without desktop support".into(),
                        },
                    )
                    .await?;
                }
            }
            StreamHello::Sync => {
                let Some(dir) = policy.sync_dir.clone() else {
                    write_frame(
                        &mut send,
                        &HelloAck::Error {
                            message: "sync service not configured".into(),
                        },
                    )
                    .await?;
                    anyhow::bail!("sync service not configured");
                };
                if !authz.try_sync_slot() {
                    write_frame(
                        &mut send,
                        &HelloAck::Error {
                            message: "sync session already active on this connection".into(),
                        },
                    )
                    .await?;
                    anyhow::bail!("concurrent sync session refused");
                }
                write_frame(&mut send, &HelloAck::Ok).await?;
                let res = rds_sync::engine::serve(conn, send, recv, dir).await;
                authz.release_sync_slot();
                res?;
            }
            StreamHello::Audio(_) => {
                // Wire shape landed in protocol v2; capture/codec support
                // is v0.3 scope. Refuse politely rather than hang.
                write_frame(
                    &mut send,
                    &HelloAck::Error {
                        message: "audio service not implemented".into(),
                    },
                )
                .await?;
                anyhow::bail!("audio service not implemented");
            }
            StreamHello::Authz(_) => {
                // `authorize` early-returns on Authz, so this is
                // unreachable — a request path still refuses rather
                // than panic if that ever stops holding.
                write_frame(
                    &mut send,
                    &HelloAck::Error {
                        message: "authz is not a service".into(),
                    },
                )
                .await?;
                anyhow::bail!("authz stream on the service path");
            }
        }
        Ok(())
    }
    .instrument(span)
    .await
}

/// The service a `StreamHello` selects; `None` for `Authz`, which is
/// not a service stream.
fn service_kind(hello: &StreamHello) -> Option<ServiceKind> {
    Some(match hello {
        StreamHello::Ping { .. } => ServiceKind::Ping,
        StreamHello::Info => ServiceKind::Info,
        StreamHello::TcpConnect { .. } => ServiceKind::Tcp,
        StreamHello::Desktop(_) => ServiceKind::Desktop,
        StreamHello::Sync => ServiceKind::Sync,
        StreamHello::Audio(_) => ServiceKind::Audio,
        StreamHello::Authz(_) => return None,
    })
}

/// Whether `hello`'s service is inside `grant`'s scope — service kind
/// plus the constraint that applies to that service.
fn scope_check(grant: &VerifiedGrant, hello: &StreamHello) -> Result<(), String> {
    match hello {
        StreamHello::TcpConnect { port, .. } if !grant.permits_port(*port) => {
            return Err(format!("port {port} outside grant constraints"));
        }
        StreamHello::Desktop(h) if !grant.permits_display(h.display) => {
            return Err(format!("display {} outside grant constraints", h.display));
        }
        _ => {}
    }
    let Some(kind) = service_kind(hello) else {
        return Err("authz is not a service".into());
    };
    if !grant.permits(kind) {
        return Err(format!("service {kind:?} not granted"));
    }
    Ok(())
}

fn desktop_caps(enabled: bool) -> Option<rds_core::DesktopCaps> {
    #[cfg(feature = "desktop")]
    if enabled {
        return rds_desktop::capabilities().ok();
    }
    let _ = enabled;
    None
}

fn hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
}
