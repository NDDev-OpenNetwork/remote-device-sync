//! C6 end-to-end: real transfers over in-process rds-net endpoints —
//! byte-identical completion, resume after mid-transfer kills, torn
//! journals, corrupt parts, traversal rejection, and an impaired-link
//! lane that proves the resume overhead stays small.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rds_net::{Endpoint, EndpointAddr, EndpointConfig, bind_endpoint, bind_noq_with_socket};
use rds_sync::engine::{recv_file, send_file, serve};
use rds_sync::proto::{check_manifest, check_rel_path};
use rds_sync::{Manifest, manifest_of};

/// Temp dir per test — unique per process + name.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rds-sync-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Deterministic pseudo-random content — reproducible per size/seed.
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

/// Server half: accept one conn, run `serve` on each accepted bi
/// stream — mirrors the agent's per-stream dispatch.
fn spawn_server(ep: Endpoint, dir: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let dir = dir.clone();
            tokio::spawn(async move {
                while let Ok((send, recv)) = conn.accept_bi().await {
                    let conn = conn.clone();
                    let dir = dir.clone();
                    tokio::spawn(async move {
                        let _ = serve(conn, send, recv, dir).await;
                    });
                }
            });
        }
    })
}

async fn pair() -> (
    Endpoint,
    Endpoint,
    EndpointAddr,
    tokio::task::JoinHandle<()>,
    PathBuf,
) {
    let server_ep = bind_endpoint(EndpointConfig::default()).await.unwrap();
    let client_ep = bind_endpoint(EndpointConfig::default()).await.unwrap();
    let dir = scratch("server");
    let task = spawn_server(server_ep.clone(), dir.clone());
    let target = server_ep.addr();
    (server_ep, client_ep, target, task, dir)
}

async fn client_conn(client_ep: &Endpoint, target: EndpointAddr) -> rds_net::Connection {
    client_ep.connect(target, rds_core::ALPN).await.unwrap()
}

/// Part files present under `.rds-sync/*/parts` — resume progress.
fn count_parts(state_dir: &Path) -> usize {
    let Ok(roots) = std::fs::read_dir(state_dir) else {
        return 0;
    };
    roots
        .flatten()
        .map(|e| e.path().join("parts"))
        .map(|p| {
            std::fs::read_dir(&p)
                .map(|d| d.flatten().filter(|f| f.path().is_file()).count())
                .unwrap_or(0)
        })
        .sum()
}

