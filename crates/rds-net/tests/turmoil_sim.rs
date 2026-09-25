//! Deterministic simulation of the owned transport under turmoil:
//! two noq endpoints on simulated hosts, a seeded partition, then
//! repair — proving the transport survives link loss and reconnects.
//! Runs single-threaded on virtual time: what would be a flaky soak
//! test becomes a fast, repeatable one.
#![cfg(feature = "transport-noq")]

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use iroh::{EndpointAddr, EndpointId, SecretKey, TransportAddr};
use noq::udp::RecvMeta;
use noq::{AsyncUdpSocket, UdpSender};
use rds_net::EndpointConfig;
use rds_net::backends::noq as rds_noq;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const PING: &[u8] = b"rds-ping";

/// An `AsyncUdpSocket` over a turmoil host socket.
///
/// Turmoil exposes async `recv_from`/`send_to` rather than poll
/// readiness, so pump tasks move datagrams between the sim socket and
/// unbounded channels. The sim is single-threaded and the channels
/// only cross the pump boundary — ordering and delivery stay exact.
struct SimSocket {
    rx: mpsc::UnboundedReceiver<(Vec<u8>, SocketAddr)>,
    tx: mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>,
    local: SocketAddr,
    _pumps: (JoinHandle<()>, JoinHandle<()>),
}

impl SimSocket {
    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let sock = Arc::new(turmoil::net::UdpSocket::bind(addr).await?);
        let local = sock.local_addr()?;
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<(Vec<u8>, SocketAddr)>();

        let recv_pump = tokio::spawn({
            let sock = sock.clone();
            async move {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match sock.recv_from(&mut buf).await {
                        Ok((n, src)) if in_tx.send((buf[..n].to_vec(), src)).is_ok() => {}
                        _ => break,
                    }
                }
            }
        });
        let send_pump = tokio::spawn(async move {
            while let Some((data, dst)) = out_rx.recv().await {
                if sock.send_to(&data, dst).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            rx: in_rx,
            tx: out_tx,
            local,
            _pumps: (recv_pump, send_pump),
        })
    }
}

impl fmt::Debug for SimSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimSocket")
            .field("local", &self.local)
            .finish()
    }
}

impl AsyncUdpSocket for SimSocket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(SimSender {
            tx: self.tx.clone(),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
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
            Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "sim recv pump stopped",
            ))),
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

#[derive(Debug)]
struct SimSender {
    tx: mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>,
}

impl UdpSender for SimSender {
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &noq::udp::Transmit<'_>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match self
            .tx
            .send((transmit.contents.to_vec(), transmit.destination))
        {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("sim send pump stopped: {e}"),
            ))),
        }
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        NonZeroUsize::MIN
    }
}

/// Bind a noq endpoint on a sim socket bound to `bind`.
///
/// `local_addrs` advertises the host's real sim address (`lookup(host)`)
/// rather than the `0.0.0.0` the socket is bound to — kernel egress
/// hints are meaningless inside a simulated host.
async fn sim_endpoint(
    bind: SocketAddr,
    host: &str,
    key: SecretKey,
) -> anyhow::Result<rds_noq::Endpoint> {
    let socket = SimSocket::bind(bind).await?;
    let local = socket.local_addr()?;
    let sim_ip = turmoil::lookup(host);
    rds_noq::bind_with_socket(
        EndpointConfig {
            secret_key: Some(key),
            ..Default::default()
        },
        Box::new(socket),
        vec![SocketAddr::new(sim_ip, local.port())],
        Arc::new(noq::TokioRuntime),
        None,
    )
    .await
}

fn target_of(id: EndpointId, sock: SocketAddr) -> EndpointAddr {
    let mut addrs = BTreeSet::new();
    addrs.insert(TransportAddr::Ip(sock));
    EndpointAddr { id, addrs }
}

async fn ping(conn: &rds_noq::Connection) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(PING).await?;
    send.finish()?;
    let mut buf = vec![0u8; PING.len()];
    recv.read_exact(&mut buf).await?;
    assert_eq!(&buf, PING);
    Ok(())
}

