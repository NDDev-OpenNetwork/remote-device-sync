//! Network layer for rds: endpoint lifecycle, persistent identity,
//! ticket encoding, and transport backends.
//!
//! The network identity of an rds peer is an Ed25519 key pair; QUIC-TLS
//! authentication is derived from it, so a connection is always pinned
//! to a key rather than an IP address.
//!
//! # Backends
//!
//! Two backends live behind this crate's facade:
//!
//! - [`backends::iroh`] — the shipping substrate: iroh endpoint with
//!   hole punching, relay fallback and n0/pkarr discovery plumbing.
//! - [`backends::noq`] — the owned transport: a noq endpoint with our
//!   own TLS, candidate pipeline and path policy (feature
//!   `transport-noq`).
//!
//! Dependent crates program against the facade re-exported here —
//! [`Endpoint`], [`Connection`], [`Incoming`] — and select the backend
//! in [`EndpointConfig`] at `bind_endpoint` time. The iroh backend is
//! the default until the owned backend passes the C1 parity gate.
//!
//! Stream and error types ([`SendStream`], [`RecvStream`],
//! [`ConnectionError`], [`VarInt`], ...) are shared: the iroh backend
//! is itself built on `noq`, so both backends speak the same types.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::Context as _;

pub mod announce;
pub mod config;
pub mod backends {
    /// Current transport substrate (iroh 1.x on noq underneath).
    pub mod iroh;
    /// Owned transport: noq endpoint + our socket/path/discovery layer.
    #[cfg(feature = "transport-noq")]
    pub mod noq;
}
pub mod metrics;
mod uni;
pub use uni::{UniRoutingStats, UniStreams};
pub mod resolve;

// Shared identity and address types — the same key material works on
// both backends.
pub use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr};

// Shared stream/error types (re-exported through `iroh::endpoint`,
// which aliases the `noq` types the owned backend also returns).
pub use iroh::endpoint::{
    AcceptBi, AcceptUni, ClosedStream, ConnectionError, OpenBi, OpenUni, ReadDatagram, ReadError,
    ReadExactError, RecvStream, SendDatagramError, SendStream, VarInt, WriteError,
};

pub use announce::{Announce, AnnounceConfig, announce};
pub use backends::iroh::{
    Ticket, default_key_path, load_or_create_key, parse_target, relay_url_of,
};
pub use config::{ConfigError, EndpointOverrides, EndpointSettings, RelaySettings};
pub use resolve::resolve_target;

/// Which transport substrate an endpoint binds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// iroh endpoint: relay fallback, hole punching, n0 discovery.
    /// The default until `noq` passes the C1 parity gate.
    #[default]
    Iroh,
    /// Owned transport on `noq`: our TLS, socket, candidate pipeline and
    /// path policy. Requires the `transport-noq` feature.
    #[cfg(feature = "transport-noq")]
    Noq,
}

