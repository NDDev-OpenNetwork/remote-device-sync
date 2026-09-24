//! Client-side name trust: directory transport is not an identity authority.

use rds_discovery::{
    EndpointKey,
    client::Client,
    http::{self, Response},
};

use ed25519_dalek::{Signer, SigningKey};
use rds_discovery::{
    DiscoveryError, MemoryStore, now_unix,
    registry::{NameBindingPayload, SignedNameBinding, SignedRegistry},
    service::{self, ServiceConfig},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

fn issuer() -> SigningKey {
    SigningKey::from_bytes(&[17; 32])
}

fn payload() -> NameBindingPayload {
    let now = now_unix().unwrap();
    NameBindingPayload {
        version: 1,
        name: "device-a".into(),
        key: EndpointKey([18; 32]),
        issued_at: now - 10,
        expires_at: now + 120,
    }
}

async fn responder(responses: Vec<Response>) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client =
        Client::new(listener.local_addr().unwrap()).with_registry_key(issuer().verifying_key());
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut sock, _) = listener.accept().await.unwrap();
            http::read_request(&mut sock).await.unwrap();
            http::write_response(&mut sock, &response).await.unwrap();
        }
    });
    (client, task)
}

#[tokio::test]
async fn configured_anchor_rejects_unsigned_wrong_name_issuer_expiry_and_redirect() {
    let key = issuer();
    let now = now_unix().unwrap();
    let good = payload();
    let mut bad_name = good.clone();
    bad_name.name = "device-b".into();
    let mut expired = good.clone();
    expired.expires_at = now;
    let mut future = good.clone();
    future.issued_at = now + 60;
    let mut too_long = good.clone();
    too_long.expires_at = now + 90_000;
    let signed =
        |p: &NameBindingPayload| Response::json(200, SignedNameBinding::sign(p, &key).unwrap());
    let mut tampered = SignedNameBinding::sign(&good, &key).unwrap();
    tampered.payload[3] ^= 1;
    let raw = postcard::to_stdvec(&good).unwrap();
    let cross_protocol = SignedNameBinding {
        signature: key.sign(&raw).to_bytes().to_vec(),
        payload: raw,
    };
    let responses = vec![
        Response::json(200, serde_json::json!({"key": good.key.to_string()})),
        signed(&bad_name),
        signed(&expired),
        signed(&future),
        signed(&too_long),
        Response::json(
            200,
            SignedNameBinding::sign(&good, &SigningKey::from_bytes(&[19; 32])).unwrap(),
        ),
        Response::json(200, tampered),
        Response::json(200, cross_protocol),
        Response::json(302, serde_json::json!({"location": "/elsewhere"})),
    ];
    let count = responses.len();
    let (client, task) = responder(responses).await;
    for i in 0..count {
        assert!(
            client.resolve_name("device-a").await.is_err(),
            "accepted case {i}"
        );
    }
    task.await.unwrap();
}

#[tokio::test]
async fn cloned_clients_reject_rollback_and_same_revision_equivocation() {
    let key = issuer();
    let initial = payload();
    let mut newer = initial.clone();
    newer.issued_at += 1;
    newer.key = EndpointKey([20; 32]);
    let mut conflict = newer.clone();
    conflict.key = EndpointKey([21; 32]);
    let responses = [&initial, &newer, &newer, &initial, &conflict]
        .map(|p| Response::json(200, SignedNameBinding::sign(p, &key).unwrap()));
    let (client, task) = responder(responses.into()).await;
    let clone = client.clone();
    assert_eq!(client.resolve_name("device-a").await.unwrap(), initial.key);
    assert_eq!(clone.resolve_name("device-a").await.unwrap(), newer.key);
    assert_eq!(client.resolve_name("device-a").await.unwrap(), newer.key);
    for _ in 0..2 {
        assert!(matches!(
            clone.resolve_name("device-a").await,
            Err(DiscoveryError::Stale)
        ));
    }
    task.await.unwrap();
}

#[tokio::test]
async fn response_discloses_only_requested_binding_and_expired_names_stop_serving() {
    let key = issuer();
    let snapshot = SignedRegistry::publish(
        &key,
        BTreeMap::from([
            ("device-a".into(), EndpointKey([18; 32])),
            ("device-b".into(), EndpointKey([19; 32])),
        ]),
        Duration::from_secs(3),
    )
    .unwrap();
    let expiry = snapshot.verify(&key.verifying_key()).unwrap().expires_at;
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(MemoryStore::default()),
        ServiceConfig {
            registry_key: Some(key.verifying_key()),
            registry: Some(snapshot),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut socket = tokio::net::TcpStream::connect(directory.addr())
        .await
        .unwrap();
    http::write_request(&mut socket, "GET", "/v1/names/device-a", &[])
        .await
        .unwrap();
    let response = http::read_response(&mut socket).await.unwrap();
    assert_eq!(response.status, 200);
    let proof: SignedNameBinding = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(
        proof
            .verify(&key.verifying_key(), "device-a", now_unix().unwrap())
            .unwrap()
            .key,
        EndpointKey([18; 32])
    );
    assert!(
        !proof
            .payload
            .windows(b"device-b".len())
            .any(|w| w == b"device-b")
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(&response.body)
            .unwrap()
            .get("bindings")
            .is_none()
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while now_unix().unwrap() < expiry {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let client = Client::new(directory.addr()).with_registry_key(key.verifying_key());
    assert!(matches!(
        client.resolve_name("device-a").await,
        Err(DiscoveryError::Http { status: 410, .. })
    ));
}

#[test]
fn registry_requires_complete_matching_proofs_and_bounds() {
    let key = issuer();
    let snapshot = SignedRegistry::publish(
        &key,
        BTreeMap::from([("device-a".into(), EndpointKey([18; 32]))]),
        Duration::from_secs(60),
    )
    .unwrap();
    let mut legacy = snapshot.clone();
    legacy.bindings.clear();
    assert!(legacy.verify(&key.verifying_key()).is_err());
    let mut mismatch = snapshot.clone();
    mismatch.bindings.insert(
        "device-a".into(),
        SignedNameBinding::sign(&payload(), &key).unwrap(),
    );
    assert!(mismatch.verify(&key.verifying_key()).is_err());
    let mut oversized = snapshot.bindings["device-a"].clone();
    oversized.payload.resize(257, 0);
    assert!(
        oversized
            .verify(&key.verifying_key(), "device-a", now_unix().unwrap())
            .is_err()
    );
    assert!(SignedRegistry::publish(&key, BTreeMap::new(), Duration::from_secs(u64::MAX)).is_err());
    assert!(
        Client::new("127.0.0.1:1".parse().unwrap())
            .with_registry_key_base32("bad")
            .is_err()
    );
}

#[tokio::test]
async fn unsigned_directory_name_cannot_select_an_identity() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::new(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        http::read_request(&mut sock).await.unwrap();
        http::write_response(
            &mut sock,
            &Response::json(
                200,
                serde_json::json!({"key": EndpointKey([7; 32]).to_string()}),
            ),
        )
        .await
        .unwrap();
    });
    let result = client.resolve_name("device-a").await;
    server.abort();
    assert!(
        result.is_err(),
        "unsigned directory selected an identity: {result:?}"
    );
}
