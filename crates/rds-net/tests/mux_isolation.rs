//! One failed child transport must not poison a validated sibling path.
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
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

#[derive(Debug, Default)]
struct Fault {
    enabled: AtomicBool,
    hits: AtomicUsize,
    recv: AtomicBool,
    recv_waker: Mutex<Option<Waker>>,
}
impl Fault {
    fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
        // Changing a pending socket's outcome must wake its reader. A QUIC
        // ping may use a different validated path and cannot do this for us.
        if self.recv.load(Ordering::Acquire) {
            let waker = self.recv_waker.lock().unwrap().take();
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }
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
        // Register before checking the fault, so enable cannot race with
        // registration and leave an already-pending read asleep.
        if self.fault.recv.load(Ordering::Acquire) {
            *self.fault.recv_waker.lock().unwrap() = Some(cx.waker().clone());
            if self.fault.enabled.load(Ordering::Acquire) {
                self.fault.hits.fetch_add(1, Ordering::AcqRel);
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected receive failure",
                )));
            }
        }
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
        if self.fault.enabled.load(Ordering::Acquire) && !self.fault.recv.load(Ordering::Acquire) {
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
async fn terminal_child_send_does_not_stop_healthy_path() {
    exercise(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_child_receive_does_not_stop_healthy_path() {
    exercise(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_last_path_is_not_selected_while_another_socket_lives() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let runtime = Arc::new(noq::TokioRuntime);
        let fault = Arc::new(Fault::default());
        let primary = runtime
            .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
            .unwrap();
        let address = primary.local_addr().unwrap();
        let secondary = runtime
            .wrap_udp_socket(std::net::UdpSocket::bind("[::1]:0").unwrap())
            .unwrap();
        let mux = owned::socket::Mux::new(vec![
            Box::new(Socket {
                inner: primary,
                fault: fault.clone(),
            }),
            secondary,
        ])
        .unwrap();
        let health = mux.health();
        let config = || EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        // This peer only has IPv4. The healthy server IPv6 socket cannot
        // supply a validated replacement for the connection's last path.
        let server = owned::bind_with_mux(config(), mux, vec![address], runtime, None)
            .await
            .unwrap();
        let client = owned::bind_endpoint(config()).await.unwrap();
        let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        let (a, b) = (a.unwrap(), b.unwrap());
        let facade = rds_net::Connection::from(b.clone());
        fault.enable();
        b.inner().path(noq::PathId::ZERO).unwrap().ping().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while fault.hits.load(Ordering::Acquire) == 0 || facade.current_path_stats().is_some() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("failed last path was still selected");
        assert!(!health.all_failed());
        assert_eq!(health.snapshot()[0].failed, Some(io::ErrorKind::BrokenPipe));
        assert!(health.snapshot()[1].failed.is_none());
        for path in facade.path_stats() {
            assert!(!path.selected);
        }
        a.close(0u32.into(), b"cleanup");
        b.close(0u32.into(), b"cleanup");
        tokio::join!(client.close(), server.close());
    })
    .await
    .expect("last-path fixture stalled");
}

async fn exercise(receive: bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        let runtime = Arc::new(noq::TokioRuntime);
        let fault = Arc::new(Fault::default());
        fault.recv.store(receive, Ordering::Release);
        let primary = runtime
            .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
            .unwrap();
        let secondary = runtime
            .wrap_udp_socket(std::net::UdpSocket::bind("[::1]:0").unwrap())
            .unwrap();
        let primary_addr = primary.local_addr().unwrap();
        let secondary_addr = secondary.local_addr().unwrap();
        let secondary_fault = Arc::new(Fault::default());
        let mux = owned::socket::Mux::new(vec![
            Box::new(Socket {
                inner: primary,
                fault: fault.clone(),
            }),
            Box::new(Socket {
                inner: secondary,
                fault: secondary_fault.clone(),
            }),
        ])
        .unwrap();
        let config = || EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap(), "[::1]:0".parse().unwrap()],
            ..Default::default()
        };
        let health = mux.health();
        let server = owned::bind_with_mux(config(), mux, vec![primary_addr], runtime, None)
            .await
            .unwrap();
        let client = owned::bind_endpoint(config()).await.unwrap();
        let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        let (a, b) = (a.unwrap(), b.unwrap());
        let extra = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match a
                    .inner()
                    .open_path_ensure(secondary_addr, noq::PathStatus::Backup)
                    .await
                {
                    Ok(path) => break path,
                    Err(noq::PathError::RemoteCidsExhausted | noq::PathError::MaxPathIdReached) => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    Err(error) => panic!("extra path failed: {error}"),
                }
            }
        })
        .await
        .unwrap();
        // Wait for the receiving policy to observe the validated sibling too.
        let facade = rds_net::Connection::from(b.clone());
        tokio::time::timeout(Duration::from_secs(2), async {
            while !facade
                .path_stats()
                .iter()
                .any(|path| path.path_id == extra.id().to_string().parse::<u64>().unwrap())
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        b.send_datagram(b"before fault".to_vec().into()).unwrap();
        assert_eq!(&a.read_datagram().await.unwrap()[..], b"before fault");
        let (mut stream_send, mut stream_recv) = a.open_bi().await.unwrap();
        stream_send.write_all(b"before").await.unwrap();
        let (mut reply, mut request) = b.accept_bi().await.unwrap();
        let mut prefix = [0; 6];
        request.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"before");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !a
                .inner()
                .get_remote_nat_traversal_addresses()
                .unwrap()
                .contains(&primary_addr)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fixture did not receive the original advertisement");
        fault.enable();
        if !receive {
            b.inner().path(noq::PathId::ZERO).unwrap().ping().unwrap();
            a.inner().path(noq::PathId::ZERO).unwrap().ping().unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while fault.hits.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // The endpoint owns retirement. No explicit path-close call may
        // conceal a missed health notification or a stale selected path.
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            // QNT can concurrently establish another path to the same healthy
            // socket. Selection may legitimately differ between the peers;
            // require a live selected route, not this particular PathId.
            while !selected_remote_is(&a, secondary_addr)
                || facade
                    .current_path_stats()
                    .is_none_or(|path| path.path_id == 0)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            a.send_datagram(b"surviving sibling".to_vec().into())
                .unwrap();
            let datagram = b
                .read_datagram()
                .await
                .expect("server receives via sibling");
            b.send_datagram(datagram).unwrap();
            assert_eq!(&a.read_datagram().await.unwrap()[..], b"surviving sibling");
            stream_send.write_all(b"after").await.unwrap();
            stream_send.finish().unwrap();
            assert_eq!(request.read_to_end(5).await.unwrap(), b"after");
            reply.write_all(b"same stream").await.unwrap();
            reply.finish().unwrap();
            assert_eq!(stream_recv.read_to_end(11).await.unwrap(), b"same stream");
        })
        .await;
        assert!(
            result.is_ok(),
            "one child failure stopped the healthy sibling: {:?}; {:?}; client={:?}; server={:?}",
            extra.status(),
            health.snapshot(),
            path_details(&a),
            path_details(&b),
        );
        assert!(
            !server
                .addr()
                .addrs
                .contains(&rds_net::TransportAddr::Ip(primary_addr))
        );
        assert_eq!(health.snapshot()[0].failed, Some(io::ErrorKind::BrokenPipe));
        assert!(health.snapshot()[1].failed.is_none());
        assert!(
            facade
                .current_path_stats()
                .is_some_and(|path| path.path_id != 0)
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while a
                .inner()
                .get_remote_nat_traversal_addresses()
                .unwrap()
                .contains(&primary_addr)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("failed local address remained advertised to the peer");
        // Accept a fresh connection on the surviving transport, not merely
        // traffic buffered on the existing connection.
        let target = rds_net::EndpointAddr::new(server.id()).with_ip_addr(secondary_addr);
        let (new_client, new_server) =
            tokio::join!(client.connect(target, rds_core::ALPN), async {
                server.accept().await.unwrap().await
            });
        let (new_client, new_server) = (new_client.unwrap(), new_server.unwrap());
        new_server
            .send_datagram(b"new connection".to_vec().into())
            .unwrap();
        assert_eq!(
            &new_client.read_datagram().await.unwrap()[..],
            b"new connection"
        );
        // Loss of the last socket must close held Connection/Path/Stream I/O
        // without waiting for an explicit endpoint.close or application drop.
        let held_path = b.inner().path(extra.id()).unwrap();
        secondary_fault.enable();
        held_path.ping().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !health.all_failed() || !facade.is_closed() || server.active_path_drivers() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("last-child failure retained a live policy/connection");
        assert!(new_server.inner().close_reason().is_some());
        assert!(server.addr().addrs.is_empty());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), server.accept())
                .await
                .unwrap()
                .is_none()
        );
        let rejected = server.connect(client.addr(), rds_core::ALPN).await;
        assert!(rejected.is_err(), "terminal endpoint still admitted a dial");
        a.close(0u32.into(), b"cleanup");
        b.close(0u32.into(), b"cleanup");
        tokio::join!(client.close(), server.close());
    })
    .await
    .expect("child-failure fixture stalled");
}

fn selected_remote_is(conn: &owned::Connection, remote: SocketAddr) -> bool {
    rds_net::Connection::from(conn.clone())
        .current_path_stats()
        .and_then(|stats| {
            conn.inner()
                .path(noq::PathId::from(u32::try_from(stats.path_id).unwrap()))
        })
        .is_some_and(|path| {
            path.status().ok() == Some(noq::PathStatus::Available)
                && path.remote_address().ok() == Some(remote)
        })
}

fn path_details(conn: &owned::Connection) -> Vec<(u64, bool, String)> {
    rds_net::Connection::from(conn.clone())
        .path_stats()
        .into_iter()
        .map(|stats| {
            let path = conn
                .inner()
                .path(noq::PathId::from(u32::try_from(stats.path_id).unwrap()))
                .unwrap();
            (
                stats.path_id,
                stats.selected,
                format!("{:?} {:?}", path.network_path(), path.status()),
            )
        })
        .collect()
}
