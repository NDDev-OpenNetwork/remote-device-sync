//! A requested service protocol must be the protocol negotiated by TLS.
#![cfg(feature = "transport-noq")]
use rds_net::backends::noq::{self as owned, Connection};
use rds_net::{Backend, EndpointConfig};
use std::time::Duration;
const FIRST: &[u8] = b"rds-test-first/0";
const SECOND: &[u8] = b"rds-test-second/0";
fn config(alpns: &[&[u8]]) -> EndpointConfig {
    EndpointConfig {
        backend: Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: alpns.iter().map(|p| p.to_vec()).collect(),
        ..Default::default()
    }
}
fn protocol(connection: &Connection) -> Vec<u8> {
    connection
        .inner()
        .handshake_data()
        .unwrap()
        .downcast::<noq::crypto::rustls::HandshakeData>()
        .unwrap()
        .protocol
        .unwrap()
}
#[tokio::test]
async fn requested_protocol_overrides_the_servers_other_preference() {
    let client = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let server = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let requested = [SECOND, FIRST, SECOND, FIRST];
    let mut observed = Vec::new();
    for requested in requested {
        let (a, b) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(client.connect(server.addr(), requested), async {
                server.accept().await.unwrap().await
            })
        })
        .await
        .unwrap();
        let a = a.unwrap();
        let b = b.unwrap();
        observed.push((protocol(&a), protocol(&b)));
        a.send_datagram(requested.to_vec().into()).unwrap();
        let payload = tokio::time::timeout(Duration::from_secs(2), b.read_datagram())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&payload[..], requested);
        a.close(0u32.into(), b"test iteration complete");
        b.close(0u32.into(), b"test iteration complete");
    }
    client.close().await;
    server.close().await;
    for (actual, wanted) in observed.into_iter().zip(requested) {
        assert_eq!(actual, (wanted.to_vec(), wanted.to_vec()));
    }
}
#[tokio::test]
async fn missing_requested_protocol_cannot_fall_back_to_another() {
    let client = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let server = owned::bind_endpoint(config(&[FIRST])).await.unwrap();
    let (a, b) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(client.connect(server.addr(), SECOND), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    client.close().await;
    server.close().await;
    assert!(
        a.is_err() && b.is_err(),
        "unsupported requested protocol silently connected using another"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_protocol_requests_cannot_change_each_others_offer() {
    let client = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let server = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let (a, b, incoming) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(
            client.connect(server.addr(), FIRST),
            client.connect(server.addr(), SECOND),
            async {
                let first = server.accept().await.unwrap();
                let second = server.accept().await.unwrap();
                let (a, b) = tokio::join!(first, second);
                (a.unwrap(), b.unwrap())
            }
        )
    })
    .await
    .unwrap();
    let a = a.unwrap();
    let b = b.unwrap();
    let actual = (protocol(&a), protocol(&b));
    let mut accepted = vec![protocol(&incoming.0), protocol(&incoming.1)];
    accepted.sort();
    client.close().await;
    server.close().await;
    assert_eq!(actual, (FIRST.to_vec(), SECOND.to_vec()));
    let mut expected = vec![FIRST.to_vec(), SECOND.to_vec()];
    expected.sort();
    assert_eq!(accepted, expected);
}

#[tokio::test]
async fn immediate_candidate_failure_preserves_the_requested_protocol() {
    let client = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let server = owned::bind_endpoint(config(&[FIRST, SECOND]))
        .await
        .unwrap();
    let invalid = "127.0.0.1:0".parse().unwrap();
    let mut target = server.addr();
    target.addrs.insert(rds_net::TransportAddr::Ip(invalid));
    assert_eq!(owned::policy::ip_candidates(&target)[0], invalid);
    // noq rejects remote port zero synchronously, before starting the valid
    // candidate. This gives a deterministic failure-before-success ordering.
    let (a, b) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(client.connect(target, SECOND), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    let a = a.unwrap();
    let b = b.unwrap();
    let actual = (protocol(&a), protocol(&b));
    client.close().await;
    server.close().await;
    assert_eq!(actual, (SECOND.to_vec(), SECOND.to_vec()));
}