/// Partition, then repair: a handshake mid-partition must fail (the
/// wire drops every packet — no silent success), and a fresh connect
/// must succeed after repair.
#[test]
fn noq_partition_then_repair_reconnects() -> turmoil::Result {
    // Fixed key so the client knows the server's EndpointId without a
    // discovery round trip inside the sim.
    let server_key = SecretKey::from_bytes(&[7u8; 32]);
    let server_id = server_key.public();
    let client_key = SecretKey::from_bytes(&[9u8; 32]);

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(120))
        .build();

    sim.host("server", move || {
        let server_key = server_key.clone();
        async move {
            let ep =
                sim_endpoint(SocketAddr::from(([0, 0, 0, 0], 4433)), "server", server_key).await?;
            while let Some(incoming) = ep.accept().await {
                tokio::spawn(async move {
                    if let Ok(conn) = incoming.await {
                        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                            if let Ok(buf) = recv.read_to_end(1024).await {
                                let _ = send.write_all(&buf).await;
                                let _ = send.finish();
                            }
                        }
                    }
                });
            }
            Ok(())
        }
    });

    sim.client("client", async move {
        let ep = sim_endpoint(SocketAddr::from(([0, 0, 0, 0], 0)), "client", client_key).await?;
        let server_ip = turmoil::lookup("server");
        let target = target_of(server_id, SocketAddr::new(server_ip, 4433));

        // Baseline: connect + ping on the healthy link.
        let conn = timeout(
            Duration::from_secs(10),
            ep.connect(target.clone(), rds_core::ALPN),
        )
        .await??;
        ping(&conn).await?;
        conn.close(0u32.into(), b"phase one done");

        // Cut the link: a fresh handshake must fail — the wire drops
        // every packet, so success would be a lie.
        turmoil::partition("client", "server");
        let during = timeout(
            Duration::from_secs(10),
            ep.connect(target.clone(), rds_core::ALPN),
        )
        .await;
        assert!(
            during.is_err() || during.unwrap().is_err(),
            "handshake succeeded during partition"
        );

        // Repair: a fresh connect must complete and carry traffic.
        turmoil::repair("client", "server");
        let conn2 = timeout(Duration::from_secs(10), ep.connect(target, rds_core::ALPN)).await??;
        ping(&conn2).await?;

        sleep(Duration::from_millis(50)).await;
        ep.close().await;
        Ok(())
    });

    sim.run()
}

