//! Real relay overload must remain bounded and never return a partial datagram.
#![cfg(feature = "owned-relay")]
use noq::{
    AsyncUdpSocket,
    udp::{RecvMeta, Transmit},
};
use rds_net::backends::noq::relay::{RelaySocket, synthetic_for};
use rds_net::{Backend, EndpointConfig, RelayLimits, SecretKey};
use std::{future::poll_fn, io::IoSliceMut, time::Duration};

async fn receive(socket: &mut RelaySocket) -> Vec<u8> {
    let mut bytes = [0u8; 128];
    let mut bufs = [IoSliceMut::new(&mut bytes)];
    let mut meta = [RecvMeta::default()];
    assert_eq!(
        poll_fn(|cx| socket.poll_recv(cx, &mut bufs, &mut meta))
            .await
            .unwrap(),
        1
    );
    bytes[..meta[0].len].to_vec()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overload_drops_whole_datagrams_and_diagnostic_handles_do_not_retain_payloads() {
    tokio::time::timeout(Duration::from_secs(15), exercise())
        .await
        .expect("queue fixture exceeded deadline");
}
async fn exercise() {
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
    let key = SecretKey::from_bytes(&[121; 32]);
    let aid = key.public();
    let limits = RelayLimits {
        max_peers: 2.try_into().unwrap(),
        datagram_queue: 2.try_into().unwrap(),
        ..Default::default()
    };
    let (mut a, ah) = RelaySocket::connect_with_limits(
        relay.endpoint_addr(),
        key,
        "127.0.0.1:0".parse().unwrap(),
        limits,
    )
    .await
    .unwrap();
    let (mut b, bh) = RelaySocket::connect(
        relay.endpoint_addr(),
        SecretKey::from_bytes(&[122; 32]),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    let _route = bh.register_peer(aid).unwrap();
    let mut sender = b.create_sender();
    for seq in 0u8..64 {
        let bytes = [seq; 32];
        let tx = Transmit {
            destination: synthetic_for(&aid),
            ecn: None,
            contents: &bytes,
            segment_size: None,
            src_ip: None,
        };
        poll_fn(|cx| sender.as_mut().poll_send(&tx, cx))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let stats = ah.stats();
            assert_eq!(stats.queue_capacity, 2);
            assert!(stats.queued_datagrams <= 2 && stats.peer_entries <= 2);
            if stats.dropped_queue_full > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("fixture never filled the bounded relay queue");
    // A one-byte caller buffer must discard the entire 32-byte packet,
    // never report a successfully received one-byte prefix.
    let mut byte = [0u8; 1];
    let mut bufs = [IoSliceMut::new(&mut byte)];
    let mut meta = [RecvMeta::default()];
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            poll_fn(|cx| a.poll_recv(cx, &mut bufs, &mut meta))
        )
        .await
        .is_err()
    );
    assert!(ah.stats().dropped_oversized > 0);
    let marker = b"after overload";
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let tx = Transmit {
                destination: synthetic_for(&aid),
                ecn: None,
                contents: marker,
                segment_size: None,
                src_ip: None,
            };
            poll_fn(|cx| sender.as_mut().poll_send(&tx, cx))
                .await
                .unwrap();
            if let Ok(body) = tokio::time::timeout(Duration::from_millis(50), receive(&mut a)).await
                && body == marker
            {
                break;
            }
        }
    })
    .await
    .expect("relay did not recover after queue pressure");
    let tx = Transmit {
        destination: synthetic_for(&aid),
        ecn: None,
        contents: b"queued before drop",
        segment_size: None,
        src_ip: None,
    };
    poll_fn(|cx| sender.as_mut().poll_send(&tx, cx))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while ah.stats().queued_datagrams == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    drop(a);
    tokio::time::timeout(Duration::from_secs(1), async {
        while ah.stats().queued_datagrams != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("diagnostic handle retained queued payloads");
    assert!(!ah.is_available());
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(b.close(), relay.close());
    })
    .await
    .expect("queue fixture cleanup stalled");
}
