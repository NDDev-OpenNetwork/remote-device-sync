//! A valid endpoint identity must remain reachable when its address hash has
//! zero in the virtual port bits. These sockets have no direct transport child.
#![cfg(feature = "owned-relay")]

use noq::AsyncUdpSocket;
use rds_net::backends::noq::{self as owned, relay::RelaySocket};
use rds_net::{Backend, EndpointAddr, EndpointConfig, SecretKey};
use std::{sync::Arc, time::Duration};

fn zero_port_key() -> SecretKey {
    // Public fixture found once offline; normal tests perform no search.
    let mut bytes = [135u8; 32];
    bytes[..4].copy_from_slice(&33349u32.to_le_bytes());
    SecretKey::from_bytes(&bytes)
}

async fn attached(key: SecretKey, relay: EndpointAddr) -> owned::Endpoint {
    let (socket, handle) =
        RelaySocket::connect(relay.clone(), key.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
    let local = socket.local_addr().unwrap();
    owned::bind_with_socket(
        EndpointConfig {
            backend: Backend::Noq,
            secret_key: Some(key),
            discovery: false,
            relay_endpoint: Some(relay),
            ..Default::default()
        },
        Box::new(socket),
        vec![local],
        Arc::new(noq::TokioRuntime),
        Some(handle),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_hash_port_identity_supports_relay_only_streams_in_both_directions() {
    tokio::time::timeout(Duration::from_secs(20), exercise())
        .await
        .unwrap();
}

async fn exercise() {
    let relay = rds_relay::server::serve(
        EndpointConfig {
            backend: Backend::Noq,
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        },
        Vec::new(),
    )
    .await
    .unwrap();
    let a = attached(SecretKey::from_bytes(&[136; 32]), relay.endpoint_addr()).await;
    let b = attached(zero_port_key(), relay.endpoint_addr()).await;
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        for (client, server) in [(&a, &b), (&b, &a)] {
            let target = server.addr();
            assert_eq!(target.addrs.len(), 1);
            assert!(
                target
                    .addrs
                    .iter()
                    .all(|address| matches!(address, rds_net::TransportAddr::Relay(_)))
            );
            let (local, remote) =
                tokio::try_join!(client.connect(target, rds_core::ALPN), async {
                    server
                        .accept()
                        .await
                        .ok_or_else(|| anyhow::anyhow!("fixture listener closed"))?
                        .await
                })?;
            assert_eq!(local.remote_id(), server.id());
            assert_eq!(remote.remote_id(), client.id());
            let (mut send, mut recv) = local.open_bi().await?;
            send.write_all(b"request").await?;
            send.finish()?;
            let (mut reply, mut request) = remote.accept_bi().await?;
            let mut bytes = [0; 7];
            request.read_exact(&mut bytes).await?;
            anyhow::ensure!(&bytes == b"request");
            reply.write_all(b"response").await?;
            reply.finish()?;
            let mut bytes = [0; 8];
            recv.read_exact(&mut bytes).await?;
            anyhow::ensure!(&bytes == b"response");
            local.close(0u32.into(), b"fixture direction complete");
            remote.close(0u32.into(), b"fixture direction complete");
        }
        anyhow::ensure!(relay.stats().0 > 0, "fixture bypassed relay forwarding");
        Ok::<_, anyhow::Error>(())
    })
    .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        let (_, _, closed_2) = tokio::join!(a.close(), b.close(), relay.close());
        closed_2.unwrap();
    })
    .await
    .expect("zero-port fixture cleanup stalled");
    assert!(
        matches!(&result, Ok(Ok(()))),
        "valid identity was not relay reachable: {result:?}"
    );
}
