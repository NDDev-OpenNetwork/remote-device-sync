//! Wire protocol for sync: control-stream messages, chunk-stream
//! framing, and the validation that keeps a hostile or corrupt peer
//! from writing outside the sync root or poisoning the manifest.
//!
//! Flow (holder = the side with the file, receiver = the side that
//! wants it; either may be the connection initiator):
//!
//! ```text
//! control bi stream `StreamHello::Sync`:
//!   receiver → holder  Request { rel_path }        (pull)
//!   holder → receiver  Offer { rel_path, size, root, chunk_count }
//!   holder → receiver  ManifestPart { chunks } ×N  (≤512 per frame)
//!   receiver → holder  Need { bits }               (bitmap of missing)
//! holder → receiver    4× uni chunk streams:
//!     ChunkSet { indices } (ChunkHdr + bytes)* [repeat] SetDone
//!   receiver → holder  Done { root }
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Chunk, ChunkHash, MAX_CHUNK, SyncError};

/// Parallel chunk streams per transfer — each carries a disjoint
/// batch of chunk indices.
pub const FETCH_STREAMS: usize = 4;

/// Cap on a manifest's chunk list: 256K chunks covers ≥4 GiB files at
/// the 16 KiB minimum and keeps a `Need` bitmap under 32 KiB — inside
/// `MAX_MESSAGE_LEN`.
pub const MAX_CHUNKS: usize = 1 << 18;

/// Cap on a relative path the protocol accepts.
pub const MAX_REL_PATH: usize = 512;

/// Chunks per `ManifestPart` frame (~23 KiB postcard — well under
/// `MAX_MESSAGE_LEN`).
pub const MANIFEST_BATCH: usize = 512;

/// Chunk indices per `ChunkSet` frame on a chunk stream.
pub const CHUNKSET_BATCH: usize = 4096;

/// Control-stream and chunk-stream messages (postcard-framed via
/// `rds_core::write_frame`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMsg {
    /// Holder → receiver: offer `rel_path`. `chunks` arrive as
    /// `ManifestPart` frames until `chunk_count` entries land.
    Offer {
        rel_path: String,
        size: u64,
        root: ChunkHash,
        chunk_count: u32,
    },
    /// Receiver → holder: pull `rel_path`.
    Request { rel_path: String },
    /// Either direction: the transfer cannot proceed.
    Refuse { reason: String },
    /// Holder → receiver: a manifest slice (`MANIFEST_BATCH` per frame).
    ManifestPart { chunks: Vec<Chunk> },
    /// Receiver → holder: bitmap of missing chunk indices.
    Need { bits: Vec<u64> },
    /// First frame of each batch on a chunk stream (uni, holder →
    /// receiver): the manifest indices the following chunks fill.
    ChunkSet { indices: Vec<u32> },
    /// Per chunk: header frame then `len` raw bytes.
    ChunkHdr {
        index: u32,
        hash: ChunkHash,
        len: u32,
    },
    /// Chunk-stream terminator.
    SetDone,
    /// Receiver → holder: transfer complete, file assembled and
    /// root-verified.
    Done { root: ChunkHash },
}

/// Validate a peer-supplied relative path. Returns the safe
/// normalized form. Rejects absolute paths, `..` escapes, empty results,
/// NULs and overlong input. Empty and dot components are normalized away.
/// This is only lexical validation. The journal and transfer engine separately
/// bind I/O to no-follow directory/file handles; a validated path string alone
/// is never a confinement proof.
pub fn check_rel_path(rel: &str) -> Result<PathBuf, SyncError> {
    if rel.is_empty() || rel.len() > MAX_REL_PATH {
        return Err(SyncError::Manifest(format!("bad rel_path {rel:?}")));
    }
    if rel.contains('\0') {
        return Err(SyncError::Manifest("rel_path contains NUL".into()));
    }
    // Absolute paths must be refused, not silently relativized —
    // `/a/b` and `C:\x` both split into legal-looking components.
    if rel.starts_with(['/', '\\']) || rel.as_bytes().get(1) == Some(&b':') || rel.starts_with('~')
    {
        return Err(SyncError::Manifest(format!("absolute rel_path {rel:?}")));
    }
    let mut out = PathBuf::new();
    let mut first = true;
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                return Err(SyncError::Manifest(format!(
                    "traversal in rel_path {rel:?}"
                )));
            }
            // `.rds-sync` at the root is the journal namespace — a peer
            // may not read or plant state files in it.
            p if first && p.eq_ignore_ascii_case(crate::journal::STATE_DIR) => {
                return Err(SyncError::Manifest(format!(
                    "rel_path {rel:?} enters the sync journal"
                )));
            }
            p => {
                first = false;
                out.push(p);
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(SyncError::Manifest(format!("empty rel_path {rel:?}")));
    }
    Ok(out)
}

/// Structural validation of a reassembled manifest: chunks sorted,
/// non-overlapping, exactly covering `size`, individually bounded,
/// list length capped. Content correctness is proven per-chunk by
/// BLAKE3 — this rejects malformed structure early.
pub fn check_manifest(m: &Manifest) -> Result<(), SyncError> {
    if m.chunks.len() > MAX_CHUNKS {
        return Err(SyncError::Manifest("manifest too large".into()));
    }
    let mut covered = 0u64;
    for c in &m.chunks {
        if c.len == 0 || c.len > MAX_CHUNK {
            return Err(SyncError::Manifest(format!("bad chunk len {}", c.len)));
        }
        if c.offset != covered {
            return Err(SyncError::Manifest(format!(
                "chunks not contiguous at offset {}",
                c.offset
            )));
        }
        covered = covered
            .checked_add(u64::from(c.len))
            .ok_or_else(|| SyncError::Manifest("size overflow".into()))?;
    }
    if covered != m.size {
        return Err(SyncError::Manifest(format!(
            "chunks cover {covered} bytes, manifest size {}",
            m.size
        )));
    }
    Ok(())
}

use crate::Manifest;

/// Build the `Need` bitmap: bit i set ⇔ chunk i is missing.
pub fn need_bits(total: usize, have: &std::collections::HashSet<u32>) -> Vec<u64> {
    let mut bits = vec![0u64; total.div_ceil(64)];
    for i in 0..total as u32 {
        if !have.contains(&i) {
            bits[i as usize / 64] |= 1 << (i % 64);
        }
    }
    bits
}

/// Decode a `Need` bitmap into missing indices.
pub fn bits_to_indices(bits: &[u64], total: usize) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..total as u32 {
        if bits
            .get(i as usize / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
        {
            out.push(i);
        }
    }
    out
}
