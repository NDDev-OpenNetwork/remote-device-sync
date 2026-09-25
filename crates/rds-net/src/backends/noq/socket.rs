//! Socket multiplexing: one [`AsyncUdpSocket`] facade over N child
//! transports.
//!
//! QUIC sees a single socket; the mux fans receives across children and
//! dispatches sends by destination family and `src_ip`. Children are
//! plain UDP sockets today; the WS2 relay transport plugs in as another
//! child — to QUIC it is just another way to move datagrams.

mod health;
#[cfg(test)]
mod tests;
pub use health::{ChildHealth, Health};

use std::fmt;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use noq::AsyncUdpSocket;
use noq::UdpSender;
use noq::udp::{RecvMeta, Transmit};

use super::relay;

/// One logical endpoint socket over N child transports.
pub struct Mux {
    children: Vec<Box<dyn AsyncUdpSocket>>,
    /// Round-robin receive cursor: starting the scan where the last
    /// datagram came from avoids starving later sockets.
    recv_cursor: usize,
    /// noq uses the logical socket family for every connection. A mixed mux
    /// behaves like a dual-stack IPv6 socket at the engine boundary.
    ipv6: bool,
    address: SocketAddr,
    health: Health,
    changes: health::Changes,
    receive_retry: Vec<Option<Pin<Box<tokio::time::Sleep>>>>,
}

impl Mux {
    /// A mux over `children`. The logical address reports an IPv6 child when
    /// available so noq permits both families, regardless of binding order.
    pub fn new(children: Vec<Box<dyn AsyncUdpSocket>>) -> io::Result<Self> {
        if children.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mux needs at least one transport",
            ));
        }
        let addresses = children
            .iter()
            .map(|child| child.local_addr())
            .collect::<io::Result<Vec<_>>>()?;
        let address = addresses
            .iter()
            .find(|address| address.is_ipv6())
            .copied()
            .unwrap_or(addresses[0]);
        let health = Health::new(addresses);
        Ok(Self {
            receive_retry: (0..children.len()).map(|_| None).collect(),
            children,
            recv_cursor: 0,
            ipv6: address.is_ipv6(),
            address,
            changes: health.changes(),
            health,
        })
    }

    /// Local addresses of all children — the endpoint's direct
    /// candidates across interfaces and families.
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.health
            .snapshot()
            .into_iter()
            .map(|child| child.address)
            .collect()
    }

    /// Metadata-only child state; retaining it cannot keep transport I/O alive.
    pub fn health(&self) -> Health {
        self.health.clone()
    }
}

impl fmt::Debug for Mux {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mux")
            .field("local_addrs", &self.local_addrs())
            .finish()
    }
}

