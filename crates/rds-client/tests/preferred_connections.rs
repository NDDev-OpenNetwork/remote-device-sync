//! Reachability hints preserve identity, admission and manager deduplication.
use rds_client::local::{Client, Prepared, Server};
use rds_core::{
    HelloAck, StreamHello,
    local::{Command, ErrorCode, Reply},
};
use rds_net::{
    Backend, Endpoint, EndpointAddr, EndpointConfig, TransportAddr, read_frame, write_frame,
};
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

/// A stale LAN hint must not become identity, and fallback must actually use
/// a verified directory record, rather than a cached direct ticket. Iroh uses
/// an isolated relay; owned Noq uses its isolated direct transport.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_or_wrong_key_hint_falls_back_to_signed_directory() {
    use rds_discovery::{EndpointRecord, MemoryStore, Service, client, service};
    let _telemetry = std::env::var_os("RUST_LOG")
        .map(|_| rds_observe::init(rds_observe::Service::Bench, "warn").unwrap());
    for backend in [
        Backend::Iroh,
        #[cfg(feature = "transport-noq")]
        Backend::Noq,
    ] {
        for (wrong_key, previous_auth, mapped) in [
            (false, false, false),
            (true, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            eprintln!(
                "preferred fallback case: backend={backend:?} wrong_key={wrong_key} previous_auth={previous_auth} mapped={mapped}"
            );
            let (relay, urls) = if matches!(backend, Backend::Iroh) {
                let mut config = iroh_relay::server::ServerConfig::default();
                config.relay = Some(iroh_relay::server::RelayConfig::new(
                    "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
                ));
                let relay = iroh_relay::server::Server::spawn(config).await.unwrap();
                let url = format!("http://{}", relay.http_addr().unwrap());
                (Some(relay), vec![url])
            } else {
                (None, vec![])
            };
            let key = rds_net::SecretKey::from_bytes(&[91; 32]);
            let config = || {
                let mut config = EndpointConfig {
                    backend,
                    discovery: false,
                    bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                    ..Default::default()
                };
                if let Some(url) = urls.first() {
                    config = config.with_relay(url).unwrap();
                }
                config
            };
            let local = rds_net::bind_endpoint(config()).await.unwrap();
            let final_transport = if matches!(backend, Backend::Iroh) {
                rds_net::Transports::RelayOnly
            } else {
                rds_net::Transports::DirectOnly
            };
            let mut remote = rds_net::bind_endpoint(EndpointConfig {
                secret_key: Some(key.clone()),
                transports: if previous_auth {
                    rds_net::Transports::All
                } else {
                    final_transport
                },
                ..config()
            })
            .await
            .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(local.online(), remote.online());
            })
            .await
            .expect("isolated relay registration");
            let reused_address = if previous_auth {
                let address = remote
                    .addr()
                    .addrs
                    .into_iter()
                    .find_map(|addr| match addr {
                        TransportAddr::Ip(addr) => Some(addr),
                        _ => None,
                    })
                    .unwrap();
                let warming = serve(remote.clone(), false, Arc::new(AtomicUsize::new(0)));
                let connection = local
                    .connect(
                        EndpointAddr::from_parts(remote.id(), [TransportAddr::Ip(address)]),
                        rds_core::ALPN,
                    )
                    .await
                    .unwrap();
                rds_client::ping(&connection, rand::random()).await.unwrap();
                connection.close(0u32.into(), b"fixture address reuse");
                remote.close().await;
                warming.await.unwrap();
                drop(connection);
                drop(remote);
                remote = rds_net::bind_endpoint(EndpointConfig {
                    secret_key: Some(key.clone()),
                    transports: final_transport,
                    ..config()
                })
                .await
                .unwrap();
                tokio::time::timeout(Duration::from_secs(10), remote.online())
                    .await
                    .unwrap();
                Some(address)
            } else {
                None
            };
            let directory = service::serve(
                "127.0.0.1:0".parse().unwrap(),
                Arc::new(MemoryStore::default()),
                service::ServiceConfig::open_ephemeral(),
            )
            .await
            .unwrap();
            let records = client::Client::new(directory.addr());
            let record = EndpointRecord::publish(
                &ed25519_dalek::SigningKey::from_bytes(&key.to_bytes()),
                1,
                remote
                    .addr()
                    .addrs
                    .into_iter()
                    .filter_map(|addr| match addr {
                        TransportAddr::Ip(addr) => Some(addr),
                        _ => None,
                    })
                    .collect(),
                urls.clone(),
                vec![Service::Ping],
                Duration::from_secs(120),
            )
            .unwrap();
            records.publish(&record).await.unwrap();
            let expected_accepts = Arc::new(AtomicUsize::new(0));
            let expected_server = serve(remote.clone(), false, expected_accepts.clone());
            let other = if let Some(address) = reused_address {
                // Owned Noq closes its UDP driver asynchronously. Address reuse
                // begins only once the old fixture has released the socket.
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        match rds_net::bind_endpoint(EndpointConfig {
                            backend,
                            discovery: false,
                            bind_addrs: vec![address],
                            ..Default::default()
                        })
                        .await
                        {
                            Ok(endpoint) => break endpoint,
                            Err(error)
                                if error.chain().any(|cause| {
                                    cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                                        error.kind() == std::io::ErrorKind::AddrInUse
                                    })
                                }) =>
                            {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                            Err(error) => panic!("address-reuse fixture bind: {error:#}"),
                        }
                    }
                })
                .await
                .expect("old fixture UDP driver must release its address")
            } else {
                endpoint(backend).await
            };
            let other_accepts = Arc::new(AtomicUsize::new(0));
            let other_server = serve(other.clone(), false, other_accepts.clone());
            let unused = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let hint = if wrong_key {
                other
                    .addr()
                    .addrs
                    .into_iter()
                    .find_map(|addr| match addr {
                        TransportAddr::Ip(addr) => Some(addr),
                        _ => None,
                    })
                    .unwrap()
            } else {
                unused.local_addr().unwrap()
            };
            let hint = if mapped {
                let std::net::SocketAddr::V4(v4) = hint else {
                    panic!("fixture binds IPv4")
                };
                std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                    v4.ip().to_ipv6_mapped(),
                    v4.port(),
                    0,
                    0,
                ))
            } else {
                hint
            };
            let root = std::path::Path::new("/tmp")
                .canonicalize()
                .unwrap()
                .join(format!("rds-pref-relay-{:032x}", rand::random::<u128>()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            let mut manager = Server::start(
                Some(Prepared::bind(&root).await.unwrap()),
                local.clone(),
                Some(records),
            );
            let client = Client::new(&root);
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                client.request(Command::ConnectPreferred {
                    target: remote.id().to_string(),
                    grant: None,
                    local_addresses: vec![hint],
                }),
            )
            .await
            .expect("bounded hint and directory/relay fallback")
            .unwrap_or_else(|error| {
                panic!(
                    "backend={backend:?} wrong_key={wrong_key} previous_auth={previous_auth} mapped={mapped}: {error}; expected_accepts={}, other_accepts={}, directory_gets={}",
                    expected_accepts.load(Ordering::SeqCst),
                    other_accepts.load(Ordering::SeqCst),
                    directory.metrics().snapshot()["rds_directory_gets_total"],
                )
            });
            assert!(matches!(result, Reply::Connected(_)));
            let snapshot = client.snapshot().await.unwrap();
            assert_eq!(snapshot.sessions.len(), 1);
            assert_eq!(snapshot.sessions[0].peer, remote.id().to_string());
            assert_eq!(expected_accepts.load(Ordering::SeqCst), 1);
            assert_eq!(other_accepts.load(Ordering::SeqCst), 0);
            assert_eq!(
                directory.metrics().snapshot()["rds_directory_gets_total"],
                1
            );
            // The manager samples its connection once a second. Await that
            // observation, rather than treating the pre-sample zero as a path.
            let counter = if matches!(backend, Backend::Iroh) {
                "rds_net_bytes_received_total{via=\"relay\"}"
            } else {
                "rds_net_bytes_received_total{via=\"direct\"}"
            };
            tokio::time::timeout(Duration::from_secs(3), async {
                while local.metrics().snapshot()[counter] == 0 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("fallback payload must be observed by the manager sampler");
            manager.close().await.unwrap();
            local.close().await;
            remote.close().await;
            other.close().await;
            expected_server.await.unwrap();
            other_server.await.unwrap();
            directory.close().await.unwrap();
            if let Some(relay) = relay {
                relay.shutdown().await.unwrap();
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
