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

/// Negotiated sync-session protocol version. Version is bound in the
/// route tag (`SyncTransferV2`), not negotiated downward: a peer that
/// cannot decode the tag rejects before any filesystem operation.
pub const SESSION_VERSION: u16 = 2;

/// Per-session bounds declared in `Hello`/`HelloAck`. A session runs at
/// the pairwise minimum — never wider than the tighter peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLimits {
    /// Largest single chunk payload the peer accepts.
    pub max_chunk: u32,
    /// Largest manifest chunk count the peer accepts.
    pub max_chunks: u32,
    /// Parallel chunk streams the peer opens/consumes.
    pub fetch_streams: u8,
}

impl SessionLimits {
    /// This implementation's declared bounds.
    pub const LOCAL: Self = Self {
        max_chunk: MAX_CHUNK,
        max_chunks: MAX_CHUNKS as u32,
        fetch_streams: FETCH_STREAMS as u8,
    };

    /// Resolve the session bounds from both declarations. Zero-valued
    /// declarations make the transfer impossible and are refused.
    pub fn negotiate(local: Self, peer: Self) -> Result<Self, SyncError> {
        if peer.max_chunk == 0
            || peer.max_chunks == 0
            || peer.fetch_streams == 0
            || local.max_chunk == 0
            || local.max_chunks == 0
            || local.fetch_streams == 0
        {
            return Err(SyncError::Manifest(
                "peer declared unusable sync session limits".into(),
            ));
        }
        Ok(Self {
            max_chunk: local.max_chunk.min(peer.max_chunk).min(MAX_CHUNK),
            max_chunks: local.max_chunks.min(peer.max_chunks).min(MAX_CHUNKS as u32),
            fetch_streams: local
                .fetch_streams
                .min(peer.fetch_streams)
                .min(FETCH_STREAMS as u8),
        })
    }
}

/// Messages inside a [`SyncMsg::Session`] envelope (v2 flow). Every frame
/// is bound to the negotiated transfer ID by the envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionMsg {
    /// Control opener → responder: protocol version + declared limits.
    Hello { version: u16, limits: SessionLimits },
    /// Responder → opener: accepted version + declared limits.
    HelloAck { version: u16, limits: SessionLimits },
    /// Holder → receiver: offer `rel_path`.
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
    /// Either direction: deliberate abort — distinguishable from a
    /// connection failure; verified journal state remains resumable.
    Cancel { reason: String },
    /// Holder → receiver: a manifest slice (`MANIFEST_BATCH` per frame).
    ManifestPart { chunks: Vec<Chunk> },
    /// Receiver → holder: bitmap of missing chunk indices.
    Need { bits: Vec<u64> },
    /// Receiver → holder: transfer complete, file assembled and
    /// root-verified.
    Done { root: ChunkHash },
    /// First frame of each batch on a chunk stream.
    ChunkSet { indices: Vec<u32> },
    /// Per chunk: header frame then `len` raw bytes.
    ChunkHdr {
        index: u32,
        hash: ChunkHash,
        len: u32,
    },
    /// Chunk-stream terminator.
    SetDone,
}

/// Control-stream and chunk-stream messages (postcard-framed via
/// `rds_net::write_frame`).
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
    /// Deliberate abort; v1 sends it as `Refuse`, v2 keeps it typed
    /// inside the session envelope.
    Cancel { reason: String },
    /// Version-2 session envelope (route tag `SyncTransferV2`): every
    /// frame binds the negotiated transfer ID. Unknown to v1 peers —
    /// they reject at decode, before filesystem operations.
    Session {
        transfer_id: [u8; 16],
        msg: SessionMsg,
    },
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
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                return Err(SyncError::Manifest(format!(
                    "traversal in rel_path {rel:?}"
                )));
            }
            // Every destination parent may own private receive state.
            p if p.eq_ignore_ascii_case(crate::journal::STATE_DIR) => {
                return Err(SyncError::Manifest(format!(
                    "rel_path {rel:?} enters the sync journal"
                )));
            }
            p => {
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

/// Decode an exact, canonical `Need` bitmap. Missing words, surplus words and
/// nonzero padding must not silently change the receiver's requested content.
pub fn bits_to_indices(bits: &[u64], total: usize) -> Result<Vec<u32>, SyncError> {
    if total > MAX_CHUNKS
        || bits.len() != total.div_ceil(64)
        || (!total.is_multiple_of(64) && bits.last().is_some_and(|word| *word >> (total % 64) != 0))
    {
        return Err(SyncError::Manifest(
            "invalid Need bitmap bounds or padding".into(),
        ));
    }
    let mut out = Vec::new();
    for i in 0..total as u32 {
        if bits
            .get(i as usize / 64)
            .is_some_and(|w| w & (1 << (i % 64)) != 0)
        {
            out.push(i);
        }
    }
    Ok(out)
}
