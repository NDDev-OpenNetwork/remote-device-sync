//! Relay transport: an [`AsyncUdpSocket`] that tunnels datagrams through
//! the owned relay protocol (`rds-relay/0`), plugged into the socket
//! mux as just another child.
//!
//! QUIC paths are 4-tuples of `SocketAddr`s, so relayed remotes get a
//! **synthetic address**: an IPv4 in the RFC 2544 benchmark range
//! `198.19.0.0/16` derived from the peer's `EndpointId`. The address is
//! never routed — a transmit to it only ever means "via the relay
//! tunnel", and a received `RecvMeta` with it only ever means "arrived
//! via the relay". IPv4 specifically because noq rejects IPv6 remotes
//! on IPv4-bound endpoints; the reserved range cannot collide with real
//! traffic.
//!
//! Ownership: the socket keeps a helper endpoint (same secret key →
//! same `EndpointId`) connected to the relay server. The relay sees
//! the endpoint's real identity; datagrams are forwarded as
//! `[src_key][payload]`.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey};
use noq::udp::{RecvMeta, Transmit};
use noq::{AsyncUdpSocket, Runtime, UdpSender};
use rds_core::relay::{self, RelayControl};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::Connection;

/// First two octets marking a synthetic relay-mapped address.
const SYNTHETIC_PREFIX: [u8; 2] = [198, 19];

/// Whether `ip` is a synthetic relay-mapped address.
pub fn is_synthetic_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[0..2] == SYNTHETIC_PREFIX)
}

/// Whether `addr` is a synthetic relay-mapped address.
pub fn is_synthetic(addr: SocketAddr) -> bool {
    is_synthetic_ip(addr.ip())
}

/// The synthetic remote address representing `peer` over the relay.
///
/// Deterministic and symmetric: A computes it for B exactly as B
/// computes it for A. BLAKE3 over the endpoint id fills the free bits
/// (16 host bits inside `198.19.0.0/16`, 16 port bits); collisions merge
/// paths only in the pathological case and are noted in the C2 report.
pub fn synthetic_for(id: &EndpointId) -> SocketAddr {
    let h = blake3::hash(id.as_bytes());
    let b = h.as_bytes();
    SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(198, 19, b[0], b[1])),
        u16::from_be_bytes([b[2], b[3]]),
    )
}

/// Encode the relay's endpoint address as the `TransportAddr::Relay`
/// payload: `rds-relay://<id>@<ip>:<port>`.
pub fn relay_url_for(relay: &EndpointAddr) -> Option<RelayUrl> {
    let sock = relay.addrs.iter().find_map(|a| match a {
        iroh::TransportAddr::Ip(s) => Some(*s),
        _ => None,
    })?;
    format!("rds-relay://{}@{sock}", relay.id).parse().ok()
}

/// Parse an `rds-relay://` url back into `(relay_id, socket_addr)`.
pub fn parse_relay_url(url: &RelayUrl) -> Option<(EndpointId, SocketAddr)> {
    if url.scheme() != "rds-relay" {
        return None;
    }
    let id: EndpointId = url.username().parse().ok()?;
    let host = url.host_str()?.parse().ok()?;
    let port = url.port()?;
    Some((id, SocketAddr::new(host, port)))
}

/// Shared handle the endpoint keeps to steer the relay socket inside
/// the mux.
#[derive(Clone)]
pub struct RelayHandle {
    /// The relay server's endpoint id — matches peers' advertised
    /// `TransportAddr::Relay` urls.
    pub relay_id: EndpointId,
    /// The relay server's socket address — what we dial.
    pub relay_sock: SocketAddr,
    /// The `rds-relay://` url we advertise as `TransportAddr::Relay`.
    pub url: RelayUrl,
    /// synthetic → EndpointId, filled on connect/accept/receive so
    /// `poll_send` can decode transmit destinations.
    peers: Arc<Mutex<HashMap<SocketAddr, EndpointId>>>,
    /// Set when the relay announces drain — sends fail, recv stalls.
    drained: Arc<AtomicBool>,
}

impl RelayHandle {
    /// Teach the socket that `id` is reachable as its synthetic addr.
    pub fn register_peer(&self, id: EndpointId) {
        self.peers.lock().unwrap().insert(synthetic_for(&id), id);
    }

