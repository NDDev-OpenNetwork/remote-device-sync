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

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use rds_core::{read_frame, write_frame};
use rds_net::{Connection, RecvStream, SendStream};

use crate::journal::Journal;
use crate::proto::{
    CHUNKSET_BATCH, FETCH_STREAMS, MANIFEST_BATCH, MAX_CHUNKS, SyncMsg, bits_to_indices,
    check_manifest, check_rel_path, need_bits, resolve_under,
};
use crate::{Manifest, manifest_of};

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
    match read_frame::<_, SyncMsg>(&mut recv).await? {
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
            // The journal creates the root on demand; the resolve below
            // needs it to exist.
            if let Err(e) = std::fs::create_dir_all(&dir) {
                refuse(&mut send, &e.to_string()).await?;
                bail!("sync root not writable: {e}");
            }
            // Fail fast when the resolved destination would escape the
            // root through a symlinked component — assemble re-checks at
            // write time, but refusing here saves moving the chunks.
            if let Err(e) = resolve_under(&dir, &rel) {
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
            // Lexical check passed — now prove the resolved path stays
            // inside the sync root (a symlinked component can't be used
            // to read outside it).
            let path = match resolve_under(&dir, &rel) {
                Ok(p) if p.is_file() => p,
                _ => {
                    refuse(&mut send, "no such file").await?;
                    bail!("requested file absent or outside root: {}", rel.display());
                }
            };
            let manifest = manifest_of(&std::fs::read(&path)?);
            tracing::info!(
                peer = %conn.remote_id(),
                rel = %rel.display(),
                size = manifest.size,
                chunks = manifest.chunks.len(),
                "sync pull serving"
            );
            send_manifest(&mut send, &rel.to_string_lossy(), &manifest).await?;
            let SyncMsg::Need { bits } = read_frame::<_, SyncMsg>(&mut recv).await? else {
                bail!("expected Need");
            };
            let indices = bits_to_indices(&bits, manifest.chunks.len());
            push_chunks(&conn, &path, &manifest, &indices).await?;
            match read_frame::<_, SyncMsg>(&mut recv).await? {
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
    let manifest = manifest_of(&std::fs::read(path)?);
    tracing::info!(
        peer = %conn.remote_id(),
        rel = %rel,
        size = manifest.size,
        chunks = manifest.chunks.len(),
        "sync push start"
    );
    send_manifest(&mut send, &rel, &manifest).await?;
    let indices = match read_frame::<_, SyncMsg>(&mut recv).await? {
        SyncMsg::Need { bits } => bits_to_indices(&bits, manifest.chunks.len()),
        SyncMsg::Refuse { reason } => bail!("offer refused: {reason}"),
        other => bail!("expected Need, got {other:?}"),
    };
    push_chunks(conn, path, &manifest, &indices).await?;
    match read_frame::<_, SyncMsg>(&mut recv).await? {
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
    let (rel, size, root, chunk_count) = match read_frame::<_, SyncMsg>(&mut recv).await? {
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

/// Receiver half, shared by push and pull: journal the offer, answer
/// `Need`, collect chunk streams until complete, assemble, `Done`.
/// Returns the assembled destination path (resolved under the root).
async fn receive(
    conn: &Connection,
    send: &mut SendStream,
    dir: &Path,
    rel: &str,
    manifest: &Manifest,
) -> anyhow::Result<(PathBuf, Stats)> {
    let mut journal = Journal::open(dir, rel, manifest)?;
    let bits = need_bits(journal.total(), journal.have_set());
    write_frame(send, &SyncMsg::Need { bits }).await?;

    // Chunk streams arrive tagged `UniHello::Sync` — routed by the
    // connection's demux so a concurrent desktop session on the same
    // connection can't consume them.
    let mut uni = conn
        .uni_streams(rds_core::UniHello::Sync)
        .context("claim sync uni streams")?;
    let mut fetched_bytes = 0u64;
    while !journal.complete() {
        let mut stream = uni.recv().await.context("chunk streams ended")?;
        loop {
            match read_frame::<_, SyncMsg>(&mut stream).await? {
                SyncMsg::ChunkSet { indices } => {
                    for index in indices {
                        let SyncMsg::ChunkHdr { index: i, len, .. } =
                            read_frame::<_, SyncMsg>(&mut stream).await?
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
                        stream.read_exact(&mut buf).await?;
                        journal.store(i, &buf).map_err(|e| anyhow::anyhow!("{e}"))?;
                        fetched_bytes += u64::from(len);
                    }
                }
                SyncMsg::SetDone => break,
                other => bail!("unexpected chunk-stream message {other:?}"),
            }
        }
    }
    let dest = journal.assemble(dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    write_frame(
        send,
        &SyncMsg::Done {
            root: manifest.root,
        },
    )
    .await?;
    tracing::debug!(?dest, "sync file assembled");
    Ok((
        dest,
        Stats {
            fetched: journal.fetched(),
            total: journal.total() as u64,
            bytes: fetched_bytes,
        },
    ))
}

/// Holder half: open [`FETCH_STREAMS`] uni streams, each walking an
/// interleaved share of `indices` in `CHUNKSET_BATCH` batches.
async fn push_chunks(
    conn: &Connection,
    path: &Path,
    manifest: &Manifest,
    indices: &[u32],
) -> anyhow::Result<()> {
    let mut tasks = Vec::new();
    for k in 0..FETCH_STREAMS {
        let conn = conn.clone();
        let path = path.to_path_buf();
        let manifest = manifest.clone();
        let mine: Vec<u32> = indices
            .iter()
            .copied()
            .skip(k)
            .step_by(FETCH_STREAMS)
            .collect();
        tasks.push(tokio::spawn(async move {
            if mine.is_empty() {
                return Ok::<(), anyhow::Error>(());
            }
            let mut stream = conn.open_uni().await?;
            // First frame on every uni stream is its UniHello tag —
            // the receiver's demux routes on it.
            write_frame(&mut stream, &rds_core::UniHello::Sync).await?;
            let mut file = std::fs::File::open(&path)?;
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
                    let mut buf = vec![0u8; c.len as usize];
                    file.seek(SeekFrom::Start(c.offset))?;
                    file.read_exact(&mut buf)?;
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
        }));
    }
    for t in tasks {
        t.await??;
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
        match read_frame::<_, SyncMsg>(recv).await? {
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
