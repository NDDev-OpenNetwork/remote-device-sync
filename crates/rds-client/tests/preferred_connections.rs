//! Reachability hints preserve identity, admission and manager deduplication.
use rds_client::local::{Client, Prepared, Server};
use rds_core::{
    HelloAck, StreamHello,
    local::{Command, ErrorCode, Reply},
};
use rds_net::{Backend, Endpoint, EndpointConfig, TransportAddr, read_frame, write_frame};
use std::{
    os::unix::fs::DirBuilderExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

async fn endpoint(backend: Backend) -> Endpoint {
    rds_net::bind_endpoint(EndpointConfig {
        backend,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        discovery: false,
        ..Default::default()
    })
    .await
    .unwrap()
}
fn serve(remote: Endpoint, refuse: bool, accepts: Arc<AtomicUsize>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        while let Some(incoming) = remote.accept().await {
            let Ok(conn) = incoming.await else {
                continue;
            };
            accepts.fetch_add(1, Ordering::SeqCst);
            tasks.spawn(async move {
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    let StreamHello::Ping { nonce } = read_frame(&mut recv).await.unwrap() else {
                        panic!("unexpected request");
                    };
                    let ack = if refuse {
                        HelloAck::Error {
                            message: "fixture refusal".into(),
                        }
                    } else {
                        HelloAck::Ok
                    };
                    write_frame(&mut send, &ack).await.unwrap();
                    if !refuse {
                        send.write_all(&nonce.to_be_bytes()).await.unwrap();
                    }
                    send.finish().unwrap();
                }
            });
        }
        tasks.shutdown().await;
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preferred_dial_is_pinned_deduplicated_and_admission_is_not_replayed() {
    for backend in [
        Backend::Iroh,
        #[cfg(feature = "transport-noq")]
        Backend::Noq,
    ] {
        for (dead_hint, refuse) in [(false, false), (true, false), (false, true)] {
            let local = endpoint(backend).await;
            let remote = endpoint(backend).await;
            let count = Arc::new(AtomicUsize::new(0));
            let serving = serve(remote.clone(), refuse, count.clone());
            let root = std::path::Path::new("/tmp")
                .canonicalize()
                .unwrap()
                .join(format!("rds-pref-{:032x}", rand::random::<u128>()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            // A dead directory must not prevent a working direct connection.
            let unavailable = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let directory_addr = unavailable.local_addr().unwrap();
            drop(unavailable);
            let mut manager = Server::start(
                Some(Prepared::bind(&root).await.unwrap()),
                local.clone(),
                Some(rds_discovery::client::Client::new(directory_addr)),
            );
            let client = Client::new(&root);
            let address = remote
                .addr()
                .addrs
                .into_iter()
                .find_map(|a| match a {
                    TransportAddr::Ip(a) => Some(a),
                    _ => None,
                })
                .unwrap();
            let unused = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let hint = if dead_hint {
                unused.local_addr().unwrap()
            } else {
                address
            };
            let command = Command::ConnectPreferred {
                target: if dead_hint {
                    rds_net::Ticket::of(&remote).to_string()
                } else {
                    remote.id().to_string()
                },
                grant: None,
                local_addresses: vec![hint],
            };
            let results = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(client.request(command.clone()), client.request(command))
            })
            .await
            .unwrap();
            if refuse {
                assert!(results.0.is_err() && results.1.is_err());
            } else {
                let (Reply::Connected(a), Reply::Connected(b)) =
                    (results.0.unwrap(), results.1.unwrap())
                else {
                    panic!("missing sessions");
                };
                assert_eq!(a, b);
                assert_eq!(client.snapshot().await.unwrap().sessions.len(), 1);
            }
            assert_eq!(
                count.load(Ordering::SeqCst),
                1,
                "one dial/admission for concurrent monitors"
            );
            let invalid = client
                .request(Command::ConnectPreferred {
                    target: remote.id().to_string(),
                    grant: None,
                    local_addresses: vec!["0.0.0.0:1".parse().unwrap()],
                })
                .await;
            assert!(matches!(
                invalid,
                Err(rds_client::local::Error::Rejected(
                    ErrorCode::InvalidRequest
                ))
            ));
            let self_peer = client
                .request(Command::ConnectPreferred {
                    target: local.id().to_string(),
                    grant: None,
                    local_addresses: vec![address],
                })
                .await;
            assert!(matches!(
                self_peer,
                Err(rds_client::local::Error::Rejected(
                    ErrorCode::InvalidRequest
                ))
            ));
            manager.close().await.unwrap();
            local.close().await;
            remote.close().await;
            serving.await.unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
