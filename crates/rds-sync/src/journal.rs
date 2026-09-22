//! Resumable receive state under the destination directory.
//!
//! Layout: `<dir>/.rds-sync/<root-hex>/` holds one in-flight transfer:
//! `meta` pins `{rel_path, size, root}` (postcard + BLAKE3 trailer so
//! a torn write is detected and discarded) and `parts/<chunk-hash-hex>`
//! holds one verified chunk per file. Parts are the source of truth —
//! a chunk counts as present only when its file's content hashes to
//! its name — so a torn `meta`, a partial part write, or a lost
//! journal all degrade to a clean rescan, never to corruption.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ChunkHash, Manifest, SyncError,
    proto::{check_manifest, resolve_under},
};

/// Directory name (under the sync root) holding in-flight state.
pub const STATE_DIR: &str = ".rds-sync";

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    rel_path: String,
    size: u64,
    root: ChunkHash,
}

/// Resumable state for one in-flight receive.
pub struct Journal {
    dir: PathBuf,
    meta: Meta,
    manifest: Manifest,
    /// Chunk indices whose parts verified on disk.
    have: HashSet<u32>,
    /// Chunks verified-and-stored this session (re-fetch accounting).
    fetched: u64,
}

impl Journal {
    /// Open (or create) receive state for `manifest` destined at
    /// `dir/rel_path`. Verifies every existing part by content hash;
    /// a corrupt part is deleted and re-fetched, a torn `meta` is
    /// rebuilt from the offer.
    pub fn open(dest_dir: &Path, rel_path: &str, manifest: &Manifest) -> Result<Self, SyncError> {
        check_manifest(manifest)?;
        std::fs::create_dir_all(dest_dir)?;
        // The state dir is resolved under the canonical root: a
        // symlinked `.rds-sync` cannot redirect journal writes outside.
        let dir = resolve_under(dest_dir, &Path::new(STATE_DIR).join(hex(&manifest.root)))?;
        std::fs::create_dir_all(dir.join("parts"))?;
        // Meta is advisory: pin the destination but never trust it for
        // chunk truth. A torn/absent meta just gets rewritten.
        let meta = Meta {
            rel_path: rel_path.to_string(),
            size: manifest.size,
            root: manifest.root,
        };
        write_meta(&dir, &meta)?;
        let mut j = Self {
            dir,
            meta,
            manifest: manifest.clone(),
            have: HashSet::new(),
            fetched: 0,
        };
        j.rescan();
        j.seed_from_destination(dest_dir);
        Ok(j)
    }

    /// Dedup against the already-assembled destination: if
    /// `dest_dir/rel_path` exists, chunk it and count every matching
    /// hash as present — identical content needs zero wire chunks.
    /// Chunk boundaries are content-defined, so the same bytes cut
    /// identically. Streamed: memory stays at one max-size chunk
    /// regardless of file size.
    fn seed_from_destination(&mut self, dest_dir: &Path) {
        // No seeding through a symlink that escapes the root.
        let Ok(dest) = resolve_under(dest_dir, Path::new(&self.meta.rel_path)) else {
            return;
        };
        // Cheap gate first: only a regular file of identical size can
        // contribute chunks — a manifest scan of anything else is waste.
        let Ok(meta) = std::fs::metadata(&dest) else {
            return;
        };
        if !meta.is_file() || meta.len() != self.manifest.size {
            return;
        }
        let Ok(file) = std::fs::File::open(&dest) else {
            return;
        };
        let Ok(existing) = crate::manifest_of_reader(file) else {
            return;
        };
        let present: HashSet<ChunkHash> = existing.chunks.iter().map(|c| c.hash).collect();
        for (i, c) in self.manifest.chunks.iter().enumerate() {
            if present.contains(&c.hash) {
                self.have.insert(i as u32);
            }
        }
    }

    /// Re-verify every part on disk against the manifest; drop any
    /// whose bytes do not hash to the manifest entry.
    fn rescan(&mut self) {
        self.have.clear();
        for (i, c) in self.manifest.chunks.iter().enumerate() {
            let p = self.part_path(&c.hash);
            let Ok(data) = std::fs::read(&p) else {
                continue;
            };
            if data.len() == c.len as usize && blake3::hash(&data).as_bytes() == &c.hash {
                self.have.insert(i as u32);
            } else {
                let _ = std::fs::remove_file(&p); // corrupt part: re-fetch
            }
        }
    }

    /// Manifest indices still missing — what the receiver asks for.
    pub fn need(&self) -> Vec<u32> {
        (0..self.manifest.chunks.len() as u32)
            .filter(|i| !self.have.contains(i))
            .collect()
    }

