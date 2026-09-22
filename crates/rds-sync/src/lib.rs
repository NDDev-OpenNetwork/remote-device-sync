//! File synchronization for rds: content-defined chunking, BLAKE3
//! addressing, resumable transfer.
//!
//! Design: a file maps to a [`Manifest`] of variable-length chunks cut
//! by FastCDC (rolling-hash boundaries survive inserts/edits, so changed
//! files share most chunks). Each chunk is addressed by its BLAKE3
//! digest — transfer, resume and verification all key on content, not
//! offsets. Reconciliation and transfer run over rds streams; the sync
//! engine is independent of the interactive desktop path on purpose.
//!
//! Scope: the content model (chunking, manifesting, hashing) plus the
//! transfer protocol — `proto` frames, `journal` resumable state, and
//! `engine` driving send/receive/serve over `rds-net` streams.

pub mod engine;
pub mod journal;
pub mod proto;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// BLAKE3 digest of a chunk's content.
pub type ChunkHash = [u8; 32];

/// Default chunk sizing: 16 KiB min, 64 KiB average, 256 KiB max.
pub const MIN_CHUNK: u32 = 16 * 1024;
pub const AVG_CHUNK: u32 = 64 * 1024;
pub const MAX_CHUNK: u32 = 256 * 1024;

/// One content-addressed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// BLAKE3 digest of the chunk bytes.
    pub hash: ChunkHash,
    /// Byte offset within the file.
    pub offset: u64,
    /// Chunk length in bytes.
    pub len: u32,
}

/// A file as a set of content-addressed chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Total file length in bytes.
    pub size: u64,
    /// BLAKE3 digest of the whole file (fast equality check).
    pub root: ChunkHash,
    /// Chunks in file order.
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest malformed: {0}")]
    Manifest(String),
}

/// Build the manifest for `data` using FastCDC 2020 chunking.
pub fn manifest_of(data: &[u8]) -> Manifest {
    manifest_with(data, MIN_CHUNK, AVG_CHUNK, MAX_CHUNK)
}

/// Build the manifest with explicit FastCDC size bounds.
pub fn manifest_with(data: &[u8], min: u32, avg: u32, max: u32) -> Manifest {
    let chunks = fastcdc::v2020::FastCDC::new(data, min as usize, avg as usize, max as usize)
        .map(|c| Chunk {
            hash: *blake3::hash(&data[c.offset..c.offset + c.length]).as_bytes(),
            offset: c.offset as u64,
            len: c.length as u32,
        })
        .collect();
    Manifest {
        size: data.len() as u64,
        root: *blake3::hash(data).as_bytes(),
        chunks,
    }
}

/// Chunks in `b`'s manifest that `a`'s manifest lacks — the wire delta.
pub fn missing_chunks<'a>(have: &Manifest, want: &'a Manifest) -> Vec<&'a Chunk> {
    use std::collections::HashSet;
    let present: HashSet<ChunkHash> = have.chunks.iter().map(|c| c.hash).collect();
    want.chunks
        .iter()
        .filter(|c| !present.contains(&c.hash))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_is_content_defined() {
        // Deterministic aperiodic data: a periodic source would make the
        // insert genuinely change subsequent content.
        // ~1 MiB → ~16 chunks at the default 64 KiB average, enough for
        // the sharing ratio to be meaningful.
        let mut a = vec![0u8; 1_000_000];
        let mut state = 0x9E3779B97F4A7C15u64;
        for b in &mut a {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *b = state as u8;
        }
        // Same content with an insert near the front — later chunks
        // should still match despite shifted offsets.
        let mut b = a.clone();
        b.splice(10_000..10_000, vec![0xAA; 3000]);
        let ma = manifest_of(&a);
        let mb = manifest_of(&b);
        let common = ma
            .chunks
            .iter()
            .filter(|c| mb.chunks.iter().any(|d| d.hash == c.hash))
            .count();
        // The bulk of both files must share chunks.
        assert!(common as f64 > ma.chunks.len() as f64 * 0.8);
        // Delta is much smaller than the file.
        assert!(missing_chunks(&ma, &mb).len() < ma.chunks.len() / 4);
    }

    #[test]
    fn manifest_roundtrip() {
        let data = b"remote-device-sync sync engine";
        let m = manifest_of(data);
        assert_eq!(m.size as usize, data.len());
        assert_eq!(m.chunks.iter().map(|c| c.len as u64).sum::<u64>(), m.size);
    }
}
