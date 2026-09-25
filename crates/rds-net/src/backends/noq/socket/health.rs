//! Shared, monotonic child retirement. Diagnostic handles never retain sockets.
use std::{
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

#[derive(Debug)]
struct Child {
    address: SocketAddr,
    stopped: CancellationToken,
    error: Mutex<Option<io::ErrorKind>>,
    send_errors: AtomicU64,
    receive_errors: AtomicU64,
}

/// Metadata-only view of mux transports. A failed child stays retired until
/// the endpoint is rebuilt; remote reachability is not inferred from this view.
#[derive(Debug, Clone)]
pub struct Health(Arc<Vec<Child>>);

#[derive(Debug, Clone, Copy)]
pub struct ChildHealth {
    pub address: SocketAddr,
    pub failed: Option<io::ErrorKind>,
    pub send_errors: u64,
    pub receive_errors: u64,
}

impl Health {
    pub(super) fn new(addresses: Vec<SocketAddr>) -> Self {
        Self(Arc::new(
            addresses
                .into_iter()
                .map(|address| Child {
                    address,
                    stopped: CancellationToken::new(),
                    error: Mutex::new(None),
                    send_errors: AtomicU64::new(0),
                    receive_errors: AtomicU64::new(0),
                })
                .collect(),
        ))
    }

    pub fn snapshot(&self) -> Vec<ChildHealth> {
        self.0
            .iter()
            .map(|child| ChildHealth {
                address: child.address,
                failed: *child.error.lock().unwrap_or_else(|p| p.into_inner()),
                send_errors: child.send_errors.load(Ordering::Relaxed),
                receive_errors: child.receive_errors.load(Ordering::Relaxed),
            })
            .collect()
    }

    pub fn all_failed(&self) -> bool {
        self.0.iter().all(|child| child.stopped.is_cancelled())
    }

    pub(crate) fn bound_available(&self, address: SocketAddr) -> bool {
        self.0
            .iter()
            .any(|child| child.address == address && !child.stopped.is_cancelled())
    }

    pub(crate) fn advertisement_available(&self, address: SocketAddr) -> bool {
        self.0.iter().any(|child| {
            !child.stopped.is_cancelled()
                && child.address.port() == address.port()
                && child.address.is_ipv4() == address.is_ipv4()
                && (child.address.ip().is_unspecified() || child.address.ip() == address.ip())
        })
    }

    pub(crate) fn live_addrs(&self) -> Vec<SocketAddr> {
        self.0
            .iter()
            .filter(|child| !child.stopped.is_cancelled())
            .map(|child| child.address)
            .collect()
    }

    pub(super) fn alive(&self, index: usize) -> bool {
        !self.0[index].stopped.is_cancelled()
    }

    pub(super) fn error(&self, index: usize, error: &io::Error, receive: bool) {
        let child = &self.0[index];
        if receive {
            &child.receive_errors
        } else {
            &child.send_errors
        }
        .fetch_add(1, Ordering::Relaxed);
        if packet_error(error)
            || matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
            )
        {
            return;
        }
        let mut failed = child.error.lock().unwrap_or_else(|p| p.into_inner());
        if failed.is_none() {
            *failed = Some(error.kind());
            tracing::warn!(child = index, kind = ?error.kind(), receive, "retiring failed mux transport");
            // Release this mutex before waking policy/engine tasks.
            drop(failed);
            child.stopped.cancel();
        }
    }

    /// Same routing rule for I/O and path retirement. An explicit source is
    /// either served by its exact bind or a same-family wildcard. It must never
    /// silently turn into a different specifically bound source address.
    pub(crate) fn route(&self, remote: SocketAddr, source: Option<IpAddr>) -> Option<usize> {
        let remote = super::super::candidates::canonical(remote);
        let source = source.map(|ip| ip.to_canonical());
        if super::relay::is_synthetic(remote) {
            return self
                .0
                .iter()
                .position(|c| super::relay::is_synthetic(c.address));
        }
        let family = |c: &Child| {
            !super::relay::is_synthetic(c.address)
                && c.address.ip().to_canonical().is_ipv4() == remote.is_ipv4()
        };
        if let Some(source) = source {
            if source.is_ipv4() != remote.is_ipv4() {
                return None;
            }
            if let Some(index) = self
                .0
                .iter()
                .position(|c| family(c) && c.address.ip().to_canonical() == source)
            {
                return Some(index);
            }
            return self
                .0
                .iter()
                .position(|c| family(c) && c.address.ip().is_unspecified());
        }
        // Keep the same source selection for source-less engine paths. Moving
        // them to a different child/port without validation would be migration.
        self.0.iter().position(family)
    }

    pub(crate) fn path_available(&self, remote: SocketAddr, source: Option<IpAddr>) -> bool {
        self.route(remote, source)
            .is_some_and(|index| self.alive(index))
    }

    pub(crate) fn changes(&self) -> Changes {
        Changes(
            self.0
                .iter()
                .map(|child| Some(Box::pin(child.stopped.clone().cancelled_owned())))
                .collect(),
        )
    }
}

/// Each waiting sender/receiver/policy owns registrations, so a failed child
/// wakes all waiters without replacing another connection's readiness waker.
#[derive(Debug)]
pub(crate) struct Changes(Vec<Option<Pin<Box<WaitForCancellationFutureOwned>>>>);
impl Changes {
    pub fn poll_changed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut changed = false;
        for wait in &mut self.0 {
            if wait
                .as_mut()
                .is_some_and(|future| future.as_mut().poll(cx).is_ready())
            {
                *wait = None;
                changed = true;
            }
        }
        if changed {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// UDP errors can describe an earlier packet to another destination. Never
/// permanently retire a socket based solely on ICMP, policy, PMTU or pressure.
pub(super) fn packet_error(error: &io::Error) -> bool {
    use io::ErrorKind::*;
    matches!(
        error.kind(),
        ConnectionRefused
            | ConnectionReset
            | ConnectionAborted
            | HostUnreachable
            | NetworkUnreachable
            | AddrNotAvailable
            | PermissionDenied
            | TimedOut
            | OutOfMemory
    ) || error.raw_os_error().is_some_and(|code| {
        [
            rustix::io::Errno::MSGSIZE.raw_os_error(),
            rustix::io::Errno::NOBUFS.raw_os_error(),
        ]
        .contains(&code)
    })
}

pub(super) fn exhausted() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "all mux transports failed")
}
