//! Transfer engine: the three session roles over `rds-net` streams.
//!
//! - [`serve`] — the agent side: drives whichever direction the peer
//!   asks for on the control stream (`Offer` = push to us, `Request`
//!   = pull from us), rooted at the configured sync dir.
//! - [`send_file`] — CLI push: offer + stream missing chunks.
//! - [`recv_file`] — CLI pull: request + need + collect chunks.
//!
//! Chunk payloads ride uni-directional streams opened by the holder —
//! [`FETCH_STREAMS`] at a time, each carrying disjoint batches — so a
//! dead stream only stalls its own indices and a dropped connection
//! resumes from the receiver's journal.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use rds_core::{read_frame, write_frame};
use rds_net::{Connection, RecvStream, SendStream};
use tokio::sync::mpsc;

use crate::proto::{
    CHUNKSET_BATCH, FETCH_STREAMS, MANIFEST_BATCH, MAX_CHUNKS, SyncMsg, bits_to_indices,
    check_manifest, check_rel_path, need_bits,
};
use crate::{MAX_CHUNK, Manifest, manifest_of_reader};
use crate::{confined::Directory, journal::Journal};

/// No protocol read may stall longer than this — a peer that is alive
/// but silent still must not hang a transfer forever. Generous because
/// reads gate on the peer's disk work (manifest scans, journal
/// rescans); a dead connection ends them regardless.
const READ_STALL: Duration = Duration::from_secs(300);

/// Progress/counters a completed (or interrupted) transfer reports.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    /// Chunks fetched from the wire this session.
    pub fetched: u64,
    /// Total chunks the file needed.
    pub total: u64,
    /// Payload bytes received and verified.
    pub bytes: u64,
}

/// Agent-side entry: one control stream, whichever direction the peer
/// picks. `dir` is the agent's sync root — every path is validated
/// under it.
pub async fn serve(
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    dir: PathBuf,
) -> anyhow::Result<()> {
    let first = read_timed::<_, SyncMsg>(&mut recv).await?;
    match first {
        SyncMsg::Offer {
            rel_path,
            size,
            root,
            chunk_count,
        } => {
            let rel = match check_rel_path(&rel_path) {
                Ok(r) => r,
                Err(e) => {
                    refuse(&mut send, &e.to_string()).await?;
                    bail!("offer refused: {e}");
                }
            };
            // Preserve early refusal before requesting a manifest. This is
            // only a preflight: Journal::open independently pins and checks
            // every handle again before any state or destination I/O.
            let preflight = {
                let (dir, rel) = (dir.clone(), rel.clone());
                tokio::task::spawn_blocking(move || {
                    match Directory::open_root(&dir, false).and_then(|root| root.read_path(&rel)) {
                        Ok(_) => Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(e),
                    }
                })
                .await
                .context("destination preflight task")?
            };
            if let Err(e) = preflight {
                refuse(&mut send, &e.to_string()).await?;
                bail!("offer refused: {e}");
            }
            let manifest = match read_manifest(&mut recv, size, root, chunk_count).await {
                Ok(m) => m,
                Err(e) => {
                    refuse(&mut send, &e.to_string()).await?;
                    return Err(e);
                }
            };
            tracing::info!(
                peer = %conn.remote_id(),
                rel = %rel.display(),
                size,
                chunks = manifest.chunks.len(),
                "sync push accepted"
            );
            let (_dest, stats) =
                receive(&conn, &mut send, &dir, &rel.to_string_lossy(), &manifest).await?;
            tracing::info!(?stats, "push receive complete");
            Ok(())
        }
        SyncMsg::Request { rel_path } => {
            let rel = match check_rel_path(&rel_path) {
                Ok(r) => r,
                Err(e) => {
                    refuse(&mut send, &e.to_string()).await?;
                    bail!("request refused: {e}");
                }
            };
            // Pin the source once. Both manifest and chunk reads use this
            // same inode, even if the path is replaced after the offer.
            let source = {
                let rel = rel.clone();
                tokio::task::spawn_blocking(move || {
                    Directory::open_root(&dir, false)?.read_path(&rel)
                })
                .await
                .context("open source task")?
            };
            let source = match source {
                Ok(file) => Arc::new(file),
                Err(_) => {
                    refuse(&mut send, "no such file").await?;
                    bail!("requested file absent or outside root: {}", rel.display());
                }
            };
            let manifest = manifest_from_file(source.clone()).await?;
            tracing::info!(
                peer = %conn.remote_id(),
                rel = %rel.display(),
                size = manifest.size,
                chunks = manifest.chunks.len(),
                "sync pull serving"
            );
            send_manifest(&mut send, &rel.to_string_lossy(), &manifest).await?;
            let SyncMsg::Need { bits } = read_timed::<_, SyncMsg>(&mut recv).await? else {
                bail!("expected Need");
            };
            let indices = bits_to_indices(&bits, manifest.chunks.len());
            push_chunks(&conn, source, &manifest, &indices).await?;
            match read_timed::<_, SyncMsg>(&mut recv).await? {
                SyncMsg::Done { .. } => {
                    tracing::info!(sent = indices.len(), "sync pull complete");
                    Ok(())
                }
                other => bail!("expected Done, got {other:?}"),
            }
        }
        other => bail!("unexpected first sync message {other:?}"),
    }
}

