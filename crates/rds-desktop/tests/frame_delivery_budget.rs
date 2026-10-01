//! Unacknowledged media must backpressure capture, then resume after ACKs.
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use noq::{AsyncUdpSocket, Runtime, UdpSender};
use rds_net::{Endpoint, EndpointConfig};

// Isolate timing scenarios so one fixture's deliberately paused transport
// cannot introduce unrelated scheduler/path pressure in the other. Each
// scenario retains its concurrent producer, receiver and ACK workers.
static DELIVERY_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    bitrate: Arc<std::sync::atomic::AtomicU64>,
    misses: Arc<std::sync::atomic::AtomicU64>,
}
impl rds_desktop::FrameProducer for CountingSource {
    fn resume_after_backpressure(&mut self) {
        self.inner.resume_after_backpressure();
    }
    fn produce(
        &mut self,
        seq: u64,
        controls: &rds_desktop::ProducerControls,
        clock: &rds_desktop::SessionClock,
    ) -> Option<rds_desktop::Produced> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.bitrate.store(
            controls.bitrate.load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::SeqCst,
        );
        let before = controls
            .deadline_misses
            .load(std::sync::atomic::Ordering::Relaxed);
        let frame = self.inner.produce(seq, controls, clock);
        let after = controls
            .deadline_misses
            .load(std::sync::atomic::Ordering::Relaxed);
        self.misses.fetch_add(
            after.saturating_sub(before),
            std::sync::atomic::Ordering::Relaxed,
        );
        frame
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_acknowledgements_bound_capture_and_resume_without_closing_connection() {
    let _isolation = DELIVERY_TEST.lock().await;
    delivery_pause(Duration::from_millis(800), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovered_delay_does_not_reduce_quality_while_media_receipts_continue() {
    let _isolation = DELIVERY_TEST.lock().await;
    delivery_pause(Duration::from_millis(200), false).await;
}

async fn delivery_pause(extra_hold: Duration, sustained: bool) {
    use rds_core::{Codec, DesktopCaps, DesktopHello, HelloAck, StreamHello, UniHello};
    use rds_desktop::client::{DesktopSession, SessionOpts};
    use rds_desktop::{SessionConfig, SyntheticProducer, serve_desktop_with};
    use rds_net::{read_frame, write_frame};
    use std::sync::atomic::{AtomicU64, Ordering};

    tokio::time::timeout(Duration::from_secs(10), async {
        // Pause before server datagrams are emitted. This creates delayed
        // media without deliberately losing packets or receiver ACKs.
        let (server, gate) = endpoint().await;
        let (client, _) = endpoint().await;
        let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        let (a, b) = (a.unwrap(), b.unwrap());
        let observer = b.clone();
        let calls = Arc::new(AtomicU64::new(0));
        let bitrate = Arc::new(AtomicU64::new(0));
        let observed_bitrate = bitrate.clone();
        let misses = Arc::new(AtomicU64::new(0));
        let observed_misses = misses.clone();
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
                        // Measure delivery pressure without also imposing a
                        // 4 ms synthetic CPU cadence under concurrent tests.
                        // Encoder starvation remains an independent signal
                        // covered by the controller/cadence regressions.
                        inner: SyntheticProducer::new(30, 64, 64, 1024).keyframe_every(1),
                        calls: produced,
                        bitrate: observed_bitrate,
                        misses: observed_misses,
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
                max_fps: 30,
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
        while calls.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let held = calls.load(Ordering::SeqCst);
        let initial_bitrate = bitrate.load(Ordering::SeqCst);
        tokio::time::sleep(extra_hold).await;
        assert_eq!(
            held, 1,
            "a recovery keyframe must finish before encoding successors"
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
        if sustained {
            assert!(
                bitrate.load(Ordering::SeqCst) < initial_bitrate,
                "sustained delivery blockage must reduce load even without reported packet loss"
            );
        } else {
            // Observe more than one pacing tick after delivery recovered.
            // The old cumulative delay event was consumed here and cut the
            // rate despite healthy receipts already arriving again.
            tokio::time::sleep(Duration::from_millis(550)).await;
            assert!(
                bitrate.load(Ordering::SeqCst) >= initial_bitrate,
                "a recovered delay unnecessarily reduced image quality: initial={initial_bitrate}, current={}, observed_misses={}, path={:?}",
                bitrate.load(Ordering::SeqCst), misses.load(Ordering::Relaxed), observer.current_path_stats()
            );
        }
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
