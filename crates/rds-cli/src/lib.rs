//! Client-side operations for `rds`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use rds_core::{AgentInfo, HelloAck, StreamHello, read_frame, write_frame};
use rds_net::{Connection, Endpoint, EndpointAddr};
use tokio::net::TcpListener;

/// Every acknowledgement wait is bounded — a peer that opens the
/// stream but never answers must not hang the CLI forever. Generous:
/// grant verification and the far side's TCP connect gate on it.
const ACK_TIMEOUT: Duration = Duration::from_secs(15);

/// One `HelloAck` read, bounded by [`ACK_TIMEOUT`].
async fn read_ack(recv: &mut rds_net::RecvStream) -> anyhow::Result<HelloAck> {
    match tokio::time::timeout(ACK_TIMEOUT, read_frame::<_, HelloAck>(recv)).await {
        Ok(r) => r.map_err(Into::into),
        Err(_) => anyhow::bail!("peer did not answer within {ACK_TIMEOUT:?}"),
    }
}

/// Bound on the whole dial — hole punching and relay fallback retry
/// internally, so the CLI gives them room but not forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Open a connection to `target` and return it.
pub async fn connect(endpoint: &Endpoint, target: EndpointAddr) -> anyhow::Result<Connection> {
    tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(target, rds_core::ALPN))
        .await
        .context("connect timed out")?
        .context("connect to peer")
}

/// Connect and present `grant` on the connection's first stream —
/// required when the agent runs in grant mode (`policy.issuers`
/// non-empty). The grant must verify before any service stream opens;
/// a rejected grant fails the connect.
pub async fn connect_authorized(
    endpoint: &Endpoint,
    target: EndpointAddr,
    grant: &rds_core::grant::Grant,
) -> anyhow::Result<Connection> {
    let conn = connect(endpoint, target).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &StreamHello::Authz(grant.clone())).await?;
    match read_ack(&mut recv).await? {
        HelloAck::Ok => Ok(conn),
        HelloAck::Error { message } => anyhow::bail!("grant rejected: {message}"),
        other => anyhow::bail!("unexpected ack {other:?}"),
    }
}

/// Send a `Ping` and measure the full round trip.
pub async fn ping(conn: &Connection, nonce: u64) -> anyhow::Result<Duration> {
    let start = Instant::now();
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &StreamHello::Ping { nonce }).await?;
    match read_ack(&mut recv).await? {
        HelloAck::Ok => {}
        HelloAck::Error { message } => anyhow::bail!("ping rejected: {message}"),
        other => anyhow::bail!("unexpected ack {other:?}"),
    }
    let mut buf = [0u8; 8];
    match tokio::time::timeout(ACK_TIMEOUT, recv.read_exact(&mut buf)).await {
        Ok(r) => r?,
        Err(_) => anyhow::bail!("ping echo timed out"),
    }
    let echoed = u64::from_be_bytes(buf);
    if echoed != nonce {
        anyhow::bail!("ping echo mismatch: {echoed} != {nonce}");
    }
    Ok(start.elapsed())
}

/// Fetch peer metadata.
pub async fn info(conn: &Connection) -> anyhow::Result<AgentInfo> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &StreamHello::Info).await?;
    match read_ack(&mut recv).await? {
        HelloAck::Info(info) => Ok(info),
        HelloAck::Error { message } => anyhow::bail!("info rejected: {message}"),
        other => anyhow::bail!("unexpected ack {other:?}"),
    }
}

/// Open a forwarded TCP stream to `host:port` on the peer side.
pub async fn open_tcp(
    conn: &Connection,
    host: &str,
    port: u16,
) -> anyhow::Result<(rds_net::SendStream, rds_net::RecvStream)> {
    let (host, port) = rds_core::TcpTarget::new(host, port)?.into_parts();
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &StreamHello::TcpConnect { host, port }).await?;
    match read_ack(&mut recv).await? {
        HelloAck::Ok => Ok((send, recv)),
        HelloAck::Error { message } => anyhow::bail!("forward rejected: {message}"),
        other => anyhow::bail!("unexpected ack {other:?}"),
    }
}

/// Open a `Sync` control stream — the first frame of a sync session.
/// Push: `rds_sync::engine::send_file`; pull: `recv_file`.
pub async fn open_sync(
    conn: &Connection,
) -> anyhow::Result<(rds_net::SendStream, rds_net::RecvStream)> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &StreamHello::Sync).await?;
    match read_ack(&mut recv).await? {
        HelloAck::Ok => Ok((send, recv)),
        HelloAck::Error { message } => anyhow::bail!("sync rejected: {message}"),
        other => anyhow::bail!("unexpected ack {other:?}"),
    }
}

/// Listen on `bind` and splice every accepted TCP connection into a new
/// `TcpConnect` stream on `conn` to `remote_host:remote_port`.
pub async fn forward_listener(
    conn: Arc<Connection>,
    bind: SocketAddr,
    remote_host: String,
    remote_port: u16,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(bind).await?;
    println!("listening on {bind} -> {remote_host}:{remote_port}");
    loop {
        let (mut local, peer) = listener.accept().await?;
        let conn = conn.clone();
        let host = remote_host.clone();
        tokio::spawn(async move {
            match open_tcp(&conn, &host, remote_port).await {
                Ok((send, recv)) => {
                    let mut remote = tokio::io::join(recv, send);
                    if let Err(e) = tokio::io::copy_bidirectional(&mut local, &mut remote).await {
                        tracing::debug!(%peer, "forward closed: {e}");
                    }
                }
                Err(e) => tracing::warn!(%peer, "forward rejected: {e}"),
            }
        });
    }
}