    /// Whether the relay announced it is draining.
    pub fn drained(&self) -> bool {
        self.drained.load(Ordering::Relaxed)
    }
}

/// A mux child that forwards datagrams through the relay connection.
pub struct RelaySocket {
    /// Helper endpoint holding the relay connection; same key → same id.
    _endpoint: super::Endpoint,
    conn: Connection,
    local: SocketAddr,
    peers: Arc<Mutex<HashMap<SocketAddr, EndpointId>>>,
    drained: Arc<AtomicBool>,
    rx: mpsc::UnboundedReceiver<(Vec<u8>, SocketAddr)>,
    _tasks: (JoinHandle<()>, JoinHandle<()>),
}

impl RelaySocket {
    /// Attach to `relay` with endpoint key `key`.
    ///
    /// The helper endpoint binds a plain UDP socket (no relay recursion)
    /// and dials the relay on [`proto::RELAY_ALPN`]; registration
    /// completes before the socket reports ready.
    pub async fn connect(
        relay: EndpointAddr,
        key: SecretKey,
        bind: SocketAddr,
    ) -> anyhow::Result<(Self, RelayHandle)> {
        let relay_id = relay.id;
        let relay_sock = relay
            .addrs
            .iter()
            .find_map(|a| match a {
                iroh::TransportAddr::Ip(s) => Some(*s),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("relay address has no IP candidate"))?;

        // Helper endpoint on a plain UDP socket — no mux, no relay
        // recursion. Same key → same EndpointId, so the relay maps this
        // connection to our real identity.
        let our_id = key.public();
        let runtime = Arc::new(noq::TokioRuntime);
        let udp = std::net::UdpSocket::bind(bind)?;
        let local = udp.local_addr()?;
        let socket = runtime.wrap_udp_socket(udp)?;
        let endpoint = super::bind_with_socket(
            crate::EndpointConfig {
                secret_key: Some(key),
                alpns: vec![relay::RELAY_ALPN.to_vec()],
                ..Default::default()
            },
            socket,
            vec![local],
            runtime,
            None,
        )
        .await?;
        let conn = endpoint.connect(relay.clone(), relay::RELAY_ALPN).await?;

        // Registration handshake on the first bidi stream.
        let (mut ctrl_send, mut ctrl_recv) = conn.open_bi().await?;
        let body = postcard::to_stdvec(&RelayControl::Register)?;
        ctrl_send
            .write_all(&(body.len() as u32).to_be_bytes())
            .await?;
        ctrl_send.write_all(&body).await?;
        ctrl_send.flush().await?;
        let mut len = [0u8; 4];
        ctrl_recv.read_exact(&mut len).await?;
        let n = u32::from_be_bytes(len) as usize;
        let mut buf = vec![0u8; n.min(4096)];
        ctrl_recv.read_exact(&mut buf).await?;
        match postcard::from_bytes::<RelayControl>(&buf) {
            Ok(RelayControl::Registered) => {}
            other => anyhow::bail!("relay registration rejected: {other:?}"),
        }

        let peers: Arc<Mutex<HashMap<SocketAddr, EndpointId>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let drained = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded_channel();

        // Datagram pump: relay → synthetic-addressed receives.
        let dgram_pump = tokio::spawn({
            let conn = conn.clone();
            let peers = peers.clone();
            async move {
                loop {
                    match conn.read_datagram().await {
                        Ok(frame) => {
                            tracing::trace!(len = frame.len(), "relay recv datagram");
                            let Some((src, payload)) = relay::decode_frame(&frame) else {
                                continue;
                            };
                            let Ok(src) = EndpointId::from_bytes(&src) else {
                                continue;
                            };
                            let syn = synthetic_for(&src);
                            peers.lock().unwrap().insert(syn, src);
                            if tx.send((payload.to_vec(), syn)).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
        });

        // Control reader: Drain/PeerGone/liveness replies.
        let ctrl_pump = tokio::spawn({
            let drained = drained.clone();
            let ctrl_send = tokio::sync::Mutex::new(ctrl_send);
            async move {
                loop {
                    let mut len = [0u8; 4];
                    if ctrl_recv.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let n = u32::from_be_bytes(len) as usize;
                    if n > 4096 {
                        return;
                    }
                    let mut buf = vec![0u8; n];
                    if ctrl_recv.read_exact(&mut buf).await.is_err() {
                        return;
                    }
                    match postcard::from_bytes::<RelayControl>(&buf) {
                        Ok(RelayControl::Drain) => {
                            warn!("relay draining — relayed paths go dark");
                            drained.store(true, Ordering::SeqCst);
                        }
                        Ok(RelayControl::PeerGone { peer }) => {
                            debug!(peer = %data_encoding::HEXLOWER.encode(&peer[..8]), "relay peer gone");
                        }
                        Ok(RelayControl::Ping { seq }) => {
                            let pong = postcard::to_stdvec(&RelayControl::Pong { seq })
                                .unwrap_or_default();
                            let mut frame = (pong.len() as u32).to_be_bytes().to_vec();
                            frame.extend_from_slice(&pong);
                            let w = ctrl_send.lock().await;
                            let mut w = w;
                            let _ = w.write_all(&frame).await;
                        }
                        _ => {}
                    }
                }
            }
        });

        // Our own synthetic address — what this endpoint advertises and
        // what reply traffic from peers resolves against.
        let local = synthetic_for(&our_id);
        let handle = RelayHandle {
            relay_id,
            relay_sock,
            url: relay_url_for(&relay)
                .ok_or_else(|| anyhow::anyhow!("relay addr has no IP candidate"))?,
            peers: peers.clone(),
            drained: drained.clone(),
        };
        Ok((
            Self {
                _endpoint: endpoint,
                conn,
                local,
                peers,
                drained,
                rx,
                _tasks: (dgram_pump, ctrl_pump),
            },
            handle,
        ))
    }
}

impl fmt::Debug for RelaySocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelaySocket")
            .field("local", &self.local)
            .finish()
    }
}

impl AsyncUdpSocket for RelaySocket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(RelaySender {
            conn: self.conn.clone(),
            peers: self.peers.clone(),
            drained: self.drained.clone(),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if self.drained.load(Ordering::SeqCst) {
            // Drain semantics: stop surfacing packets so the path dies
            // and the driver migrates traffic off the relay.
            return Poll::Pending;
        }
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some((data, src))) => {
                let n = data.len().min(bufs[0].len());
                bufs[0][..n].copy_from_slice(&data[..n]);
                let mut m = RecvMeta::default();
                m.addr = src;
                m.len = n;
                m.stride = n;
                m.dst_ip = Some(self.local.ip());
                meta[0] = m;
                Poll::Ready(Ok(1))
            }
            // The tunnel conn is dead — behave like a link that went
            // down: surface nothing and let the paths over it time out.
            // An I/O error here would poison the whole mux endpoint.
            Poll::Ready(None) => Poll::Pending,
            Poll::Pending => Poll::Pending,
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

/// Sends outer-QUIC packets into the relay tunnel.
struct RelaySender {
    conn: Connection,
    peers: Arc<Mutex<HashMap<SocketAddr, EndpointId>>>,
    drained: Arc<AtomicBool>,
}

impl fmt::Debug for RelaySender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelaySender").finish()
    }
}

impl UdpSender for RelaySender {
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        if self.drained.load(Ordering::SeqCst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "relay draining",
            )));
        }
        let dst = transmit.destination;
        tracing::trace!(%dst, len = transmit.contents.len(), "relay socket send");
        if !is_synthetic(dst) {
            warn!(%dst, "relay socket got non-relay transmit");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("relay socket got non-relay transmit to {dst}"),
            )));
        }
        let Some(id) = self.peers.lock().unwrap().get(&dst).copied() else {
            warn!(%dst, "no endpoint registered for synthetic address");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                format!("no endpoint registered for {dst}"),
            )));
        };
        let frame = relay::encode_forward(id.as_bytes(), transmit.contents);
        match self.conn.send_datagram(frame) {
            Ok(()) => Poll::Ready(Ok(())),
            // The tunnel is a link with a smaller MTU than the direct
            // path: drop oversized transmits instead of erroring, and
            // path-MTU discovery converges below the ceiling — exactly
            // what a real link does to packets it cannot carry.
            Err(noq::SendDatagramError::TooLarge) => {
                tracing::trace!(%dst, len = transmit.contents.len(), "relay drop: exceeds tunnel MTU");
                Poll::Ready(Ok(()))
            }
            Err(e) => {
                warn!(error = %e, "relay datagram send failed");
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("relay datagram send failed: {e}"),
                )))
            }
        }
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        NonZeroUsize::MIN
    }
}
