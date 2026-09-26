//! Warm secondary relay: two attachments on one endpoint, primary
//! drain/kill during traffic, the same connection migrating onto the
//! surviving slot without re-authentication.
#![cfg(feature = "owned-relay")]

use std::time::{Duration, Instant};

use rds_net::{Backend, EndpointAddr, EndpointConfig, SecretKey, TransportAddr};

const ALPN: &[u8] = b"rds/0";
/// Relay drain grace is 2s; migration must beat it by a wide margin.
const MIGRATION_BUDGET: Duration = Duration::from_secs(10);

fn key(seed: u8) -> SecretKey {
    SecretKey::from_bytes(&[seed; 32])
}

async fn relay(seed: u8) -> rds_relay::server::Relay {
    rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key(seed)),
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap()
}

async fn endpoint(seed: u8, relays: Vec<EndpointAddr>) -> rds_net::Endpoint {
    rds_net::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        secret_key: Some(key(seed)),
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        alpns: vec![ALPN.to_vec()],
        relay_endpoints: relays,
        // Relay→relay migration must be proven, not vacuously satisfied
        // by a direct-path migration — so direct candidates are refused.
        transports: rds_net::Transports::RelayOnly,
        ..Default::default()
    })
    .await
    .expect("bind endpoint")
}

/// Keep only the relay candidates — the dialer cannot reach the peer
/// directly, so every path traverses an attached tunnel.
fn relay_only(mut addr: EndpointAddr) -> EndpointAddr {
    addr.addrs.retain(|a| matches!(a, TransportAddr::Relay(_)));
    addr
}

async fn echo(a: &rds_net::Connection, b: &rds_net::Connection, payload: &[u8]) -> bool {
    if a.send_datagram(payload.to_vec().into()).is_err() {
        return false;
    }
    let Ok(Ok(inbound)) = tokio::time::timeout(Duration::from_secs(2), b.read_datagram()).await
    else {
        return false;
    };
    if inbound != payload {
        return false;
    }
    if b.send_datagram(inbound.to_vec().into()).is_err() {
        return false;
    }
    matches!(
        tokio::time::timeout(Duration::from_secs(2), a.read_datagram()).await,
        Ok(Ok(reply)) if reply == inbound
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_secondary_advertises_both_relay_urls() {
    let r0 = relay(50).await;
    let r1 = relay(51).await;
    let a = endpoint(52, vec![r0.endpoint_addr(), r1.endpoint_addr()]).await;
    let relayed = a
        .addr()
        .addrs
        .iter()
        .filter(|a| matches!(a, TransportAddr::Relay(_)))
        .count();
    assert_eq!(relayed, 2, "both attachments must be advertised");
    assert_eq!(r0.endpoints(), 1);
    assert_eq!(r1.endpoints(), 1);
    a.close().await;
    r0.close().await.unwrap();
    r1.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_migrates_to_secondary_when_primary_drains() {
    failover_holds(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_migrates_to_secondary_when_primary_is_killed() {
    failover_holds(false).await;
}

async fn failover_holds(graceful: bool) {
    let r0 = relay(53).await;
    let r1 = relay(54).await;
    let attachments = || vec![r0.endpoint_addr(), r1.endpoint_addr()];
    let a = endpoint(55, attachments()).await;
    let b = endpoint(56, attachments()).await;
    let target = relay_only(b.addr());
    assert_eq!(
        target.addrs.len(),
        2,
        "the ticket must offer both relay paths"
    );

    let (outgoing, incoming) = tokio::join!(a.connect(target, ALPN), async {
        b.accept().await.unwrap().await
    });
    let outgoing = outgoing.unwrap();
    let incoming = incoming.unwrap();
    assert!(echo(&outgoing, &incoming, b"pre-failover").await);
    // Fail whichever relay slot the outgoing connection's egress path
    // rides. `selected` is suppressed while both relay paths stay
    // Available, so identify egress by datagram-counter delta instead.
    let sent0: std::collections::BTreeMap<u64, u64> = outgoing
        .path_stats()
        .iter()
        .map(|s| (s.path_id, s.sent))
        .collect();
    for probe in 0..3u8 {
        assert!(echo(&outgoing, &incoming, &[probe, 0x77]).await);
    }
    let active_slot = outgoing
        .path_stats()
        .iter()
        .filter(|s| s.via_relay)
        .max_by_key(|s| s.sent - sent0.get(&s.path_id).copied().unwrap_or(0))
        .and_then(|s| s.relay_slot)
        .expect("outgoing has no relay path to fail");
    let survivor_slot = active_slot ^ 1;
    let (primary, survivor) = match active_slot {
        0 => (r0, r1),
        _ => (r1, r0),
    };

    // Kill vs. drain: drain announces and keeps the grace window open;
    // kill removes the tunnel outright. Either way traffic must resume
    // on the sibling relay on this same connection.
    let stop = if graceful {
        tokio::spawn(async move { primary.drain().await })
    } else {
        tokio::spawn(async move { primary.close().await })
    };

    // Drive echo rounds until traffic lands on the surviving relay.
    let started = Instant::now();
    let mut migrated = false;
    let mut gap = Duration::ZERO;
    let mut round = 0u8;
    while started.elapsed() < MIGRATION_BUDGET {
        if echo(&outgoing, &incoming, &[round, 0xAB]).await {
            let now = outgoing.current_path_stats().and_then(|p| p.relay_slot);
            if now == Some(survivor_slot) {
                migrated = true;
                break;
            }
        } else {
            gap += Duration::from_millis(20);
        }
        round = round.wrapping_add(1);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        migrated,
        "selected path never moved off the failed relay slot"
    );
    // Post-migration traffic is steady on the surviving attachment.
    for round in 0..3u8 {
        assert!(echo(&outgoing, &incoming, &[round, 0xCD]).await);
    }
    assert!(outgoing.close_kind().is_none(), "connection stayed open");
    assert!(
        gap < MIGRATION_BUDGET,
        "interruption {gap:?} exceeded the budget"
    );
    assert!(survivor.stats().0 > 0, "secondary relay carried traffic");

    outgoing.close(0u32.into(), b"done");
    a.close().await;
    b.close().await;
    let _ = stop.await;
    survivor.close().await.unwrap();
}
