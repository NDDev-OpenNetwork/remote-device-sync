//! Unacknowledged media must backpressure capture, then resume after ACKs.
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use noq::{AsyncUdpSocket, Runtime, UdpSender};
use rds_net::{Endpoint, EndpointConfig};

#[derive(Clone, Debug)]
struct Gate(Arc<Mutex<(bool, Vec<Waker>)>>);
impl Gate {
    fn new() -> Self {
        Self(Arc::new(Mutex::new((true, Vec::new()))))
    }
    fn close(&self) {
        self.0.lock().unwrap().0 = false;
    }
    fn open(&self) {
        let wakers = {
            let mut state = self.0.lock().unwrap();
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        for waker in wakers {
            waker.wake();
        }
    }
    fn ready(&self, cx: &Context<'_>) -> bool {
        let mut state = self.0.lock().unwrap();
        if state.0 {
            return true;
        }
        if !state.1.iter().any(|w| w.will_wake(cx.waker())) {
            assert!(state.1.len() < 32, "bounded fixture has too many senders");
            state.1.push(cx.waker().clone());
        }
        false
    }
}

#[derive(Debug)]
struct Socket {
    inner: Box<dyn AsyncUdpSocket>,
    gate: Gate,
}
#[derive(Debug)]
struct Sender {
    inner: Pin<Box<dyn UdpSender>>,
    gate: Gate,
}
impl UdpSender for Sender {
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &noq::udp::Transmit<'_>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.gate.ready(cx) {
            return Poll::Pending;
        }
        this.inner.as_mut().poll_send(transmit, cx)
    }
}
impl AsyncUdpSocket for Socket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(Sender {
            inner: self.inner.create_sender(),
            gate: self.gate.clone(),
        })
    }
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [noq::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_recv(cx, bufs, meta)
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

async fn endpoint() -> (Endpoint, Gate) {
    let runtime = Arc::new(noq::TokioRuntime);
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = udp.local_addr().unwrap();
    let gate = Gate::new();
    let endpoint = rds_net::bind_noq_with_socket(
        EndpointConfig::default()
            .without_discovery()
            .with_path_pinning(),
        Box::new(Socket {
            inner: runtime.wrap_udp_socket(udp).unwrap(),
            gate: gate.clone(),
        }),
        vec![addr],
        runtime,
    )
    .await
    .unwrap();
    (endpoint, gate)
}

struct CountingSource {
    inner: rds_desktop::SyntheticProducer,
    calls: Arc<std::sync::atomic::AtomicU64>,
}
impl rds_desktop::FrameProducer for CountingSource {
    fn produce(
        &mut self,
        seq: u64,
        controls: &rds_desktop::ProducerControls,
        clock: &rds_desktop::SessionClock,
    ) -> Option<rds_desktop::Produced> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.produce(seq, controls, clock)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_acknowledgements_bound_capture_and_resume_without_closing_connection() {
    use rds_core::{Codec, DesktopCaps, DesktopHello, HelloAck, StreamHello, UniHello};
    use rds_desktop::client::{DesktopSession, SessionOpts};
    use rds_desktop::{SessionConfig, SyntheticProducer, serve_desktop_with};
    use rds_net::{read_frame, write_frame};
    use std::sync::atomic::{AtomicU64, Ordering};

    tokio::time::timeout(Duration::from_secs(10), async {
        let (server, _) = endpoint().await;
        let (client, gate) = endpoint().await;
        let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        let (a, b) = (a.unwrap(), b.unwrap());
        let calls = Arc::new(AtomicU64::new(0));
        let produced = calls.clone();
        let (start, ready) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(async move {
            let (mut send, mut recv) = b.accept_bi().await.unwrap();
            let StreamHello::DesktopV2 { session, hello } = read_frame(&mut recv).await.unwrap()
            else {
                panic!("isolated desktop expected")
            };
            write_frame(
                &mut send,
                &HelloAck::Desktop(DesktopCaps {
                    displays: vec![],
                    codecs: vec![Codec::H264],
                }),
            )
            .await
            .unwrap();
            ready.await.unwrap();
            serve_desktop_with(
                b,
                send,
                recv,
                hello,
                SessionConfig {
                    view_only: true,
                    producer: Some(Box::new(CountingSource {
                        inner: SyntheticProducer::new(240, 64, 64, 1024).keyframe_every(1),
                        calls: produced,
                    })),
                    frame_route: Some(UniHello::DesktopFrames { id: session }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        });
        let mut session = DesktopSession::connect_opts(
            &a,
            DesktopHello {
                display: 0,
                codec: Codec::H264,
                max_fps: 240,
                input_acks: false,
            },
            SessionOpts {
                session: Some([71; 16]),
                relay_encoded: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        gate.close();
        start.send(()).unwrap();
        while calls.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let held = calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            held <= 14,
            "frames buffered without acknowledgements: {held}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            held,
            "capture kept encoding into an unacknowledged transport backlog"
        );
        gate.open();
        let encoded = session.encoded.as_mut().unwrap();
        let first = encoded.recv().await.unwrap().header.seq;
        let mut last = first;
        while calls.load(Ordering::SeqCst) <= held + 5 || last <= first + 3 {
            last = encoded.recv().await.unwrap().header.seq;
        }
        assert!(last > first, "delivery must resume on the same connection");
        drop(session);
        serving.abort();
        let _ = serving.await;
        a.close(0u32.into(), b"done");
        client.close().await;
        server.close().await;
    })
    .await
    .expect("media acknowledgement recovery stalled");
}
