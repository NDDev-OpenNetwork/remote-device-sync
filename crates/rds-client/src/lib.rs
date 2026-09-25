//! Shared outgoing client operations for `rds` and its agent-owned manager.

#[cfg(unix)]
pub mod local;

use std::time::{Duration, Instant};

use anyhow::Context;
use rds_core::{AgentInfo, HelloAck, StreamHello};
use rds_net::{Connection, Endpoint, EndpointAddr};

mod forward;
mod request;
pub use forward::{DEFAULT_FORWARD_LIMIT, forward_bound_listener, forward_listener};
use request::{Authorization, bounded, exchange};

/// Bound on the whole dial — hole punching and relay fallback retry
/// internally, so the CLI gives them room but not forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Open a connection to `target` and return it.
pub async fn connect(endpoint: &Endpoint, target: EndpointAddr) -> anyhow::Result<Connection> {
    rds_observe::observe(rds_observe::Operation::Connect, async {
        tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(target, rds_core::ALPN))
            .await
            .context("connect timed out")?
            .context("connect to peer")
    })
    .await
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
    let mut authorization = Authorization::new(&conn);
    bounded("authorization", async {
        let (streams, ack) = exchange(&conn, &StreamHello::Authz(grant.clone())).await?;
        match ack {
            HelloAck::Ok => streams.complete().await,
            HelloAck::Error { message } => anyhow::bail!("grant rejected: {message}"),
            other => anyhow::bail!("unexpected ack {other:?}"),
        }
    })
    .await?;
    authorization.commit();
    drop(authorization);
    Ok(conn)
}

/// Send a `Ping` and measure the full round trip, with one request deadline
/// covering stream credit, writing, acknowledgement and the complete echo.
pub async fn ping(conn: &Connection, nonce: u64) -> anyhow::Result<Duration> {
    let start = Instant::now();
    bounded("ping", async {
        let (mut streams, ack) = exchange(conn, &StreamHello::Ping { nonce }).await?;
        match ack {
            HelloAck::Ok => {}
            HelloAck::Error { message } => anyhow::bail!("ping rejected: {message}"),
            other => anyhow::bail!("unexpected ack {other:?}"),
        }
        let mut buf = [0u8; 8];
        streams.get_mut().1.read_exact(&mut buf).await?;
        let echoed = u64::from_be_bytes(buf);
        if echoed != nonce {
            anyhow::bail!("ping echo mismatch: {echoed} != {nonce}");
        }
        streams.complete().await?;
        Ok(start.elapsed())
    })
    .await
}

/// Fetch peer metadata within one bounded request.
pub async fn info(conn: &Connection) -> anyhow::Result<AgentInfo> {
    bounded("info", async {
        let (streams, ack) = exchange(conn, &StreamHello::Info).await?;
        match ack {
            HelloAck::Info(info) => {
                streams.complete().await?;
                Ok(info)
            }
            HelloAck::Error { message } => anyhow::bail!("info rejected: {message}"),
            other => anyhow::bail!("unexpected ack {other:?}"),
        }
    })
    .await
}

/// Open a forwarded TCP stream to `host:port` on the peer side.
/// The prelude has a deadline; the returned long-lived body does not inherit it.
pub async fn open_tcp(
    conn: &Connection,
    host: &str,
    port: u16,
) -> anyhow::Result<(rds_net::SendStream, rds_net::RecvStream)> {
    let (host, port) = rds_core::TcpTarget::new(host, port)?.into_parts();
    bounded("TCP", async {
        let (streams, ack) = exchange(conn, &StreamHello::TcpConnect { host, port }).await?;
        match ack {
            HelloAck::Ok => Ok(streams.release()),
            HelloAck::Error { message } => anyhow::bail!("forward rejected: {message}"),
            other => anyhow::bail!("unexpected ack {other:?}"),
        }
    })
    .await
}

/// Open a `Sync` control stream within one bounded prelude.
/// Push: `rds_sync::engine::send_file`; pull: `recv_file`.
pub async fn open_sync(
    conn: &Connection,
) -> anyhow::Result<(rds_net::SendStream, rds_net::RecvStream)> {
    bounded("sync", async {
        let (streams, ack) = exchange(conn, &StreamHello::Sync).await?;
        match ack {
            HelloAck::Ok => Ok(streams.release()),
            HelloAck::Error { message } => anyhow::bail!("sync rejected: {message}"),
            other => anyhow::bail!("unexpected ack {other:?}"),
        }
    })
    .await
}
