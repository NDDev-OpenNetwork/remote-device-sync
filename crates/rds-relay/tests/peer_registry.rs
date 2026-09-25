//! Relay address-table integrity. The payloads here are transport fixtures,
//! not application messages; end-to-end QUIC identity pinning is unchanged.
#![cfg(feature = "owned-relay")]
use noq::{
    AsyncUdpSocket,
    udp::{RecvMeta, Transmit},
};
use rds_net::backends::noq::relay::{RelaySocket, synthetic_for};
use rds_net::{Backend, EndpointConfig, SecretKey};
use std::{future::poll_fn, io::IoSliceMut, time::Duration};

// Public deterministic test keys, found once by an offline bounded search.
// Normal test runs perform no search and use no deployment credentials.
fn collision_key(index: u32) -> SecretKey {
    let mut bytes = [123; 32];
    bytes[..4].copy_from_slice(&index.to_le_bytes());
    SecretKey::from_bytes(&bytes)
}
async fn received(socket: &mut RelaySocket) -> Vec<u8> {
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
async fn colliding_peer_registration_cannot_replace_the_existing_route() {
    tokio::time::timeout(Duration::from_secs(10), collision_case())
        .await
        .expect("collision fixture exceeded deadline");
}
async fn collision_case() {
    let ka = collision_key(153039);
    let kb = collision_key(167304);
    let aid = ka.public();
    let bid = kb.public();
    assert_ne!(aid, bid);
    let alias = synthetic_for(&aid);
    assert_eq!(
        alias,
        synthetic_for(&bid),
        "public fixture keys must actually collide"
    );
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
    let bind = "127.0.0.1:0".parse().unwrap();
    let (mut a, _ah) = RelaySocket::connect(relay.endpoint_addr(), ka, bind)
        .await
        .unwrap();
    let (mut b, _bh) = RelaySocket::connect(relay.endpoint_addr(), kb, bind)
        .await
        .unwrap();
    let (mut c, ch) = RelaySocket::connect(
        relay.endpoint_addr(),
        SecretKey::from_bytes(&[119; 32]),
        bind,
    )
    .await
    .unwrap();
    let _first_route = ch.register_peer(aid).unwrap();
    assert_eq!(
        ch.register_peer(bid).unwrap_err(),
        rds_net::backends::noq::relay::PeerRegistrationError::Collision
    );
    let payload = b"intended for the first peer";
    let mut sender = c.create_sender();
    let transmit = Transmit {
        destination: alias,
        ecn: None,
        contents: payload,
        segment_size: None,
        src_ip: None,
    };
    poll_fn(|cx| sender.as_mut().poll_send(&transmit, cx))
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(1), received(&mut a)),
        tokio::time::timeout(Duration::from_secs(1), received(&mut b))
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        let (_, _, _, closed_3) = tokio::join!(a.close(), b.close(), c.close(), relay.close());
        closed_3.unwrap();
    })
    .await
    .expect("collision fixture cleanup stalled");
    let reached_first = first.as_ref().is_ok_and(|body| body == payload);
    let reached_second = second.as_ref().is_ok_and(|body| body == payload);
    assert!(
        reached_first && !reached_second,
        "synthetic alias overwritten: first_received={reached_first}, second_received={reached_second}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_peer_lease_survives_facade_drop_until_the_last_stream_closes() {
    tokio::time::timeout(Duration::from_secs(15), lease_lifetime_case())
        .await
        .expect("lease fixture exceeded deadline");
}
async fn lease_lifetime_case() {
    use rds_net::backends::noq as owned;
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
    let limits = rds_net::RelayLimits {
        max_peers: 1.try_into().unwrap(),
        ..Default::default()
    };
    let (a, handle) = managed_endpoint(relay.endpoint_addr(), 124, limits).await;
    let b = owned::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    let pair = async {
        let (ca, cb) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        (ca.unwrap(), cb.unwrap())
    };
    let (ca, cb) = tokio::time::timeout(Duration::from_secs(3), pair)
        .await
        .unwrap();
    assert_eq!(handle.stats().pinned_peers, 1);
    let (mut send, recv) = ca.open_bi().await.unwrap();
    send.write_all(b"a").await.unwrap();
    let (_bs, mut br) = cb.accept_bi().await.unwrap();
    let mut byte = [0u8; 1];
    br.read_exact(&mut byte).await.unwrap();
    drop(ca);
    let other = SecretKey::from_bytes(&[125; 32]).public();
    assert_eq!(
        handle.register_peer(other).unwrap_err(),
        rds_net::backends::noq::relay::PeerRegistrationError::Capacity
    );
    send.write_all(b"b").await.unwrap();
    br.read_exact(&mut byte).await.unwrap();
    assert_eq!(&byte, b"b");
    drop(send);
    drop(recv);
    tokio::time::timeout(Duration::from_secs(2), async {
        while a.active_path_drivers() != 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(handle.stats().pinned_peers, 0);
    let _successor = handle.register_peer(other).unwrap();
    assert_eq!(handle.stats().peer_entries, 1);
    tokio::time::timeout(Duration::from_secs(3), async {
        let (_, _, closed_2) = tokio::join!(a.close(), b.close(), relay.close());
        closed_2.unwrap();
    })
    .await
    .expect("lease fixture cleanup stalled");
}

async fn managed_endpoint(
    relay: rds_net::EndpointAddr,
    seed: u8,
    limits: rds_net::RelayLimits,
) -> (
    rds_net::backends::noq::Endpoint,
    rds_net::backends::noq::relay::RelayHandle,
) {
    use noq::Runtime;
    use rds_net::backends::noq as owned;
    use std::sync::Arc;
    let key = SecretKey::from_bytes(&[seed; 32]);
    let (socket, handle) = RelaySocket::connect_with_limits(
        relay.clone(),
        key.clone(),
        "127.0.0.1:0".parse().unwrap(),
        limits,
    )
    .await
    .unwrap();
    let runtime = Arc::new(noq::TokioRuntime);
    let direct = runtime
        .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
        .unwrap();
    let mux = owned::socket::Mux::new(vec![direct, Box::new(socket)]).unwrap();
    let local = mux.local_addrs();
    let a = owned::bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key),
            discovery: false,
            relay_endpoint: Some(relay),
            relay_limits: limits,
            ..Default::default()
        },
        Box::new(mux),
        local,
        runtime,
        Some(handle.clone()),
    )
    .await
    .unwrap();
    (a, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_peer_table_refuses_relay_only_dial_but_preserves_direct_dial_and_accept() {
    tokio::time::timeout(Duration::from_secs(15), capacity_case())
        .await
        .expect("capacity fixture exceeded deadline");
}
async fn capacity_case() {
    use rds_net::backends::noq as owned;
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
    let limits = rds_net::RelayLimits {
        max_peers: 1.try_into().unwrap(),
        ..Default::default()
    };
    let (a, ah) = managed_endpoint(relay.endpoint_addr(), 126, limits).await;
    let (b, bh) = managed_endpoint(relay.endpoint_addr(), 127, limits).await;
    // Fill both sides so the incoming and outgoing admission paths are tested.
    let _ap = ah
        .register_peer(SecretKey::from_bytes(&[128; 32]).public())
        .unwrap();
    let _bp = bh
        .register_peer(SecretKey::from_bytes(&[129; 32]).public())
        .unwrap();
    let mut relay_only = b.addr();
    relay_only
        .addrs
        .retain(|address| matches!(address, rds_net::TransportAddr::Relay(_)));
    assert!(!relay_only.addrs.is_empty());
    let error = tokio::time::timeout(
        Duration::from_millis(250),
        a.connect(relay_only, rds_core::ALPN),
    )
    .await
    .expect("full relay table must refuse before dialing")
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<owned::relay::PeerRegistrationError>(),
        Some(&owned::relay::PeerRegistrationError::Capacity)
    );
    let pair = async {
        let (ca, cb) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
            b.accept().await.unwrap().await
        });
        (ca.unwrap(), cb.unwrap())
    };
    let (ca, cb) = tokio::time::timeout(Duration::from_secs(3), pair)
        .await
        .unwrap();
    ca.send_datagram(b"direct with full tables".to_vec().into())
        .unwrap();
    assert_eq!(
        &cb.read_datagram().await.unwrap()[..],
        b"direct with full tables"
    );
    cb.send_datagram(b"direct response".to_vec().into())
        .unwrap();
    assert_eq!(&ca.read_datagram().await.unwrap()[..], b"direct response");
    for handle in [&ah, &bh] {
        assert_eq!(handle.stats().peer_entries, 1);
        assert_eq!(handle.stats().pinned_peers, 1);
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        let (_, _, closed_2) = tokio::join!(a.close(), b.close(), relay.close());
        closed_2.unwrap();
    })
    .await
    .expect("capacity fixture cleanup stalled");
}
