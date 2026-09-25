use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_datagrams_are_charged_before_validation() {
    tokio::time::timeout(Duration::from_secs(10), malformed_case())
        .await
        .unwrap();
}

async fn malformed_case() {
    let config = || EndpointConfig {
        backend: rds_net::Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![proto::RELAY_ALPN.to_vec()],
        ..Default::default()
    };
    let relay = serve(config(), Vec::new()).await.unwrap();
    let client = rds_noq::bind_endpoint(config()).await.unwrap();
    let conn = client
        .connect(relay.endpoint_addr(), proto::RELAY_ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_control(&mut send, &RelayControl::Register)
        .await
        .unwrap();
    assert!(matches!(
        read_control(&mut recv).await.unwrap(),
        RelayControl::Registered
    ));
    let source = relay
        .state
        .conns
        .lock()
        .unwrap()
        .get(&client.id())
        .unwrap()
        .clone();
    // A finite deterministic public fixture search, independent of RNG/keys.
    let invalid_key = (0..=255u8)
        .map(|byte| [byte; 32])
        .find(|bytes| EndpointId::from_bytes(bytes).is_err())
        .unwrap();
    let mut observations = Vec::new();
    for frame in [Vec::new(), vec![1, 2, 3], invalid_key.to_vec()] {
        let before = source.bucket.lock().unwrap().last;
        let dropped = relay.stats().1;
        conn.send_datagram(frame.into()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while relay.stats().1 == dropped {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fixture datagram did not reach the relay");
        observations.push(source.bucket.lock().unwrap().last > before);
    }
    let (_, stopped) = tokio::join!(client.close(), relay.close());
    stopped.unwrap();
    assert_eq!(relay.lifecycle_stats().active_connections, 0);
    assert_eq!(relay.stats().0, 0);
    assert_eq!(
        observations,
        vec![true; 3],
        "malformed input bypassed admission accounting"
    );
}