/// CLI push: offer `path` to the peer under its file name, then stream
/// whatever chunks the receiver still needs.
pub async fn send_file(
    conn: &Connection,
    path: &Path,
    mut send: SendStream,
    mut recv: RecvStream,
) -> anyhow::Result<Stats> {
    let rel = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("{path:?} has no file name"))?;
    let source = Arc::new(tokio::fs::File::open(path).await?.into_std().await);
    let manifest = manifest_from_file(source.clone()).await?;
    tracing::info!(
        peer = %conn.remote_id(),
        rel = %rel,
        size = manifest.size,
        chunks = manifest.chunks.len(),
        "sync push start"
    );
    send_manifest(&mut send, &rel, &manifest).await?;
    let indices = match read_timed::<_, SyncMsg>(&mut recv).await? {
        SyncMsg::Need { bits } => bits_to_indices(&bits, manifest.chunks.len()),
        SyncMsg::Refuse { reason } => bail!("offer refused: {reason}"),
        other => bail!("expected Need, got {other:?}"),
    };
    push_chunks(conn, source, &manifest, &indices).await?;
    match read_timed::<_, SyncMsg>(&mut recv).await? {
        SyncMsg::Done { root } if root == manifest.root => {}
        SyncMsg::Refuse { reason } => bail!("receiver refused: {reason}"),
        other => bail!("expected Done, got {other:?}"),
    }
    send.finish()?;
    let stats = Stats {
        fetched: indices.len() as u64,
        total: manifest.chunks.len() as u64,
        bytes: manifest.size,
    };
    tracing::info!(?stats, "sync push complete");
    Ok(stats)
}

