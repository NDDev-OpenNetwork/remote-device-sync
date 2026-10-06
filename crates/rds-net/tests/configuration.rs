use rds_net::{EndpointConfig, bind_endpoint};

#[test]
fn persistent_relay_file_requires_bounded_custom_origins_before_bind() {
    use rds_net::config::EndpointSettings;
    let legacy = EndpointSettings::from_json(br#"{"schema_version":1}"#).unwrap();
    assert!(!legacy.keep_relays_connected);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("keep_relays_connected")
    );
    for relay in [
        r#"{"mode":"default"}"#,
        r#"{"mode":"disabled"}"#,
        r#"{"mode":"iroh","urls":["https://a.example","https://b.example","https://c.example","https://d.example"]}"#,
    ] {
        let json =
            format!(r#"{{"schema_version":1,"keep_relays_connected":true,"relay":{relay}}}"#);
        assert!(
            EndpointSettings::from_json(json.as_bytes())
                .unwrap()
                .into_endpoint()
                .is_err()
        );
    }
    let valid = EndpointSettings::from_json(br#"{"schema_version":1,"keep_relays_connected":true,"relay":{"mode":"iroh","urls":["https://a.example","https://b.example","https://c.example"]}}"#).unwrap().into_endpoint().unwrap();
    assert!(valid.keep_relays_connected);
}

#[tokio::test]
async fn both_congestion_choices_transfer_complete_payloads_on_each_backend() {
    let backends = [
        rds_net::Backend::Iroh,
        #[cfg(feature = "transport-noq")]
        rds_net::Backend::Noq,
    ];
    for backend in backends {
        for congestion_control in [
            rds_net::CongestionControl::Bbr3,
            rds_net::CongestionControl::Cubic,
        ] {
            let config = || EndpointConfig {
                backend,
                congestion_control,
                discovery: false,
                bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                ..Default::default()
            };
            let server = bind_endpoint(config()).await.unwrap();
            let client = bind_endpoint(config()).await.unwrap();
            let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
                    server.accept().await.unwrap().await
                })
            })
            .await
            .unwrap();
            let (a, b) = (a.unwrap(), b.unwrap());
            let body = vec![0x5a; 256 * 1024];
            let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(
                    async {
                        let (mut tx, mut rx) = a.open_bi().await.unwrap();
                        tx.write_all(&body).await.unwrap();
                        tx.finish().unwrap();
                        let mut echoed = vec![0; body.len()];
                        rx.read_exact(&mut echoed).await.unwrap();
                        echoed
                    },
                    async {
                        let (mut tx, mut rx) = b.accept_bi().await.unwrap();
                        let mut bytes = vec![0; body.len()];
                        rx.read_exact(&mut bytes).await.unwrap();
                        tx.write_all(&bytes).await.unwrap();
                        tx.finish().unwrap();
                        bytes
                    }
                )
            })
            .await
            .unwrap();
            assert_eq!(sent, body);
            assert_eq!(received, body);
            a.close(0u32.into(), b"qualification complete");
            b.close(0u32.into(), b"qualification complete");
            client.close().await;
            server.close().await;
        }
    }
}

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
