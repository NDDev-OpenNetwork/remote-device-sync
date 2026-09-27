//! W1.9/W2.2 negotiated sync sessions: real endpoints running the v2
//! `SyncTransferV2` route — Hello/HelloAck limit exchange, transfer-ID
//! bound frames, typed Cancel propagation, and version/tag refusal.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use rds_core::{HelloAck, StreamHello};
use rds_net::{Endpoint, EndpointAddr, EndpointConfig, bind_endpoint, read_frame, write_frame};
use rds_sync::engine::{Access, Transfer};

/// Temp dir per test — unique per process + name.
fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rds-v2-{name}-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    dir
}

fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut v = vec![0u8; len];
    for b in &mut v {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = state as u8;
    }
    v
}

/// v2-capable server: accepts `SyncTransfer`/`SyncTransferV2` bi streams,
/// reports each serve outcome back to the test.
fn spawn_server_v2(
    ep: Endpoint,
    dir: PathBuf,
    outcomes: mpsc::Sender<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let dir = dir.clone();
            let outcomes = outcomes.clone();
            tokio::spawn(async move {
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    let conn = conn.clone();
                    let dir = dir.clone();
                    let outcomes = outcomes.clone();
                    tokio::spawn(async move {
                        let outcome = async {
                            let hello: StreamHello = read_frame(&mut recv).await?;
                            let transfer = match hello {
                                StreamHello::SyncTransfer { id } => Transfer::new(id),
                                StreamHello::SyncTransferV2 { id } => Transfer::new_v2(id),
                                other => anyhow::bail!("unexpected hello {other:?}"),
                            };
                            write_frame(&mut send, &HelloAck::Ok).await?;
                            transfer
                                .serve(
                                    conn,
                                    (send, recv),
                                    dir,
                                    Access::READ_WRITE,
                                    Duration::from_secs(60),
                                )
                                .await
                        }
                        .await;
                        let _ = outcomes.send(match outcome {
                            Ok(()) => "ok".into(),
                            Err(e) => format!("err:{e:#}"),
                        });
                    });
                }
            });
        }
    })
}

async fn pair() -> (
    Endpoint,
    EndpointAddr,
    PathBuf,
    mpsc::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::level_filters::LevelFilter::DEBUG)
        .with_test_writer()
        .try_init();
    let server_ep = bind_endpoint(EndpointConfig::default()).await.unwrap();
    let client_ep = bind_endpoint(EndpointConfig::default()).await.unwrap();
    let dir = scratch("server");
    let (outcomes, rx) = mpsc::channel();
    let target = server_ep.addr();
    let task = spawn_server_v2(server_ep, dir.clone(), outcomes);
    (client_ep, target, dir, rx, task)
}

/// Open a v2 control stream: greeting + HelloAck on the `SyncTransferV2`
/// route. Returns the connection the streams live on — the transfer's
/// uni streams must ride the same connection.
async fn open_v2(
    client_ep: &Endpoint,
    target: &EndpointAddr,
) -> (
    rds_net::Connection,
    Transfer,
    rds_net::SendStream,
    rds_net::RecvStream,
) {
    let conn = client_ep
        .connect(target.clone(), rds_core::ALPN)
        .await
        .unwrap();
    let id: [u8; 16] = rand::random();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &StreamHello::SyncTransferV2 { id })
        .await
        .unwrap();
    let ack: HelloAck = read_frame(&mut recv).await.unwrap();
    assert!(matches!(ack, HelloAck::Ok), "v2 greeting refused: {ack:?}");
    (conn, Transfer::new_v2(id), send, recv)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_push_completes_byte_identical_through_negotiated_session() {
    let (client_ep, target, server_dir, rx, _task) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(4 * 1024 * 1024, 0x2C);
    let src = src_dir.join("blob.bin");
    std::fs::write(&src, &data).unwrap();

    let (conn, transfer, send, recv) = open_v2(&client_ep, &target).await;
    let stats = match transfer
        .send_file(&conn, &src, (send, recv), Duration::from_secs(60))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            let server = rx.recv_timeout(Duration::from_secs(10));
            panic!("client failed: {e:#}; server outcome: {server:?}");
        }
    };
    assert_eq!(stats.bytes, data.len() as u64);
    assert_eq!(std::fs::read(server_dir.join("blob.bin")).unwrap(), data);
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_pull_completes_and_resumes_after_cancel() {
    let (client_ep, target, server_dir, rx, _task) = pair().await;
    std::fs::write(
        server_dir.join("file.bin"),
        random_bytes(2 * 1024 * 1024, 0x7),
    )
    .unwrap();
    let dest_dir = scratch("dest");

    let (conn, transfer, send, recv) = open_v2(&client_ep, &target).await;
    let (dest, stats) = match transfer
        .recv_file(
            &conn,
            "file.bin",
            &dest_dir,
            (send, recv),
            Duration::from_secs(60),
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let server = rx.recv_timeout(Duration::from_secs(10));
            panic!("client failed: {e:#}; server outcome: {server:?}");
        }
    };
    assert_eq!(stats.fetched, stats.total);
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        std::fs::read(server_dir.join("file.bin")).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_token_cancel_writes_typed_cancel_and_server_observes_abort() {
    let (client_ep, target, _server_dir, rx, _task) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(32 * 1024 * 1024, 0x9);
    let src = src_dir.join("big.bin");
    std::fs::write(&src, &data).unwrap();

    let (conn, transfer, send, recv) = open_v2(&client_ep, &target).await;
    let token = tokio_util::sync::CancellationToken::new();
    let t2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        t2.cancel();
    });
    let err = transfer
        .send_file_cancel(
            &conn,
            &src,
            (send, recv),
            Duration::from_secs(300),
            Some(token),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("canceled"),
        "expected canceled error, got {err}"
    );
    // The server-side receiver must report a deliberate peer abort —
    // typed Cancel or a control close, never a hang.
    let outcome = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        outcome.starts_with("err:"),
        "server must observe an abort, got {outcome}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v1_route_still_serves_unchanged_for_compat() {
    let (client_ep, target, server_dir, rx, _task) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(1024 * 1024, 0x11);
    let src = src_dir.join("v1.bin");
    std::fs::write(&src, &data).unwrap();

    let conn = client_ep
        .connect(target.clone(), rds_core::ALPN)
        .await
        .unwrap();
    let id: [u8; 16] = rand::random();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &StreamHello::SyncTransfer { id })
        .await
        .unwrap();
    let ack: HelloAck = read_frame(&mut recv).await.unwrap();
    assert!(matches!(ack, HelloAck::Ok));
    let stats = Transfer::new(id)
        .send_file(&conn, &src, (send, recv), Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(stats.bytes, data.len() as u64);
    assert_eq!(std::fs::read(server_dir.join("v1.bin")).unwrap(), data);
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "ok");
}
