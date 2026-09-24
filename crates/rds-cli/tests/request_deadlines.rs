//! Real QUIC peers with no stream credit or no receive window.
use rds_net::Connection;
use std::time::Duration;

async fn endpoint(credit: u32, receive: u32, send: u64) -> iroh::Endpoint {
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(credit.into())
        .stream_receive_window(receive.into())
        .receive_window(receive.into())
        .send_window(send)
        .build();
    iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .alpns(vec![rds_core::ALPN.to_vec()])
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .transport_config(transport)
        .bind()
        .await
        .unwrap()
}

async fn blocked_requests(credit: u32, receive: u32, send: u64) {
    let client = endpoint(16, 4096, send).await;
    let server = endpoint(credit, receive, 4096).await;
    let (conn, peer) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    let conn = Connection::from(conn.unwrap());
    let peer = peer.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(17), async {
        let (ping, info, tcp, sync) = tokio::join!(
            rds_cli::ping(&conn, 17),
            rds_cli::info(&conn),
            rds_cli::open_tcp(&conn, "127.0.0.1", 9),
            rds_cli::open_sync(&conn)
        );
        [
            ping.map(|_| ()),
            info.map(|_| ()),
            tcp.map(|_| ()),
            sync.map(|_| ()),
        ]
    })
    .await;
    assert!(
        peer.close_reason().is_none(),
        "request expiry must preserve the shared connection"
    );
    client.close().await;
    server.close().await;
    for result in result.expect("service request had no complete prelude deadline") {
        assert!(format!("{:#}", result.unwrap_err()).contains("timed out"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_stream_credit_bounds_every_service_request() {
    blocked_requests(0, 4096, 4096).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_receive_window_bounds_request_writes() {
    blocked_requests(16, 0, 1).await;
}

async fn pair() -> (iroh::Endpoint, iroh::Endpoint, Connection, Connection) {
    let client = endpoint(16, 4096, 4096).await;
    let server = endpoint(16, 4096, 4096).await;
    let (conn, peer) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    (client, server, conn.unwrap().into(), peer.unwrap().into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_request_resets_its_stream_without_closing_shared_connection() {
    let (client, server, conn, peer) = pair().await;
    let task = tokio::spawn({
        let conn = conn.clone();
        async move { rds_cli::ping(&conn, 4).await }
    });
    let (_send, mut recv) = peer.accept_bi().await.unwrap();
    let hello: rds_core::StreamHello = rds_core::read_frame(&mut recv).await.unwrap();
    assert!(matches!(hello, rds_core::StreamHello::Ping { nonce: 4 }));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let error = tokio::time::timeout(Duration::from_secs(2), recv.read_to_end(32))
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        error,
        iroh::endpoint::ReadToEndError::Read(rds_net::ReadError::Reset(_))
    ));
    assert!(!conn.is_closed());
    assert!(!peer.is_closed());
    client.close().await;
    server.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authorization_deadline_includes_stream_credit() {
    let client = rds_net::bind_endpoint(rds_net::EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let server = endpoint(0, 4096, 4096).await;
    // The server never grants a stream: no grant bytes are consumed/verified.
    let grant = rds_core::grant::Grant {
        payload: vec![],
        signature: vec![],
    };
    let task = tokio::spawn({
        let client = client.clone();
        let addr = server.addr();
        async move {
            tokio::time::timeout(
                Duration::from_secs(17),
                rds_cli::connect_authorized(&client, addr, &grant),
            )
            .await
        }
    });
    let peer = server.accept().await.unwrap().await.unwrap();
    let result = task.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), peer.closed())
        .await
        .unwrap();
    client.close().await;
    server.close().await;
    let error = result
        .expect("Authz stream credit was unbounded")
        .unwrap_err();
    assert!(format!("{error:#}").contains("authorization request timed out"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_authorization_after_ack_without_fin_closes_connection() {
    let client = rds_net::bind_endpoint(rds_net::EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let server = endpoint(16, 4096, 4096).await;
    let grant = rds_core::grant::Grant {
        payload: vec![],
        signature: vec![],
    };
    let mut task = tokio::spawn({
        let client = client.clone();
        let addr = server.addr();
        async move { rds_cli::connect_authorized(&client, addr, &grant).await }
    });
    let peer = server.accept().await.unwrap().await.unwrap();
    let (mut send, mut recv) = peer.accept_bi().await.unwrap();
    let hello: rds_core::StreamHello = rds_core::read_frame(&mut recv).await.unwrap();
    assert!(matches!(hello, rds_core::StreamHello::Authz(_)));
    rds_core::write_frame(&mut send, &rds_core::HelloAck::Ok)
        .await
        .unwrap();
    // Keep the server's reply open, modeling an interrupted response transaction.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut task)
            .await
            .is_err()
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), peer.closed())
        .await
        .unwrap();
    client.close().await;
    server.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_echo_shares_the_prelude_deadline() {
    let (client, server, conn, peer) = pair().await;
    let task = tokio::spawn(async move {
        let (mut send, mut recv) = peer.accept_bi().await.unwrap();
        let _: rds_core::StreamHello = rds_core::read_frame(&mut recv).await.unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        rds_core::write_frame(&mut send, &rds_core::HelloAck::Ok)
            .await
            .unwrap();
        // A fresh timeout for echo would allow 25 seconds; one budget allows 15.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let _ = send.write_all(&7u64.to_be_bytes()).await;
        let _ = send.finish();
    });
    let result = tokio::time::timeout(Duration::from_secs(17), rds_cli::ping(&conn, 7))
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    client.close().await;
    server.close().await;
    assert!(format!("{:#}", result.unwrap_err()).contains("ping request timed out"));
}