/// How an endpoint reaches the network.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Transport substrate to bind. Default: iroh.
    pub backend: Backend,
    /// Persisted or generated Ed25519 secret key.
    pub secret_key: Option<SecretKey>,
    /// UDP bind addresses. Empty binds `0.0.0.0:0`. Multiple entries
    /// bind multiple interfaces on backends that support socket muxing
    /// (`noq`); iroh refuses more than one entry.
    pub bind_addrs: Vec<SocketAddr>,
    /// Custom iroh relay URLs. Refused by the owned backend, which uses
    /// `relay_endpoint`. Empty uses the backend's preset (iroh public relays
    /// when discovery is enabled; otherwise direct only).
    pub relays: Vec<RelayUrl>,
    /// Publish/resolve addresses via the backend's lookup services
    /// (iroh: n0 DNS + pkarr). `false` binds the `Minimal` preset —
    /// dialing uses exactly the `EndpointAddr` given, which is what
    /// impairment tests need to keep every datagram on the proxied
    /// path. Default `true`.
    pub discovery: bool,
    /// Cap on concurrent QUIC multipath paths. `Some(1)` pins every
    /// connection to its established path — impairment tests use this to
    /// keep traffic on a proxied link instead of migrating to a
    /// discovered direct path. `None` uses the backend default.
    pub max_multipath_paths: Option<u32>,
    /// Owned-relay attachment (`noq` backend only): the relay server's
    /// endpoint address. When set, a relay tunnel socket joins the
    /// socket mux and the endpoint advertises it as
    /// `TransportAddr::Relay` — peers sharing the relay can open
    /// relayed paths that migrate like any other QUIC path.
    #[cfg(feature = "transport-noq")]
    pub relay_endpoint: Option<EndpointAddr>,
    /// QUIC ALPN protocol ids. Defaults to `rds/0`.
    pub alpns: Vec<Vec<u8>>,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            secret_key: None,
            bind_addrs: Vec::new(),
            relays: Vec::new(),
            discovery: true,
            max_multipath_paths: None,
            #[cfg(feature = "transport-noq")]
            relay_endpoint: None,
            alpns: vec![rds_core::ALPN.to_vec()],
        }
    }
}

impl EndpointConfig {
    /// Relay URL string, e.g. `https://relay.example.com` or `http://127.0.0.1:3340`.
    pub fn with_relay(mut self, url: &str) -> anyhow::Result<Self> {
        use std::str::FromStr;
        self.relays.push(RelayUrl::from_str(url)?);
        Ok(self)
    }

    /// Multiple relay URLs — the client fails over between them.
    pub fn with_relays<I, S>(mut self, urls: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        use std::str::FromStr;
        for url in urls {
            self.relays.push(RelayUrl::from_str(url.as_ref())?);
        }
        Ok(self)
    }

    /// Select the transport backend.
    pub fn with_backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    /// Disable address lookup services — the endpoint publishes nothing
    /// and resolves nothing; only explicitly dialed addresses are used.
    pub fn without_discovery(mut self) -> Self {
        self.discovery = false;
        self
    }

    /// Pin connections to their established path (single path, no
    /// multipath migration). Impairment and determinism tests use this.
    pub fn with_path_pinning(mut self) -> Self {
        self.max_multipath_paths = Some(1);
        self
    }
}

/// Bind an endpoint on the configured backend.
pub async fn bind_endpoint(config: EndpointConfig) -> anyhow::Result<Endpoint> {
    match config.backend {
        Backend::Iroh => backends::iroh::bind_endpoint(config)
            .await
            .map(Endpoint::new_iroh),
        #[cfg(feature = "transport-noq")]
        Backend::Noq => backends::noq::bind_endpoint(config)
            .await
            .map(Endpoint::new_noq),
    }
}

/// Bind a noq endpoint on a caller-provided socket — the injection seam
/// for impairment sockets, sim harnesses, or any custom
/// [`noq::AsyncUdpSocket`]. The endpoint drives the socket like any
/// other transport, so impairment sits underneath QUIC and cannot be
/// bypassed by path selection.
#[cfg(feature = "transport-noq")]
pub async fn bind_noq_with_socket(
    mut config: EndpointConfig,
    socket: Box<dyn noq::AsyncUdpSocket>,
    local_addrs: Vec<SocketAddr>,
    runtime: std::sync::Arc<dyn noq::Runtime>,
) -> anyhow::Result<Endpoint> {
    config.backend = Backend::Noq;
    backends::noq::bind_with_socket(config, socket, local_addrs, runtime, None)
        .await
        .map(Endpoint::new_noq)
}

/// A bound endpoint on either backend.
///
/// Cloneable handle; dropping the last clone closes the endpoint.
#[derive(Clone)]
pub struct Endpoint {
    inner: EndpointInner,
    metrics: metrics::Registry,
}

