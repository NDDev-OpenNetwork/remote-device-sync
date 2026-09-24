//! Bounded forwarding workers own both local TCP and remote QUIC streams.
use rds_net::{Backend, Connection, Endpoint, EndpointConfig, bind_endpoint};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn backends() -> Vec<Backend> {
    #[cfg(feature = "transport-noq")]
    {
        vec![Backend::Iroh, Backend::Noq]
    }
    #[cfg(not(feature = "transport-noq"))]
    {
        vec![Backend::Iroh]
    }
}
async fn pair(backend: Backend) -> (Endpoint, Endpoint, Connection, Connection) {
    let config = || EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let client = bind_endpoint(config()).await.unwrap();
    let server = bind_endpoint(config()).await.unwrap();
    let (a, b) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    (client, server, a.unwrap(), b.unwrap())
}
async fn accept(peer: &Connection) -> (rds_net::SendStream, rds_net::RecvStream) {
    tokio::time::timeout(Duration::from_secs(2), async {
        let (send, mut recv) = peer.accept_bi().await.unwrap();
        let hello: rds_core::StreamHello = rds_core::read_frame(&mut recv).await.unwrap();
        assert!(matches!(
            hello,
            rds_core::StreamHello::TcpConnect { port: 9, .. }
        ));
        (send, recv)
    })
    .await
    .unwrap()
}
async fn closed(tcp: &mut TcpStream) {
    let mut byte = [0];
    match tokio::time::timeout(Duration::from_secs(2), tcp.read(&mut byte))
        .await
        .unwrap()
    {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("local TCP remained open: {other:?}"),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_forwarder_closes_live_splice_and_preserves_shared_connection() {
    for backend in backends() {
        let (client, server, conn, peer) = pair(backend).await;
        let conn = Arc::new(conn);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(rds_cli::forward_bound_listener(
            conn.clone(),
            listener,
            "127.0.0.1:9".parse().unwrap(),
            NonZeroU16::new(1).unwrap(),
        ));
        let mut local = TcpStream::connect(addr).await.unwrap();
        let (mut send, mut recv) = accept(&peer).await;
        rds_core::write_frame(&mut send, &rds_core::HelloAck::Ok)
            .await
            .unwrap();
        local.write_all(b"body").await.unwrap();
        let mut body = [0; 4];
        tokio::time::timeout(Duration::from_secs(2), recv.read_exact(&mut body))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&body, b"body");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        closed(&mut local).await;
        let error = tokio::time::timeout(Duration::from_secs(2), recv.read_to_end(32))
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error,
            iroh::endpoint::ReadToEndError::Read(rds_net::ReadError::Reset(_))
        ));
        assert!(TcpStream::connect(addr).await.is_err());
        assert!(!conn.is_closed());
        // A different service can still use the shared QUIC connection.
        let ping = tokio::spawn({
            let conn = conn.clone();
            async move { rds_cli::ping(&conn, 3).await }
        });
        let (mut send, mut recv) = peer.accept_bi().await.unwrap();
        let hello: rds_core::StreamHello = rds_core::read_frame(&mut recv).await.unwrap();
        assert!(matches!(hello, rds_core::StreamHello::Ping { nonce: 3 }));
        rds_core::write_frame(&mut send, &rds_core::HelloAck::Ok)
            .await
            .unwrap();
        send.write_all(&3u64.to_be_bytes()).await.unwrap();
        send.finish().unwrap();
        tokio::time::timeout(Duration::from_secs(2), ping)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        client.close().await;
        server.close().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_budget_recovers_and_connection_close_joins_workers() {
    for backend in backends() {
        let (client, server, conn, peer) = pair(backend).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(rds_cli::forward_bound_listener(
            Arc::new(conn),
            listener,
            "127.0.0.1:9".parse().unwrap(),
            NonZeroU16::new(2).unwrap(),
        ));
        let mut locals = Vec::new();
        for _ in 0..3 {
            locals.push(TcpStream::connect(addr).await.unwrap());
        }
        let (mut first, _first_recv) = accept(&peer).await;
        let (_second, _second_recv) = accept(&peer).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), peer.accept_bi())
                .await
                .is_err()
        );
        rds_core::write_frame(
            &mut first,
            &rds_core::HelloAck::Error {
                message: "fixture refusal".into(),
            },
        )
        .await
        .unwrap();
        first.finish().unwrap();
        let (_third, _third_recv) = accept(&peer).await;
        peer.close(0u32.into(), b"fixture close");
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        for local in &mut locals {
            closed(local).await;
        }
        assert!(TcpStream::connect(addr).await.is_err());
        client.close().await;
        server.close().await;
    }
}
