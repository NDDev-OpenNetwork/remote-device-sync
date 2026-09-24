//! End-to-end: in-process iroh relay + agent + client over the real QUIC
//! path. Proves rendezvous, allowlist auth, ping RTT and TCP forwarding.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use rds_agent::{Agent, AgentPolicy};
use rds_net::{EndpointConfig, Ticket, bind_endpoint};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn test_relay() -> (Server, String) {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(
        "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
    ));
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap());
    (server, url)
}

async fn tcp_echo() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                while let Ok(n) = sock.read(&mut buf).await {
                    if n == 0 || sock.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_and_tcp_forward_over_relay() {
    let (_relay, relay_url) = test_relay().await;
    let echo_port = tcp_echo().await;

    // Agent side.
    let agent_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    agent_ep.online().await;

    // Client side.
    let client_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    client_ep.online().await;

    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), echo_port));
    policy.allow.insert(client_ep.id());
    let agent = Agent::new(agent_ep, policy);
    let agent_ticket = Ticket::of(&agent.endpoint);
    let agent_task = tokio::spawn({
        let agent = Arc::new(agent);
        let a = agent.clone();
        async move { a.run().await }
    });

    // Resolve through the ticket the agent published.
    let target = rds_net::parse_target(&agent_ticket.to_string()).unwrap();
    let conn = rds_cli::connect(&client_ep, target).await.unwrap();
    assert_eq!(conn.remote_id(), agent_ticket.endpoint_id());

    // RTT probe.
    let rtt = rds_cli::ping(&conn, 42).await.unwrap();
    assert!(rtt < Duration::from_secs(5));

    // TCP forward through the tunnel to the echo server.
    let (mut send, mut recv) = rds_cli::open_tcp(&conn, "127.0.0.1", echo_port)
        .await
        .unwrap();
    send.write_all(b"hello over rds").await.unwrap();
    let mut buf = vec![0u8; 14];
    recv.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello over rds");

    // A target outside the allowlist is refused at the service level.
    let err = rds_cli::open_tcp(&conn, "127.0.0.1", echo_port + 1).await;
    assert!(err.is_err(), "unexpectedly allowed off-policy target");

    agent_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthorized_peer_is_rejected() {
    let (_relay, relay_url) = test_relay().await;

    let agent_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    agent_ep.online().await;
    let stranger = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    stranger.online().await;

    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 22));
    policy.allow.insert(rds_net::SecretKey::generate().public()); // never binds
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &stranger,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();
    // Handshake completes (any key can dial), but the first stream is cut.
    let result = tokio::time::timeout(Duration::from_secs(10), rds_cli::ping(&conn, 1)).await;
    match result {
        Err(_) => panic!("ping hung instead of failing"),
        Ok(r) => assert!(r.is_err(), "unauthorized peer got a pong"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_connection_without_relay() {
    // LAN-style path: no relay, direct UDP addresses in the ticket.
    let agent_ep = bind_endpoint(EndpointConfig {
        relays: Vec::new(),
        ..Default::default()
    })
    .await
    .unwrap();
    // RelayMode::Default endpoints still publish direct addrs; for a fully
    // offline check disable relays by constructing the ticket from addr().
    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    let client_ep = bind_endpoint(EndpointConfig::default()).await.unwrap();
    policy.allow.insert(client_ep.id());
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &client_ep,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();
    rds_cli::ping(&conn, 7).await.unwrap();
}

/// The `desktop` feature's service plumbing end-to-end: the client
/// sends `StreamHello::Desktop` on the control stream and the agent
/// answers — capabilities when a capture backend is live, an honest
/// `desktop unavailable` error headless or on macOS. Either way the
/// handshake must complete, never hang or desynchronize.
#[cfg(feature = "desktop")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn desktop_handshake_completes() {
    let (_relay, relay_url) = test_relay().await;
    let agent_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    agent_ep.online().await;
    let client_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    client_ep.online().await;

    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.allow.insert(client_ep.id());
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &client_ep,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();
    match tokio::time::timeout(
        Duration::from_secs(10),
        rds_desktop::client::DesktopSession::connect(&conn, 0, 30, rds_core::Codec::H264),
    )
    .await
    {
        Err(_) => panic!("desktop handshake hung"),
        Ok(Ok(session)) => assert!(!session.caps().codecs.is_empty()),
        Ok(Err(e)) => assert!(
            matches!(e, rds_desktop::DesktopError::Capture(_)),
            "unexpected desktop error: {e:?}"
        ),
    }
}

// ---- WS4: capability grants -------------------------------------------

use rds_core::grant::{Grant, GrantConstraints};
use rds_core::{HelloAck, ServiceKind, StreamHello, read_frame, write_frame};

fn issuer() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[77u8; 32])
}

