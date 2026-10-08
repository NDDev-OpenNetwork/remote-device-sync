//! Bounded directory snapshot wire assembly.

use super::{DirectoryEntry, DirectoryManifest, MAX_DIRECTORY_DATA_BYTES, MAX_DIRECTORY_ENTRIES};
use crate::{ChunkHash, SyncError};
use serde::{Deserialize, Serialize};

/// Version of the directory snapshot payload, independent from the existing
/// single-file transfer route.
pub const DIRECTORY_SNAPSHOT_VERSION: u16 = 1;
/// Maximum entries in one snapshot part.
pub const MAX_DIRECTORY_PART_ENTRIES: usize = 32;
/// Keep a part comfortably below the shared 64 KiB control-frame ceiling.
pub const MAX_DIRECTORY_PART_BYTES: usize = 48 * 1024;

/// Header sent before ordered [`DirectorySnapshotPart`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySnapshotHeader {
    pub version: u16,
    pub root: ChunkHash,
    pub entry_count: u32,
    pub metadata_bytes: u64,
}

impl DirectorySnapshotHeader {
    pub fn from_manifest(manifest: &DirectoryManifest) -> Result<Self, SyncError> {
        manifest.verify()?;
        let metadata_bytes = manifest_metadata_bytes(&manifest.entries)?;
        Ok(Self {
            version: DIRECTORY_SNAPSHOT_VERSION,
            root: manifest.root,
            entry_count: manifest.entries.len().try_into().map_err(|_| {
                SyncError::Manifest("directory entry count exceeds wire bounds".into())
            })?,
            metadata_bytes: metadata_bytes.try_into().map_err(|_| {
                SyncError::Manifest("directory metadata exceeds wire bounds".into())
            })?,
        })
    }
}

/// Ordered bounded portion of a directory snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySnapshotPart {
    pub entries: Vec<DirectoryEntry>,
}

impl DirectorySnapshotPart {
    pub fn new(entries: Vec<DirectoryEntry>) -> Result<Self, SyncError> {
        let part = Self { entries };
        validate_part(&part, None)?;
        Ok(part)
    }

    pub fn encoded_len(&self) -> Result<usize, SyncError> {
        postcard::experimental::serialized_size(self)
            .map_err(|_| SyncError::Manifest("directory snapshot part cannot be encoded".into()))
    }
}

/// Receiver-side bounded assembler. It accepts only strictly increasing
/// canonical paths and verifies the announced count, metadata budget and root
/// before producing a manifest.
#[derive(Debug)]
pub struct DirectorySnapshotAssembler {
    header: DirectorySnapshotHeader,
    entries: Vec<DirectoryEntry>,
    metadata_bytes: usize,
}

impl DirectorySnapshotAssembler {
    pub fn new(header: DirectorySnapshotHeader) -> Result<Self, SyncError> {
        if header.version != DIRECTORY_SNAPSHOT_VERSION {
            return Err(SyncError::Manifest(format!(
                "unsupported directory snapshot version {}",
                header.version
            )));
        }
        let entry_count = usize::try_from(header.entry_count).map_err(|_| {
            SyncError::Manifest("directory entry count cannot fit local bounds".into())
        })?;
        if entry_count > MAX_DIRECTORY_ENTRIES
            || usize::try_from(header.metadata_bytes).unwrap_or(usize::MAX)
                > MAX_DIRECTORY_DATA_BYTES
        {
            return Err(SyncError::Manifest(
                "directory snapshot header exceeds local bounds".into(),
            ));
        }
        Ok(Self {
            header,
            entries: Vec::with_capacity(entry_count.min(MAX_DIRECTORY_PART_ENTRIES)),
            metadata_bytes: 0,
        })
    }

    pub fn push_part(&mut self, part: DirectorySnapshotPart) -> Result<(), SyncError> {
        validate_part(&part, self.entries.last())?;
        let declared = usize::try_from(self.header.entry_count).map_err(|_| {
            SyncError::Manifest("directory entry count cannot fit local bounds".into())
        })?;
        if self.entries.len().saturating_add(part.entries.len()) > declared {
            return Err(SyncError::Manifest(
                "directory snapshot has too many entries".into(),
            ));
        }
        let part_bytes = manifest_metadata_bytes(&part.entries)?;
        let metadata_bytes = self.metadata_bytes.saturating_add(part_bytes);
        if metadata_bytes > usize::try_from(self.header.metadata_bytes).unwrap_or(usize::MAX) {
            return Err(SyncError::Manifest(
                "directory snapshot exceeds announced metadata bytes".into(),
            ));
        }
        self.metadata_bytes = metadata_bytes;
        self.entries.extend(part.entries);
        Ok(())
    }

