//! Managed policy failures exercised over real loopback QUIC connections.
use ed25519_dalek::SigningKey;
use rds_agent::{Agent, AgentPolicy, RevocationPolicy, watch_revocations};
use rds_core::{
    ServiceKind,
    grant::{Grant, GrantConstraints},
};
use rds_discovery::{
    MemoryStore,
    authority::Authority,
    client::Client,
    clock::Reading,
    policy::PolicyStore,
    revocations::SignedRevocations,
    service::{self, Directory, ServiceConfig},
};
use rds_net::{Connection, Endpoint, EndpointConfig, bind_endpoint};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

#[cfg(feature = "transport-noq")]
const BACKEND: rds_net::Backend = rds_net::Backend::Noq;
#[cfg(not(feature = "transport-noq"))]
const BACKEND: rds_net::Backend = rds_net::Backend::Iroh;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rds-feed-{}",
            rds_net::SecretKey::generate().public()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> PolicyStore {
        PolicyStore::open(&self.0, authority(), Reading::now().unwrap()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn issuer() -> SigningKey {
    SigningKey::from_bytes(&[71; 32])
}
fn authority() -> Authority {
    Authority::new(&issuer().verifying_key(), 1).unwrap()
}
fn snapshot(revision: u64, ids: BTreeSet<[u8; 32]>, ttl: u64) -> SignedRevocations {
    SignedRevocations::publish(&issuer(), 1, revision, ids, Duration::from_secs(ttl)).unwrap()
}
async fn directory() -> Directory {
    service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig {
            registry_key: Some(issuer().verifying_key()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}
async fn endpoint() -> Endpoint {
    bind_endpoint(EndpointConfig {
        backend: BACKEND,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap()
}
struct Running {
    agent: Arc<Agent>,
    client: Endpoint,
    task: tokio::task::JoinHandle<()>,
}
impl Running {
    async fn new() -> Self {
        let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        policy.issuers.insert(issuer().verifying_key().to_bytes());
        let client = endpoint().await;
        policy.allow.insert(client.id());
        let agent = Arc::new(Agent::new(endpoint().await, policy));
        let serving = agent.clone();
        let task = tokio::spawn(async move {
            serving.run().await.unwrap();
        });
        Self {
            agent,
            client,
            task,
        }
    }
    fn grant(&self) -> Grant {
        Grant::issue(
            &issuer(),
            *self.client.id().as_bytes(),
            vec![ServiceKind::Ping],
            Duration::from_secs(120),
            GrantConstraints::default(),
        )
    }
    async fn connect(&self, grant: &Grant) -> anyhow::Result<Connection> {
        tokio::time::timeout(
            Duration::from_secs(5),
            rds_cli::connect_authorized(&self.client, self.agent.endpoint.addr(), grant),
        )
        .await
        .unwrap()
    }
    async fn shutdown(&self) {
        self.task.abort();
        self.agent.endpoint.close().await;
        self.client.close().await;
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn policy_changed(policy: &AgentPolicy, predicate: impl Fn(&RevocationPolicy) -> bool) {
    let mut rx = policy.denylist.subscribe();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if predicate(&rx.borrow_and_update()) {
                break;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
async fn closed(conn: &Connection) {
    tokio::time::timeout(Duration::from_secs(7), async {
        while !conn.is_closed() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("managed connection survived policy invalidation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_policy_denies_then_durable_revocation_closes_live_connection() {
    let tmp = Temp::new();
    let directory = directory().await;
    let client = Client::new(directory.addr());
    let running = Running::new().await;
    let grant = running.grant();
    let feed = watch_revocations(
        client.clone(),
        tmp.open(),
        running.agent.policy.clone(),
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(running.connect(&grant).await.is_err());
    client
        .update_revocations(&snapshot(1, BTreeSet::new(), 60))
        .await
        .unwrap();
    policy_changed(&running.agent.policy, |p| p.fresh()).await;
    let conn = running.connect(&grant).await.unwrap();
    rds_cli::ping(&conn, 1).await.unwrap();
    client
        .update_revocations(&snapshot(2, BTreeSet::from([grant.id()]), 60))
        .await
        .unwrap();
    policy_changed(&running.agent.policy, |p| p.contains(&grant.id())).await;
    closed(&conn).await;
    assert!(running.connect(&grant).await.is_err());
    drop(feed);
    running.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outage_expires_cached_policy_while_grant_is_still_valid() {
    let tmp = Temp::new();
    let directory = directory().await;
    let client = Client::new(directory.addr());
    let running = Running::new().await;
    let mut store = tmp.open();
    store
        .accept_revocations(&snapshot(1, BTreeSet::new(), 5), Reading::now().unwrap())
        .unwrap();
    // Startup cache permits use even before any successful network fetch.
    let feed = watch_revocations(
        client,
        store,
        running.agent.policy.clone(),
        Duration::from_secs(1),
    )
    .unwrap();
    let grant = running.grant();
    let conn = running.connect(&grant).await.unwrap();
    rds_cli::ping(&conn, 2).await.unwrap();
    drop(directory);
    closed(&conn).await;
    assert!(!running.agent.policy.denylist.borrow().fresh());
    assert!(running.connect(&running.grant()).await.is_err());
    drop(feed);
    running.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_feed_closes_admission_and_live_connections() {
    let tmp = Temp::new();
    let directory = directory().await;
    let running = Running::new().await;
    let mut store = tmp.open();
    store
        .accept_revocations(
            &snapshot(1, BTreeSet::from([[9; 32]]), 60),
            Reading::now().unwrap(),
        )
        .unwrap();
    let feed = watch_revocations(
        Client::new(directory.addr()),
        store,
        running.agent.policy.clone(),
        Duration::from_secs(1),
    )
    .unwrap();
    let conn = running.connect(&running.grant()).await.unwrap();
    drop(feed);
    assert!(running.agent.policy.denied().contains(&[9; 32]));
    closed(&conn).await;
    assert!(running.connect(&running.grant()).await.is_err());
    running.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_disk_commit_closes_live_policy_without_touching_link_target() {
    let tmp = Temp::new();
    let outside = Temp::new();
    let target = outside.0.join("sentinel");
    std::fs::write(&target, b"preserve").unwrap();
    let directory = directory().await;
    let client = Client::new(directory.addr());
    let running = Running::new().await;
    let mut store = tmp.open();
    store
        .accept_revocations(&snapshot(1, BTreeSet::new(), 60), Reading::now().unwrap())
        .unwrap();
    let feed = watch_revocations(
        client.clone(),
        store,
        running.agent.policy.clone(),
        Duration::from_secs(1),
    )
    .unwrap();
    let conn = running.connect(&running.grant()).await.unwrap();
    std::fs::remove_file(tmp.0.join("policy.json")).unwrap();
    std::os::unix::fs::symlink(&target, tmp.0.join("policy.json")).unwrap();
    client
        .update_revocations(&snapshot(2, BTreeSet::from([[9; 32]]), 60))
        .await
        .unwrap();
    policy_changed(&running.agent.policy, |p| !p.fresh()).await;
    closed(&conn).await;
    assert!(running.connect(&running.grant()).await.is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"preserve");
    drop(feed);
    running.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_and_replayed_network_snapshot_cannot_erase_revocations_or_renew_lease() {
    let tmp = Temp::new();
    let directory = directory().await;
    let client = Client::new(directory.addr());
    let running = Running::new().await;
    let revoked = running.grant();
    // The remote directory serves an older, otherwise valid snapshot for 60s.
    client
        .update_revocations(&snapshot(1, BTreeSet::new(), 60))
        .await
        .unwrap();
    let mut store = tmp.open();
    store
        .accept_revocations(
            &snapshot(2, BTreeSet::from([revoked.id()]), 5),
            Reading::now().unwrap(),
        )
        .unwrap();
    drop(store);
    let feed = watch_revocations(
        client,
        tmp.open(),
        running.agent.policy.clone(),
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(running.agent.policy.denied().contains(&revoked.id()));
    assert!(running.connect(&revoked).await.is_err());
    let conn = running.connect(&running.grant()).await.unwrap();
    closed(&conn).await;
    assert!(!running.agent.policy.denylist.borrow().fresh());
    assert!(running.agent.policy.denied().contains(&revoked.id()));
    assert!(running.connect(&running.grant()).await.is_err());
    drop(feed);
    running.shutdown().await;
}
