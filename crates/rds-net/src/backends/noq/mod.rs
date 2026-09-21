//! Owned transport backend on `noq` — work in progress (WS1).
//!
//! Design (docs/research.md §9.1): noq is the QUIC engine; everything
//! above it is ours:
//!
//! - The endpoint binds a plain UDP socket through `noq`'s tokio runtime
//!   today; `socket::Mux` (direct UDP + relay tunnels + multi-interface)
//!   replaces it once `relay_link` lands.
//! - TLS carries raw Ed25519 public keys (RFC 7250), the same scheme the
//!   iroh backend uses — both backends are wire-compatible on `rds/0`.
//! - `Connection::open_path` turns discovered candidate addresses into
//!   QUIC paths; `PathEvents`/`NatTraversalUpdates` report QNT progress.
//! - `observed_external_addr` yields our public address in-band — no STUN
//!   service required.
//! - Path policy (`policy` module) orders candidates and opens extra
//!   paths; per-service pinning lands with the media protocol.
//!
//! Until this backend reaches parity (same-harness benchmarks vs the
//! iroh backend), `iroh` remains the default selected at bind time.

mod hmac;
pub mod policy;
mod tls;

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use anyhow::{Context as _, bail};
use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr};
use tracing::debug;

pub use tls::peer_endpoint_id;

/// Path-lifecycle values mirrored from iroh's defaults: heartbeat every
/// 5s, per-path idle timeout 15s, at most 8 concurrent multipath paths,
/// up to 32 remote NAT-traversal address candidates.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const PATH_MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_MULTIPATH_PATHS: u32 = 8;
const MAX_QNT_ADDRESSES: u8 = 32;

/// QUIC datagram buffer caps: 1 MiB each direction, oldest dropped first.
const DATAGRAM_BUFFER_SIZE: usize = 1 << 20;

/// How a noq endpoint reaches the network.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Persisted or generated Ed25519 secret key. `None` generates one.
    pub secret_key: Option<SecretKey>,
    /// UDP bind address. `None` binds `0.0.0.0:0`.
    pub bind_addr: Option<SocketAddr>,
    /// QUIC ALPN protocol ids. Defaults to `rds/0`.
    pub alpns: Vec<Vec<u8>>,
    /// Custom relay URL. Reserved: relay transport lands with `relay_link`.
    pub relay: Option<RelayUrl>,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            secret_key: None,
            bind_addr: None,
            alpns: vec![rds_core::ALPN.to_vec()],
            relay: None,
        }
    }
}

/// QUIC transport parameters, mirroring the iroh backend's choices so
/// behavior — and benchmark numbers — are comparable across backends.
fn transport_config() -> Arc<noq::TransportConfig> {
    let mut cfg = noq::TransportConfig::default();
    cfg.keep_alive_interval(Some(HEARTBEAT_INTERVAL));
    cfg.default_path_keep_alive_interval(Some(HEARTBEAT_INTERVAL));
    cfg.default_path_max_idle_timeout(Some(PATH_MAX_IDLE_TIMEOUT));
    cfg.max_concurrent_multipath_paths(MAX_MULTIPATH_PATHS);
    cfg.max_remote_nat_traversal_addresses(MAX_QNT_ADDRESSES);
    cfg.server_handshake_migration(true);
    cfg.datagram_receive_buffer_size(Some(DATAGRAM_BUFFER_SIZE));
    cfg.datagram_send_buffer_size(DATAGRAM_BUFFER_SIZE);
    Arc::new(cfg)
}