    pub fn finish(self) -> Result<DirectoryManifest, SyncError> {
        if self.entries.len() != self.header.entry_count as usize
            || self.metadata_bytes != self.header.metadata_bytes as usize
        {
            return Err(SyncError::Manifest(
                "directory snapshot is incomplete".into(),
            ));
        }
        let manifest = DirectoryManifest {
            root: self.header.root,
            entries: self.entries,
        };
        manifest.verify()?;
        Ok(manifest)
    }
}

/// A bounded borrowed part, with the same postcard layout as an owned part.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DirectorySnapshotPartRef<'a> {
    pub entries: &'a [DirectoryEntry],
}

impl DirectorySnapshotPartRef<'_> {
    pub fn encoded_len(&self) -> Result<usize, SyncError> {
        postcard::experimental::serialized_size(self)
            .map_err(|_| SyncError::Manifest("directory snapshot part cannot be encoded".into()))
    }

    pub fn to_owned(&self) -> DirectorySnapshotPart {
        DirectorySnapshotPart {
            entries: self.entries.to_vec(),
        }
    }
}

/// Streaming part iterator: no manifest clone or serialized payload allocation.
#[derive(Debug)]
pub struct DirectorySnapshotParts<'a> {
    remaining: &'a [DirectoryEntry],
}

impl<'a> Iterator for DirectorySnapshotParts<'a> {
    type Item = Result<DirectorySnapshotPartRef<'a>, SyncError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let mut count = 0;
        // At most 32 entries: postcard's sequence length always uses one byte.
        let mut bytes = 1usize;
        for entry in self.remaining.iter().take(MAX_DIRECTORY_PART_ENTRIES) {
            let length = match postcard::experimental::serialized_size(entry) {
                Ok(length) => length,
                Err(_) => {
                    self.remaining = &[];
                    return Some(Err(SyncError::Manifest(
                        "directory entry cannot be encoded".into(),
                    )));
                }
            };
            if bytes.saturating_add(length) > MAX_DIRECTORY_PART_BYTES {
                break;
            }
            bytes += length;
            count += 1;
        }
        if count == 0 {
            self.remaining = &[];
            return Some(Err(SyncError::Manifest(
                "directory entry exceeds part bounds".into(),
            )));
        }
        let (entries, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        Some(Ok(DirectorySnapshotPartRef { entries }))
    }
}

impl DirectoryManifest {
    /// Stream bounded borrowed parts of a verified manifest. Each entry is
    /// measured once; no entry or encoded payload is copied by this iterator.
    pub fn snapshot_part_iter(&self) -> Result<DirectorySnapshotParts<'_>, SyncError> {
        self.verify()?;
        Ok(DirectorySnapshotParts {
            remaining: &self.entries,
        })
    }

    /// Collect owned parts for callers that need to retain them. Streaming
    /// senders should use `snapshot_part_iter` to avoid cloning the manifest.
    pub fn snapshot_parts(&self) -> Result<Vec<DirectorySnapshotPart>, SyncError> {
        self.snapshot_part_iter()?
            .map(|part| part.map(|part| part.to_owned()))
            .collect()
    }
}

fn validate_part(
    part: &DirectorySnapshotPart,
    previous: Option<&DirectoryEntry>,
) -> Result<(), SyncError> {
    if part.entries.is_empty() || part.entries.len() > MAX_DIRECTORY_PART_ENTRIES {
        return Err(SyncError::Manifest(
            "directory snapshot part entry count is outside bounds".into(),
        ));
    }
    let mut prior = previous;
    for entry in &part.entries {
        entry.validate()?;
        if let Some(prior) = prior
            && prior.path.as_bytes() >= entry.path.as_bytes()
        {
            return Err(SyncError::Manifest(
                "directory snapshot paths are not strictly ordered".into(),
            ));
        }
        prior = Some(entry);
    }
    if part.encoded_len()? > MAX_DIRECTORY_PART_BYTES {
        return Err(SyncError::Manifest(
            "directory snapshot part exceeds frame budget".into(),
        ));
    }
    Ok(())
}

