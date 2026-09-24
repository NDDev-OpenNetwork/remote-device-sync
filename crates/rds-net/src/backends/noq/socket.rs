//! Socket multiplexing: one [`AsyncUdpSocket`] facade over N child
//! transports.
//!
//! QUIC sees a single socket; the mux fans receives across children and
//! dispatches sends by destination family and `src_ip`. Children are
//! plain UDP sockets today; the WS2 relay transport plugs in as another
//! child — to QUIC it is just another way to move datagrams.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};

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
        let ipv6 = children
            .iter()
            .any(|c| c.local_addr().is_ok_and(|a| a.is_ipv6()));
        Ok(Self {
            children,
            recv_cursor: 0,
            ipv6,
        })
    }

    /// Local addresses of all children — the endpoint's direct
    /// candidates across interfaces and families.
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.children
            .iter()
            .filter_map(|c| c.local_addr().ok())
            .collect()
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
            senders: self
                .children
                .iter()
                .map(|c| (c.local_addr().ok(), c.create_sender()))
                .collect(),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let n = self.children.len();
        for i in 0..n {
            let idx = (self.recv_cursor + i) % n;
            match self.children[idx].poll_recv(cx, bufs, meta) {
                Poll::Ready(out) => {
                    self.recv_cursor = (idx + 1) % n;
                    if self.ipv6
                        && let Ok(count) = out
                    {
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
                    return Poll::Ready(out);
                }
                Poll::Pending => {}
            }
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        if self.ipv6 {
            for child in &self.children {
                if let Ok(addr) = child.local_addr()
                    && addr.is_ipv6()
                {
                    return Ok(addr);
                }
            }
        }
        self.children[0].local_addr()
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

type ChildSender = (Option<SocketAddr>, Pin<Box<dyn UdpSender>>);

/// One [`UdpSender`] per child; `poll_send` picks the child that can
/// legally source this transmit.
#[derive(Debug)]
struct MuxSender {
    senders: Vec<ChildSender>,
}

impl MuxSender {
    /// Child index able to carry `transmit`: synthetic destinations go
    /// to the relay child, an explicit `src_ip` wins, else the child
    /// whose family matches the destination.
    fn pick(&self, transmit: &Transmit<'_>) -> Option<usize> {
        // Synthetic relay-mapped destinations only resolve through the
        // tunnel socket — the one whose own local address is synthetic.
        if relay::is_synthetic(transmit.destination) {
            return self
                .senders
                .iter()
                .position(|(addr, _)| addr.is_some_and(relay::is_synthetic));
        }
        if let Some(src) = transmit.src_ip
            && let Some(i) = self
                .senders
                .iter()
                .position(|(addr, _)| addr.is_some_and(|a| a.ip() == src))
        {
            return Some(i);
        }
        self.senders.iter().position(|(addr, _)| {
            // The relay child's synthetic local is IPv4 — exclude it
            // from the family fallback or every v4 transmit could
            // land in the tunnel.
            addr.is_some_and(|a| {
                !relay::is_synthetic(a) && a.is_ipv4() == transmit.destination.is_ipv4()
            })
        })
    }
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
        let Some(idx) = self.pick(transmit) else {
            // noq can emit QNT probes itself, before the application receives
            // their candidates. Lack of a transport is packet loss on that
            // candidate, not an I/O failure of every healthy connection/path.
            // Returning Err here would tear down the QUIC driver.
            tracing::debug!(dst = %transmit.destination, "discard datagram without a local transport");
            return Poll::Ready(Ok(()));
        };
        tracing::trace!(dst = %transmit.destination, child = idx, "mux send");
        self.senders[idx].1.as_mut().poll_send(transmit, cx)
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        self.senders
            .iter()
            .map(|(_, s)| s.max_transmit_segments())
            .min()
            .unwrap_or(NonZeroUsize::MIN)
    }
}
