//! Managed file transfers over real IPC and QUIC. Fixtures own all tasks.
use rds_agent::{Agent, AgentPolicy};
use rds_client::local::{Client, Error, Prepared, Server};
use rds_core::local::{Command, ErrorCode, Reply, SessionId, SyncOperation};
use rds_net::{Backend, EndpointConfig, Ticket, bind_endpoint};
use std::{path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinSet;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::path::Path::new("/tmp")
            .canonicalize()
            .unwrap()
            .join(format!(
                "rds-local-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(backend: Backend) -> EndpointConfig {
    EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    }
}

fn backends() -> Vec<Backend> {
    #[cfg(feature = "transport-noq")]
    {
        vec![Backend::Iroh, Backend::Noq]
    }
    #[cfg(not(feature = "transport-noq"))]
    {
        vec![Backend::Iroh]
    }
}

async fn connect(client: &Client, target: String) -> SessionId {
    match client
        .request(Command::Connect {
            target,
            grant: None,
        })
        .await
        .unwrap()
    {
        Reply::Connected(id) => id,
        reply => panic!("unexpected {reply:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_transfers_pin_devices_and_reuse_the_agent_identity() {
    for backend in backends() {
        let root = Scratch::new();
        let local = bind_endpoint(config(backend)).await.unwrap();
        let path = root.0.join("control");
        let mut manager = Server::start(
            Some(Prepared::bind(&path).await.unwrap()),
            local.clone(),
            None,
        );
        let client = Client::new(&path);
        let mut tasks = JoinSet::new();
        let mut peers = Vec::new();
        for index in 0..2 {
            let endpoint = bind_endpoint(config(backend)).await.unwrap();
            let dir = root.0.join(format!("peer-{index}"));
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("marker"), [index]).unwrap();
            let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
            policy.allow.insert(local.id());
            policy.sync_dir = Some(dir.clone());
            let agent = Agent::new(endpoint.clone(), policy);
            tasks.spawn(async move {
                agent.run().await.unwrap();
            });
            let id = connect(&client, Ticket::of(&endpoint).to_string()).await;
            peers.push((endpoint, dir, id));
        }
        let first = peers[0].2;
        let second = peers[1].2;
        let bytes: Vec<u8> = (0..1024 * 1024).map(|i| (i * 73 + i / 251) as u8).collect();
        let source = root.0.join("payload.bin");
        std::fs::write(&source, &bytes).unwrap();
        client
            .request(Command::Select { session: second })
            .await
            .unwrap();
        let before = client.snapshot().await.unwrap();
        let stats = client.send_file(first, &source).await.unwrap();
        assert_eq!(stats.bytes, bytes.len() as u64);
        assert_eq!(
            std::fs::read(peers[0].1.join("payload.bin")).unwrap(),
            bytes
        );
        assert!(!peers[1].1.join("payload.bin").exists());
        // Immediate sequential transfer uses a fresh route and resumes verified content.
        let stats = client.send_file(first, &source).await.unwrap();
        assert_eq!(stats.bytes, 0);
        let dest = root.0.join("download");
        client.recv_file(first, "payload.bin", &dest).await.unwrap();
        assert_eq!(std::fs::read(dest.join("payload.bin")).unwrap(), bytes);
        let selected = client.selected(None).await.unwrap();
        client.recv_file(selected, "marker", &dest).await.unwrap();
        assert_eq!(std::fs::read(dest.join("marker")).unwrap(), [1]);
        let after = client.snapshot().await.unwrap();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.endpoint, local.id().to_string());
        assert_eq!(after.sessions.len(), 2);
        assert_eq!(after.selected, Some(second));
        // Invalid requests never create a destination or mutate the active selection.
        for operation in [
            SyncOperation::Send {
                path: "relative.bin".into(),
            },
            SyncOperation::Recv {
                rel_path: "../escape".into(),
                directory: dest.to_str().unwrap().into(),
            },
            SyncOperation::Recv {
                rel_path: "marker".into(),
                directory: "relative".into(),
            },
        ] {
            assert!(matches!(
                client
                    .request(Command::Sync {
                        session: first,
                        operation
                    })
                    .await,
                Err(Error::Rejected(ErrorCode::InvalidRequest))
            ));
        }
        client
            .request(Command::Disconnect { session: first })
            .await
            .unwrap();
        assert!(matches!(
            client.send_file(first, &source).await,
            Err(Error::Rejected(ErrorCode::NotFound))
        ));
        assert!(matches!(
            client
                .request(Command::Ping {
                    session: Some(second),
                    nonce: 17
                })
                .await
                .unwrap(),
            Reply::Pong { .. }
        ));
        manager.close().await.unwrap();
        local.close().await;
        for (peer, _, _) in peers {
            peer.close().await;
        }
        tasks.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_transfer_releases_its_slot_without_closing_tcp_or_the_session() {
    use rds_core::{HelloAck, StreamHello, read_frame, write_frame};
    for (backend, upload) in backends()
        .into_iter()
        .flat_map(|b| [false, true].map(move |upload| (b, upload)))
    {
        let root = Scratch::new();
        let local = bind_endpoint(config(backend)).await.unwrap();
        let remote = bind_endpoint(config(backend)).await.unwrap();
        let path = root.0.join("control");
        let mut manager = Server::start(
            Some(Prepared::bind(&path).await.unwrap()),
            local.clone(),
            None,
        );
        let client = Client::new(&path);
        let source = root.0.join("source");
        std::fs::create_dir(&source).unwrap();
        let remote_file = source.join("file");
        std::fs::write(
            &remote_file,
            if upload {
                b"previous bytes".as_slice()
            } else {
                b"after cancellation".as_slice()
            },
        )
        .unwrap();
        let (started, mut starts) = tokio::sync::mpsc::channel(2);
        let (canceled, mut cancels) = tokio::sync::mpsc::channel(1);
        let remote_task = remote.clone();
        let service = tokio::spawn(async move {
            let conn = remote_task.accept().await.unwrap().await.unwrap();
            let mut workers = JoinSet::new();
            let mut first_id = None;
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                let hello: StreamHello = read_frame(&mut recv).await.unwrap();
                match hello {
                    StreamHello::Ping { nonce } => {
                        write_frame(&mut send, &HelloAck::Ok).await.unwrap();
                        send.write_all(&nonce.to_be_bytes()).await.unwrap();
                        send.finish().unwrap();
                    }
                    StreamHello::TcpConnect { .. } => {
                        write_frame(&mut send, &HelloAck::Ok).await.unwrap();
                        workers.spawn(async move {
                            let _ = tokio::io::copy(&mut recv, &mut send).await;
                            let _ = send.finish();
                        });
                    }
                    StreamHello::SyncTransfer { id } => {
                        write_frame(&mut send, &HelloAck::Ok).await.unwrap();
                        let started = started.clone();
                        if let Some(old) = first_id {
                            assert_ne!(id, old, "replacement reused a transfer route");
                            let conn = conn.clone();
                            let source = source.clone();
                            workers.spawn(async move {
                                started.send(id).await.unwrap();
                                rds_sync::engine::Transfer::new(id)
                                    .serve(
                                        conn,
                                        (send, recv),
                                        source,
                                        rds_sync::engine::Access::READ_WRITE,
                                        Duration::from_secs(10),
                                    )
                                    .await
                                    .unwrap();
                            });
                        } else {
                            first_id = Some(id);
                            let canceled = canceled.clone();
                            workers.spawn(async move {
                                let _: rds_sync::proto::SyncMsg =
                                    read_frame(&mut recv).await.unwrap();
                                started.send(id).await.unwrap();
                                let _ = send.stopped().await;
                                canceled.send(()).await.unwrap();
                            });
                        }
                    }
                    _ => panic!("unexpected service"),
                }
            }
            workers.shutdown().await;
        });
        let session = connect(&client, Ticket::of(&remote).to_string()).await;
        let mut tcp = client
            .open_tcp(session, "127.0.0.1:22".parse().unwrap())
            .await
            .unwrap();
        let worker_client = client.clone();
        let destination = root.0.join("destination");
        if upload {
            std::fs::create_dir(&destination).unwrap();
            std::fs::write(destination.join("file"), b"after cancellation").unwrap();
        }
        let worker_dest = destination.clone();
        let worker = tokio::spawn(async move {
            if upload {
                worker_client
                    .send_file(session, &worker_dest.join("file"))
                    .await
            } else {
                worker_client.recv_file(session, "file", &worker_dest).await
            }
        });
        tokio::time::timeout(Duration::from_secs(3), starts.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            client.recv_file(session, "file", &destination).await,
            Err(Error::Rejected(ErrorCode::TransferBusy))
        ));
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(3), cancels.recv())
            .await
            .unwrap()
            .unwrap();
        let stats = if upload {
            client
                .send_file(session, &destination.join("file"))
                .await
                .unwrap()
        } else {
            client
                .recv_file(session, "file", &destination)
                .await
                .unwrap()
        };
        assert_eq!(stats.bytes, 18);
        assert_eq!(
            std::fs::read(if upload {
                remote_file
            } else {
                destination.join("file")
            })
            .unwrap(),
            b"after cancellation"
        );
        tcp.write_all(b"still connected").await.unwrap();
        let mut echo = [0; 15];
        tokio::time::timeout(Duration::from_secs(3), tcp.read_exact(&mut echo))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&echo, b"still connected");
        assert_eq!(client.snapshot().await.unwrap().sessions.len(), 1);
        tcp.close().await;
        manager.close().await.unwrap();
        local.close().await;
        remote.close().await;
        service.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_transfer_extension_preserves_directional_grant_enforcement() {
    use ed25519_dalek::SigningKey;
    use rds_core::{ServiceKind, grant::Grant};
    for backend in backends() {
        let root = Scratch::new();
        let local = bind_endpoint(config(backend)).await.unwrap();
        let remote = bind_endpoint(config(backend)).await.unwrap();
        let source = root.0.join("upload");
        std::fs::write(&source, b"scoped payload").unwrap();
        let remote_root = root.0.join("remote");
        std::fs::create_dir(&remote_root).unwrap();
        std::fs::write(remote_root.join("download"), b"scoped payload").unwrap();
        let issuer = SigningKey::from_bytes(&[71; 32]);
        let mut policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        policy.allow.insert(local.id());
        policy.issuers.insert(issuer.verifying_key().to_bytes());
        policy.sync_dir = Some(remote_root.clone());
        policy.use_local_revocations();
        let agent = Agent::new(remote.clone(), policy);
        let service = tokio::spawn(async move {
            agent.run().await.unwrap();
        });
        let path = root.0.join("control");
        let mut manager = Server::start(
            Some(Prepared::bind(&path).await.unwrap()),
            local.clone(),
            None,
        );
        let client = Client::new(&path);
        for (index, scope) in [
            ServiceKind::SyncRead,
            ServiceKind::SyncWrite,
            ServiceKind::Ping,
        ]
        .into_iter()
        .enumerate()
        {
            let grant = Grant::issue(
                &issuer,
                *local.id().as_bytes(),
                *remote.id().as_bytes(),
                [index as u8; 16],
                vec![scope],
                Duration::from_secs(120),
                Default::default(),
            );
            let Reply::Connected(session) = client
                .request(Command::Connect {
                    target: Ticket::of(&remote).to_string(),
                    grant: Some(Box::new(grant)),
                })
                .await
                .unwrap()
            else {
                panic!("unexpected reply");
            };
            let destination = root.0.join(format!("destination-{index}"));
            if scope == ServiceKind::SyncRead {
                client
                    .recv_file(session, "download", &destination)
                    .await
                    .unwrap();
                assert_eq!(
                    std::fs::read(destination.join("download")).unwrap(),
                    b"scoped payload"
                );
                assert!(client.send_file(session, &source).await.is_err());
                assert!(!remote_root.join("upload").exists());
            } else if scope == ServiceKind::SyncWrite {
                client.send_file(session, &source).await.unwrap();
                assert_eq!(
                    std::fs::read(remote_root.join("upload")).unwrap(),
                    b"scoped payload"
                );
                assert!(
                    client
                        .recv_file(session, "download", &destination)
                        .await
                        .is_err()
                );
                assert!(!destination.exists());
            } else {
                assert!(matches!(
                    client.recv_file(session, "download", &destination).await,
                    Err(Error::Rejected(ErrorCode::Remote))
                ));
                assert!(!destination.exists());
            }
            assert_eq!(client.snapshot().await.unwrap().sessions.len(), 1);
            client
                .request(Command::Disconnect { session })
                .await
                .unwrap();
        }
        manager.close().await.unwrap();
        local.close().await;
        remote.close().await;
        service.await.unwrap();
    }
}
