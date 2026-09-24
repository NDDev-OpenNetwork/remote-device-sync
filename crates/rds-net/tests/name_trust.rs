//! A valid self-signed endpoint record cannot replace an estate-bound identity.

use std::collections::BTreeMap;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use rds_discovery::{
    EndpointKey, EndpointRecord,
    client::Client,
    http::{self, Response},
    registry::SignedRegistry,
};

#[tokio::test]
async fn resolver_binds_the_verified_name_to_the_record_identity() {
    let issuer = SigningKey::from_bytes(&[61; 32]);
    let expected = SigningKey::from_bytes(&[62; 32]);
    let substituted = SigningKey::from_bytes(&[63; 32]);
    for wrong_record in [false, true] {
        let snapshot = SignedRegistry::publish(
            &issuer,
            1,
            1,
            BTreeMap::from([(
                "device-a".into(),
                EndpointKey(expected.verifying_key().to_bytes()),
            )]),
            Duration::from_secs(60),
        )
        .unwrap();
        let record = EndpointRecord::publish(
            if wrong_record {
                &substituted
            } else {
                &expected
            },
            1,
            vec!["127.0.0.1:9876".parse().unwrap()],
            vec![],
            vec![],
            Duration::from_secs(60),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            Client::new(listener.local_addr().unwrap()).with_registry_key(issuer.verifying_key());
        let expected_path = format!(
            "/v1/records/{}",
            EndpointKey(expected.verifying_key().to_bytes())
        );
        let server = tokio::spawn(async move {
            for (path, response) in [
                (
                    "/v1/names/device-a".to_string(),
                    Response::json(200, &snapshot.bindings["device-a"]),
                ),
                (expected_path, Response::json(200, record)),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                assert_eq!(
                    http::read_request(&mut socket).await.unwrap().unwrap().path,
                    path
                );
                http::write_response(&mut socket, &response).await.unwrap();
            }
        });
        let result = rds_net::resolve_target(Some(client), "device-a").await;
        if wrong_record {
            assert!(result.is_err(), "accepted a different record identity");
        } else {
            assert_eq!(
                result.unwrap().id.as_bytes(),
                &expected.verifying_key().to_bytes()
            );
        }
        server.await.unwrap();
    }
}
