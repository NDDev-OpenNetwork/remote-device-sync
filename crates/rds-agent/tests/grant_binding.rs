//! Destination binding and idle expiry over real loopback transports.
use std::{sync::Arc, time::Duration};

use ed25519_dalek::SigningKey;
use rds_agent::{Agent, AgentPolicy};
use rds_core::{
    ServiceKind,
    grant::{Grant, GrantConstraints},
};
use rds_net::{Backend, Endpoint, EndpointConfig};

struct Serving {
    agent: Arc<Agent>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Serving {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Serving {
    async fn new(backend: Backend, client: &Endpoint, issuer: &SigningKey) -> Self {
        Self::with_target(backend, client, issuer, "127.0.0.1:9".parse().unwrap()).await
    }
    async fn with_target(
        backend: Backend,
        client: &Endpoint,
        issuer: &SigningKey,
        target: std::net::SocketAddr,
    ) -> Self {
        let endpoint = endpoint(backend).await;
        let mut policy = AgentPolicy::ssh_only((target.ip().to_string(), target.port()));
        policy.allow.insert(client.id());
        policy.issuers.insert(issuer.verifying_key().to_bytes());
        policy.use_local_revocations();
        let agent = Arc::new(Agent::new(endpoint, policy).with_limits(
            rds_agent::AgentLimits::new(32.try_into().unwrap(), 2.try_into().unwrap()),
        ));
        let runner = agent.clone();
        let task = tokio::spawn(async move {
            runner.run().await.unwrap();
        });
        Self { agent, task }
    }
    async fn close(mut self) {
        self.agent.endpoint.close().await;
        tokio::time::timeout(Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn endpoint(backend: Backend) -> Endpoint {
    rds_net::bind_endpoint(EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap()
}

async fn destination_binding(backend: Backend) {
    let client = endpoint(backend).await;
    let issuer = SigningKey::from_bytes(&[61; 32]);
    let a = Serving::new(backend, &client, &issuer).await;
    let b = Serving::new(backend, &client, &issuer).await;
    let grant = Grant::issue(
        &issuer,
        *client.id().as_bytes(),
        *a.agent.endpoint.id().as_bytes(),
        [1; 16],
        vec![ServiceKind::Ping],
        Duration::from_secs(60),
        GrantConstraints::default(),
    );
    let conn = rds_client::connect_authorized(&client, a.agent.endpoint.addr(), &grant)
        .await
        .unwrap();
    rds_client::ping(&conn, 1).await.unwrap();
    // Identical trusted issuer and subject, separate replay sets. Neither
    // simultaneous use nor reuse after the intended destination closes is valid.
    for close_first in [false, true] {
        if close_first {
            conn.close(0u32.into(), b"done");
        }
        assert!(
            tokio::time::timeout(
                Duration::from_secs(5),
                rds_client::connect_authorized(&client, b.agent.endpoint.addr(), &grant),
            )
            .await
            .unwrap()
            .is_err()
        );
        assert!(b.agent.policy.active_grants.lock().unwrap().is_empty());
    }
    client.close().await;
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grants_are_bound_to_the_serving_endpoint() {
    destination_binding(Backend::Iroh).await;
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_transport_keeps_the_same_destination_boundary() {
    destination_binding(Backend::Noq).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_expiry_closes_connection_and_releases_replay_slot() {
    let client = endpoint(Backend::Iroh).await;
    let issuer = SigningKey::from_bytes(&[62; 32]);
    let server = Serving::new(Backend::Iroh, &client, &issuer).await;
    let grant = Grant::issue(
        &issuer,
        *client.id().as_bytes(),
        *server.agent.endpoint.id().as_bytes(),
        [2; 16],
        vec![ServiceKind::Ping],
        Duration::from_secs(3),
        GrantConstraints::default(),
    );
    let conn = rds_client::connect_authorized(&client, server.agent.endpoint.addr(), &grant)
        .await
        .unwrap();
    rds_client::ping(&conn, 2).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), conn.wait_closed())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !server.agent.policy.active_grants.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.close().await;
    server.close().await;
}

fn replacement(
    grant: &Grant,
    issuer: &SigningKey,
    client: &Endpoint,
    server: &Serving,
) -> rds_core::grant::GrantPayload {
    let mut payload = grant
        .verify(
            &std::collections::HashSet::from([issuer.verifying_key().to_bytes()]),
            client.id().as_bytes(),
            server.agent.endpoint.id().as_bytes(),
            Duration::from_secs(300),
            rds_core::grant::now_unix(),
        )
        .unwrap()
        .payload;
    payload.revision += 1;
    payload.not_before = rds_core::grant::now_unix();
    payload.expires_at = payload.not_before + 30;
    payload
}

async fn renewal_preserves_stream(backend: Backend) {
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = tcp.local_addr().unwrap();
    struct Echo(tokio::task::JoinHandle<()>);
    impl Drop for Echo {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let mut echo = Echo(tokio::spawn(async move {
        let (stream, _) = tcp.accept().await.unwrap();
        let (mut read, mut write) = stream.into_split();
        let _ = tokio::io::copy(&mut read, &mut write).await;
    }));
    let client = endpoint(backend).await;
    let issuer = SigningKey::from_bytes(&[63; 32]);
    let server = Serving::with_target(backend, &client, &issuer, target).await;
    let grant = Grant::issue(
        &issuer,
        *client.id().as_bytes(),
        *server.agent.endpoint.id().as_bytes(),
        [3; 16],
        vec![ServiceKind::Ping, ServiceKind::Tcp],
        Duration::from_secs(5),
        GrantConstraints::default(),
    );
    let conn = rds_client::connect_authorized(&client, server.agent.endpoint.addr(), &grant)
        .await
        .unwrap();
    let (mut send, mut recv) = rds_client::open_tcp(&conn, "127.0.0.1", target.port())
        .await
        .unwrap();
    send.write_all(b"before").await.unwrap();
    let mut first = [0; 6];
    recv.read_exact(&mut first).await.unwrap();
    assert_eq!(&first, b"before");
    let renewed = Grant::issue_at(&issuer, replacement(&grant, &issuer, &client, &server));
    assert_eq!(grant.id().unwrap(), renewed.id().unwrap());
    rds_client::renew_authorization(&conn, &renewed)
        .await
        .unwrap();
    rds_client::renew_authorization(&conn, &renewed)
        .await
        .unwrap(); // exact retry
    assert_eq!(server.agent.policy.active_grants.lock().unwrap().len(), 1);
    assert!(
        rds_client::connect_authorized(&client, server.agent.endpoint.addr(), &renewed)
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_secs(5)).await; // original lease is now expired
    // Its only service slot is held by TCP; the reserved slot still admitted renewal.
    assert!(rds_client::ping(&conn, 3).await.is_err());
    send.write_all(b"after").await.unwrap();
    let mut last = [0; 5];
    recv.read_exact(&mut last).await.unwrap();
    assert_eq!(&last, b"after");
    server.agent.policy.revoke(grant.id().unwrap()); // initial id revokes every revision
    tokio::time::timeout(Duration::from_secs(3), conn.wait_closed())
        .await
        .unwrap();
    client.close().await;
    server.close().await;
    tokio::time::timeout(Duration::from_secs(3), &mut echo.0)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn renewal_keeps_live_tcp_and_original_revocation_identity() {
    renewal_preserves_stream(Backend::Iroh).await;
}

#[cfg(feature = "transport-noq")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_transport_renews_without_replacing_the_connection() {
    renewal_preserves_stream(Backend::Noq).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_scope_or_lineage_closes_only_the_invalid_renewal_connection() {
    let client = endpoint(Backend::Iroh).await;
    let issuer = SigningKey::from_bytes(&[64; 32]);
    let server = Serving::new(Backend::Iroh, &client, &issuer).await;
    let independent = Grant::issue(
        &issuer,
        *client.id().as_bytes(),
        *server.agent.endpoint.id().as_bytes(),
        [90; 16],
        vec![ServiceKind::Ping],
        Duration::from_secs(60),
        GrantConstraints::default(),
    );
    let unaffected =
        rds_client::connect_authorized(&client, server.agent.endpoint.addr(), &independent)
            .await
            .unwrap();
    for case in 0..5 {
        let grant = Grant::issue(
            &issuer,
            *client.id().as_bytes(),
            *server.agent.endpoint.id().as_bytes(),
            [case + 1; 16],
            vec![ServiceKind::Ping],
            Duration::from_secs(20),
            GrantConstraints::default(),
        );
        let conn = rds_client::connect_authorized(&client, server.agent.endpoint.addr(), &grant)
            .await
            .unwrap();
        let mut next = replacement(&grant, &issuer, &client, &server);
        match case {
            0 => next.services.push(ServiceKind::Tcp),
            1 => next.services.clear(),
            2 => next.nonce = [99; 16],
            3 => next.revision = 1,
            4 => next.expires_at = next.not_before + 1,
            _ => unreachable!(),
        }
        assert!(
            rds_client::renew_authorization(&conn, &Grant::issue_at(&issuer, next))
                .await
                .is_err()
        );
        assert!(conn.is_closed());
        rds_client::ping(&unaffected, u64::from(case))
            .await
            .unwrap();
    }
    client.close().await;
    server.close().await;
}