/// Bind an owned-transport endpoint.
///
/// One UDP socket carries all QUIC traffic; multipath and QNT are
/// negotiated so additional paths and NAT traversal can be layered on
/// after connect.
pub async fn bind_endpoint(config: EndpointConfig) -> anyhow::Result<Endpoint> {
    let secret_key = config.secret_key.unwrap_or_else(SecretKey::generate);
    let tls = tls::TlsConfig::new(secret_key.clone());

    let client_crypto = tls.client_config(config.alpns.clone())?;
    let server_crypto = tls.server_config(config.alpns.clone())?;

    let endpoint_config =
        noq::EndpointConfig::new(Arc::new(hmac::Blake3HmacKey::new(&mut rand::rng())));

    let transport = transport_config();
    let mut server_config = noq::ServerConfig::with_crypto(Arc::new(server_crypto));
    server_config.transport = transport.clone();
    let mut client_config = noq::ClientConfig::new(Arc::new(client_crypto));
    client_config.transport_config(transport);

    let bind = config
        .bind_addr
        .unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0)));
    let socket = std::net::UdpSocket::bind(bind).context("bind udp socket")?;

    let endpoint = noq::Endpoint::new(
        endpoint_config,
        Some(server_config),
        socket,
        Arc::new(noq::TokioRuntime),
    )
    .context("create noq endpoint")?;
    endpoint.set_default_client_config(client_config);

    let local_addr = endpoint
        .local_addr()
        .context("endpoint has no local address")?;
    debug!(%local_addr, id = %secret_key.public(), "noq endpoint bound");

    Ok(Endpoint {
        inner: endpoint,
        id: secret_key.public(),
        local_addr,
        alpns: config.alpns,
        _relay: config.relay,
    })
}

/// An owned-transport endpoint: our socket, our TLS, our path policy.
#[derive(Clone)]
pub struct Endpoint {
    inner: noq::Endpoint,
    id: EndpointId,
    local_addr: SocketAddr,
    alpns: Vec<Vec<u8>>,
    _relay: Option<RelayUrl>,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("id", &self.id)
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

impl Endpoint {
    /// This endpoint's public identity.
    pub fn id(&self) -> EndpointId {
        self.id
    }

    /// The bound UDP address of this endpoint.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Advertised address: our identity plus the direct IP candidates we
    /// know about. Observed external and NAT-traversal addresses join this
    /// set once the candidate pipeline tracks them.
    pub fn addr(&self) -> EndpointAddr {
        let mut addrs = std::collections::BTreeSet::new();
        addrs.insert(TransportAddr::Ip(self.local_addr));
        EndpointAddr { id: self.id, addrs }
    }

    /// Connect to a peer by advertised address.
    ///
    /// The first direct candidate becomes the primary path; remaining
    /// candidates are opened as additional QUIC paths after the handshake.
    /// `alpn` must be one of the protocol ids configured at bind time.
    pub async fn connect(&self, target: EndpointAddr, alpn: &[u8]) -> anyhow::Result<Connection> {
        if !self.alpns.iter().any(|a| a.as_slice() == alpn) {
            bail!("alpn {alpn:?} not configured on this endpoint");
        }
        let remote_id = target.id;
        let candidates = policy::ip_candidates(&target);
        let primary = candidates
            .first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no direct addresses for {remote_id}"))?;

        let server_name = tls::name::encode(remote_id);
        let connecting = self
            .inner
            .connect(primary, &server_name)
            .context("initiate connect")?;
        let conn = connecting.await.context("handshake")?;

        policy::open_extra_paths(&conn, &candidates);

        // Kick a NAT traversal round: the peer learns our candidates, we
        // learn theirs; direct paths open in-band when both sides probe.
        match conn.initiate_nat_traversal_round() {
            Ok(addrs) if !addrs.is_empty() => {
                debug!(n = addrs.len(), "nat traversal round started")
            }
            Ok(_) => {}
            Err(e) => debug!("nat traversal round not started: {e}"),
        }

        Ok(Connection {
            inner: conn,
            remote_id,
        })
    }

    /// Accept the next incoming connection attempt.
    pub fn accept(&self) -> impl Future<Output = Option<Incoming>> + '_ {
        let accept = self.inner.accept();
        async move { accept.await.map(Incoming::new) }
    }

    /// Close all connections and the endpoint.
    pub async fn close(&self) {
        self.inner.close(0u32.into(), b"closed");
    }
}

/// An inbound connection attempt. Awaiting it completes the handshake
/// and yields the peer-verified [`Connection`].
pub struct Incoming {
    incoming: Option<noq::Incoming>,
    connecting: Option<noq::Connecting>,
}

