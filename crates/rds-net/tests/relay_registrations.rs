//! Real, isolated relay registrations; no external hosts or ambient desktop.

use std::{collections::BTreeSet, time::Duration};

use rds_net::{EndpointConfig, PathPreference, Transports, bind_endpoint};
use tokio::time::{Instant, sleep, timeout};

async fn relay() -> (iroh_relay::server::Server, String) {
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new(
        "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let server = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap());
    (server, url)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_idle_registrations_survive_two_relay_failures_on_the_same_stream() {
    let (first, first_url) = relay().await;
    let (second, second_url) = relay().await;
    let (third, third_url) = relay().await;
    let config = || EndpointConfig {
        keep_relays_connected: true,
        prefer_relay_order: true,
        transports: Transports::RelayOnly,
        path_preference: PathPreference::Latency,
        ..EndpointConfig::default()
            .with_relays([&first_url, &second_url, &third_url])
            .unwrap()
    };
    let server = rds_net::backends::iroh::bind_endpoint(config())
        .await
        .unwrap();
    let client = rds_net::backends::iroh::bind_endpoint(config())
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        while server.addr().addrs.len() != 3 || client.addr().addrs.len() != 3 {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("three ready registrations before idle");
    // Exceed the upstream 60-second inactive cleanup without peer traffic.
    sleep(Duration::from_secs(65)).await;
    assert_eq!(server.addr().addrs.len(), 3);
    assert_eq!(client.addr().addrs.len(), 3);

    let ticket = server.addr();
    let (outgoing, incoming) = timeout(Duration::from_secs(5), async {
        tokio::join!(client.connect(ticket, rds_core::ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await
    .unwrap();
    let outgoing = outgoing.unwrap();
    let incoming = incoming.unwrap();
    let (mut tx, mut rx) = outgoing.open_bi().await.unwrap();
    tx.write_all(&[0]).await.unwrap();
    let (mut echo_tx, mut echo_rx) = incoming.accept_bi().await.unwrap();
    let echo = tokio::spawn(async move {
        let mut byte = [0];
        while echo_rx.read_exact(&mut byte).await.is_ok() {
            echo_tx.write_all(&byte).await.unwrap();
        }
    });
    let mut byte = [0];
    timeout(Duration::from_secs(3), rx.read_exact(&mut byte))
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(10), async {
        while outgoing.paths().iter().count() < 3 || incoming.paths().iter().count() < 3 {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("three live relay paths must exist before fault injection");

    for (value, failed, expected_url) in [(1, first, &first_url), (2, second, &second_url)] {
        let expected: iroh::TransportAddr =
            iroh::TransportAddr::Relay(expected_url.parse().unwrap());
        timeout(Duration::from_secs(10), async {
            while outgoing
                .paths()
                .iter()
                .find(|path| path.is_selected())
                .is_none_or(|path| path.remote_addr() != &expected)
                || incoming
                    .paths()
                    .iter()
                    .find(|path| path.is_selected())
                    .is_none_or(|path| path.remote_addr() != &expected)
            {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("fault injection requires the expected tier selected at both peers");
        let prior = outgoing
            .paths()
            .iter()
            .find(|path| path.is_selected())
            .unwrap()
            .id();
        failed.shutdown().await.unwrap();
        let started = Instant::now();
        tx.write_all(&[value]).await.unwrap();
        timeout(Duration::from_secs(5), rx.read_exact(&mut byte))
            .await
            .expect("existing stream did not recover within five seconds")
            .unwrap();
        assert_eq!(byte, [value]);
        assert!(outgoing.close_reason().is_none());
        assert_ne!(
            outgoing
                .paths()
                .iter()
                .find(|path| path.is_selected())
                .expect("recovered selected path")
                .id(),
            prior,
            "fault must retire the selected tier, not merely an unused relay"
        );
        eprintln!(
            "relay fault {value}: same-stream response {} ms",
            started.elapsed().as_millis()
        );
    }
    timeout(Duration::from_secs(10), async {
        while server.addr().addrs.len() != 1 || client.addr().addrs.len() != 1 {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("failed registrations withdrawn");
    outgoing.close(0u32.into(), b"fixture complete");
    incoming.close(0u32.into(), b"fixture complete");
    echo.await.unwrap();
    client.close().await;
    server.close().await;
    third.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_advertises_only_home_relay_and_opt_in_is_bounded() {
    let (a, a_url) = relay().await;
    let (b, b_url) = relay().await;
    let config = EndpointConfig {
        transports: Transports::RelayOnly,
        ..EndpointConfig::default()
            .with_relays([&a_url, &b_url])
            .unwrap()
    };
    let endpoint = bind_endpoint(config).await.unwrap();
    timeout(Duration::from_secs(10), endpoint.online())
        .await
        .unwrap();
    assert_eq!(endpoint.addr().addrs.len(), 1);
    endpoint.close().await;
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    let mut invalid = EndpointConfig {
        keep_relays_connected: true,
        ..Default::default()
    };
    assert!(invalid.validate().is_err());
    invalid.relays = (1..=4)
        .map(|i| format!("https://relay{i}.example").parse().unwrap())
        .collect();
    assert!(invalid.validate().is_err());
    assert_eq!(invalid.relays.iter().collect::<BTreeSet<_>>().len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_warm_relay_withdraws_readiness_without_expanding_the_initial_budget() {
    use n0_watcher::Watcher as _;
    let (a, a_url) = relay().await;
    let (b, b_url) = relay().await;
    let a_url: iroh::RelayUrl = a_url.parse().unwrap();
    let b_url: iroh::RelayUrl = b_url.parse().unwrap();
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([
            a_url.clone(),
            b_url.clone(),
        ])))
        .keep_relays_connected(true)
        .clear_ip_transports()
        .bind()
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        while endpoint.addr().addrs.len() != 2 {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    timeout(Duration::from_secs(10), endpoint.online())
        .await
        .unwrap();
    assert_eq!(endpoint.home_relay_status().get().len(), 1);
    endpoint.remove_relay(&b_url).await.unwrap();
    timeout(Duration::from_secs(2), async {
        while endpoint
            .addr()
            .addrs
            .contains(&iroh::TransportAddr::Relay(b_url.clone()))
        {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("removed registration remained advertised");
    let (c, c_url) = relay().await;
    let c_url: iroh::RelayUrl = c_url.parse().unwrap();
    endpoint
        .insert_relay(
            c_url.clone(),
            std::sync::Arc::new(iroh::RelayConfig::from(c_url)),
        )
        .await;
    // Dynamic insertion must not advertise an unregistered warm address.
    assert!(endpoint.addr().addrs.len() <= 1);
    let _ = endpoint.home_relay_status().get();
    endpoint.close().await;
    assert!(
        endpoint.addr().addrs.is_empty(),
        "closed actors retained registration readiness"
    );
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}