#[derive(Clone)]
enum EndpointInner {
    Iroh(iroh::Endpoint),
    #[cfg(feature = "transport-noq")]
    Noq(Box<backends::noq::Endpoint>),
}

impl Endpoint {
    fn new_iroh(inner: iroh::Endpoint) -> Self {
        Self {
            inner: EndpointInner::Iroh(inner),
            metrics: metrics::Registry::default(),
        }
    }

    #[cfg(feature = "transport-noq")]
    fn new_noq(inner: backends::noq::Endpoint) -> Self {
        let metrics = inner.metrics();
        Self {
            inner: EndpointInner::Noq(Box::new(inner)),
            metrics,
        }
    }

    /// This endpoint's metrics registry: connection counters, sampled
    /// per-path datagram/loss/congestion totals split relay-vs-direct,
    /// QNT attempts (noq backend), and live gauges. Share it with a
    /// scraper; `render_prometheus` needs the `metrics` feature.
    pub fn metrics(&self) -> metrics::Registry {
        self.metrics.clone()
    }

    /// This endpoint's public identity.
    pub fn id(&self) -> EndpointId {
        match &self.inner {
            EndpointInner::Iroh(ep) => ep.id(),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.id(),
        }
    }

    /// The advertised address of this endpoint: identity plus the
    /// transport addresses it knows about (direct IPs, home relay).
    pub fn addr(&self) -> EndpointAddr {
        match &self.inner {
            EndpointInner::Iroh(ep) => ep.addr(),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.addr(),
        }
    }

    /// Wait until the endpoint's relay path is usable (iroh).
    ///
    /// The `noq` backend returns immediately — relay transport lands
    /// with `relay_link` (WS2).
    pub async fn online(&self) {
        match &self.inner {
            EndpointInner::Iroh(ep) => ep.online().await,
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(_) => {}
        }
    }

    /// Connect to a peer by advertised address on `alpn`.
    pub async fn connect(&self, target: EndpointAddr, alpn: &[u8]) -> anyhow::Result<Connection> {
        let conn = match &self.inner {
            EndpointInner::Iroh(ep) => ep
                .connect(target, alpn)
                .await
                .map(Connection::new_iroh)
                .context("connect to peer"),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.connect(target, alpn).await.map(Connection::new_noq),
        }?;
        self.metrics.connection_opened();
        Ok(conn)
    }

    /// Accept the next incoming connection attempt.
    pub async fn accept(&self) -> Option<Incoming> {
        let metrics = self.metrics.clone();
        let counted = move |fut: Connection| {
            metrics.connection_accepted();
            fut
        };
        match &self.inner {
            EndpointInner::Iroh(ep) => ep.accept().await.map(|incoming| {
                Incoming(Box::pin(async move {
                    incoming
                        .await
                        .map(Connection::new_iroh)
                        .map(counted)
                        .map_err(anyhow::Error::from)
                }))
            }),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.accept().await.map(|incoming| {
                Incoming(Box::pin(async move {
                    incoming.await.map(Connection::new_noq).map(counted)
                }))
            }),
        }
    }

    /// Close all connections and the endpoint.
    pub async fn close(&self) {
        match &self.inner {
            EndpointInner::Iroh(ep) => ep.close().await,
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.close().await,
        }
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint").field("id", &self.id()).finish()
    }
}

/// An inbound connection attempt; awaiting completes the handshake.
pub struct Incoming(Pin<Box<dyn Future<Output = anyhow::Result<Connection>> + Send>>);

impl Future for Incoming {
    type Output = anyhow::Result<Connection>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}

/// A QUIC connection to a peer, on either backend.
///
/// Stream futures and error types are shared — both backends return
/// the same `noq` types.
#[derive(Clone)]
pub struct Connection {
    inner: ConnectionInner,
    /// Routes inbound uni streams to the consumer that claimed their
    /// `UniHello` tag — see [`Connection::uni_streams`].
    demux: std::sync::Arc<uni::Demux>,
}