impl Incoming {
    /// Wrap a raw incoming attempt.
    pub fn new(incoming: noq::Incoming) -> Self {
        Self {
            incoming: Some(incoming),
            connecting: None,
        }
    }

    /// Address the attempt arrived from.
    pub fn remote_address(&self) -> SocketAddr {
        match (&self.incoming, &self.connecting) {
            (Some(i), _) => i.remote_address(),
            (None, Some(c)) => c.remote_address(),
            (None, None) => SocketAddr::from(([0, 0, 0, 0], 0)),
        }
    }
}

impl Future for Incoming {
    type Output = anyhow::Result<Connection>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            if let Some(connecting) = &mut self.connecting {
                let res = ready!(Pin::new(connecting).poll(cx));
                return Poll::Ready(match res {
                    Err(e) => Err(e.into()),
                    Ok(inner) => match peer_endpoint_id(&inner) {
                        Some(remote_id) => Ok(Connection { inner, remote_id }),
                        None => Err(anyhow::anyhow!("peer presented no identity")),
                    },
                });
            }
            match self.incoming.take() {
                Some(incoming) => match incoming.accept() {
                    Ok(connecting) => self.connecting = Some(connecting),
                    Err(e) => return Poll::Ready(Err(e.into())),
                },
                None => return Poll::Ready(Err(anyhow::anyhow!("incoming consumed"))),
            }
        }
    }
}

/// A QUIC connection to a peer, owned backend.
///
/// API-compatible with the surface `rds-agent`/`rds-cli` use on the iroh
/// backend: stream open/accept, datagrams, `remote_id`, `close`.
#[derive(Clone)]
pub struct Connection {
    inner: noq::Connection,
    remote_id: EndpointId,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("remote_id", &self.remote_id)
            .field("remote_address", &self.remote_address())
            .finish()
    }
}

impl Connection {
    /// Verified peer identity (from the TLS raw public key).
    pub fn remote_id(&self) -> EndpointId {
        self.remote_id
    }

    /// Remote socket address on the initial path, if it still exists.
    pub fn remote_address(&self) -> Option<SocketAddr> {
        self.inner
            .path(noq::PathId::ZERO)
            .and_then(|p| p.remote_address().ok())
    }

    /// Open a bidirectional stream.
    pub fn open_bi(&self) -> noq::OpenBi<'_> {
        self.inner.open_bi()
    }

    /// Accept the next bidirectional stream opened by the peer.
    pub fn accept_bi(&self) -> noq::AcceptBi<'_> {
        self.inner.accept_bi()
    }

    /// Open a unidirectional stream.
    pub fn open_uni(&self) -> noq::OpenUni<'_> {
        self.inner.open_uni()
    }

    /// Accept the next unidirectional stream opened by the peer.
    pub fn accept_uni(&self) -> noq::AcceptUni<'_> {
        self.inner.accept_uni()
    }

    /// Send an unreliable datagram (QUIC DATAGRAM extension).
    pub fn send_datagram(&self, data: bytes::Bytes) -> Result<(), noq::SendDatagramError> {
        self.inner.send_datagram(data)
    }

    /// Receive the next unreliable datagram.
    pub fn read_datagram(&self) -> noq::ReadDatagram<'_> {
        self.inner.read_datagram()
    }

    /// RTT measured on a path, if it exists.
    pub fn rtt(&self, path: noq::PathId) -> Option<Duration> {
        self.inner.rtt(path)
    }

    /// Stream of path lifecycle events (established/abandoned/observed).
    pub fn path_events(&self) -> noq::PathEvents {
        self.inner.path_events()
    }

    /// Our external address as observed by the peer (QAD).
    pub fn observed_external_addr(&self) -> noq::ObservedExternalAddr {
        self.inner.observed_external_addr()
    }

    /// Close the connection.
    pub fn close(&self, error_code: noq::VarInt, reason: &[u8]) {
        self.inner.close(error_code, reason);
    }

    /// Raw noq connection for the path-policy driver.
    pub fn inner(&self) -> &noq::Connection {
        &self.inner
    }
}