/// CLI pull: request `rel_path` from the peer into `dest_dir`,
/// resuming from the journal.
pub async fn recv_file(
    conn: &Connection,
    rel_path: &str,
    dest_dir: &Path,
    mut send: SendStream,
    mut recv: RecvStream,
) -> anyhow::Result<(PathBuf, Stats)> {
    check_rel_path(rel_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    write_frame(
        &mut send,
        &SyncMsg::Request {
            rel_path: rel_path.to_string(),
        },
    )
    .await?;
    let (rel, size, root, chunk_count) = match read_timed::<_, SyncMsg>(&mut recv).await? {
        SyncMsg::Offer {
            rel_path,
            size,
            root,
            chunk_count,
        } => (
            check_rel_path(&rel_path).map_err(|e| anyhow::anyhow!("{e}"))?,
            size,
            root,
            chunk_count,
        ),
        SyncMsg::Refuse { reason } => bail!("request refused: {reason}"),
        other => bail!("expected Offer, got {other:?}"),
    };
    let manifest = read_manifest(&mut recv, size, root, chunk_count).await?;
    let (dest, stats) =
        receive(conn, &mut send, dest_dir, &rel.to_string_lossy(), &manifest).await?;
    tracing::info!(rel = %rel.display(), ?stats, "sync pull complete");
    Ok((dest, stats))
}

/// Frame read bounded by the read-stall bound — a peer that stops talking
/// mid-transfer aborts instead of parking the session forever.
async fn read_timed<S, T>(stream: &mut S) -> anyhow::Result<T>
where
    S: tokio::io::AsyncRead + Unpin,
    T: serde::de::DeserializeOwned,
{
    match tokio::time::timeout(READ_STALL, read_frame(stream)).await {
        Ok(r) => r.map_err(Into::into),
        Err(_) => bail!("peer stalled mid-transfer"),
    }
}

/// Manifest of a pinned file on the blocking pool — chunking + hashing a
/// large file must not park an async worker.
async fn manifest_from_file(file: Arc<File>) -> anyhow::Result<Manifest> {
    tokio::task::spawn_blocking(move || manifest_of_reader(&*file))
        .await
        .context("manifest task")?
        .context("build manifest")
}

/// Filesystem-bound chunk store: writes run on a dedicated blocking
/// task so disk latency never parks an async worker mid-transfer, and
/// the wire read pipeline never waits on a flush. Owns the journal;
/// closing `jobs` ends it and hands the journal back for assembly.
struct JournalSink {
    jobs: mpsc::Sender<(u32, Vec<u8>)>,
    /// Verified chunks on disk — the receive loop's completion signal.
    present: Arc<AtomicU64>,
    /// First store failure, for error reporting across the task split.
    error: Arc<std::sync::Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<Result<Journal, crate::SyncError>>,
}

impl JournalSink {
    fn start(mut journal: Journal) -> Self {
        let (jobs, mut job_rx) = mpsc::channel::<(u32, Vec<u8>)>(FETCH_STREAMS * 4);
        let present = Arc::new(AtomicU64::new(journal.have_set().len() as u64));
        let error = Arc::new(std::sync::Mutex::new(None));
        let (present_w, error_w) = (present.clone(), error.clone());
        let task = tokio::task::spawn_blocking(move || {
            while let Some((index, data)) = job_rx.blocking_recv() {
                match journal.store(index, &data) {
                    Ok(true) => {
                        present_w.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(false) => {}
                    Err(e) => {
                        *error_w.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                        return Err(e);
                    }
                }
            }
            Ok(journal)
        });
        Self {
            jobs,
            present,
            error,
            task,
        }
    }

    fn present(&self) -> u64 {
        self.present.load(Ordering::Relaxed)
    }

    /// Queue one verified chunk for storage; backpressures when the
    /// disk side falls behind.
    async fn put(&self, index: u32, data: Vec<u8>) -> anyhow::Result<()> {
        self.jobs.send((index, data)).await.map_err(|_| {
            let why = self
                .error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
                .unwrap_or_default();
            anyhow::anyhow!("journal writer died {why}")
        })
    }

    /// Drain queued stores and take the journal back.
    async fn finish(self) -> anyhow::Result<Journal> {
        drop(self.jobs);
        match self.task.await {
            Ok(Ok(j)) => Ok(j),
            Ok(Err(e)) => Err(anyhow::anyhow!("chunk store failed: {e}")),
            Err(e) => Err(anyhow::anyhow!("journal task join: {e}")),
        }
    }
}

/// Receiver half, shared by push and pull: journal the offer, answer
/// `Need`, collect chunk streams until complete, assemble, `Done`.
/// Returns the destination's informational path; I/O stays on held handles.
async fn receive(
    conn: &Connection,
    send: &mut SendStream,
    dir: &Path,
    rel: &str,
    manifest: &Manifest,
) -> anyhow::Result<(PathBuf, Stats)> {
    // Journal open walks and re-verifies every stored part — disk-bound
    // work belongs on the blocking pool, not an async worker.
    let journal = {
        let (dir, rel, manifest) = (dir.to_path_buf(), rel.to_string(), manifest.clone());
        tokio::task::spawn_blocking(move || Journal::open(&dir, &rel, &manifest))
            .await
            .context("journal open task")?
    };
    let journal = match journal {
        Ok(journal) => journal,
        Err(e) => {
            refuse(send, &e.to_string()).await?;
            return Err(e.into());
        }
    };
    let bits = need_bits(journal.total(), journal.have_set());
    write_frame(send, &SyncMsg::Need { bits }).await?;

    let result = tokio::select! {
        result = receive_chunks(conn, journal, manifest) => result?,
        _ = send.stopped() => bail!("sync control stream closed during receive"),
    };
    write_frame(
        send,
        &SyncMsg::Done {
            root: manifest.root,
        },
    )
    .await?;
    Ok(result)
}

async fn receive_chunks(
    conn: &Connection,
    journal: Journal,
    manifest: &Manifest,
) -> anyhow::Result<(PathBuf, Stats)> {
    let total = journal.total() as u64;

    // Chunk streams arrive tagged `UniHello::Sync` — routed by the
    // connection's demux so a concurrent desktop session on the same
    // connection can't consume them.
    let mut uni = conn
        .uni_streams(rds_core::UniHello::Sync)
        .context("claim sync uni streams")?;
    let sink = JournalSink::start(journal);
    // The peer sends exactly the chunks `Need` asked for — count them
    // on the wire, not via `sink.present()`, which the writer task
    // advances asynchronously and would lag the final chunk (the loop
    // would park in `recv` waiting for streams that never come).
    let mut remaining = total - sink.present();
    let mut fetched_bytes = 0u64;
    while remaining > 0 {
        // `recv` only parks once every routed stream is consumed — a
        // stall here means the holder under-delivered.
        let mut stream = match tokio::time::timeout(READ_STALL, uni.recv()).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                let _ = sink.finish().await;
                bail!("chunk streams ended before transfer completed")
            }
            Err(_) => {
                let _ = sink.finish().await;
                bail!("chunk streams stalled")
            }
        };
        loop {
            match read_timed::<_, SyncMsg>(&mut stream).await? {
                SyncMsg::ChunkSet { indices } => {
                    for index in indices {
                        let SyncMsg::ChunkHdr { index: i, len, .. } =
                            read_timed::<_, SyncMsg>(&mut stream).await?
                        else {
                            bail!("expected ChunkHdr");
                        };
                        if i != index {
                            bail!("chunk stream out of order: {i} != {index}");
                        }
                        // The wire len is untrusted: validate it against
                        // the manifest before it sizes the receive
                        // buffer (a forged u32 len would otherwise force
                        // a multi-GiB allocation).
                        match manifest.chunks.get(i as usize) {
                            Some(c) if c.len == len => {}
                            _ => bail!("chunk {i} header len {len} != manifest"),
                        }
                        let mut buf = vec![0u8; len as usize];
                        match tokio::time::timeout(READ_STALL, stream.read_exact(&mut buf)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => bail!("chunk body read: {e}"),
                            Err(_) => bail!("chunk body stalled"),
                        }
                        sink.put(i, buf).await?;
                        remaining = remaining.saturating_sub(1);
                        fetched_bytes += u64::from(len);
                    }
                }
                SyncMsg::SetDone => break,
                other => bail!("unexpected chunk-stream message {other:?}"),
            }
        }
    }
    let journal = sink.finish().await?;
    let fetched = journal.fetched();
    // Assembly concatenates and rehashes every part — blocking pool.
    let dest = {
        tokio::task::spawn_blocking(move || journal.assemble())
            .await
            .context("assemble task")?
            .map_err(|e| anyhow::anyhow!("{e}"))?
    };
    tracing::debug!(?dest, "sync file assembled");
    Ok((
        dest,
        Stats {
            fetched,
            total,
            bytes: fetched_bytes,
        },
    ))
}