/// A grant-mode agent halves: bound endpoint, policy with `issuers`
/// set (caller adds `allow` before `Agent::new`), issuer key, ticket.
async fn grant_agent(
    relay_url: &str,
) -> (
    rds_net::Endpoint,
    AgentPolicy,
    ed25519_dalek::SigningKey,
    Ticket,
) {
    let agent_ep = bind_endpoint(EndpointConfig::default().with_relay(relay_url).unwrap())
        .await
        .unwrap();
    agent_ep.online().await;
    let iss = issuer();
    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.issuers.insert(iss.verifying_key().to_bytes());
    policy.use_local_revocations(); // This fixture tests local grant rules; managed feeds have their own suite.
    let ticket = Ticket::of(&agent_ep);
    (agent_ep, policy, iss, ticket)
}

fn grant_for(
    iss: &ed25519_dalek::SigningKey,
    subject: rds_net::EndpointId,
    services: Vec<ServiceKind>,
    ttl: Duration,
) -> Grant {
    Grant::issue(
        iss,
        *subject.as_bytes(),
        services,
        ttl,
        GrantConstraints::default(),
    )
}

fn serve(agent: Agent) -> tokio::task::JoinHandle<()> {
    tokio::spawn({
        let agent = Arc::new(agent);
        async move {
            let _ = agent.run().await;
        }
    })
}