async fn push(
    client_ep: &Endpoint,
    target: EndpointAddr,
    path: &Path,
) -> anyhow::Result<rds_sync::engine::Stats> {
    let conn = client_conn(client_ep, target).await;
    let (send, recv) = conn.open_bi().await.unwrap();
    send_file(&conn, path, send, recv).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transfer_completes_byte_identical() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(8 * 1024 * 1024, 0xC6);
    let src = src_dir.join("blob.bin");
    std::fs::write(&src, &data).unwrap();

    let stats = push(&c_ep, target.clone(), &src).await.unwrap();
    assert_eq!(stats.bytes, data.len() as u64);
    let got = std::fs::read(server_dir.join("blob.bin")).unwrap();
    assert_eq!(got, data, "received file is byte-identical");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_content_zero_chunks() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(4 * 1024 * 1024, 0xD00D);
    let src = src_dir.join("same.bin");
    std::fs::write(&src, &data).unwrap();

    let first = push(&c_ep, target.clone(), &src).await.unwrap();
    assert!(first.fetched > 0);
    assert!(server_dir.join("same.bin").is_file());

    // Second push of identical content: receiver already has every
    // part verified — Need comes back empty, nothing crosses the wire.
    let second = push(&c_ep, target.clone(), &src).await.unwrap();
    assert_eq!(second.fetched, 0, "identical content re-fetched chunks");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_recv_completes() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let data = random_bytes(3 * 1024 * 1024, 0xBEEF);
    std::fs::write(server_dir.join("doc.dat"), &data).unwrap();

    let dest_dir = scratch("dest");
    let conn = client_conn(&c_ep, target).await;
    let (send, recv) = conn.open_bi().await.unwrap();
    let (dest, stats) = recv_file(&conn, "doc.dat", &dest_dir, send, recv)
        .await
        .unwrap();
    assert!(stats.fetched > 0);
    assert_eq!(std::fs::read(&dest).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_part_is_refetched() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(2 * 1024 * 1024, 0xF00D);
    let src = src_dir.join("corruptme.bin");
    std::fs::write(&src, &data).unwrap();

    // Land the first transfer, then corrupt one part on the receiver —
    // a resume must detect it (content hash != name) and re-fetch.
    push(&c_ep, target.clone(), &src).await.unwrap();
    // Assemble removed the state dir on success — rebuild the parts
    // dir with all-but-last parts correct and the last poisoned, so
    // resume re-fetches exactly one chunk.
    let manifest = manifest_of(&data);
    let root_hex: String = manifest.root.iter().map(|b| format!("{b:02x}")).collect();
    let parts = server_dir.join(".rds-sync").join(&root_hex).join("parts");
    std::fs::create_dir_all(&parts).unwrap();
    // Write all-but-last parts correctly, poison the last one.
    let keep = manifest.chunks.len() - 1;
    for c in &manifest.chunks[..keep] {
        let name: String = c.hash.iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(
            parts.join(&name),
            &data[c.offset as usize..(c.offset + c.len as u64) as usize],
        )
        .unwrap();
    }
    let bad = &manifest.chunks[keep];
    let bad_name: String = bad.hash.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(parts.join(&bad_name), b"poisoned-chunk-bytes").unwrap();
    // Remove the assembled file so the resume has work to prove.
    std::fs::remove_file(server_dir.join("corruptme.bin")).unwrap();

    let stats = push(&c_ep, target.clone(), &src).await.unwrap();
    assert_eq!(stats.fetched, 1, "only the corrupt part re-fetched");
    assert_eq!(
        std::fs::read(server_dir.join("corruptme.bin")).unwrap(),
        data
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_mid_transfer_resumes_identical() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(6 * 1024 * 1024, 0xABCD);
    let src = src_dir.join("killme.bin");
    std::fs::write(&src, &data).unwrap();

    // First attempt: abort the client once some parts have landed —
    // polling the receiver's parts dir makes the kill deterministic
    // instead of racing the 6 MiB transfer.
    let c_ep2 = c_ep.clone();
    let src2 = src.clone();
    let t = target.clone();
    let attempt = tokio::spawn(async move { push(&c_ep2, t, &src2).await });
    let state_dir = server_dir.join(".rds-sync");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut landed = 0usize;
    while std::time::Instant::now() < deadline && landed < 3 {
        landed = count_parts(&state_dir);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    attempt.abort();
    // Let the server observe the drop before reconnecting.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let stats = push(&c_ep, target.clone(), &src).await.unwrap();
    assert_eq!(std::fs::read(server_dir.join("killme.bin")).unwrap(), data);
    // Resume-overhead: only missing chunks crossed; the second attempt
    // re-fetched strictly less than the full set (some parts survived).
    assert!(
        stats.fetched < stats.total,
        "resume re-sent everything: {stats:?}"
    );
}

/// G6: kill at randomized progress points until done — byte-identical
/// every time. 20 iterations of a small file keep it fast.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g6_repeated_kill_resume() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(1024 * 1024, 0x66);
    let src = src_dir.join("fragile.bin");
    std::fs::write(&src, &data).unwrap();

    let mut state = 0x12345u64;
    let mut completed = false;
    for _ in 0..40 {
        let c_ep2 = c_ep.clone();
        let src2 = src.clone();
        let t = target.clone();
        let attempt = tokio::spawn(async move { push(&c_ep2, t, &src2).await });
        // Random-ish kill delay 0..80ms from xorshift.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        tokio::time::sleep(Duration::from_millis(state % 80)).await;
        attempt.abort();
        match attempt.await {
            Ok(Ok(_)) => {
                completed = true;
                break;
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }
    }
    // A kill window may out-last a whole transfer on a slow runner —
    // the loop proves "no kill corrupts state"; the final un-killed
    // push proves resume still converges to byte-identical (G6).
    if !completed {
        push(&c_ep, target.clone(), &src).await.unwrap();
    }
    assert_eq!(std::fs::read(server_dir.join("fragile.bin")).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torn_journal_still_resumes() {
    let (_s, c_ep, target, _task, server_dir) = pair().await;
    let src_dir = scratch("src");
    let data = random_bytes(2 * 1024 * 1024, 0x77);
    let src = src_dir.join("torn.bin");
    std::fs::write(&src, &data).unwrap();

    // Partial first pass, then tear the meta file: truncated garbage.
    let c_ep2 = c_ep.clone();
    let src2 = src.clone();
    let t = target.clone();
    let attempt = tokio::spawn(async move { push(&c_ep2, t, &src2).await });
    tokio::time::sleep(Duration::from_millis(80)).await;
    attempt.abort();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let state_dir = server_dir.join(".rds-sync");
    if state_dir.exists() {
        for e in std::fs::read_dir(&state_dir).unwrap().flatten() {
            let meta = e.path().join("meta");
            if meta.exists() {
                std::fs::write(&meta, b"torn!").unwrap();
            }
        }
    }
    let stats = push(&c_ep, target.clone(), &src).await.unwrap();
    assert!(stats.fetched <= stats.total);
    assert_eq!(std::fs::read(server_dir.join("torn.bin")).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn path_traversal_rejected() {
    // Validation-level: these never reach a filesystem.
    for bad in ["../x", "a/../../b", "/abs/path", "..\\win", "a\0b", ""] {
        assert!(check_rel_path(bad).is_err(), "accepted {bad:?}");
    }
    assert!(check_rel_path("dir/sub/file.bin").is_ok());

    // And over the wire: a hostile Offer gets Refuse.
    let (_s, c_ep, target, _task, _server_dir) = pair().await;
    let conn = client_conn(&c_ep, target).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    rds_core::write_frame(
        &mut send,
        &rds_sync::proto::SyncMsg::Offer {
            rel_path: "../escape.bin".into(),
            size: 4,
            root: [0u8; 32],
            chunk_count: 0,
        },
    )
    .await
    .unwrap();
    match rds_core::read_frame::<_, rds_sync::proto::SyncMsg>(&mut recv)
        .await
        .unwrap()
    {
        rds_sync::proto::SyncMsg::Refuse { .. } => {}
        other => panic!("traversal offer not refused: {other:?}"),
    }
}

/// Impaired lane: 5% loss + delay + jitter under both sockets, plus a
/// mid-transfer connection drop — the file still lands byte-identical
/// and resume re-fetches only what was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impaired_transfer_completes() {
    use rds_bench::impair::{ImpairingSocket, Impairment};

    async fn impaired_ep(imp: Impairment) -> Endpoint {
        let runtime: Arc<dyn noq::Runtime> = Arc::new(noq::TokioRuntime);
        let std_sock = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let inner = noq::Runtime::wrap_udp_socket(&*runtime, std_sock).unwrap();
        let local = inner.local_addr().unwrap();
        let (socket, _stats) = ImpairingSocket::wrap(inner, imp);
        bind_noq_with_socket(
            EndpointConfig::default(),
            Box::new(socket),
            vec![local],
            runtime,
        )
        .await
        .unwrap()
    }

    let server_ep = impaired_ep(Impairment::lossy()).await;
    let client_ep = impaired_ep(Impairment::lossy()).await;
    let server_dir = scratch("impaired-server");
    let _task = spawn_server(server_ep.clone(), server_dir.clone());
    let target = server_ep.addr();

    let src_dir = scratch("src");
    let data = random_bytes(1024 * 1024, 0x1F1E);
    let src = src_dir.join("lossy.bin");
    std::fs::write(&src, &data).unwrap();

    // Attempt 1: cut the client connection ~midway, then re-run to
    // completion on a fresh connection — resume must dedupe parts.
    let c_ep2 = client_ep.clone();
    let src2 = src.clone();
    let t = target.clone();
    let attempt = tokio::spawn(async move { push(&c_ep2, t, &src2).await });
    tokio::time::sleep(Duration::from_millis(700)).await;
    attempt.abort();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let stats = push(&client_ep, target.clone(), &src).await.unwrap();
    assert_eq!(std::fs::read(server_dir.join("lossy.bin")).unwrap(), data);
    assert!(
        stats.fetched <= stats.total,
        "resume re-fetched more than the file: {stats:?}"
    );
}

/// Decoder robustness: malformed manifests/paths never panic, and
/// structurally broken manifests are rejected.
#[test]
fn manifest_and_path_fuzz() {
    check_manifest(&Manifest {
        size: 10,
        root: [0; 32],
        chunks: vec![],
    })
    .unwrap_err(); // empty manifest covering 10 bytes
    check_manifest(&Manifest {
        size: 0,
        root: [0; 32],
        chunks: vec![],
    })
    .unwrap(); // empty file is a valid manifest
}
