//! Socket multiplexing: one [`AsyncUdpSocket`] facade over N child
//! transports.
//!
//! QUIC sees a single socket; the mux fans receives across children and
//! dispatches sends by destination family and `src_ip`. Children are
//! plain UDP sockets today; the WS2 relay transport plugs in as another
//! child — to QUIC it is just another way to move datagrams.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
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
}

impl Mux {
    /// A mux over `children`; the first child's local address is the
    /// primary one reported by [`AsyncUdpSocket::local_addr`].
    pub fn new(children: Vec<Box<dyn AsyncUdpSocket>>) -> io::Result<Self> {
        if children.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mux needs at least one transport",
            ));
        }
        Ok(Self {
            children,
            recv_cursor: 0,
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
                    return Poll::Ready(out);
                }
                Poll::Pending => {}
            }
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
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
        self.senders
            .iter()
            .position(|(addr, _)| {
                // The relay child's synthetic local is IPv4 — exclude it
                // from the family fallback or every v4 transmit could
                // land in the tunnel.
                addr.is_some_and(|a| {
                    !relay::is_synthetic(a) && a.is_ipv4() == transmit.destination.is_ipv4()
                })
            })
            .or(if self.senders.is_empty() {
                None
            } else {
                Some(0)
            })
    }
}

impl UdpSender for MuxSender {
    fn poll_send(
        mut self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(idx) = self.pick(transmit) else {
            tracing::warn!(dst = %transmit.destination, "no mux transport can carry transmit");
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no mux transport can carry this transmit",
            )));
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
