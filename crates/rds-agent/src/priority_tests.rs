//! A queued media body must not prevent control from crossing an exhausted
//! congestion window. Blocking peer ACKs exposes scheduling order without
//! depending on wall-clock bandwidth or an ambient display/network.
use std::future::{Future, poll_fn};
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

async fn compete(client_direction: bool, info: bool) {
    let (server, server_gate) = endpoint().await;
    let (client, client_gate) = endpoint().await;
    let (client_conn, server_conn) =
        tokio::join!(rds_client::connect(&client, server.addr()), async {
            server.accept().await.unwrap().await.unwrap()
        });
    let client_conn = client_conn.unwrap();
    // Establish a bidirectional body lane before the gated phase. Its 256 KiB
    // body exceeds the initial cwnd, and neither receiver drains it during
    // the assertion. Priority is the same class as real frame streams.
    let (mut client_bulk, mut client_recv) = client_conn.open_bi().await.unwrap();
    client_bulk.write_all(b"c").await.unwrap();
    let (mut server_bulk, mut server_recv) = server_conn.accept_bi().await.unwrap();
    server_recv.read_exact(&mut [0]).await.unwrap();
    server_bulk.write_all(b"s").await.unwrap();
    client_recv.read_exact(&mut [0]).await.unwrap();
    client_bulk
        .set_priority(rds_net::wire::MEDIA_STREAM_PRIORITY)
        .unwrap();
    server_bulk
        .set_priority(rds_net::wire::MEDIA_STREAM_PRIORITY)
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    server_gate.close();
    if client_direction {
        client_gate.close();
    }

    let body = vec![0xA5; 256 * 1024];
    let gate = client_gate.clone();
    let release = server_gate.clone();
    let conn = server_conn.clone();
    let audience = *server.id().as_bytes();
    let serving = tokio::spawn(async move {
        let (send, recv) = conn.accept_bi().await.unwrap();
        if !client_direction {
            // The greeting has arrived. Suppress client ACKs before queueing
            // the reply and media together on this single-thread test runtime.
            gate.close();
        }
        super::serve_stream(
            conn,
            send,
            recv,
            Arc::new(super::AgentPolicy::ssh_only(("127.0.0.1".into(), 9))),
            Arc::new(super::ConnAuthz::new(false, audience, 4)),
            super::DesktopBackend::default(),
        )
        .await
        .unwrap();
        if !client_direction {
            server_bulk.write_all(&body).await.unwrap();
            release.open();
        }
        server_bulk
    });

    let request = async {
        if info {
            let info = rds_client::info(&client_conn).await.unwrap();
            assert!(!info.services.is_empty());
        } else {
            rds_client::ping(&client_conn, rand::random())
                .await
                .unwrap();
        }
    };
    tokio::pin!(request);
    // Queue the complete greeting before the bulk write, without yielding to
    // the transport driver. The lower-priority greeting must still be denied
    // progress by the old code: FIFO enqueue order alone cannot pass the test.
    assert!(
        poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    if client_direction {
        client_bulk
            .write_all(&vec![0x5A; 256 * 1024])
            .await
            .unwrap();
        client_gate.open();
        let server_bulk = tokio::time::timeout(Duration::from_secs(1), serving)
            .await
            .expect("control greeting starved behind media while peer ACKs were blocked")
            .unwrap();
        server_gate.open();
        tokio::time::timeout(Duration::from_secs(1), &mut request)
            .await
            .expect("control response did not complete");
        drop(server_bulk);
    } else {
        tokio::time::timeout(Duration::from_secs(1), &mut request)
            .await
            .expect("control reply starved behind media while peer ACKs were blocked");
        drop(serving.await.unwrap());
    }
    client_gate.open();
    server_gate.open();
    let _ = client_bulk.reset(0u32.into());
    client_conn.close(0u32.into(), b"test complete");
    server_conn.close(0u32.into(), b"test complete");
    client.close().await;
    server.close().await;
}

#[tokio::test]
async fn client_control_precedes_queued_media() {
    tokio::time::timeout(Duration::from_secs(8), async {
        for info in [false, true] {
            compete(true, info).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn agent_control_reply_precedes_queued_media() {
    tokio::time::timeout(Duration::from_secs(8), async {
        for info in [false, true] {
            compete(false, info).await;
        }
    })
    .await
    .unwrap();
}
