//! Client-side operations for `rds`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use rds_core::{AgentInfo, HelloAck, StreamHello, read_frame, write_frame};
use rds_net::{Connection, Endpoint, EndpointAddr};
use tokio::net::TcpListener;

/// Open a connection to `target` and return it.
pub async fn connect(endpoint: &Endpoint, target: EndpointAddr) -> anyhow::Result<Connection> {
    endpoint
        .connect(target, rds_core::ALPN)
        .await
        .context("connect to peer")
}

/// Send a `Ping` and measure the full round trip.
pub async fn ping(conn: &Connection, nonce: u64) -> anyhow::Result<std::time::Duration> {
    let start = Instant::now();
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &StreamHello::Ping { nonce }).await?;
    match read_frame::<_, HelloAck>(&mut recv).await? {
        HelloAck::Ok => {}
        HelloAck::Error { message } => anyhow::bail!("ping rejected: {message}"),
        other => anyhow::bail!("unexpected ack {other:?}"),
    }
    let mut buf = [0u8; 8];
    recv.read_exact(&mut buf).await?;
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
    match read_frame::<_, HelloAck>(&mut recv).await? {
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
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(
        &mut send,
        &StreamHello::TcpConnect {
            host: host.to_string(),
            port,
        },
    )
    .await?;
    match read_frame::<_, HelloAck>(&mut recv).await? {
        HelloAck::Ok => Ok((send, recv)),
        HelloAck::Error { message } => anyhow::bail!("forward rejected: {message}"),
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
