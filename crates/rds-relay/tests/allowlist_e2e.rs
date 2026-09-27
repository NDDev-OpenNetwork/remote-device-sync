//! End-to-end: the iroh relay's endpoint allowlist admits registered
//! identities and denies unknown ones, observed client-side through the
//! endpoint's home-relay connection status.
use std::{net::SocketAddr, str::FromStr, time::Duration};

use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, SecretKey, Watcher, endpoint::presets};

fn key(seed: u8) -> SecretKey {
    SecretKey::from_bytes(&[seed; 32])
}

fn relay_url(addr: SocketAddr) -> RelayUrl {
    RelayUrl::from_str(&format!("http://{addr}")).unwrap()
}

async fn endpoint(key: SecretKey, url: &RelayUrl) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .relay_mode(RelayMode::Custom(RelayMap::from_iter([url.clone()])))
        .bind()
        .await
        .unwrap()
}

#[tokio::test]
async fn allowlist_admits_registered_and_denies_unknown_endpoints() {
    let allowed = key(17);
    let allowed_id = rds_core::EndpointId::from_bytes(allowed.public().as_bytes()).unwrap();
    let relay = rds_relay::serve("127.0.0.1:0".parse().unwrap(), vec![allowed_id], None)
        .await
        .unwrap();
    let url = relay_url(relay.http_addr().expect("http relay listener"));

    let admitted = endpoint(allowed, &url).await;
    let denied = endpoint(key(42), &url).await;

    // The admitted identity reaches its home relay.
    let mut admitted_status = admitted.home_relay_status();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if admitted_status.get().iter().any(|s| s.is_connected()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("allowed endpoint never reached its home relay");

    // An unlisted identity is rejected at relay authentication: it reports an
    // auth denial and must never appear connected.
    let mut denied_status = denied.home_relay_status();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = denied_status.get();
            assert!(
                !status.iter().any(|s| s.is_connected()),
                "denied endpoint must never reach its home relay"
            );
            if status.iter().any(|s| s.auth_denied_reason().is_some()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("denied endpoint never reported a relay auth denial");
}
