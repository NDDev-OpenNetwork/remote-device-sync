//! Isolated engine boundary: pending reliable bytes on a blackholed path.
use super::{Endpoint, bind_with_socket, socket, tls};
use crate::{Backend, CongestionControl, EndpointConfig, Packetization, SecretKey};
use noq::{AsyncUdpSocket, PathStatus, Runtime, UdpSender};
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct LossSocket {
    inner: Box<dyn AsyncUdpSocket>,
    enabled: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}
impl AsyncUdpSocket for LossSocket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(LossSender {
            inner: self.inner.create_sender(),
            enabled: self.enabled.clone(),
            dropped: self.dropped.clone(),
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
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}
#[derive(Debug)]
struct LossSender {
    inner: Pin<Box<dyn UdpSender>>,
    enabled: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}
impl UdpSender for LossSender {
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &noq::udp::Transmit<'_>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.enabled.load(Ordering::Acquire) {
            this.dropped.fetch_add(1, Ordering::Relaxed);
            Poll::Ready(Ok(()))
        } else {
            this.inner.as_mut().poll_send(transmit, cx)
        }
    }
}

async fn endpoints() -> (
    Endpoint,
    Endpoint,
    SocketAddr,
    Arc<AtomicBool>,
    Arc<AtomicU64>,
) {
    let runtime = Arc::new(noq::TokioRuntime);
    let bind = |addr| {
        runtime
            .wrap_udp_socket(std::net::UdpSocket::bind(addr).unwrap())
            .unwrap()
    };
    let enabled = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicU64::new(0));
    let primary = LossSocket {
        inner: bind("127.0.0.1:0"),
        enabled: enabled.clone(),
        dropped: dropped.clone(),
    };
    let secondary = bind("[::1]:0");
    let secondary_addr = secondary.local_addr().unwrap();
    let primary_addr = primary.local_addr().unwrap();
    let server = bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(SecretKey::from_bytes(&[112; 32])),
            discovery: false,
            observed_address_reports: false,
            congestion_control: CongestionControl::Cubic,
            packetization: Packetization::Conservative,
            path_preference: crate::PathPreference::Latency,
            ..Default::default()
        },
        Box::new(socket::Mux::new(vec![Box::new(primary), secondary]).unwrap()),
        vec![primary_addr],
        runtime.clone(),
        Vec::new(),
    )
    .await
    .unwrap();
    let client_mux = socket::Mux::new(vec![bind("127.0.0.1:0"), bind("[::1]:0")]).unwrap();
    let client_addrs = client_mux.local_addrs();
    let client = bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(SecretKey::from_bytes(&[113; 32])),
            discovery: false,
            observed_address_reports: false,
            congestion_control: CongestionControl::Cubic,
            packetization: Packetization::Conservative,
            path_preference: crate::PathPreference::Latency,
            ..Default::default()
        },
        Box::new(client_mux),
        client_addrs,
        runtime,
        Vec::new(),
    )
    .await
    .unwrap();
    (client, server, secondary_addr, enabled, dropped)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retiring_a_blackholed_path_preserves_already_sent_reliable_bytes() {
    exercise(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledgement_starvation_policy_recovers_pending_bytes_without_reconnecting() {
    exercise(true).await;
}

async fn exercise(automatic: bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        let (client, server, secondary_addr, enabled, dropped) = endpoints().await;
        // Raw connections deliberately bypass QNT advertisement and the policy
        // driver, so no extra route can hide the injected loss or manual switch.
        let cfg = client.client_configs.get(rds_core::ALPN).unwrap().clone();
        let name = tls::name::encode(server.id());
        let (a, b) = tokio::join!(
            client
                .inner
                .connect_with(cfg, server.local_addr(), &name)
                .unwrap(),
            async { server.inner.accept().await.unwrap().await }
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        // Subscribe before establishing the standby. Do not start discovery
        // during setup: QNT can legitimately add/promote routes before the
        // intended failure, making the injected socket no longer primary.
        let path_events = b.path_events();
        let qnt = b.nat_traversal_updates();
        let (mut request, mut reply) = a.open_bi().await.unwrap();
        request.write_all(b"warm").await.unwrap();
        let (mut send, mut recv) = b.accept_bi().await.unwrap();
        let mut warm = [0; 4];
        recv.read_exact(&mut warm).await.unwrap();
        assert_eq!(&warm, b"warm");
        send.write_all(b"ok").await.unwrap();
        let mut ack = [0; 2];
        reply.read_exact(&mut ack).await.unwrap();
        assert_eq!(&ack, b"ok");
        // Handshake completion may precede the additional CID credit needed
        // to open a path. Await that protocol precondition, not a fixed sleep.
        let secondary = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match a.open_path_ensure(secondary_addr, PathStatus::Backup).await {
                    Ok(path) => break path,
                    Err(noq::PathError::RemoteCidsExhausted) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("secondary path validation failed: {error}"),
                }
            }
        })
        .await
        .expect("peer did not provide secondary path CID credit");
        let remote_secondary = b.path(secondary.id()).unwrap();
        remote_secondary.set_status(PathStatus::Backup).unwrap();
        b.path(noq::PathId::ZERO)
            .unwrap()
            .set_status(PathStatus::Available)
            .unwrap();
        // Validation completion and positive delivery acknowledgement are
        // separate boundaries. Prove the standby can acknowledge before
        // blackholing the path used to deliver its validation/ACK traffic.
        remote_secondary.ping().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if b.congestion_state(remote_secondary.id())
                    .and_then(crate::ack_progress::snapshot)
                    .is_some_and(|state| state.confirmed(remote_secondary.stats().rtt))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("standby validation did not yield acknowledgement proof");
        let before = b.path_stats(noq::PathId::ZERO).unwrap().frame_tx.stream;
        enabled.store(true, Ordering::Release);
        send.write_all(b"pending reliable bytes").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while dropped.load(Ordering::Relaxed) == 0
                || b.path_stats(noq::PathId::ZERO).unwrap().frame_tx.stream <= before
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fixture never emitted and dropped the pending stream frame");
        let mut body = [0; 22];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), reply.read(&mut body))
                .await
                .is_err()
        );
        let (_relay_sender, relay_dead) = tokio::sync::watch::channel(0u64);
        let driver = if automatic {
            let telemetry = super::telemetry::Telemetry::new(&b);
            let mut validated = telemetry.paths();
            // The completed open_path future and the real remote path prove
            // this fixture's already-established standby, not guessed credit.
            validated.insert(remote_secondary.id(), remote_secondary.weak_handle());
            telemetry.publish(&validated, Some(noq::PathId::ZERO));
            Some(tokio::spawn(super::policy::connection_driver_observed(
                b.weak_handle(),
                qnt,
                super::policy::Observer {
                    events: path_events,
                    telemetry,
                    transport: None,
                },
                crate::metrics::Registry::default(),
                Vec::new(),
                Vec::new(),
                relay_dead,
                true,
            )))
        } else {
            None
        };
        let started = Instant::now();
        if !automatic {
            remote_secondary.set_status(PathStatus::Available).unwrap();
            b.path(noq::PathId::ZERO).unwrap().close().unwrap();
        }
        let result =
            tokio::time::timeout(Duration::from_secs(2), reply.read_exact(&mut body)).await;
        let delivered_ms = started.elapsed().as_millis();
        if result.is_err() {
            for id in [noq::PathId::ZERO, remote_secondary.id()] {
                let Some(path) = b.path(id) else {
                    eprintln!("recovery path {id} no longer open");
                    continue;
                };
                eprintln!(
                    "recovery path {} status={:?} stats={:?} progress={:?}",
                    path.id(),
                    path.status(),
                    path.stats(),
                    b.congestion_state(path.id())
                        .and_then(crate::ack_progress::snapshot)
                );
            }
        }
        // Clean up the isolated endpoints even when the engine boundary fails.
        tokio::join!(client.close(), server.close());
        result
            .expect("retired path left reliable bytes stranded")
            .unwrap();
        assert_eq!(&body, b"pending reliable bytes");
        assert!(delivered_ms < 2000);
        if let Some(driver) = driver {
            tokio::time::timeout(Duration::from_secs(1), driver)
                .await
                .expect("policy survived connection closure")
                .unwrap();
        }
    })
    .await
    .expect("isolated blackhole fixture did not terminate");
}
