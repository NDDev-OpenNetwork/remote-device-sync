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
