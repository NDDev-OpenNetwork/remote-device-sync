//! Enrolled writers must retain renewal capacity under another writer's load.
use ed25519_dalek::SigningKey;
use rds_discovery::{
    DiscoveryError, EndpointRecord, MemoryStore, RecordStore, Service,
    client::Client,
    service::{self, Limits, ServiceConfig},
};
use std::{sync::Arc, time::Duration};

fn record(seed: u8, revision: u64) -> EndpointRecord {
    EndpointRecord::publish(
        &SigningKey::from_bytes(&[seed; 32]),
        revision,
        vec!["127.0.0.1:4000".parse().unwrap()],
        vec![],
        vec![Service::Ping],
        Duration::from_secs(300),
    )
    .unwrap()
}

#[tokio::test]
async fn refused_writer_cannot_spend_another_devices_renewal_budget() {
    let store = Arc::new(MemoryStore::default());
    store.put(&record(121, 1)).unwrap();
    store.put(&record(122, 1)).unwrap();
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            enrollment: rds_discovery::Enrollment::new([record(121, 1).key, record(122, 1).key])
                .unwrap(),
            limits: Limits {
                put_min_interval: Duration::from_secs(60),
                put_per_minute: 2,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    client.publish(&record(121, 2)).await.unwrap();
    for _ in 0..4 {
        assert!(matches!(
            client.publish(&record(121, 3)).await,
            Err(DiscoveryError::RateLimited)
        ));
    }
    client
        .publish(&record(122, 2))
        .await
        .expect("one refused writer starved another enrolled device");
}

#[tokio::test]
async fn default_directory_refuses_unenrolled_publishers() {
    let store = Arc::new(MemoryStore::default());
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        Client::new(dir.addr()).publish(&record(123, 1)).await,
        Err(DiscoveryError::Http { status: 403, .. })
    ));
    assert!(store.is_empty());
}

#[tokio::test]
async fn replayed_or_forged_operations_do_not_spend_the_owners_budget() {
    let store = Arc::new(MemoryStore::default());
    let original = record(124, 1);
    store.put(&original).unwrap();
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            enrollment: rds_discovery::Enrollment::new([original.key]).unwrap(),
            limits: Limits {
                writer_per_minute: 1,
                put_per_minute: 0,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    for _ in 0..3 {
        client.publish(&original).await.unwrap();
    }
    let mut forged = record(124, 2);
    forged.signature[0] ^= 1;
    assert!(matches!(
        client.publish(&forged).await,
        Err(DiscoveryError::Http { status: 401, .. })
    ));
    let next = record(124, 2);
    client.publish(&next).await.unwrap();
    for _ in 0..3 {
        client.publish(&next).await.unwrap();
        assert!(matches!(
            client.publish(&original).await,
            Err(DiscoveryError::Http { status: 409, .. })
        ));
    }
    assert!(matches!(
        client.publish(&record(124, 3)).await,
        Err(DiscoveryError::RateLimited)
    ));
}

#[tokio::test]
async fn new_admission_and_extra_write_saturation_preserve_known_renewal() {
    let store = Arc::new(MemoryStore::default());
    store.put(&record(125, 1)).unwrap();
    store.put(&record(126, 1)).unwrap();
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            enrollment: rds_discovery::Enrollment::new((125..129).map(|seed| record(seed, 1).key))
                .unwrap(),
            limits: Limits {
                admissions_per_minute: 1,
                put_per_minute: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    client.publish(&record(127, 1)).await.unwrap();
    assert!(matches!(
        client.publish(&record(128, 1)).await,
        Err(DiscoveryError::RateLimited)
    ));
    client.publish(&record(125, 2)).await.unwrap(); // Protected renewal.
    client.publish(&record(125, 3)).await.unwrap(); // Exhaust shared extra budget.
    assert!(matches!(
        client.publish(&record(125, 4)).await,
        Err(DiscoveryError::RateLimited)
    ));
    client.publish(&record(126, 2)).await.unwrap();
}

#[tokio::test]
async fn unenrollment_hides_retained_records_and_refuses_delete_without_losing_history() {
    let store = Arc::new(MemoryStore::default());
    let original = record(129, 1);
    store.put(&original).unwrap();
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
        ServiceConfig::default(),
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    assert!(matches!(
        client.fetch(&original.key).await,
        Err(DiscoveryError::Http { status: 403, .. })
    ));
    let tomb = rds_discovery::DeleteRequest::new(&SigningKey::from_bytes(&[129; 32]), 2).unwrap();
    assert!(matches!(
        client.remove(&tomb).await,
        Err(DiscoveryError::Http { status: 403, .. })
    ));
    assert_eq!(store.get(&original.key).unwrap(), original);
}

#[tokio::test]
async fn concurrent_exact_publications_spend_one_revision_not_one_budget_per_request() {
    let store = Arc::new(MemoryStore::default());
    store.put(&record(130, 1)).unwrap();
    let next = record(130, 2);
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            enrollment: rds_discovery::Enrollment::new([next.key]).unwrap(),
            limits: Limits {
                writer_per_minute: 1,
                put_per_minute: 0,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let client = client.clone();
        let next = next.clone();
        tasks.spawn(async move { client.publish(&next).await });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap().unwrap();
    }
}

#[tokio::test]
async fn policy_replay_and_record_flood_cannot_spend_a_new_revocations_update() {
    use rds_discovery::{registry::SignedRegistry, revocations::SignedRevocations};
    let issuer = SigningKey::from_bytes(&[132; 32]);
    let store = Arc::new(MemoryStore::default());
    let device = record(133, 1);
    store.put(&device).unwrap();
    let dir = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store,
        ServiceConfig {
            enrollment: rds_discovery::Enrollment::new([device.key]).unwrap(),
            registry_key: Some(issuer.verifying_key()),
            limits: Limits {
                policy_per_minute: 1,
                writer_per_minute: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new(dir.addr());
    client.publish(&record(133, 2)).await.unwrap();
    for _ in 0..3 {
        assert!(matches!(
            client.publish(&record(133, 3)).await,
            Err(DiscoveryError::RateLimited)
        ));
    }
    let registry =
        SignedRegistry::publish(&issuer, 1, 1, Default::default(), Duration::from_secs(300))
            .unwrap();
    client.update_registry(&registry).await.unwrap();
    for _ in 0..3 {
        client.update_registry(&registry).await.unwrap();
    }
    let denied =
        SignedRegistry::publish(&issuer, 1, 2, Default::default(), Duration::from_secs(300))
            .unwrap();
    assert!(matches!(
        client.update_registry(&denied).await,
        Err(DiscoveryError::RateLimited)
    ));
    let revocations =
        SignedRevocations::publish(&issuer, 1, 1, Default::default(), Duration::from_secs(300))
            .unwrap();
    let mut forged = revocations.clone();
    forged.signature[0] ^= 1;
    assert!(matches!(
        client.update_revocations(&forged).await,
        Err(DiscoveryError::Http { status: 401, .. })
    ));
    client.update_revocations(&revocations).await.unwrap();
    client.update_revocations(&revocations).await.unwrap();
}
