//! A silent first address must not delay a healthy authenticated candidate.
#![cfg(feature = "transport-noq")]
use rds_net::backends::noq::{self as owned};
use rds_net::{Backend, EndpointConfig, SecretKey, TransportAddr};
use std::time::Duration;

fn config(bind: std::net::SocketAddr, key: SecretKey) -> EndpointConfig {
    EndpointConfig {
        backend: Backend::Noq,
        secret_key: Some(key),
        discovery: false,
        bind_addrs: vec![bind],
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blackholed_first_candidate_does_not_block_a_healthy_second() {
    let one = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let two = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (blackhole, working) = if one.local_addr().unwrap() < two.local_addr().unwrap() {
        (one, two)
    } else {
        (two, one)
    };
    let dead = blackhole.local_addr().unwrap();
    let live = working.local_addr().unwrap();
    drop(working);
    let server = owned::bind_endpoint(config(live, SecretKey::from_bytes(&[91; 32])))
        .await
        .unwrap();
    let client = owned::bind_endpoint(config(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::from_bytes(&[92; 32]),
    ))
    .await
    .unwrap();
    let mut addr = server.addr();
    addr.addrs.insert(TransportAddr::Ip(dead));
    assert_eq!(owned::policy::ip_candidates(&addr)[0], dead);
    let (outgoing, incoming) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(2), client.connect(addr, rds_core::ALPN)),
        tokio::time::timeout(Duration::from_secs(2), async {
            server.accept().await.unwrap().await
        })
    );
    let connected = match (&outgoing, &incoming) {
        (Ok(Ok(a)), Ok(Ok(b))) => a.remote_id() == server.id() && b.remote_id() == client.id(),
        _ => false,
    };
    client.close().await;
    server.close().await;
    drop(blackhole);
    assert!(connected, "working secondary candidate was never reached");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fast_wrong_identity_cannot_win_or_cancel_the_valid_candidate() {
    let one = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let two = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (wrong, valid) = if one.local_addr().unwrap() < two.local_addr().unwrap() {
        (one, two)
    } else {
        (two, one)
    };
    let wrong_addr = wrong.local_addr().unwrap();
    let valid_addr = valid.local_addr().unwrap();
    drop(wrong);
    drop(valid);
    let wrong = owned::bind_endpoint(config(wrong_addr, SecretKey::from_bytes(&[93; 32])))
        .await
        .unwrap();
    let server = owned::bind_endpoint(config(valid_addr, SecretKey::from_bytes(&[94; 32])))
        .await
        .unwrap();
    let client = owned::bind_endpoint(config(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::from_bytes(&[95; 32]),
    ))
    .await
    .unwrap();
    let mut target = server.addr();
    target.addrs.insert(TransportAddr::Ip(wrong_addr));
    let wrong_accept = tokio::spawn({
        let wrong = wrong.clone();
        async move {
            if let Some(incoming) = wrong.accept().await {
                let _ = tokio::time::timeout(Duration::from_secs(2), incoming).await;
            }
        }
    });
    let (outgoing, incoming) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(client.connect(target, rds_core::ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    let outgoing = outgoing.unwrap();
    let incoming = incoming.unwrap();
    outgoing
        .send_datagram(b"authenticated winner".to_vec().into())
        .unwrap();
    let payload = tokio::time::timeout(Duration::from_secs(2), incoming.read_datagram())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&payload[..], b"authenticated winner");
    assert_eq!(outgoing.remote_id(), server.id());
    client.close().await;
    server.close().await;
    wrong.close().await;
    wrong_accept.abort();
    let _ = wrong_accept.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_blackholed_candidates_have_a_total_handshake_bound() {
    let blackhole = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let client = owned::bind_endpoint(config(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::from_bytes(&[96; 32]),
    ))
    .await
    .unwrap();
    let target = rds_net::EndpointAddr {
        id: SecretKey::from_bytes(&[97; 32]).public(),
        addrs: std::collections::BTreeSet::from([TransportAddr::Ip(
            blackhole.local_addr().unwrap(),
        )]),
    };
    let result = tokio::time::timeout(
        Duration::from_secs(17),
        client.connect(target, rds_core::ALPN),
    )
    .await;
    client.close().await;
    drop(blackhole);
    let error = result
        .expect("dial had no initial-handshake deadline")
        .unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    assert_eq!(client.active_path_drivers(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_ipv4_candidate_can_fall_back_to_ipv6() {
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = owned::bind_endpoint(config(
        "[::1]:0".parse().unwrap(),
        SecretKey::from_bytes(&[98; 32]),
    ))
    .await
    .unwrap();
    let mut client_config = config(
        "127.0.0.1:0".parse().unwrap(),
        SecretKey::from_bytes(&[99; 32]),
    );
    client_config.bind_addrs.push("[::1]:0".parse().unwrap());
    let client = owned::bind_endpoint(client_config).await.unwrap();
    let mut target = server.addr();
    target
        .addrs
        .insert(TransportAddr::Ip(silent.local_addr().unwrap()));
    assert!(owned::policy::ip_candidates(&target)[0].is_ipv4());
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let (outgoing, incoming) = tokio::join!(client.connect(target, rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        let outgoing = outgoing.unwrap();
        let incoming = incoming.unwrap();
        outgoing
            .send_datagram(b"ipv6 winner".to_vec().into())
            .unwrap();
        assert_eq!(&incoming.read_datagram().await.unwrap()[..], b"ipv6 winner");
    })
    .await;
    client.close().await;
    server.close().await;
    assert!(result.is_ok(), "IPv6 candidate was blocked by silent IPv4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_socket_orders_preserve_both_families_in_both_directions() {
    for reverse in [false, true] {
        for ipv4 in [false, true] {
            for dual_is_client in [false, true] {
                let mut dual_config = config(
                    "127.0.0.1:0".parse().unwrap(),
                    SecretKey::from_bytes(&[102; 32]),
                );
                dual_config.bind_addrs.push("[::1]:0".parse().unwrap());
                if reverse {
                    dual_config.bind_addrs.reverse();
                }
                let dual = owned::bind_endpoint(dual_config).await.unwrap();
                let single = owned::bind_endpoint(config(
                    if ipv4 { "127.0.0.1:0" } else { "[::1]:0" }
                        .parse()
                        .unwrap(),
                    SecretKey::from_bytes(&[103; 32]),
                ))
                .await
                .unwrap();
                let (client, server) = if dual_is_client {
                    (&dual, &single)
                } else {
                    (&single, &dual)
                };
                let mut target = server.addr();
                target.addrs.retain(
                    |addr| matches!(addr, TransportAddr::Ip(addr) if addr.is_ipv4() == ipv4),
                );
                let result = tokio::time::timeout(Duration::from_secs(2), async {
                    let (a, b) = tokio::join!(client.connect(target, rds_core::ALPN), async {
                        server.accept().await.unwrap().await
                    });
                    let a = a.unwrap();
                    let b = b.unwrap();
                    a.send_datagram(b"outbound".to_vec().into()).unwrap();
                    assert_eq!(&b.read_datagram().await.unwrap()[..], b"outbound");
                    b.send_datagram(b"inbound".to_vec().into()).unwrap();
                    assert_eq!(&a.read_datagram().await.unwrap()[..], b"inbound");
                })
                .await;
                dual.close().await;
                single.close().await;
                assert!(
                    result.is_ok(),
                    "mixed socket routing failed: reverse={reverse}, ipv4={ipv4}, dual_is_client={dual_is_client}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_families_do_not_consume_the_candidate_budget() {
    let server = owned::bind_endpoint(config(
        "[::1]:0".parse().unwrap(),
        SecretKey::from_bytes(&[104; 32]),
    ))
    .await
    .unwrap();
    let client = owned::bind_endpoint(config(
        "[::1]:0".parse().unwrap(),
        SecretKey::from_bytes(&[105; 32]),
    ))
    .await
    .unwrap();
    let mut target = server.addr();
    // Documentation addresses cannot be sent on this IPv6-only endpoint.
    // Filtering must precede the eight-candidate cap.
    for port in 10000..10020 {
        target
            .addrs
            .insert(TransportAddr::Ip(([192, 0, 2, 1], port).into()));
    }
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let (a, b) = tokio::join!(client.connect(target, rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        let a = a.unwrap();
        let b = b.unwrap();
        a.send_datagram(b"supported candidate".to_vec().into())
            .unwrap();
        assert_eq!(
            &b.read_datagram().await.unwrap()[..],
            b"supported candidate"
        );
    })
    .await;
    client.close().await;
    server.close().await;
    assert!(
        result.is_ok(),
        "unsupported candidates exhausted the dial budget"
    );
}