impl AsyncUdpSocket for Mux {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(MuxSender {
            senders: self.children.iter().map(|c| c.create_sender()).collect(),
            health: self.health.clone(),
            changes: self.health.changes(),
            retry: None,
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mux receive needs buffers and metadata",
            )));
        }
        let _ = self.changes.poll_changed(cx);
        let n = self.children.len();
        let start = self.recv_cursor;
        for i in 0..n {
            let idx = (start + i) % n;
            if !self.health.alive(idx) {
                continue;
            }
            if self.receive_retry[idx]
                .as_mut()
                .is_some_and(|delay| delay.as_mut().poll(cx).is_pending())
            {
                continue;
            }
            self.receive_retry[idx] = None;
            match self.children[idx].poll_recv(cx, bufs, meta) {
                Poll::Ready(Ok(count)) => {
                    self.recv_cursor = (idx + 1) % n;
                    if self.ipv6 {
                        // Match noq's outgoing IPv4-mapped representation;
                        // otherwise one path acquires inconsistent four-tuples.
                        for received in meta.iter_mut().take(count) {
                            if let SocketAddr::V4(addr) = received.addr {
                                received.addr =
                                    SocketAddr::new(addr.ip().to_ipv6_mapped().into(), addr.port());
                            }
                            received.dst_ip = received.dst_ip.map(|ip| match ip {
                                IpAddr::V4(ip) => ip.to_ipv6_mapped().into(),
                                ip => ip,
                            });
                        }
                    }
                    return Poll::Ready(Ok(count));
                }
                Poll::Ready(Err(error)) => {
                    self.health.error(idx, &error, true);
                    if self.health.alive(idx) {
                        // Repeated ICMP/Interrupted/pressure errors must not
                        // spin the receiver or starve the other transports.
                        let mut delay = Box::pin(tokio::time::sleep(Duration::from_millis(10)));
                        let _ = delay.as_mut().poll(cx);
                        self.receive_retry[idx] = Some(delay);
                    }
                }
                Poll::Pending => {}
            }
        }
        if self.health.all_failed() {
            Poll::Ready(Err(health::exhausted()))
        } else {
            Poll::Pending
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        // QUIC's logical socket identity is immutable after bind, even when
        // the transport that supplied it is retired.
        Ok(self.address)
    }

    fn max_receive_segments(&self) -> NonZeroUsize {
        self.children
            .iter()
            .map(|c| c.max_receive_segments())
            .min()
            .unwrap_or(NonZeroUsize::MIN)
    }

    fn may_fragment(&self) -> bool {
        self.children.iter().any(|c| c.may_fragment())
    }
}

/// One sender per child and one health subscription per logical sender.
#[derive(Debug)]
struct MuxSender {
    senders: Vec<Pin<Box<dyn UdpSender>>>,
    health: Health,
    changes: health::Changes,
    retry: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl UdpSender for MuxSender {
    fn poll_send(
        mut self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // The logical dual-stack socket receives mapped IPv4 from noq, while
        // real IPv4 sockets and the relay peer table require native IPv4.
        let mut normalized = transmit.clone();
        if let SocketAddr::V6(addr) = normalized.destination
            && let Some(ip) = addr.ip().to_ipv4_mapped()
        {
            normalized.destination = SocketAddr::new(ip.into(), addr.port());
        }
        normalized.src_ip = normalized.src_ip.map(|ip| ip.to_canonical());
        let transmit = &normalized;
        let _ = self.changes.poll_changed(cx);
        if self.health.all_failed() {
            return Poll::Ready(Err(health::exhausted()));
        }
        let Some(idx) = self.health.route(transmit.destination, transmit.src_ip) else {
            // noq can emit QNT probes itself, before the application receives
            // their candidates. Lack of a transport is packet loss on that
            // candidate, not an I/O failure of every healthy connection/path.
            // Returning Err here would tear down the QUIC driver.
            tracing::debug!(dst = %transmit.destination, "discard datagram without a local transport");
            return Poll::Ready(Ok(()));
        };
        if !self.health.alive(idx) {
            self.retry = None;
            return Poll::Ready(Ok(())); // dead route: QUIC handles packet loss
        }
        if self
            .retry
            .as_mut()
            .is_some_and(|delay| delay.as_mut().poll(cx).is_pending())
        {
            return Poll::Pending;
        }
        self.retry = None;
        tracing::trace!(dst = %transmit.destination, child = idx, "mux send");
        match self.senders[idx].as_mut().poll_send(transmit, cx) {
            Poll::Ready(Err(error)) => {
                self.health.error(idx, &error, false);
                if self.health.all_failed() {
                    return Poll::Ready(Err(health::exhausted()));
                }
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) {
                    // Preserve the caller's packet and register a bounded
                    // retry even if a custom child returns WouldBlock without
                    // providing the trait's required readiness notification.
                    let mut delay = Box::pin(tokio::time::sleep(Duration::from_millis(1)));
                    let _ = delay.as_mut().poll(cx);
                    self.retry = Some(delay);
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(()))
                }
            }
            result => result,
        }
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        self.senders
            .iter()
            .map(|s| s.max_transmit_segments())
            .min()
            .unwrap_or(NonZeroUsize::MIN)
    }
}
