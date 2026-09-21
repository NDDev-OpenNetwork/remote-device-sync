//! End-to-end: in-process iroh relay + agent + client over the real QUIC
//! path. Proves rendezvous, allowlist auth, ping RTT and TCP forwarding.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use rds_agent::{Agent, AgentPolicy};
use rds_transport::{EndpointConfig, Ticket, bind_endpoint};
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
    let target = rds_transport::parse_target(&agent_ticket.to_string()).unwrap();
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
    policy.allow.insert(iroh::SecretKey::generate().public()); // never binds
    let agent = Agent::new(agent_ep, policy);
    let ticket = Ticket::of(&agent.endpoint);
    let _task = tokio::spawn({
        let agent = Arc::new(agent);
        async move { agent.run().await }
    });

    let conn = rds_cli::connect(
        &stranger,
        rds_transport::parse_target(&ticket.to_string()).unwrap(),
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
        relay: None,
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
        rds_transport::parse_target(&ticket.to_string()).unwrap(),
    )
    .await
    .unwrap();
    rds_cli::ping(&conn, 7).await.unwrap();
}
