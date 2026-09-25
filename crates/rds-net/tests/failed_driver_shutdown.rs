//! Explicit endpoint shutdown must also close a connection whose I/O driver failed.
#![cfg(feature = "transport-noq")]
use noq::{
    AsyncUdpSocket, Runtime, UdpSender,
    udp::{RecvMeta, Transmit},
};
use rds_net::backends::noq as owned;
use rds_net::{Backend, EndpointConfig};
use std::{
    io::{self, IoSliceMut},
    net::SocketAddr,
    num::NonZeroUsize,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

#[derive(Debug, Default)]
struct Fault {
    enabled: AtomicBool,
    hits: AtomicUsize,
}
#[derive(Debug)]
struct Socket {
    inner: Box<dyn AsyncUdpSocket>,
    fault: Arc<Fault>,
}
#[derive(Debug)]
struct Sender {
    inner: Pin<Box<dyn UdpSender>>,
    fault: Arc<Fault>,
}
impl AsyncUdpSocket for Socket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(Sender {
            inner: self.inner.create_sender(),
            fault: self.fault.clone(),
        })
    }
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_recv(cx, bufs, meta)
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn max_receive_segments(&self) -> NonZeroUsize {
        self.inner.max_receive_segments()
    }
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}
impl UdpSender for Sender {
    fn poll_send(
        mut self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        if self.fault.enabled.load(Ordering::Acquire) {
            self.fault.hits.fetch_add(1, Ordering::AcqRel);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected terminal socket failure",
            )));
        }
        self.inner.as_mut().poll_send(transmit, cx)
    }
    fn max_transmit_segments(&self) -> NonZeroUsize {
        self.inner.max_transmit_segments()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_close_terminates_policy_after_protocol_driver_io_failure() {
    tokio::time::timeout(Duration::from_secs(15), exercise())
        .await
        .expect("failed-driver fixture exceeded deadline");
}
async fn exercise() {
    let runtime = Arc::new(noq::TokioRuntime);
    let inner = runtime
        .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
        .unwrap();
    let local = inner.local_addr().unwrap();
    let fault = Arc::new(Fault::default());
    let a = owned::bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            ..Default::default()
        },
        Box::new(Socket {
            inner,
            fault: fault.clone(),
        }),
        vec![local],
        runtime,
        None,
    )
    .await
    .unwrap();
    let b = owned::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let (ca, cb) = tokio::time::timeout(Duration::from_secs(3), async {
        let (a, b) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        (a.unwrap(), b.unwrap())
    })
    .await
    .unwrap();
    ca.send_datagram(b"healthy before injected failure".to_vec().into())
        .unwrap();
    assert_eq!(
        &tokio::time::timeout(Duration::from_secs(2), cb.read_datagram())
            .await
            .unwrap()
            .unwrap()[..],
        b"healthy before injected failure"
    );
    fault.enabled.store(true, Ordering::Release);
    // Retain both Connection and Path I/O handles. Dropping a failed driver
    // with only one user handle can trigger implicit close and mask this case.
    let held_path = ca.inner().path(noq::PathId::ZERO).unwrap();
    held_path.ping().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while fault.hits.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fixture never returned the I/O error to noq");
    let close = tokio::time::timeout(Duration::from_secs(1), a.close()).await;
    let was_closed = ca.inner().close_reason().is_some();
    drop(held_path);
    // Clean up even the failing baseline without relying again on an event
    // queued to the now-dead protocol driver.
    ca.close(0u32.into(), b"fixture cleanup");
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(a.close(), b.close());
    })
    .await
    .expect("fixture cleanup stalled");
    assert!(
        close.is_ok(),
        "endpoint close waited forever for a stopped connection driver"
    );
    assert!(
        was_closed,
        "endpoint returned without closing the retained connection"
    );
    assert_eq!(a.active_path_drivers(), 0);
}