#[derive(Clone)]
enum ConnectionInner {
    Iroh(iroh::endpoint::Connection),
    #[cfg(feature = "transport-noq")]
    Noq(backends::noq::Connection),
}

impl ConnectionInner {
    fn accept_uni(&self) -> AcceptUni<'_> {
        match self {
            Self::Iroh(c) => c.accept_uni(),
            #[cfg(feature = "transport-noq")]
            Self::Noq(c) => c.accept_uni(),
        }
    }

    async fn closed(&self) {
        match self {
            Self::Iroh(c) => {
                c.closed().await;
            }
            #[cfg(feature = "transport-noq")]
            Self::Noq(c) => {
                c.inner().closed().await;
            }
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Self::Iroh(c) => c.close_reason().is_some(),
            #[cfg(feature = "transport-noq")]
            Self::Noq(c) => c.inner().close_reason().is_some(),
        }
    }
}

impl Connection {
    fn new_iroh(inner: iroh::endpoint::Connection) -> Self {
        Self {
            inner: ConnectionInner::Iroh(inner),
            demux: Default::default(),
        }
    }

    #[cfg(feature = "transport-noq")]
    fn new_noq(inner: backends::noq::Connection) -> Self {
        Self {
            inner: ConnectionInner::Noq(inner),
            demux: Default::default(),
        }
    }

    /// Verified peer identity.
    pub fn remote_id(&self) -> EndpointId {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.remote_id(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.remote_id(),
        }
    }

    /// Open a bidirectional stream.
    pub fn open_bi(&self) -> OpenBi<'_> {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.open_bi(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.open_bi(),
        }
    }

    /// Accept the next bidirectional stream opened by the peer.
    pub fn accept_bi(&self) -> AcceptBi<'_> {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.accept_bi(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.accept_bi(),
        }
    }

    /// Open a unidirectional stream.
    pub fn open_uni(&self) -> OpenUni<'_> {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.open_uni(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.open_uni(),
        }
    }

    /// Accept the next unidirectional stream opened by the peer.
    ///
    /// Prefer [`uni_streams`](Self::uni_streams): on a connection whose
    /// services use tagged streams, a direct `accept_uni` races the
    /// demux and can steal tagged streams from their consumers.
    pub fn accept_uni(&self) -> AcceptUni<'_> {
        self.inner.accept_uni()
    }

    /// Claim inbound uni streams tagged `kind` (the `UniHello` first
    /// frame every v3 sender writes). The first claim on a connection
    /// spawns the shared demux that owns `accept_uni`; each kind allows
    /// one live claim — a second registration while the first inbox is
    /// alive fails rather than splitting the queue, and a dropped
    /// inbox frees the kind for re-claim.
    ///
    /// Must be called inside a tokio runtime.
    pub fn uni_streams(&self, kind: rds_core::UniHello) -> anyhow::Result<UniStreams> {
        self.demux.claim(&self.inner, kind)
    }

    /// Current uni-router work and local receive limits. These are resource
    /// limits of this implementation, not negotiated protocol capabilities.
    pub fn uni_routing_stats(&self) -> UniRoutingStats {
        self.demux.stats()
    }

    /// Send an unreliable datagram.
    pub fn send_datagram(&self, data: bytes::Bytes) -> Result<(), SendDatagramError> {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.send_datagram(data),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.send_datagram(data),
        }
    }

    /// Receive the next unreliable datagram.
    pub fn read_datagram(&self) -> ReadDatagram<'_> {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.read_datagram(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.read_datagram(),
        }
    }

    /// Close the connection.
    pub fn close(&self, error_code: VarInt, reason: &[u8]) {
        match &self.inner {
            ConnectionInner::Iroh(c) => c.close(error_code, reason),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.close(error_code, reason),
        }
    }

    /// Whether the connection has closed (either side). Samplers use
    /// this as their stop condition.
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Wait for transport closure. Service owners coordinate their own cleanup;
    /// this notification does not imply all application tasks have been joined.
    pub async fn wait_closed(&self) {
        self.inner.closed().await;
    }

    /// Snapshot of every live path's transport counters, normalized
    /// across backends. Used by media pacing (WS5) and metrics (WS7).
    pub fn path_stats(&self) -> Vec<PathStats> {
        match &self.inner {
            ConnectionInner::Iroh(c) => c
                .paths()
                .iter()
                .map(|p| {
                    let s = p.stats();
                    PathStats {
                        path_id: path_id_u64(p.id()),
                        rtt: s.rtt,
                        cwnd: s.cwnd,
                        sent: s.udp_tx.datagrams,
                        lost: s.lost_packets,
                        sent_bytes: s.udp_tx.bytes,
                        recv_bytes: s.udp_rx.bytes,
                        congestion_events: s.congestion_events,
                        selected: p.is_selected(),
                        via_relay: p.is_relay(),
                    }
                })
                .collect(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => {
                // PathIds are sequential from ZERO; probe until a run of
                // misses marks the end of the live set.
                let mut out = Vec::new();
                let mut misses = 0u32;
                for raw in 0..64u32 {
                    match c.inner().path_stats(noq::PathId::from(raw)) {
                        Some(s) => {
                            misses = 0;
                            out.push(PathStats {
                                path_id: u64::from(raw),
                                rtt: s.rtt,
                                cwnd: s.cwnd,
                                sent: s.udp_tx.datagrams,
                                lost: s.lost_packets,
                                sent_bytes: s.udp_tx.bytes,
                                recv_bytes: s.udp_rx.bytes,
                                congestion_events: s.congestion_events,
                                selected: raw == 0,
                                via_relay: false,
                            });
                        }
                        None => {
                            misses += 1;
                            if misses >= 8 {
                                break;
                            }
                        }
                    }
                }
                out
            }
        }
    }

    /// Stats of the path currently selected for transmission — the one
    /// media pacing should react to. `None` before the first path exists.
    pub fn current_path_stats(&self) -> Option<PathStats> {
        let paths = self.path_stats();
        paths
            .iter()
            .find(|p| p.selected)
            .copied()
            .or_else(|| paths.into_iter().max_by_key(|p| p.sent))
    }
}

