//! Adversarial peers use real transport streams and synthetic isolated files.
use rds_core::{read_frame, write_frame};
use rds_net::{Backend, Connection, Endpoint, EndpointConfig, bind_endpoint};
use rds_sync::{Manifest, engine, manifest_of, proto::SyncMsg};
use std::{path::PathBuf, time::Duration};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rds-protocol-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn pair(backend: Backend) -> (Endpoint, Endpoint, Connection, Connection) {
    let config = || EndpointConfig {
        backend,
        discovery: false,
        bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
        ..Default::default()
    };
    let a = bind_endpoint(config()).await.unwrap();
    let b = bind_endpoint(config()).await.unwrap();
    let (outgoing, incoming) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
        b.accept().await.unwrap().await
    });
    (a, b, outgoing.unwrap(), incoming.unwrap())
}
async fn offered(send: &mut rds_net::SendStream, path: &str, m: &Manifest) {
    write_frame(
        send,
        &SyncMsg::Offer {
            rel_path: path.into(),
            size: m.size,
            root: m.root,
            chunk_count: m.chunks.len() as u32,
        },
    )
    .await
    .unwrap();
    if !m.chunks.is_empty() {
        write_frame(
            send,
            &SyncMsg::ManifestPart {
                chunks: m.chunks.clone(),
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_refuses_a_different_safe_path_before_any_destination_write() {
    for backend in [Backend::Iroh, Backend::Noq] {
        let tmp = Temp::new();
        std::fs::write(tmp.0.join("unrelated"), b"keep this file").unwrap();
        let (a, b, client, server) = pair(backend).await;
        let peer = tokio::spawn(async move {
            let (mut send, mut recv) = server.accept_bi().await.unwrap();
            assert!(matches!(
                read_frame::<_, SyncMsg>(&mut recv).await.unwrap(),
                SyncMsg::Request { .. }
            ));
            offered(&mut send, "unrelated", &manifest_of(b"")).await;
            let _ = read_frame::<_, SyncMsg>(&mut recv).await;
            let _ = read_frame::<_, SyncMsg>(&mut recv).await;
        });
        let (send, recv) = client.open_bi().await.unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            engine::recv_file(&client, "requested", &tmp.0, send, recv),
        )
        .await
        .unwrap();
        peer.abort();
        a.close().await;
        b.close().await;
        assert!(result.is_err(), "{backend:?} accepted a substituted path");
        assert_eq!(
            std::fs::read(tmp.0.join("unrelated")).unwrap(),
            b"keep this file"
        );
        assert!(!tmp.0.join("requested").exists());
        assert!(!tmp.0.join(".rds-sync").exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_sender_refuses_done_for_a_different_root() {
    for backend in [Backend::Iroh, Backend::Noq] {
        let tmp = Temp::new();
        std::fs::write(tmp.0.join("empty"), b"").unwrap();
        let (a, b, client, server) = pair(backend).await;
        let path = tmp.0.clone();
        let peer = tokio::spawn(async move {
            let (send, recv) = server.accept_bi().await.unwrap();
            engine::serve(server, send, recv, path).await
        });
        let (mut send, mut recv) = client.open_bi().await.unwrap();
        write_frame(
            &mut send,
            &SyncMsg::Request {
                rel_path: "empty".into(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame::<_, SyncMsg>(&mut recv).await.unwrap(),
            SyncMsg::Offer { chunk_count: 0, .. }
        ));
        write_frame(&mut send, &SyncMsg::Need { bits: vec![] })
            .await
            .unwrap();
        write_frame(&mut send, &SyncMsg::Done { root: [42; 32] })
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), peer)
            .await
            .unwrap()
            .unwrap();
        a.close().await;
        b.close().await;
        assert!(result.is_err(), "{backend:?} accepted a foreign Done root");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_refuses_noncanonical_need_bitmaps() {
    for bits in [vec![], vec![0, 0], vec![2]] {
        let tmp = Temp::new();
        let path = tmp.0.join("source");
        std::fs::write(&path, b"chunk").unwrap();
        let (a, b, client, server) = pair(Backend::Noq).await;
        let peer = tokio::spawn(async move {
            let (mut send, mut recv) = server.accept_bi().await.unwrap();
            let SyncMsg::Offer { root, .. } = read_frame(&mut recv).await.unwrap() else {
                panic!("offer")
            };
            assert!(matches!(
                read_frame::<_, SyncMsg>(&mut recv).await.unwrap(),
                SyncMsg::ManifestPart { .. }
            ));
            write_frame(&mut send, &SyncMsg::Need { bits })
                .await
                .unwrap();
            write_frame(&mut send, &SyncMsg::Done { root })
                .await
                .unwrap();
            let _ = read_frame::<_, SyncMsg>(&mut recv).await;
        });
        let (send, recv) = client.open_bi().await.unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            engine::send_file(&client, &path, send, recv),
        )
        .await
        .unwrap();
        peer.abort();
        a.close().await;
        b.close().await;
        assert!(
            result.is_err(),
            "noncanonical Need was silently truncated/padded"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_refuses_empty_manifest_batches_promptly() {
    let tmp = Temp::new();
    let (a, b, client, server) = pair(Backend::Noq).await;
    let path = tmp.0.clone();
    let mut peer = tokio::spawn(async move {
        let (send, recv) = server.accept_bi().await.unwrap();
        engine::serve(server, send, recv, path).await
    });
    let (mut send, _recv) = client.open_bi().await.unwrap();
    let m = manifest_of(b"chunk");
    write_frame(
        &mut send,
        &SyncMsg::Offer {
            rel_path: "target".into(),
            size: m.size,
            root: m.root,
            chunk_count: 1,
        },
    )
    .await
    .unwrap();
    write_frame(&mut send, &SyncMsg::ManifestPart { chunks: vec![] })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), &mut peer).await;
    peer.abort();
    a.close().await;
    b.close().await;
    assert!(
        matches!(result, Ok(Ok(Err(_)))),
        "empty batch held the receiver open"
    );
    assert!(!tmp.0.join("target").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunk_batches_must_be_nonempty_requested_unique_and_match_manifest() {
    for kind in [
        "empty-set",
        "empty-stream",
        "oversized-set",
        "duplicate",
        "unrequested",
        "hash",
        "body",
    ] {
        let tmp = Temp::new();
        std::fs::write(tmp.0.join("target"), b"previous content").unwrap();
        let m = manifest_of(b"x");
        let (a, b, client, server) = pair(Backend::Noq).await;
        let path = tmp.0.clone();
        let mut peer = tokio::spawn(async move {
            let (send, recv) = server.accept_bi().await.unwrap();
            engine::serve(server, send, recv, path).await
        });
        let (mut send, mut recv) = client.open_bi().await.unwrap();
        offered(&mut send, "target", &m).await;
        assert!(matches!(
            read_frame::<_, SyncMsg>(&mut recv).await.unwrap(),
            SyncMsg::Need { .. }
        ));
        let mut stream = client.open_uni().await.unwrap();
        write_frame(&mut stream, &rds_core::UniHello::Sync)
            .await
            .unwrap();
        if kind == "empty-stream" {
            write_frame(&mut stream, &SyncMsg::SetDone).await.unwrap();
        } else {
            let indices = match kind {
                "empty-set" => vec![],
                "oversized-set" => vec![0; rds_sync::proto::CHUNKSET_BATCH + 1],
                "duplicate" => vec![0, 0],
                "unrequested" => vec![1],
                _ => vec![0],
            };
            write_frame(&mut stream, &SyncMsg::ChunkSet { indices })
                .await
                .unwrap();
            if matches!(kind, "duplicate" | "hash" | "body") {
                write_frame(
                    &mut stream,
                    &SyncMsg::ChunkHdr {
                        index: 0,
                        len: 1,
                        hash: if kind == "hash" {
                            [42; 32]
                        } else {
                            m.chunks[0].hash
                        },
                    },
                )
                .await
                .unwrap();
                let _ = stream
                    .write_all(if kind == "body" { b"y" } else { b"x" })
                    .await;
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(2), &mut peer).await;
        peer.abort();
        a.close().await;
        b.close().await;
        assert!(
            matches!(result, Ok(Ok(Err(_)))),
            "{kind} did not fail promptly"
        );
        assert_eq!(
            std::fs::read(tmp.0.join("target")).unwrap(),
            b"previous content"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if rds_sync::journal::Journal::open(&tmp.0, "target", &m).is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("failed chunk receive retained journal ownership");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_manifest_batch_is_refused_before_journal_creation() {
    let tmp = Temp::new();
    let (a, b, client, server) = pair(Backend::Noq).await;
    let path = tmp.0.clone();
    let mut peer = tokio::spawn(async move {
        let (send, recv) = server.accept_bi().await.unwrap();
        engine::serve(server, send, recv, path).await
    });
    let m = tiny_chunks(rds_sync::proto::MANIFEST_BATCH + 1);
    let (mut send, _recv) = client.open_bi().await.unwrap();
    offered(&mut send, "target", &m).await;
    let result = tokio::time::timeout(Duration::from_secs(2), &mut peer).await;
    peer.abort();
    a.close().await;
    b.close().await;
    assert!(matches!(result, Ok(Ok(Err(_)))));
    assert!(!tmp.0.join(".rds-sync").exists());
}

fn tiny_chunks(count: usize) -> Manifest {
    Manifest {
        size: count as u64,
        root: *blake3::hash(&vec![b'x'; count]).as_bytes(),
        chunks: (0..count)
            .map(|i| rds_sync::Chunk {
                offset: i as u64,
                len: 1,
                hash: *blake3::hash(b"x").as_bytes(),
            })
            .collect(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuous_valid_progress_cannot_extend_the_absolute_session_budget() {
    let tmp = Temp::new();
    let (a, b, client, server) = pair(Backend::Noq).await;
    let path = tmp.0.clone();
    let mut peer = tokio::spawn(async move {
        let (send, recv) = server.accept_bi().await.unwrap();
        engine::serve_with_timeout(server, send, recv, path, Duration::from_millis(250)).await
    });
    let (mut send, _recv) = client.open_bi().await.unwrap();
    let m = tiny_chunks(100);
    write_frame(
        &mut send,
        &SyncMsg::Offer {
            rel_path: "target".into(),
            size: m.size,
            root: m.root,
            chunk_count: 100,
        },
    )
    .await
    .unwrap();
    let writer = tokio::spawn(async move {
        for chunk in m.chunks {
            if write_frame(
                &mut send,
                &SyncMsg::ManifestPart {
                    chunks: vec![chunk],
                },
            )
            .await
            .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(2), &mut peer).await;
    writer.abort();
    peer.abort();
    a.close().await;
    b.close().await;
    let error = result.unwrap().unwrap().unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error:#}");
    assert!(!tmp.0.join(".rds-sync").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_and_pull_absolute_budgets_end_silent_peer_sessions() {
    for push in [true, false] {
        let tmp = Temp::new();
        let path = tmp.0.join("source");
        std::fs::write(&path, b"data").unwrap();
        let (a, b, client, server) = pair(Backend::Noq).await;
        let peer = tokio::spawn(async move {
            let (_send, mut recv) = server.accept_bi().await.unwrap();
            let _ = read_frame::<_, SyncMsg>(&mut recv).await.unwrap();
            std::future::pending::<()>().await;
        });
        let (send, recv) = client.open_bi().await.unwrap();
        let result = if push {
            engine::send_file_with_timeout(&client, &path, send, recv, Duration::from_millis(150))
                .await
                .map(|_| ())
        } else {
            engine::recv_file_with_timeout(
                &client,
                "target",
                &tmp.0,
                send,
                recv,
                Duration::from_millis(150),
            )
            .await
            .map(|_| ())
        };
        peer.abort();
        a.close().await;
        b.close().await;
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(!tmp.0.join("target").exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_reuse_reports_zero_wire_bytes_and_normalized_pull_names_match() {
    let tmp = Temp::new();
    std::fs::write(tmp.0.join("file"), b"shared bytes").unwrap();
    let (a, b, client, server) = pair(Backend::Noq).await;
    let path = tmp.0.clone();
    let peer = tokio::spawn(async move {
        for _ in 0..2 {
            let (send, recv) = server.accept_bi().await.unwrap();
            engine::serve(server.clone(), send, recv, path.clone())
                .await
                .unwrap();
        }
    });
    let (send, recv) = client.open_bi().await.unwrap();
    let stats = engine::send_file(&client, &tmp.0.join("file"), send, recv)
        .await
        .unwrap();
    assert_eq!((stats.fetched, stats.bytes), (0, 0));
    let dest = Temp::new();
    let (send, recv) = client.open_bi().await.unwrap();
    let (file, stats) = engine::recv_file(&client, "./file", &dest.0, send, recv)
        .await
        .unwrap();
    assert_eq!(std::fs::read(file).unwrap(), b"shared bytes");
    assert_eq!(stats.bytes, 12);
    peer.await.unwrap();
    a.close().await;
    b.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_local_names_and_zero_budget_fail_before_transfer() {
    use std::os::unix::ffi::OsStringExt;
    let tmp = Temp::new();
    let (a, b, client, _server) = pair(Backend::Noq).await;
    for name in [
        std::ffi::OsString::from("a\\b"),
        std::ffi::OsString::from_vec(vec![0xff]),
    ] {
        let path = tmp.0.join(name);
        let (send, recv) = client.open_bi().await.unwrap();
        let err = engine::send_file(&client, &path, send, recv)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("name"), "{err:#}");
    }
    let (send, recv) = client.open_bi().await.unwrap();
    let err =
        engine::send_file_with_timeout(&client, &tmp.0.join("absent"), send, recv, Duration::ZERO)
            .await
            .unwrap_err();
    assert!(err.to_string().contains("invalid transfer timeout"));
    assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), 0);
    a.close().await;
    b.close().await;
}
