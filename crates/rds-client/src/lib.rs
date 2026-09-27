//! Shared outgoing client operations for `rds` and its agent-owned manager.

#[cfg(unix)]
pub mod local;

use std::time::{Duration, Instant};

use anyhow::Context;
use rds_core::{AgentInfo, HelloAck, StreamHello};
use rds_net::{Connection, DeadlinePolicy, Endpoint, EndpointAddr};

mod forward;
mod request;
pub use forward::{DEFAULT_FORWARD_LIMIT, forward_bound_listener, forward_listener};
use request::{Authorization, bounded, exchange};

/// Open a connection to `target` and return it. The whole dial —
/// hole punching and relay fallback included — is bounded by the
/// `dial` deadline class ([`DeadlinePolicy::DEFAULT`]).
pub async fn connect(endpoint: &Endpoint, target: EndpointAddr) -> anyhow::Result<Connection> {
    connect_with_deadlines(endpoint, target, &DeadlinePolicy::DEFAULT).await
}

/// [`connect`] under a caller-chosen deadline policy. The policy is
/// validated up front so a misconfigured deployment fails fast instead
/// of discovering an unbounded dial mid-incident.
pub async fn connect_with_deadlines(
    endpoint: &Endpoint,
    target: EndpointAddr,
    deadlines: &DeadlinePolicy,
) -> anyhow::Result<Connection> {
    deadlines
        .validate()
        .map_err(|class| anyhow::anyhow!("invalid {class:?} deadline"))?;
    rds_observe::observe(rds_observe::Operation::Connect, async {
        tokio::time::timeout(deadlines.dial, endpoint.connect(target, rds_core::ALPN))
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
    rds_observe::observe(
        rds_observe::Operation::GrantAuthorize,
        bounded("authorization", async {
            let (streams, ack) = exchange(&conn, &StreamHello::Authz(grant.clone())).await?;
            match ack {
                HelloAck::Ok => streams.complete().await,
                HelloAck::Error { message } => anyhow::bail!("grant rejected: {message}"),
                other => anyhow::bail!("unexpected ack {other:?}"),
            }
        }),
    )
    .await?;
    authorization.commit();
    drop(authorization);
    Ok(conn)
}

/// Extend a live connection using a higher signed grant revision with exactly
/// the same identity and scope. Cancellation/error closes the connection because
/// the remote commit may be uncertain; never reconnect or replay a command here.
pub async fn renew_authorization(
    conn: &Connection,
    grant: &rds_core::grant::Grant,
) -> anyhow::Result<()> {
    let mut authorization = Authorization::new(conn);
    rds_observe::observe(
        rds_observe::Operation::GrantRenew,
        bounded("grant renewal", async {
            let (streams, ack) = exchange(conn, &StreamHello::RenewAuthz(grant.clone())).await?;
            match ack {
                HelloAck::Ok => streams.complete().await,
                HelloAck::Error { message } => anyhow::bail!("grant renewal rejected: {message}"),
                other => anyhow::bail!("unexpected renewal ack {other:?}"),
            }
        }),
    )
    .await?;
    authorization.commit();
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use rds_net::{Backend, EndpointConfig, SecretKey, TransportAddr};

    /// The `dial` class bounds the whole attempt: against a peer that
    /// accepts nothing, startup surfaces the failure inside the
    /// configured bound instead of parking in the transport.
    #[tokio::test]
    async fn dial_deadline_fails_fast_against_a_silent_peer() {
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint = rds_net::bind_endpoint(EndpointConfig {
            backend: Backend::Iroh,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        })
        .await
        .unwrap();
        let target = EndpointAddr {
            id: SecretKey::from_bytes(&[7; 32]).public(),
            addrs: [TransportAddr::Ip(silent.local_addr().unwrap())]
                .into_iter()
                .collect(),
        };
        let deadlines = DeadlinePolicy {
            dial: Duration::from_millis(200),
            ..DeadlinePolicy::DEFAULT
        };
        let started = Instant::now();
        let error = connect_with_deadlines(&endpoint, target, &deadlines)
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "dial class must bound a dead path"
        );
        assert!(
            error.chain().any(|e| e.to_string().contains("timed out")),
            "expected the dial bound to fire, got {error:#}"
        );
        endpoint.close().await;
    }

    /// A zero or effectively-unbounded deadline is rejected before the
    /// dial starts, not discovered mid-incident.
    #[tokio::test]
    async fn invalid_deadline_policy_fails_before_dial() {
        let deadlines = DeadlinePolicy {
            dial: Duration::ZERO,
            ..DeadlinePolicy::DEFAULT
        };
        let endpoint = rds_net::bind_endpoint(EndpointConfig {
            backend: Backend::Iroh,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        })
        .await
        .unwrap();
        let target = EndpointAddr {
            id: SecretKey::from_bytes(&[9; 32]).public(),
            addrs: Default::default(),
        };
        let error = connect_with_deadlines(&endpoint, target, &deadlines)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Dial"));
        endpoint.close().await;
    }
}