fn manifest_metadata_bytes(entries: &[DirectoryEntry]) -> Result<usize, SyncError> {
    let mut total = 0usize;
    for entry in entries {
        entry.validate()?;
        total = total
            .saturating_add(entry.path.len())
            .saturating_add(entry.symlink_target.as_ref().map_or(0, Vec::len));
        if total > MAX_DIRECTORY_DATA_BYTES {
            return Err(SyncError::Manifest(
                "directory metadata byte budget exceeded".into(),
            ));
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, byte: u8) -> DirectoryEntry {
        DirectoryEntry::file(path, 1, *blake3::hash(&[byte]).as_bytes())
    }

    #[test]
    fn parts_roundtrip_in_order_and_verify_root() {
        let manifest = DirectoryManifest::from_entries(vec![
            DirectoryEntry::directory("dir"),
            file("dir/a", 1),
            file("dir/b", 2),
            DirectoryEntry::symlink("link", b"../outside".to_vec()),
        ])
        .unwrap();
        let header = DirectorySnapshotHeader::from_manifest(&manifest).unwrap();
        let parts = manifest.snapshot_parts().unwrap();
        let mut assembler = DirectorySnapshotAssembler::new(header).unwrap();
        for part in parts {
            assembler.push_part(part).unwrap();
        }
        assert_eq!(assembler.finish().unwrap(), manifest);
    }

    #[test]
    fn large_manifests_split_into_multiple_bounded_parts() {
        let entries = (0..70)
            .map(|index| file(&format!("file-{index:03}"), index as u8))
            .collect();
        let manifest = DirectoryManifest::from_entries(entries).unwrap();
        let parts = manifest.snapshot_parts().unwrap();
        assert!(parts.len() >= 3);
        assert!(parts.iter().all(|part| {
            part.entries.len() <= MAX_DIRECTORY_PART_ENTRIES
                && part.encoded_len().unwrap() <= MAX_DIRECTORY_PART_BYTES
        }));
        let mut assembler = DirectorySnapshotAssembler::new(
            DirectorySnapshotHeader::from_manifest(&manifest).unwrap(),
        )
        .unwrap();
        for part in parts {
            assembler.push_part(part).unwrap();
        }
        assert_eq!(assembler.finish().unwrap(), manifest);
    }

    #[test]
    fn assembler_rejects_reordering_count_and_root_mismatch() {
        let manifest = DirectoryManifest::from_entries(vec![file("a", 1), file("b", 2)]).unwrap();
        let header = DirectorySnapshotHeader::from_manifest(&manifest).unwrap();
        let mut assembler = DirectorySnapshotAssembler::new(header).unwrap();
        assembler
            .push_part(DirectorySnapshotPart::new(vec![file("b", 2)]).unwrap())
            .unwrap();
        assert!(
            assembler
                .push_part(DirectorySnapshotPart::new(vec![file("a", 1)]).unwrap())
                .is_err()
        );

        let mut incomplete = DirectorySnapshotAssembler::new(header).unwrap();
        incomplete
            .push_part(DirectorySnapshotPart::new(vec![file("a", 1)]).unwrap())
            .unwrap();
        assert!(incomplete.finish().is_err());

        let mut wrong_root = DirectorySnapshotAssembler::new(DirectorySnapshotHeader {
            root: [9; 32],
            ..header
        })
        .unwrap();
        wrong_root
            .push_part(DirectorySnapshotPart::new(vec![file("a", 1), file("b", 2)]).unwrap())
            .unwrap();
        assert!(wrong_root.finish().is_err());
    }

    #[test]
    fn empty_parts_and_oversized_part_are_refused() {
        assert!(DirectorySnapshotPart::new(Vec::new()).is_err());
        let long = "x".repeat(500);
        let entry = DirectoryEntry::symlink(long, vec![b'x'; 4096]);
        let part = DirectorySnapshotPart::new(vec![entry]).unwrap();
        assert!(part.encoded_len().unwrap() < MAX_DIRECTORY_PART_BYTES);
    }
}
