// Live `rds info` ↔ capability-matrix agreement (W0.5): every service
// a real agent advertises must resolve to an `implemented` or
// `experimental` row, and `stub`/`unavailable` rows must never be
// advertised as ready.
include!("../../../tests/support/capability_matrix.rs");

use rds_core::ServiceKind;
use rds_net::{EndpointConfig, bind_endpoint};
use std::sync::Arc;
use tokio::task::JoinSet;

async fn live_info(sync: bool) -> (rds_core::AgentInfo, JoinSet<()>) {
    let config = || EndpointConfig {
        backend: rds_net::Backend::Iroh,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let local = bind_endpoint(config()).await.unwrap();
    let remote = bind_endpoint(config()).await.unwrap();
    let mut policy = rds_agent::AgentPolicy::ssh_only(("127.0.0.1".into(), 22));
    policy.allow.insert(local.id());
    if sync {
        policy.sync_dir = Some(std::env::temp_dir());
    }
    let agent = Arc::new(rds_agent::Agent::new(remote.clone(), policy));
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        agent.run().await.unwrap();
    });
    let conn = rds_cli::connect(&local, remote.addr())
        .await
        .expect("connect to test agent");
    let info = rds_cli::info(&conn).await.expect("info query");
    conn.close(0u32.into(), b"done");
    local.close().await;
    (info, tasks)
}

fn row_state<'a>(rows: &'a [Row], capability: &str) -> Option<&'a str> {
    rows.iter()
        .find(|r| r.capability == capability)
        .map(|r| r.state.as_str())
}

#[tokio::test]
async fn advertised_services_resolve_to_real_rows() {
    let rows = matrix_rows();
    let (info, mut tasks) = live_info(false).await;
    assert!(!info.services.is_empty(), "agent advertised no services");
    for svc in &info.services {
        let id = format!("service:{}", kebab(&format!("{svc:?}")));
        let state = row_state(&rows, &id)
            .unwrap_or_else(|| panic!("advertised {svc:?} has no `{id}` matrix row"));
        assert!(
            matches!(state, "implemented" | "experimental"),
            "agent advertises {svc:?} but `service:` row is `{state}` — \
             a stub/unavailable capability must never be offered live"
        );
    }
    // Baseline policy must not claim what it cannot serve.
    assert!(
        !info.services.contains(&ServiceKind::Sync),
        "sync_dir unset: Sync must not be advertised"
    );
    tasks.abort_all();
}

#[tokio::test]
async fn sync_dir_flips_the_advertised_set() {
    let (info, mut tasks) = live_info(true).await;
    assert!(
        info.services.contains(&ServiceKind::Sync),
        "sync_dir set: Sync must be advertised"
    );
    tasks.abort_all();
}