/// Holder half: open [`FETCH_STREAMS`] uni streams, each walking an
/// interleaved share of `indices` in `CHUNKSET_BATCH` batches.
async fn push_chunks(
    conn: &Connection,
    file: Arc<File>,
    manifest: &Manifest,
    indices: &[u32],
) -> anyhow::Result<()> {
    let mut tasks = tokio::task::JoinSet::new();
    for k in 0..FETCH_STREAMS {
        let conn = conn.clone();
        let file = file.clone();
        let manifest = manifest.clone();
        let mine: Vec<u32> = indices
            .iter()
            .copied()
            .skip(k)
            .step_by(FETCH_STREAMS)
            .collect();
        tasks.spawn(async move {
            if mine.is_empty() {
                return Ok::<(), anyhow::Error>(());
            }
            let mut stream = conn.open_uni().await?;
            // First frame on every uni stream is its UniHello tag —
            // the receiver's demux routes on it.
            write_frame(&mut stream, &rds_core::UniHello::Sync).await?;
            // One scratch per stream — chunks are ≤256 KiB, so this is
            // a single allocation rather than one per chunk.
            let mut buf = Vec::with_capacity(MAX_CHUNK as usize);
            for batch in mine.chunks(CHUNKSET_BATCH) {
                write_frame(
                    &mut stream,
                    &SyncMsg::ChunkSet {
                        indices: batch.to_vec(),
                    },
                )
                .await?;
                for &index in batch {
                    let c = manifest.chunks[index as usize];
                    buf.clear();
                    buf.resize(c.len as usize, 0);
                    // Positioned reads do not share a seek cursor across
                    // streams and never reopen the peer-controlled path.
                    let source = file.clone();
                    buf = tokio::task::spawn_blocking(move || {
                        source.read_exact_at(&mut buf, c.offset)?;
                        Ok::<_, std::io::Error>(buf)
                    })
                    .await
                    .context("chunk read task")??;
                    write_frame(
                        &mut stream,
                        &SyncMsg::ChunkHdr {
                            index,
                            hash: c.hash,
                            len: c.len,
                        },
                    )
                    .await?;
                    stream.write_all(&buf).await?;
                }
            }
            write_frame(&mut stream, &SyncMsg::SetDone).await?;
            stream.finish()?;
            Ok(())
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

/// Write `Offer` + `ManifestPart` frames for a built manifest.
async fn send_manifest(
    send: &mut SendStream,
    rel: &str,
    manifest: &Manifest,
) -> anyhow::Result<()> {
    write_frame(
        send,
        &SyncMsg::Offer {
            rel_path: rel.to_string(),
            size: manifest.size,
            root: manifest.root,
            chunk_count: manifest.chunks.len() as u32,
        },
    )
    .await?;
    for batch in manifest.chunks.chunks(MANIFEST_BATCH) {
        write_frame(
            send,
            &SyncMsg::ManifestPart {
                chunks: batch.to_vec(),
            },
        )
        .await?;
    }
    Ok(())
}

/// Read `ManifestPart` frames until `chunk_count` entries land;
/// reassemble and validate.
async fn read_manifest(
    recv: &mut RecvStream,
    size: u64,
    root: crate::ChunkHash,
    chunk_count: u32,
) -> anyhow::Result<Manifest> {
    if chunk_count as usize > MAX_CHUNKS {
        bail!("manifest too large: {chunk_count}");
    }
    let mut chunks = Vec::with_capacity(chunk_count as usize);
    while chunks.len() < chunk_count as usize {
        match read_timed::<_, SyncMsg>(recv).await? {
            SyncMsg::ManifestPart { chunks: part } => chunks.extend(part),
            SyncMsg::Refuse { reason } => bail!("refused: {reason}"),
            other => bail!("expected ManifestPart, got {other:?}"),
        }
    }
    if chunks.len() != chunk_count as usize {
        bail!("manifest overrun: {} > {chunk_count}", chunks.len());
    }
    let m = Manifest { size, root, chunks };
    check_manifest(&m).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(m)
}

async fn refuse(send: &mut SendStream, reason: &str) -> anyhow::Result<()> {
    write_frame(
        send,
        &SyncMsg::Refuse {
            reason: reason.to_string(),
        },
    )
    .await
    .context("send refuse")
}