async fn client_ep(relay_url: &str) -> rds_net::Endpoint {
    let ep = bind_endpoint(EndpointConfig::default().with_relay(relay_url).unwrap())
        .await
        .unwrap();
    ep.online().await;
    ep
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_mode_valid_grant_opens_services() {
    let (_relay, relay_url) = test_relay().await;
    let client = client_ep(&relay_url).await;
    let (ep, mut policy, iss, ticket) = grant_agent(&relay_url).await;
    policy.allow.insert(client.id());
    let _task = serve(Agent::new(ep, policy));

    let grant = grant_for(
        &iss,
        client.id(),
        vec![ServiceKind::Ping],
        Duration::from_secs(120),
    );
    let conn = rds_cli::connect_authorized(
        &client,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
        &grant,
    )
    .await
    .unwrap();
    rds_cli::ping(&conn, 1).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streams_before_grant_are_rejected() {
    // G4: service streams raced ahead of the Authz stream must all be
    // refused — none may be served while the grant is unverified.
    let (_relay, relay_url) = test_relay().await;
    let client = client_ep(&relay_url).await;
    let (ep, mut policy, iss, ticket) = grant_agent(&relay_url).await;
    policy.allow.insert(client.id());
    let _task = serve(Agent::new(ep, policy));

    let conn = rds_cli::connect(&client, rds_net::parse_target(&ticket.to_string()).unwrap())
        .await
        .unwrap();

    // N service streams without a grant — every one refused.
    for i in 0..8 {
        match tokio::time::timeout(Duration::from_secs(10), rds_cli::ping(&conn, i)).await {
            Ok(Ok(_)) => panic!("ping served before grant verification"),
            Ok(Err(_)) => {}
            Err(_) => panic!("ping hung instead of failing"),
        }
    }

    // The same connection starts serving once the grant lands.
    let grant = grant_for(
        &iss,
        client.id(),
        vec![ServiceKind::Ping],
        Duration::from_secs(120),
    );
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &StreamHello::Authz(grant))
        .await
        .unwrap();
    match read_frame::<_, HelloAck>(&mut recv).await.unwrap() {
        HelloAck::Ok => {}
        other => panic!("grant rejected: {other:?}"),
    }
    rds_cli::ping(&conn, 99).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_and_wrong_service_grants_rejected() {
    let (_relay, relay_url) = test_relay().await;
    let client = client_ep(&relay_url).await;
    let (ep, mut policy, iss, ticket) = grant_agent(&relay_url).await;
    policy.allow.insert(client.id());
    let _task = serve(Agent::new(ep, policy));
    let target = rds_net::parse_target(&ticket.to_string()).unwrap();

    // Expired grant → Authz ack is an error.
    let expired = Grant::issue_at(
        &iss,
        rds_core::grant::GrantPayload {
            issuer: iss.verifying_key().to_bytes(),
            subject: *client.id().as_bytes(),
            nonce: 1,
            services: vec![ServiceKind::Ping],
            not_before: 1,
            expires_at: 2,
            constraints: GrantConstraints::default(),
        },
    );
    assert!(
        rds_cli::connect_authorized(&client, target.clone(), &expired)
            .await
            .is_err()
    );

    // Wrong-service grant: Ping granted, Tcp refused by scope.
    let grant = grant_for(
        &iss,
        client.id(),
        vec![ServiceKind::Ping],
        Duration::from_secs(120),
    );
    let conn = rds_cli::connect_authorized(&client, target.clone(), &grant)
        .await
        .unwrap();
    rds_cli::ping(&conn, 1).await.unwrap();
    assert!(rds_cli::open_tcp(&conn, "127.0.0.1", 9).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_grant_drops_live_and_new_connections() {
    let (_relay, relay_url) = test_relay().await;
    let client = client_ep(&relay_url).await;
    let (ep, mut policy, iss, ticket) = grant_agent(&relay_url).await;
    policy.allow.insert(client.id());
    let agent = Agent::new(ep, policy);
    let policy = agent.policy.clone();
    let _task = serve(agent);
    let target = rds_net::parse_target(&ticket.to_string()).unwrap();

    let grant = grant_for(
        &iss,
        client.id(),
        vec![ServiceKind::Ping],
        Duration::from_secs(120),
    );
    let conn = rds_cli::connect_authorized(&client, target.clone(), &grant)
        .await
        .unwrap();
    rds_cli::ping(&conn, 1).await.unwrap();

    // Push the grant id onto the denylist: the live connection dies…
    policy.revoke(grant.id());
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if rds_cli::ping(&conn, 2).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(closed.is_ok(), "revoked grant kept serving");

    // …and a fresh connection presenting it is refused at Authz.
    assert!(
        rds_cli::connect_authorized(&client, target, &grant)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grant_replay_on_concurrent_connection_rejected() {
    let (_relay, relay_url) = test_relay().await;
    let client = client_ep(&relay_url).await;
    let (ep, mut policy, iss, ticket) = grant_agent(&relay_url).await;
    policy.allow.insert(client.id());
    let _task = serve(Agent::new(ep, policy));
    let target = rds_net::parse_target(&ticket.to_string()).unwrap();

    let grant = grant_for(
        &iss,
        client.id(),
        vec![ServiceKind::Ping],
        Duration::from_secs(120),
    );
    let conn1 = rds_cli::connect_authorized(&client, target.clone(), &grant)
        .await
        .unwrap();
    rds_cli::ping(&conn1, 1).await.unwrap();

    // Same grant bytes on a second live connection = replay.
    assert!(
        rds_cli::connect_authorized(&client, target, &grant)
            .await
            .is_err()
    );
}

// ---- WS6: sync service ------------------------------------------------

#[test]
fn revocations_survive_without_watchers() {
    let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.replace_denylist(std::collections::HashSet::from([[1; 32]]));
    assert!(policy.denied().contains(&[1; 32]));
    policy.revoke([2; 32]);
    assert_eq!(policy.denied().len(), 2);

    let receiver = policy.denylist.subscribe();
    drop(receiver);
    policy.revoke([3; 32]);
    assert_eq!(policy.denied().len(), 3);
}

#[test]
fn concurrent_revocations_are_not_lost() {
    let policy = Arc::new(AgentPolicy::ssh_only(("127.0.0.1".into(), 9)));
    let _receiver = policy.denylist.subscribe();
    let barrier = Arc::new(std::sync::Barrier::new(16));
    std::thread::scope(|scope| {
        for i in 0..16 {
            let policy = policy.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                policy.revoke([i; 32]);
            });
        }
    });
    assert_eq!(policy.denied().len(), 16);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_authz_reply_closes_connection_and_releases_grant() {
    let (_relay, relay_url) = test_relay().await;
    let client = client_ep(&relay_url).await;
    let (ep, mut policy, iss, ticket) = grant_agent(&relay_url).await;
    policy.allow.insert(client.id());
    let agent = Agent::new(ep.clone(), policy);
    let policy = agent.policy.clone();
    let task = serve(agent);
    let conn = rds_cli::connect(&client, rds_net::parse_target(&ticket.to_string()).unwrap())
        .await
        .unwrap();
    let grant = grant_for(
        &iss,
        client.id(),
        vec![ServiceKind::Ping],
        Duration::from_secs(60),
    );
    let grant_id = grant.id();
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &StreamHello::Authz(grant))
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    // Let the agent accept the stream, but withhold the Authz body
    // until STOP_SENDING has reached its reply half.
    send.write_all(&encoded[..5]).await.unwrap();
    recv.stop(0u32.into()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    send.write_all(&encoded[5..]).await.unwrap();
    send.finish().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !conn.is_closed() || policy.active_grants.lock().unwrap().contains(&grant_id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed Authz ACK retained access or leaked its reservation");
    assert!(rds_cli::ping(&conn, 1).await.is_err());
    client.close().await;
    ep.close().await;
    task.abort();
}

/// One sync session per connection: a second `Sync` stream while the
/// first is open is refused, and the slot frees when the first ends.
/// Also proves `Info` advertises `Sync` only when `sync_dir` is set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_one_session_per_connection() {
    let (_relay, relay_url) = test_relay().await;
    let agent_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    agent_ep.online().await;
    let client_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    client_ep.online().await;

    let sync_root = std::env::temp_dir().join(format!("rds-agent-sync-{}", std::process::id()));
    std::fs::create_dir_all(&sync_root).unwrap();
    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.allow.insert(client_ep.id());
    policy.sync_dir = Some(sync_root.clone());
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &client_ep,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();

    // Advertised in the service list.
    let info = rds_cli::info(&conn).await.unwrap();
    assert!(
        info.services.contains(&ServiceKind::Sync),
        "sync not advertised: {:?}",
        info.services
    );

    // First session takes the slot — keep its streams alive without an
    // Offer, so the server handler stays parked inside `serve`.
    let first = rds_cli::open_sync(&conn).await.unwrap();
    let second = rds_cli::open_sync(&conn).await;
    assert!(second.is_err(), "concurrent sync session was accepted");

    // Dropping the first session frees the slot for a third.
    drop(first);
    let mut third = None;
    for _ in 0..50 {
        match rds_cli::open_sync(&conn).await {
            Ok(s) => {
                third = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    assert!(third.is_some(), "sync slot never released");

    let _ = std::fs::remove_dir_all(&sync_root);
}

/// Sync refused cleanly when the agent has no `sync_dir` configured —
/// the service is off, not broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_unconfigured_is_refused() {
    let (_relay, relay_url) = test_relay().await;
    let agent_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    agent_ep.online().await;
    let client_ep = bind_endpoint(EndpointConfig::default().with_relay(&relay_url).unwrap())
        .await
        .unwrap();
    client_ep.online().await;

    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.allow.insert(client_ep.id());
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &client_ep,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();
    let info = rds_cli::info(&conn).await.unwrap();
    assert!(!info.services.contains(&ServiceKind::Sync));
    assert!(rds_cli::open_sync(&conn).await.is_err());
}

/// Same direct-path flow on the owned `noq` backend: agent and client
/// both bind `Backend::Noq` — exercises the facade end to end, not just
/// the backend in isolation.
#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_connection_noq_backend() {
    let config = || EndpointConfig {
        backend: rds_net::Backend::Noq,
        relays: Vec::new(),
        ..Default::default()
    };
    let agent_ep = bind_endpoint(config()).await.unwrap();
    let client_ep = bind_endpoint(config()).await.unwrap();
    let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
    policy.allow.insert(client_ep.id());
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &client_ep,
        rds_net::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();
    rds_cli::ping(&conn, 7).await.unwrap();
}
