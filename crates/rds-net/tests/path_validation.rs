//! Additional paths must validate before application policy promotes them.
#![cfg(feature = "transport-noq")]
use noq::{PathStatus, Runtime};
use rds_net::backends::noq::{self as owned, policy};
use rds_net::{Backend, EndpointConfig, SecretKey};
use std::{net::SocketAddr, sync::Arc, time::Duration};

async fn fixture() -> (owned::Endpoint, owned::Endpoint, SocketAddr) {
    let runtime = Arc::new(noq::TokioRuntime);
    let primary = runtime
        .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
        .unwrap();
    let secondary = runtime
        .wrap_udp_socket(std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
        .unwrap();
    let mux = owned::socket::Mux::new(vec![primary, secondary]).unwrap();
    let addresses = mux.local_addrs();
    // The second real listener is deliberately absent from advertisements, so
    // QNT cannot pre-open it and hide a lost subscription/validation boundary.
    let server = owned::bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(SecretKey::from_bytes(&[106; 32])),
            discovery: false,
            ..Default::default()
        },
        Box::new(mux),
        vec![addresses[0]],
        runtime,
        None,
    )
    .await
    .unwrap();
    let client = owned::bind_endpoint(EndpointConfig {
        backend: Backend::Noq,
        secret_key: Some(SecretKey::from_bytes(&[107; 32])),
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    })
    .await
    .unwrap();
    (client, server, addresses[1])
}
async fn pair(
    client: &owned::Endpoint,
    server: &owned::Endpoint,
) -> (owned::Connection, owned::Connection) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (a, b) = tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
            server.accept().await.unwrap().await
        });
        (a.unwrap(), b.unwrap())
    })
    .await
    .unwrap()
}
async fn open_extra_when_ready(connection: &owned::Connection, address: SocketAddr) -> noq::PathId {
    // TLS completion can precede receipt of additional peer connection IDs.
    // Establish the test's pending-path precondition without a fixed sleep.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(id) = policy::open_extra_paths(connection.inner(), &[address]).first() {
                break *id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_additional_path_starts_as_backup() {
    let (client, server, _) = fixture().await;
    let (a, _b) = pair(&client, &server).await;
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let id = open_extra_when_ready(&a, silent.local_addr().unwrap()).await;
    let status = a.inner().path(id).unwrap().status().unwrap();
    let paths = rds_net::Connection::from(a.clone()).path_stats();
    client.close().await;
    server.close().await;
    assert_eq!(
        status,
        PathStatus::Backup,
        "pending path was made preferred before validation"
    );
    assert!(
        paths
            .iter()
            .all(|path| path.path_id.to_string() != id.to_string())
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validated_secondary_carries_data_after_primary_closes() {
    let (client, server, secondary) = fixture().await;
    let (a, b) = pair(&client, &server).await;
    let id = open_extra_when_ready(&a, secondary).await;
    assert_ne!(id, noq::PathId::ZERO);
    // This path was created by open_extra_paths above, which installed the
    // application OpenPath watcher. Await its actual validation completion.
    let path = tokio::time::timeout(
        Duration::from_secs(3),
        a.inner().open_path_ensure(secondary, PathStatus::Backup),
    )
    .await
    .unwrap()
    .unwrap();
    a.inner().path(noq::PathId::ZERO).unwrap().close().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while path.status().unwrap() != PathStatus::Available {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    a.send_datagram(b"validated replacement".to_vec().into())
        .unwrap();
    assert_eq!(
        &tokio::time::timeout(Duration::from_secs(2), b.read_datagram())
            .await
            .unwrap()
            .unwrap()[..],
        b"validated replacement"
    );
    let facade = rds_net::Connection::from(a.clone());
    // Applying engine status and publishing the policy snapshot are separate
    // synchronous steps on another worker; wait for observable convergence.
    let selected = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(path) = facade.current_path_stats()
                && path.path_id.to_string() == id.to_string()
            {
                break path;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    client.close().await;
    server.close().await;
    assert_eq!(
        selected.map(|path| path.path_id.to_string()).ok(),
        Some(id.to_string()),
        "telemetry must follow the validated replacement"
    );
}
