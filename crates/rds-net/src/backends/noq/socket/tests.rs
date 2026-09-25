use super::*;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Wake, Waker},
};

#[derive(Debug)]
enum Step {
    Ready,
    Pending,
    Error(io::ErrorKind),
    OsError(i32),
}
#[derive(Debug, Default)]
struct Script {
    send: Mutex<VecDeque<Step>>,
    receive: Mutex<VecDeque<Step>>,
    sent: Mutex<Vec<Vec<u8>>>,
    send_calls: AtomicUsize,
    receive_calls: AtomicUsize,
}
#[derive(Debug)]
struct Socket {
    address: SocketAddr,
    script: Arc<Script>,
}
#[derive(Debug)]
struct Sender(Arc<Script>);
impl AsyncUdpSocket for Socket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(Sender(self.script.clone()))
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.address)
    }
    fn poll_recv(
        &mut self,
        _: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.script.receive_calls.fetch_add(1, Ordering::Relaxed);
        match self
            .script
            .receive
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Pending)
        {
            Step::Ready => {
                bufs[0][0] = 42;
                let mut received = RecvMeta::default();
                received.addr = self.address;
                received.len = 1;
                received.stride = 1;
                received.dst_ip = Some(self.address.ip());
                meta[0] = received;
                Poll::Ready(Ok(1))
            }
            Step::Pending => Poll::Pending,
            Step::Error(kind) => Poll::Ready(Err(io::Error::from(kind))),
            Step::OsError(code) => Poll::Ready(Err(io::Error::from_raw_os_error(code))),
        }
    }
}
impl UdpSender for Sender {
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        _: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.0.send_calls.fetch_add(1, Ordering::Relaxed);
        match self
            .0
            .send
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Ready)
        {
            Step::Ready => {
                self.0.sent.lock().unwrap().push(transmit.contents.to_vec());
                Poll::Ready(Ok(()))
            }
            Step::Pending => Poll::Pending,
            Step::Error(kind) => Poll::Ready(Err(io::Error::from(kind))),
            Step::OsError(code) => Poll::Ready(Err(io::Error::from_raw_os_error(code))),
        }
    }
}
fn socket(address: &str) -> (Box<dyn AsyncUdpSocket>, Arc<Script>) {
    let script = Arc::new(Script::default());
    (
        Box::new(Socket {
            address: address.parse().unwrap(),
            script: script.clone(),
        }),
        script,
    )
}
fn packet(destination: &str, src_ip: Option<IpAddr>) -> Transmit<'static> {
    Transmit {
        destination: destination.parse().unwrap(),
        ecn: None,
        contents: b"kept packet",
        segment_size: None,
        src_ip,
    }
}
#[derive(Default)]
struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn failure_wakes_every_pending_sender_and_receiver_and_last_failure_is_explicit() {
    let (v4, a) = socket("127.0.0.1:3000");
    let (v6, b) = socket("[::1]:3000");
    let mut mux = Mux::new(vec![v4, v6]).unwrap();
    let logical = mux.local_addr().unwrap();
    let health = mux.health();
    let mut first = mux.create_sender();
    let mut second = mux.create_sender();
    let mut failing = mux.create_sender();
    let wakers: Vec<_> = (0..3).map(|_| Arc::new(WakeCount::default())).collect();
    let wakes: Vec<Waker> = wakers.iter().cloned().map(Waker::from).collect();
    let transmit = packet("192.0.2.1:4433", None);
    *a.send.lock().unwrap() = [
        Step::Pending,
        Step::Pending,
        Step::Error(io::ErrorKind::BrokenPipe),
    ]
    .into();
    assert!(
        first
            .as_mut()
            .poll_send(&transmit, &mut Context::from_waker(&wakes[0]))
            .is_pending()
    );
    assert!(
        second
            .as_mut()
            .poll_send(&transmit, &mut Context::from_waker(&wakes[1]))
            .is_pending()
    );
    let mut bytes = [0; 8];
    let mut meta = [RecvMeta::default()];
    assert!(
        mux.poll_recv(
            &mut Context::from_waker(&wakes[2]),
            &mut [IoSliceMut::new(&mut bytes)],
            &mut meta
        )
        .is_pending()
    );
    assert!(matches!(
        failing
            .as_mut()
            .poll_send(&transmit, &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
    for wake in &wakers {
        assert!(
            wake.0.load(Ordering::Relaxed) > 0,
            "health transition missed a waiter"
        );
    }
    let calls = a.send_calls.load(Ordering::Relaxed);
    assert!(matches!(
        first
            .as_mut()
            .poll_send(&transmit, &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(
        calls,
        a.send_calls.load(Ordering::Relaxed),
        "retired socket was polled again"
    );
    assert!(!health.all_failed());
    assert_eq!(mux.local_addr().unwrap(), logical);
    b.send
        .lock()
        .unwrap()
        .push_back(Step::Error(io::ErrorKind::BrokenPipe));
    assert!(matches!(
        second.as_mut().poll_send(
            &packet("[2001:db8::1]:4433", None),
            &mut Context::from_waker(Waker::noop())
        ),
        Poll::Ready(Err(_))
    ));
    assert!(health.all_failed());
    assert!(matches!(
        mux.poll_recv(
            &mut Context::from_waker(Waker::noop()),
            &mut [IoSliceMut::new(&mut bytes)],
            &mut meta
        ),
        Poll::Ready(Err(_))
    ));
    drop(first);
    drop(second);
    drop(failing);
    drop(mux);
    assert_eq!(Arc::strong_count(&a), 1, "health handle retained transport");
    assert_eq!(Arc::strong_count(&b), 1);
    assert_eq!(
        health
            .snapshot()
            .iter()
            .filter(|child| child.failed.is_some())
            .count(),
        2
    );
}

#[tokio::test]
async fn receive_errors_are_bounded_and_do_not_starve_siblings() {
    let (v4, a) = socket("127.0.0.1:3000");
    let (v6, b) = socket("[::1]:3000");
    let mut mux = Mux::new(vec![v4, v6]).unwrap();
    a.receive
        .lock()
        .unwrap()
        .extend([Step::Error(io::ErrorKind::ConnectionReset), Step::Ready]);
    b.receive.lock().unwrap().push_back(Step::Ready);
    let mut bytes = [0; 8];
    let mut meta = [RecvMeta::default()];
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        mux.poll_recv(&mut cx, &mut [IoSliceMut::new(&mut bytes)], &mut meta),
        Poll::Ready(Ok(1))
    ));
    assert!(meta[0].addr.is_ipv6());
    for _ in 0..20 {
        let _ = mux.poll_recv(&mut cx, &mut [IoSliceMut::new(&mut bytes)], &mut meta);
    }
    assert_eq!(
        a.receive_calls.load(Ordering::Relaxed),
        1,
        "receive error spun instead of backing off"
    );
    assert!(
        mux.health()
            .snapshot()
            .iter()
            .all(|child| child.failed.is_none())
    );
    tokio::time::timeout(
        Duration::from_millis(100),
        std::future::poll_fn(|cx| mux.poll_recv(cx, &mut [IoSliceMut::new(&mut bytes)], &mut meta)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(meta[0].addr.ip().to_canonical().is_ipv4());
    a.receive
        .lock()
        .unwrap()
        .push_back(Step::Error(io::ErrorKind::BrokenPipe));
    b.receive.lock().unwrap().push_back(Step::Ready);
    // Cursor may visit the healthy sibling first. A second poll must still
    // inspect the failed child rather than permanently starving it.
    for _ in 0..2 {
        let _ = mux.poll_recv(&mut cx, &mut [IoSliceMut::new(&mut bytes)], &mut meta);
    }
    assert_eq!(
        mux.health().snapshot()[0].failed,
        Some(io::ErrorKind::BrokenPipe)
    );
    assert!(mux.health().snapshot()[1].failed.is_none());
}

#[tokio::test]
async fn interrupted_and_would_block_retry_the_same_packet_without_retirement() {
    for kind in [io::ErrorKind::Interrupted, io::ErrorKind::WouldBlock] {
        let (socket, script) = socket("127.0.0.1:3000");
        let mux = Mux::new(vec![socket]).unwrap();
        let mut sender = mux.create_sender();
        script.send.lock().unwrap().push_back(Step::Error(kind));
        let transmit = packet("192.0.2.1:4433", None);
        assert!(
            sender
                .as_mut()
                .poll_send(&transmit, &mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        tokio::time::timeout(
            Duration::from_millis(100),
            std::future::poll_fn(|cx| sender.as_mut().poll_send(&transmit, cx)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(*script.sent.lock().unwrap(), vec![b"kept packet".to_vec()]);
        assert!(mux.health().snapshot()[0].failed.is_none());
    }
}

#[test]
fn packet_errors_preserve_the_only_socket_for_other_destinations() {
    let (socket, script) = socket("127.0.0.1:3000");
    let mux = Mux::new(vec![socket]).unwrap();
    let mut sender = mux.create_sender();
    let errors = [
        Step::Error(io::ErrorKind::ConnectionRefused),
        Step::Error(io::ErrorKind::NetworkUnreachable),
        Step::Error(io::ErrorKind::PermissionDenied),
        Step::Error(io::ErrorKind::AddrNotAvailable),
        Step::Error(io::ErrorKind::OutOfMemory),
        Step::OsError(rustix::io::Errno::MSGSIZE.raw_os_error()),
        Step::OsError(rustix::io::Errno::NOBUFS.raw_os_error()),
    ];
    let count = errors.len();
    script.send.lock().unwrap().extend(errors);
    for _ in 0..count {
        assert!(matches!(
            sender.as_mut().poll_send(
                &packet("192.0.2.1:4433", None),
                &mut Context::from_waker(Waker::noop())
            ),
            Poll::Ready(Ok(()))
        ));
    }
    assert!(matches!(
        sender.as_mut().poll_send(
            &packet("192.0.2.2:4433", None),
            &mut Context::from_waker(Waker::noop())
        ),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(script.sent.lock().unwrap().len(), 1);
    let child = mux.health().snapshot()[0];
    assert!(child.failed.is_none());
    assert_eq!(child.send_errors, count as u64);
}

#[test]
fn explicit_source_never_falls_through_to_an_unrelated_bind() {
    let (one, a) = socket("192.0.2.1:3000");
    let (two, b) = socket("192.0.2.2:3000");
    let mux = Mux::new(vec![one, two]).unwrap();
    let mut sender = mux.create_sender();
    let mut cx = Context::from_waker(Waker::noop());
    for source in ["192.0.2.3", "::1"] {
        assert!(matches!(
            sender.as_mut().poll_send(
                &packet("192.0.2.100:4000", Some(source.parse().unwrap())),
                &mut cx
            ),
            Poll::Ready(Ok(()))
        ));
    }
    assert_eq!(
        a.send_calls.load(Ordering::Relaxed) + b.send_calls.load(Ordering::Relaxed),
        0
    );
    assert!(matches!(
        sender.as_mut().poll_send(
            &packet(
                "[::ffff:192.0.2.100]:4000",
                Some("::ffff:192.0.2.2".parse().unwrap())
            ),
            &mut cx
        ),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(a.send_calls.load(Ordering::Relaxed), 0);
    assert_eq!(b.send_calls.load(Ordering::Relaxed), 1);
    let health = Health::new(vec!["0.0.0.0:3000".parse().unwrap()]);
    assert!(health.path_available(
        "192.0.2.100:4000".parse().unwrap(),
        Some("192.0.2.3".parse().unwrap())
    ));
    assert!(health.advertisement_available("192.0.2.3:3000".parse().unwrap()));
    assert!(!health.advertisement_available("192.0.2.3:3001".parse().unwrap()));
}