/// A silent learned path has a lower default RTT than this real 400 ms path.
/// Application policy must not prefer that unvalidated default RTT.
#[test]
fn slow_primary_remains_preferred_over_an_unvalidated_candidate() -> turmoil::Result {
    let server_key = SecretKey::from_bytes(&[108; 32]);
    let server_id = server_key.public();
    let mut sim = turmoil::Builder::new()
        .min_message_latency(Duration::from_millis(200))
        .max_message_latency(Duration::from_millis(200))
        .simulation_duration(Duration::from_secs(30))
        .build();
    sim.host("silent", || async {
        let _socket = turmoil::net::UdpSocket::bind("0.0.0.0:4434").await?;
        std::future::pending::<()>().await;
        Ok(())
    });
    sim.host("server", move || {
        let key = server_key.clone();
        async move {
            let ep = sim_endpoint("0.0.0.0:4433".parse().unwrap(), "server", key).await?;
            let conn = ep.accept().await.unwrap().await?;
            while let Ok(payload) = conn.read_datagram().await {
                if &payload[..] == b"advertise" {
                    conn.inner().add_nat_traversal_address(SocketAddr::new(
                        turmoil::lookup("silent"),
                        4434,
                    ))?;
                }
                conn.send_datagram(payload)?;
            }
            ep.close().await;
            Ok(())
        }
    });
    sim.client("client", async move {
        let ep = sim_endpoint(
            "0.0.0.0:0".parse().unwrap(),
            "client",
            SecretKey::from_bytes(&[109; 32]),
        )
        .await?;
        let target = target_of(server_id, SocketAddr::new(turmoil::lookup("server"), 4433));
        let conn = timeout(Duration::from_secs(10), ep.connect(target, rds_core::ALPN)).await??;
        conn.send_datagram(b"baseline".to_vec().into())?;
        assert_eq!(
            &timeout(Duration::from_secs(3), conn.read_datagram()).await??[..],
            b"baseline"
        );
        assert!(conn.inner().rtt(noq::PathId::ZERO).unwrap() > Duration::from_millis(350));
        conn.send_datagram(b"advertise".to_vec().into())?;
        assert_eq!(
            &timeout(Duration::from_secs(3), conn.read_datagram()).await??[..],
            b"advertise"
        );
        // There is exactly one extra candidate in this world; its first path
        // ID follows ZERO. Assert the actual address as well as its status.
        let pending = timeout(Duration::from_secs(3), async {
            loop {
                if let Some(path) = conn.inner().path(noq::PathId::from(1u32)) {
                    break path;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        assert_eq!(
            pending.remote_address()?,
            SocketAddr::new(turmoil::lookup("silent"), 4434)
        );
        sleep(Duration::from_millis(100)).await;
        assert_eq!(pending.status()?, noq::PathStatus::Backup);
        assert_eq!(
            conn.inner().path(noq::PathId::ZERO).unwrap().status()?,
            noq::PathStatus::Available
        );
        conn.send_datagram(b"still live".to_vec().into())?;
        assert_eq!(
            &timeout(Duration::from_secs(3), conn.read_datagram()).await??[..],
            b"still live"
        );
        ep.close().await;
        Ok(())
    });
    sim.run()
}

/// TLS completes before post-handshake spare CIDs cross the delayed link.
/// The second ticket address is not advertised over QNT, so only the owned
/// candidate queue can retry it after those credits arrive.
#[test]
fn initial_candidate_retries_after_delayed_path_credit() -> turmoil::Result {
    delayed_path_credit(false)
}

#[test]
fn pending_path_credit_retry_does_not_retain_connection() -> turmoil::Result {
    delayed_path_credit(true)
}

fn delayed_path_credit(drop_pending: bool) -> turmoil::Result {
    use tokio_stream::StreamExt;
    let key = SecretKey::from_bytes(&[110; 32]);
    let id = key.public();
    let mut sim = turmoil::Builder::new()
        .min_message_latency(Duration::from_millis(200))
        .max_message_latency(Duration::from_millis(200))
        .simulation_duration(Duration::from_secs(30))
        .build();
    sim.host("server", move || {
        let key = key.clone();
        async move {
            let one = SimSocket::bind("0.0.0.0:4433".parse().unwrap()).await?;
            let two = SimSocket::bind("0.0.0.0:4434".parse().unwrap()).await?;
            let mux = rds_noq::socket::Mux::new(vec![Box::new(one), Box::new(two)])?;
            let ep = rds_noq::bind_with_socket(
                EndpointConfig {
                    secret_key: Some(key),
                    ..Default::default()
                },
                Box::new(mux),
                vec![SocketAddr::new(turmoil::lookup("server"), 4433)],
                Arc::new(noq::TokioRuntime),
                None,
            )
            .await?;
            let mut tasks = tokio::task::JoinSet::new();
            while let Some(incoming) = ep.accept().await {
                tasks.spawn(async move {
                    if let Ok(conn) = incoming.await {
                        while let Ok(payload) = conn.read_datagram().await {
                            if conn.send_datagram(payload).is_err() {
                                break;
                            }
                        }
                    }
                });
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            Ok(())
        }
    });
    sim.client("client", async move {
        let ep = sim_endpoint(
            "0.0.0.0:0".parse().unwrap(),
            "client",
            SecretKey::from_bytes(&[111; 32]),
        )
        .await?;
        let server_ip = turmoil::lookup("server");
        let mut target = target_of(id, SocketAddr::new(server_ip, 4433));
        target
            .addrs
            .insert(TransportAddr::Ip(SocketAddr::new(server_ip, 4434)));
        let conn = timeout(Duration::from_secs(10), ep.connect(target, rds_core::ALPN)).await??;
        // Check the fixture really reaches temporary credit exhaustion,
        // rather than merely observing that no secondary path exists yet.
        assert_eq!(
            conn.remote_address(),
            Some(SocketAddr::new(server_ip, 4433))
        );
        let rejected = conn
            .inner()
            .open_path_ensure(SocketAddr::new(server_ip, 4434), noq::PathStatus::Backup);
        assert!(
            rejected.path_id().is_none(),
            "fixture already had spare path credit"
        );
        let error = rejected.await.unwrap_err();
        assert!(
            matches!(
                error,
                noq::PathError::RemoteCidsExhausted | noq::PathError::MaxPathIdReached
            ),
            "unexpected path refusal: {error:?}"
        );
        assert!(conn.inner().path(noq::PathId::from(1u32)).is_none());
        if drop_pending {
            // Let the policy attempt once and enter backoff, before spare
            // connection IDs can traverse this 200 ms one-way link.
            sleep(Duration::from_millis(30)).await;
            assert!(conn.inner().path(noq::PathId::from(1u32)).is_none());
            assert_eq!(ep.active_path_drivers(), 1);
            drop(conn);
            timeout(Duration::from_secs(1), async {
                while ep.active_path_drivers() != 0 {
                    sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("pending retry retained the last connection handle");
            ep.close().await;
            return Ok(());
        }
        let mut events = conn.inner().path_events();
        let validated = timeout(Duration::from_secs(4), async {
            loop {
                match events.next().await {
                    Some(Ok(noq::PathEvent::Established { id, .. })) if id != noq::PathId::ZERO => {
                        break id;
                    }
                    Some(Ok(_)) => {}
                    other => panic!("unexpected path event while waiting for retry: {other:?}"),
                }
            }
        })
        .await;
        if validated.is_err() {
            // Prove credits and the listener are now usable. The defect is
            // losing the automatic attempt, not an unavailable remote path.
            let manual = timeout(
                Duration::from_secs(3),
                conn.inner()
                    .open_path_ensure(SocketAddr::new(server_ip, 4434), noq::PathStatus::Backup),
            )
            .await
            .expect("manual retry validation timed out")
            .expect("manual retry still refused after credits");
            assert_eq!(manual.remote_address()?, SocketAddr::new(server_ip, 4434));
            panic!("automatic retry was lost; the same path now validates when opened manually");
        }
        let validated = validated.unwrap();
        let replacement = conn.inner().path(validated).unwrap();
        assert_eq!(
            replacement.remote_address()?,
            SocketAddr::new(server_ip, 4434)
        );
        conn.inner().path(noq::PathId::ZERO).unwrap().close()?;
        conn.send_datagram(b"after delayed credit".to_vec().into())?;
        assert_eq!(
            &timeout(Duration::from_secs(3), conn.read_datagram()).await??[..],
            b"after delayed credit"
        );
        ep.close().await;
        Ok(())
    });
    sim.run()
}