/// Per-path transport counters, backend-normalized. All fields are
/// cumulative since path creation except `rtt`/`cwnd` (instantaneous).
#[derive(Debug, Clone, Copy)]
pub struct PathStats {
    /// Backend path identifier.
    pub path_id: u64,
    /// Smoothed round-trip estimate.
    pub rtt: Duration,
    /// Congestion window in bytes.
    pub cwnd: u64,
    /// Datagrams sent on this path.
    pub sent: u64,
    /// Datagrams declared lost on this path.
    pub lost: u64,
    /// UDP payload bytes transmitted on this path.
    pub sent_bytes: u64,
    /// UDP payload bytes received on this path.
    pub recv_bytes: u64,
    /// Congestion events signalled on this path.
    pub congestion_events: u64,
    /// Whether the connection currently transmits on this path.
    pub selected: bool,
    /// Whether this path traverses a relay (vs a direct address).
    pub via_relay: bool,
}

fn path_id_u64(id: iroh::endpoint::PathId) -> u64 {
    // PathId's inner u32 is crate-private; its Display prints the number.
    id.to_string().parse().unwrap_or(u64::MAX)
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("remote_id", &self.remote_id())
            .finish()
    }
}

impl From<iroh::endpoint::Connection> for Connection {
    fn from(inner: iroh::endpoint::Connection) -> Self {
        Self::new_iroh(inner)
    }
}

#[cfg(feature = "transport-noq")]
impl From<backends::noq::Connection> for Connection {
    fn from(inner: backends::noq::Connection) -> Self {
        Self::new_noq(inner)
    }
}
