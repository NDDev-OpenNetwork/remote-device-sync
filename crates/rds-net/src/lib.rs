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

use anyhow::Context as _;

pub mod backends {
    /// Current transport substrate (iroh 1.x on noq underneath).
    pub mod iroh;
    /// Owned transport: noq endpoint + our socket/path/discovery layer.
    #[cfg(feature = "transport-noq")]
    pub mod noq;
}

// Shared identity and address types — the same key material works on
// both backends.
pub use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr};

// Shared stream/error types (re-exported through `iroh::endpoint`,
// which aliases the `noq` types the owned backend also returns).
pub use iroh::endpoint::{
    AcceptBi, AcceptUni, ClosedStream, ConnectionError, OpenBi, OpenUni, ReadDatagram, RecvStream,
    SendDatagramError, SendStream, VarInt,
};

pub use backends::iroh::{
    Ticket, default_key_path, load_or_create_key, parse_target, relay_url_of,
};

/// Which transport substrate an endpoint binds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    /// UDP bind address. `None` binds `0.0.0.0:0`.
    pub bind_addr: Option<SocketAddr>,
    /// Custom relay URL. `None` uses the backend's default relay set
    /// (n0 public relays for iroh; none for `noq` until `relay_link`).
    pub relay: Option<RelayUrl>,
    /// QUIC ALPN protocol ids. Defaults to `rds/0`.
    pub alpns: Vec<Vec<u8>>,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            secret_key: None,
            bind_addr: None,
            relay: None,
            alpns: vec![rds_core::ALPN.to_vec()],
        }
    }
}

impl EndpointConfig {
    /// Relay URL string, e.g. `https://relay.example.com` or `http://127.0.0.1:3340`.
    pub fn with_relay(mut self, url: &str) -> anyhow::Result<Self> {
        use std::str::FromStr;
        self.relay = Some(RelayUrl::from_str(url)?);
        Ok(self)
    }

    /// Select the transport backend.
    pub fn with_backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
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

/// A bound endpoint on either backend.
///
/// Cloneable handle; dropping the last clone closes the endpoint.
#[derive(Clone)]
pub struct Endpoint(EndpointInner);

#[derive(Clone)]
enum EndpointInner {
    Iroh(iroh::Endpoint),
    #[cfg(feature = "transport-noq")]
    Noq(backends::noq::Endpoint),
}

impl Endpoint {
    fn new_iroh(inner: iroh::Endpoint) -> Self {
        Self(EndpointInner::Iroh(inner))
    }

    #[cfg(feature = "transport-noq")]
    fn new_noq(inner: backends::noq::Endpoint) -> Self {
        Self(EndpointInner::Noq(inner))
    }

    /// This endpoint's public identity.
    pub fn id(&self) -> EndpointId {
        match &self.0 {
            EndpointInner::Iroh(ep) => ep.id(),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.id(),
        }
    }

    /// The advertised address of this endpoint: identity plus the
    /// transport addresses it knows about (direct IPs, home relay).
    pub fn addr(&self) -> EndpointAddr {
        match &self.0 {
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
        match &self.0 {
            EndpointInner::Iroh(ep) => ep.online().await,
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(_) => {}
        }
    }

    /// Connect to a peer by advertised address on `alpn`.
    pub async fn connect(&self, target: EndpointAddr, alpn: &[u8]) -> anyhow::Result<Connection> {
        match &self.0 {
            EndpointInner::Iroh(ep) => ep
                .connect(target, alpn)
                .await
                .map(Connection::new_iroh)
                .context("connect to peer"),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.connect(target, alpn).await.map(Connection::new_noq),
        }
    }

    /// Accept the next incoming connection attempt.
    pub async fn accept(&self) -> Option<Incoming> {
        match &self.0 {
            EndpointInner::Iroh(ep) => ep.accept().await.map(|incoming| {
                Incoming(Box::pin(async move {
                    incoming
                        .await
                        .map(Connection::new_iroh)
                        .map_err(anyhow::Error::from)
                }))
            }),
            #[cfg(feature = "transport-noq")]
            EndpointInner::Noq(ep) => ep.accept().await.map(|incoming| {
                Incoming(Box::pin(
                    async move { incoming.await.map(Connection::new_noq) },
                ))
            }),
        }
    }

    /// Close all connections and the endpoint.
    pub async fn close(&self) {
        match &self.0 {
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
pub struct Connection(ConnectionInner);

#[derive(Clone)]
enum ConnectionInner {
    Iroh(iroh::endpoint::Connection),
    #[cfg(feature = "transport-noq")]
    Noq(backends::noq::Connection),
}

impl Connection {
    fn new_iroh(inner: iroh::endpoint::Connection) -> Self {
        Self(ConnectionInner::Iroh(inner))
    }

    #[cfg(feature = "transport-noq")]
    fn new_noq(inner: backends::noq::Connection) -> Self {
        Self(ConnectionInner::Noq(inner))
    }

    /// Verified peer identity.
    pub fn remote_id(&self) -> EndpointId {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.remote_id(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.remote_id(),
        }
    }

    /// Open a bidirectional stream.
    pub fn open_bi(&self) -> OpenBi<'_> {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.open_bi(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.open_bi(),
        }
    }

    /// Accept the next bidirectional stream opened by the peer.
    pub fn accept_bi(&self) -> AcceptBi<'_> {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.accept_bi(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.accept_bi(),
        }
    }

    /// Open a unidirectional stream.
    pub fn open_uni(&self) -> OpenUni<'_> {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.open_uni(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.open_uni(),
        }
    }

    /// Accept the next unidirectional stream opened by the peer.
    pub fn accept_uni(&self) -> AcceptUni<'_> {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.accept_uni(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.accept_uni(),
        }
    }

    /// Send an unreliable datagram.
    pub fn send_datagram(&self, data: bytes::Bytes) -> Result<(), SendDatagramError> {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.send_datagram(data),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.send_datagram(data),
        }
    }

    /// Receive the next unreliable datagram.
    pub fn read_datagram(&self) -> ReadDatagram<'_> {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.read_datagram(),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.read_datagram(),
        }
    }

    /// Close the connection.
    pub fn close(&self, error_code: VarInt, reason: &[u8]) {
        match &self.0 {
            ConnectionInner::Iroh(c) => c.close(error_code, reason),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => c.close(error_code, reason),
        }
    }
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
