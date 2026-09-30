use rds_net::{EndpointConfig, bind_endpoint};

#[tokio::test]
async fn explicit_loopback_bind_never_advertises_an_unspecified_family() {
    let endpoint = bind_endpoint(EndpointConfig {
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    for _ in 0..10 {
        for address in endpoint.addr().addrs {
            if let rds_net::TransportAddr::Ip(address) = address {
                assert!(address.ip().is_loopback(), "unexpected interface {address}");
                assert!(address.is_ipv4(), "an unspecified IPv6 socket was retained");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    endpoint.close().await;
}

#[tokio::test]
async fn iroh_extra_bind_addresses_are_not_silently_ignored() {
    let result = bind_endpoint(EndpointConfig {
        discovery: false,
        bind_addrs: vec![
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.2:0".parse().unwrap(),
        ],
        ..Default::default()
    })
    .await;
    match result {
        Ok(endpoint) => {
            endpoint.close().await;
            panic!("iroh accepted a bind address that it never used");
        }
        Err(error) => assert!(
            error.downcast_ref::<rds_net::ConfigError>().is_some(),
            "{error}"
        ),
    }
}

#[cfg(feature = "transport-noq")]
#[tokio::test]
async fn owned_transport_rejects_iroh_relay_urls() {
    let result = bind_endpoint(EndpointConfig {
        backend: rds_net::Backend::Noq,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        relays: vec!["http://127.0.0.1:9".parse().unwrap()],
        ..Default::default()
    })
    .await;
    match result {
        Ok(endpoint) => {
            endpoint.close().await;
            panic!("owned transport accepted an iroh relay URL that it never used");
        }
        Err(error) => assert!(
            error.downcast_ref::<rds_net::ConfigError>().is_some(),
            "{error}"
        ),
    }
}
