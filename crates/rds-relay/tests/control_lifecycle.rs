//! A successful drain must be observed by the actual relay-socket client.
#![cfg(feature = "owned-relay")]
use rds_net::backends::noq::relay::RelaySocket;
use rds_net::{Backend, EndpointConfig, SecretKey};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_client_observes_drain_before_tunnel_closes() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (socket, handle) = RelaySocket::connect(
        relay.endpoint_addr(),
        SecretKey::from_bytes(&[77; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while relay.endpoints() != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let (_, observed) = tokio::join!(
        relay.drain(),
        tokio::time::timeout(Duration::from_secs(1), async {
            while !handle.drained() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    );
    drop(socket);
    assert!(observed.is_ok(), "client never decoded the Drain notice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_socket_releases_its_relay_attachment() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let (socket, handle) = RelaySocket::connect(
        relay.endpoint_addr(),
        SecretKey::from_bytes(&[78; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while relay.endpoints() != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(handle.is_available());
    drop(socket);
    assert!(!handle.is_available());
    let detached = tokio::time::timeout(Duration::from_secs(1), async {
        while relay.endpoints() != 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    relay.close().await;
    assert!(
        detached.is_ok(),
        "socket pumps retained the attachment after drop"
    );
}

async fn start_relay() -> rds_relay::server::Relay {
    rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap()
}

async fn attachment(
    seed: u8,
    relay: rds_net::EndpointAddr,
) -> (
    rds_net::backends::noq::Endpoint,
    rds_net::backends::noq::Connection,
    rds_net::SendStream,
    rds_net::RecvStream,
) {
    use rds_core::relay::{RELAY_ALPN, RelayControl};
    use rds_net::relay_control::{read_control, write_control};
    let endpoint = rds_net::backends::noq::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        secret_key: Some(SecretKey::from_bytes(&[seed; 32])),
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![RELAY_ALPN.to_vec()],
        ..Default::default()
    })
    .await
    .unwrap();
    let conn = endpoint.connect(relay, RELAY_ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_control(&mut send, &RelayControl::Register)
        .await
        .unwrap();
    assert!(matches!(
        read_control(&mut recv).await.unwrap(),
        RelayControl::Registered
    ));
    (endpoint, conn, send, recv)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_preserves_peer_history_and_actual_detach_sends_framed_notice() {
    use rds_core::relay::RelayControl;
    use rds_net::relay_control::{read_control, write_control};
    let relay = start_relay().await;
    let (a, ac, mut aw, mut ar) = attachment(81, relay.endpoint_addr()).await;
    let (b, bc, _bw, _br) = attachment(82, relay.endpoint_addr()).await;
    ac.send_datagram(rds_core::relay::encode_forward(b.id().as_bytes(), b"flow"))
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), bc.read_datagram())
        .await
        .unwrap()
        .unwrap();
    let (src, payload) = rds_core::relay::decode_frame(&frame).unwrap();
    assert_eq!(src, *a.id().as_bytes());
    assert_eq!(payload, b"flow");
    let (b2, b2c, _b2w, _b2r) = attachment(82, relay.endpoint_addr()).await;
    tokio::time::timeout(Duration::from_secs(2), bc.inner().closed())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), read_control(&mut ar))
            .await
            .is_err(),
        "stale owner emitted PeerGone for its replacement"
    );
    b2c.close(0u32.into(), b"actual detach");
    let notice = tokio::time::timeout(Duration::from_secs(2), read_control(&mut ar))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(notice,RelayControl::PeerGone{peer} if peer==*b.id().as_bytes()));
    // The next frame remains aligned, not swallowed into a malformed notice.
    write_control(&mut aw, &RelayControl::Ping { seq: 19 })
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), read_control(&mut ar))
            .await
            .unwrap()
            .unwrap(),
        RelayControl::Pong { seq: 19 }
    ));
    a.close().await;
    b.close().await;
    b2.close().await;
    relay.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_grace_keeps_actual_socket_datagrams_flowing() {
    use noq::AsyncUdpSocket;
    use noq::udp::{RecvMeta, Transmit};
    use rds_net::backends::noq::relay::synthetic_for;
    use std::future::poll_fn;
    use std::io::IoSliceMut;
    let relay = start_relay().await;
    let ka = SecretKey::from_bytes(&[83; 32]);
    let kb = SecretKey::from_bytes(&[84; 32]);
    let aid = ka.public();
    let bid = kb.public();
    let (mut a, ah) =
        RelaySocket::connect(relay.endpoint_addr(), ka, "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
    let (mut b, bh) =
        RelaySocket::connect(relay.endpoint_addr(), kb, "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
    ah.register_peer(bid);
    bh.register_peer(aid);
    let (_, traffic) = tokio::join!(
        relay.drain(),
        tokio::time::timeout(Duration::from_secs(1), async {
            while !ah.drained() || !bh.drained() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            assert!(
                ah.is_available() && bh.is_available(),
                "Drain grace must stay available"
            );
            let mut sender = a.create_sender();
            let transmit = Transmit {
                destination: synthetic_for(&bid),
                ecn: None,
                contents: b"during grace",
                segment_size: None,
                src_ip: None,
            };
            poll_fn(|cx| sender.as_mut().poll_send(&transmit, cx))
                .await
                .unwrap();
            let mut bytes = [0u8; 64];
            let mut bufs = [IoSliceMut::new(&mut bytes)];
            let mut meta = [RecvMeta::default()];
            assert_eq!(
                poll_fn(|cx| b.poll_recv(cx, &mut bufs, &mut meta))
                    .await
                    .unwrap(),
                1
            );
            assert_eq!(meta[0].addr, synthetic_for(&aid));
            assert_eq!(&bytes[..meta[0].len], b"during grace");
        })
    );
    a.close().await;
    b.close().await;
    assert!(
        traffic.is_ok(),
        "drain notice blackholed a usable grace-period tunnel"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_registration_reply_is_rejected_without_clamping() {
    use rds_core::relay::{RELAY_ALPN, RelayControl};
    use rds_net::relay_control::read_control;
    let server = rds_net::backends::noq::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![RELAY_ALPN.to_vec()],
        ..Default::default()
    })
    .await
    .unwrap();
    let (client, held) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            RelaySocket::connect(
                server.addr(),
                SecretKey::from_bytes(&[85; 32]),
                "127.0.0.1:0".parse().unwrap()
            ),
            async {
                let conn = server.accept().await.unwrap().await.unwrap();
                let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                assert!(matches!(
                    read_control(&mut recv).await.unwrap(),
                    RelayControl::Register
                ));
                let mut reply = 4097u32.to_be_bytes().to_vec();
                let mut body = postcard::to_stdvec(&RelayControl::Registered).unwrap();
                body.resize(4096, 0);
                reply.extend(body);
                let _ = send.write_all(&reply).await;
                (conn, send, recv)
            }
        )
    })
    .await
    .unwrap();
    server.close().await;
    drop(held);
    let error = match client {
        Err(error) => error,
        Ok(_) => panic!("oversized reply accepted"),
    };
    assert!(format!("{error:#}").contains("4097"));
}