    /// Store one received chunk. The payload is verified against the
    /// manifest hash BEFORE it touches the state directory — a corrupt
    /// or forged chunk is an error, never a part. Returns `true` when
    /// the chunk was newly present (a resent chunk verifies but is not
    /// rewritten).
    pub fn store(&mut self, index: u32, data: &[u8]) -> Result<bool, SyncError> {
        let c = self
            .manifest
            .chunks
            .get(index as usize)
            .ok_or_else(|| SyncError::Manifest(format!("chunk index {index} out of range")))?;
        if data.len() != c.len as usize || blake3::hash(data).as_bytes() != &c.hash {
            return Err(SyncError::Manifest(format!(
                "chunk {index} failed BLAKE3 verification"
            )));
        }
        if self.have.contains(&index) {
            return Ok(false);
        }
        let part = self.part_path(&c.hash);
        // Write tmp + rename: a crash mid-write leaves a *.tmp, which
        // rescan ignores — parts are only ever complete files.
        let tmp = part.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &part)?;
        self.have.insert(index);
        self.fetched += 1;
        Ok(true)
    }

    /// True once every manifest chunk is present and verified.
    pub fn complete(&self) -> bool {
        self.have.len() == self.manifest.chunks.len()
    }

    /// Chunks fetched this session — resume-overhead accounting.
    pub fn fetched(&self) -> u64 {
        self.fetched
    }

    /// Indices whose parts verified on disk.
    pub fn have_set(&self) -> &HashSet<u32> {
        &self.have
    }

    /// Total chunks in the manifest.
    pub fn total(&self) -> usize {
        self.manifest.chunks.len()
    }

    /// Concatenate verified parts in manifest order, check the file
    /// root, and atomically rename into place. Only runs when
    /// `complete()` — failure here means a bug or tampering.
    pub fn assemble(&self, dest_dir: &Path) -> Result<PathBuf, SyncError> {
        if !self.complete() {
            return Err(SyncError::Manifest("assemble before complete".into()));
        }
        // Resolved under the canonical root — a symlinked intermediate
        // component is refused rather than followed outside.
        let dest = resolve_under(dest_dir, Path::new(&self.meta.rel_path))?;
        // Dedup fast path: the destination may already hold the exact
        // content (identical resend) — verify its root and finish.
        // Streamed hash: no whole-file read.
        if let Ok(root) = hash_file(&dest)
            && root == self.manifest.root
        {
            let _ = std::fs::remove_dir_all(&self.dir);
            return Ok(dest);
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Suffix is appended, not substituted, so a peer's literal
        // `x.rds-part` name cannot alias the temp file to dest.
        let tmp = dest.with_added_extension("rds-part");
        // A pre-existing tmp may be a stale artifact — or a planted
        // symlink. Remove it (remove_file unlinks the link itself, not
        // its target) and create exclusively so the assembly write can
        // never follow a link.
        let _ = std::fs::remove_file(&tmp);
        let mut root = blake3::Hasher::new();
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            for c in &self.manifest.chunks {
                let data = std::fs::read(self.part_path(&c.hash))?;
                root.update(&data);
                f.write_all(&data)?;
            }
        }
        if root.finalize().as_bytes() != &self.manifest.root {
            let _ = std::fs::remove_file(&tmp);
            return Err(SyncError::Manifest(
                "assembled file failed root hash".into(),
            ));
        }
        std::fs::rename(&tmp, &dest)?;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(dest)
    }

    fn part_path(&self, hash: &ChunkHash) -> PathBuf {
        self.dir.join("parts").join(hex(hash))
    }
}

fn write_meta(dir: &Path, meta: &Meta) -> Result<(), SyncError> {
    let body = postcard::to_allocvec(meta)
        .map_err(|e| SyncError::Manifest(format!("meta encode: {e}")))?;
    let mut buf = body.clone();
    buf.extend_from_slice(blake3::hash(&body).as_bytes());
    let tmp = dir.join("meta.tmp");
    std::fs::write(&tmp, &buf)?;
    std::fs::rename(&tmp, dir.join("meta"))?;
    Ok(())
}

/// Load `meta` if it parses and checksums — used by tooling/inspect;
/// correctness never depends on it.
#[allow(dead_code)]
fn read_meta(dir: &Path) -> Option<Meta> {
    let buf = std::fs::read(dir.join("meta")).ok()?;
    let (body, trailer) = buf.split_at(buf.len().checked_sub(32)?);
    if blake3::hash(body).as_bytes() != trailer {
        return None; // torn write
    }
    postcard::from_bytes(body).ok()
}

/// Stream-hash a file — bounded memory regardless of size.
fn hash_file(path: &Path) -> std::io::Result<ChunkHash> {
    let mut f = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    std::io::copy(&mut f, &mut HasherWriter(&mut h))?;
    Ok(*h.finalize().as_bytes())
}

/// `std::io::Write` adapter that feeds a BLAKE3 hasher — lets
/// `io::copy` stream the file through the digest without buffering.
struct HasherWriter<'a>(&'a mut blake3::Hasher);

impl std::io::Write for HasherWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn hex(hash: &ChunkHash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}
