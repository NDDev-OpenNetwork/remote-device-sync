//! Deterministic directory snapshot model.
//!
//! The scanner uses held, no-follow directory handles. The model is usable
//! independently for wire manifests, dry-run previews and conflict planning.

mod scanner;
mod wire;
pub use scanner::{scan_path, scan_path_cancellable};
pub use wire::{
    DIRECTORY_SNAPSHOT_VERSION, DirectorySnapshotAssembler, DirectorySnapshotHeader,
    DirectorySnapshotPart, MAX_DIRECTORY_PART_BYTES, MAX_DIRECTORY_PART_ENTRIES,
};

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ChunkHash, SyncError, proto::check_rel_path};

/// Maximum entries accepted in one directory snapshot.
pub const MAX_DIRECTORY_ENTRIES: usize = 1 << 20;
/// Maximum symlink target bytes retained in a snapshot.
pub const MAX_SYMLINK_TARGET_BYTES: usize = 4096;
/// Maximum UTF-8 bytes in one canonical relative path.
pub const MAX_DIRECTORY_PATH_BYTES: usize = crate::proto::MAX_REL_PATH;
/// Aggregate path and symlink target byte budget for a snapshot.
pub const MAX_DIRECTORY_DATA_BYTES: usize = 64 * 1024 * 1024;
/// Maximum directory nesting; bounds open descriptors and call stack depth.
pub const MAX_DIRECTORY_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

/// Content identity and metadata needed by the first one-way mirror model.
/// Unsupported filesystem attributes are deliberately absent rather than
/// silently guessed; the W8 metadata policy will extend this type explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    /// Canonical UTF-8 relative path, with `/` separators.
    pub path: String,
    pub kind: EntryKind,
    /// File size; zero for directories and symlinks.
    pub size: u64,
    /// Verified file content root; absent for directories and symlinks.
    pub content: Option<ChunkHash>,
    /// Raw symlink target bytes; never followed by this model.
    pub symlink_target: Option<Vec<u8>>,
}

impl DirectoryEntry {
    pub fn file(path: impl Into<String>, size: u64, content: ChunkHash) -> Self {
        Self {
            path: path.into(),
            kind: EntryKind::File,
            size,
            content: Some(content),
            symlink_target: None,
        }
    }

    pub fn directory(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            kind: EntryKind::Directory,
            size: 0,
            content: None,
            symlink_target: None,
        }
    }

    pub fn symlink(path: impl Into<String>, target: Vec<u8>) -> Self {
        Self {
            path: path.into(),
            kind: EntryKind::Symlink,
            size: 0,
            content: None,
            symlink_target: Some(target),
        }
    }

    fn validate(&self) -> Result<(), SyncError> {
        if self.path.is_empty() || self.path.len() > MAX_DIRECTORY_PATH_BYTES {
            return Err(SyncError::Manifest(
                "directory entry path exceeds the configured limit".into(),
            ));
        }
        let normalized = check_rel_path(&self.path)?;
        if normalized.to_string_lossy() != self.path || self.path.contains('\\') {
            return Err(SyncError::Manifest(format!(
                "directory entry path is not canonical: {:?}",
                self.path
            )));
        }
        match self.kind {
            EntryKind::File if self.content.is_none() || self.symlink_target.is_some() => Err(
                SyncError::Manifest("file entry identity is incomplete".into()),
            ),
            EntryKind::Directory
                if self.size != 0 || self.content.is_some() || self.symlink_target.is_some() =>
            {
                Err(SyncError::Manifest(
                    "directory entry carries file data".into(),
                ))
            }
            EntryKind::Symlink
                if self.size != 0
                    || self.content.is_some()
                    || self.symlink_target.as_ref().is_none_or(|target| {
                        target.is_empty() || target.len() > MAX_SYMLINK_TARGET_BYTES
                    }) =>
            {
                Err(SyncError::Manifest(
                    "symlink entry target is invalid".into(),
                ))
            }
            _ => Ok(()),
        }
    }

    fn identity(&self) -> EntryIdentity {
        EntryIdentity {
            kind: self.kind,
            size: self.size,
            content: self.content,
            symlink_target: self.symlink_target.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EntryIdentity {
    kind: EntryKind,
    size: u64,
    content: Option<ChunkHash>,
    symlink_target: Option<Vec<u8>>,
}

/// A canonical, bounded directory snapshot. `root` covers the sorted entries
/// and is independent of the order in which a scanner observed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryManifest {
    pub root: ChunkHash,
    pub entries: Vec<DirectoryEntry>,
}

impl DirectoryManifest {
    pub fn from_entries(mut entries: Vec<DirectoryEntry>) -> Result<Self, SyncError> {
        if entries.len() > MAX_DIRECTORY_ENTRIES {
            return Err(SyncError::Manifest("directory manifest too large".into()));
        }
        validate_entries(&entries)?;
        entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
        if entries.windows(2).any(|pair| pair[0].path == pair[1].path) {
            return Err(SyncError::Manifest(
                "directory manifest contains duplicate paths".into(),
            ));
        }
        validate_hierarchy(&entries)?;
        let root = digest_entries(&entries)?;
        Ok(Self { root, entries })
    }

    pub fn verify(&self) -> Result<(), SyncError> {
        validate_entries(&self.entries)?;
        if self
            .entries
            .windows(2)
            .any(|pair| pair[0].path >= pair[1].path)
            || digest_entries(&self.entries)? != self.root
        {
            return Err(SyncError::Manifest(
                "directory manifest is not canonical".into(),
            ));
        }
        validate_hierarchy(&self.entries)?;
        Ok(())
    }

    pub fn diff(&self, other: &Self) -> Result<DirectoryDiff, SyncError> {
        self.verify()?;
        other.verify()?;
        let left: BTreeMap<&str, &DirectoryEntry> = self
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let right: BTreeMap<&str, &DirectoryEntry> = other
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut modified = Vec::new();
        let mut unchanged = Vec::new();
        for (path, entry) in &right {
            match left.get(path) {
                None => added.push((*entry).clone()),
                Some(previous) if previous.identity() == entry.identity() => {
                    unchanged.push((*entry).clone())
                }
                Some(_) => modified.push((*entry).clone()),
            }
        }
        for (path, entry) in &left {
            if !right.contains_key(path) {
                removed.push((*entry).clone());
            }
        }
        let renamed = rename_candidates(&mut added, &mut removed);
        Ok(DirectoryDiff {
            added,
            removed,
            modified,
            unchanged,
            renamed,
        })
    }
}

fn validate_entries(entries: &[DirectoryEntry]) -> Result<(), SyncError> {
    if entries.len() > MAX_DIRECTORY_ENTRIES {
        return Err(SyncError::Manifest("directory manifest too large".into()));
    }
    let mut bytes = 0usize;
    for entry in entries {
        entry.validate()?;
        bytes = bytes
            .saturating_add(entry.path.len())
            .saturating_add(entry.symlink_target.as_ref().map_or(0, Vec::len));
        if bytes > MAX_DIRECTORY_DATA_BYTES {
            return Err(SyncError::Manifest(
                "directory metadata byte budget exceeded".into(),
            ));
        }
    }
    Ok(())
}

fn validate_hierarchy(entries: &[DirectoryEntry]) -> Result<(), SyncError> {
    for entry in entries {
        if let Some((parent, _)) = entry.path.rsplit_once('/') {
            let index = entries
                .binary_search_by(|candidate| candidate.path.as_str().cmp(parent))
                .map_err(|_| SyncError::Manifest("directory entry parent is absent".into()))?;
            if entries[index].kind != EntryKind::Directory {
                return Err(SyncError::Manifest(
                    "directory entry parent is not a directory".into(),
                ));
            }
        }
        if entry.path.split('/').count() > MAX_DIRECTORY_DEPTH {
            return Err(SyncError::Manifest(
                "directory nesting limit exceeded".into(),
            ));
        }
    }
    Ok(())
}

fn digest_entries(entries: &[DirectoryEntry]) -> Result<ChunkHash, SyncError> {
    let mut hasher = blake3::Hasher::new();
    for entry in entries {
        let bytes = postcard::to_stdvec(entry)
            .map_err(|_| SyncError::Manifest("directory entry cannot be encoded".into()))?;
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(*hasher.finalize().as_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub from: DirectoryEntry,
    pub to: DirectoryEntry,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryDiff {
    pub added: Vec<DirectoryEntry>,
    pub removed: Vec<DirectoryEntry>,
    pub modified: Vec<DirectoryEntry>,
    pub unchanged: Vec<DirectoryEntry>,
    pub renamed: Vec<Rename>,
}

fn rename_candidates(
    added: &mut Vec<DirectoryEntry>,
    removed: &mut Vec<DirectoryEntry>,
) -> Vec<Rename> {
    let mut by_identity: BTreeMap<EntryIdentity, Vec<usize>> = BTreeMap::new();
    for (index, entry) in removed.iter().enumerate() {
        // Empty metadata does not establish a directory's subtree identity.
        if entry.kind == EntryKind::Directory {
            continue;
        }
        by_identity.entry(entry.identity()).or_default().push(index);
    }
    let mut used = BTreeSet::new();
    let mut renamed = Vec::new();
    for entry in added.iter() {
        let Some(indices) = by_identity.get_mut(&entry.identity()) else {
            continue;
        };
        let Some(index) = indices.pop() else { continue };
        used.insert(index);
        renamed.push(Rename {
            from: removed[index].clone(),
            to: entry.clone(),
        });
    }
    let renamed_paths: BTreeSet<String> = renamed
        .iter()
        .map(|rename| rename.to.path.clone())
        .collect();
    added.retain(|entry| !renamed_paths.contains(&entry.path));
    let old_removed = std::mem::take(removed);
    *removed = old_removed
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !used.contains(index))
        .map(|(_, entry)| entry)
        .collect();
    renamed.sort_by(|a, b| a.to.path.cmp(&b.to.path));
    renamed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, byte: u8) -> DirectoryEntry {
        DirectoryEntry::file(path, 1, *blake3::hash(&[byte]).as_bytes())
    }

    #[test]
    fn manifest_is_sorted_and_order_independent() {
        let a = DirectoryManifest::from_entries(vec![file("z", 1), file("a", 2)]).unwrap();
        let b = DirectoryManifest::from_entries(vec![file("a", 2), file("z", 1)]).unwrap();
        assert_eq!(a, b);
        assert!(a.verify().is_ok());
    }

    #[test]
    fn traversal_duplicate_and_oversized_entries_fail_closed() {
        assert!(DirectoryManifest::from_entries(vec![file("../escape", 1)]).is_err());
        assert!(DirectoryManifest::from_entries(vec![file("a", 1), file("a", 2)]).is_err());
        let entries = (0..=MAX_DIRECTORY_ENTRIES)
            .map(|i| file(&format!("{i}"), i as u8))
            .collect();
        assert!(DirectoryManifest::from_entries(entries).is_err());
    }

    #[test]
    fn diff_preserves_changes_and_pairs_only_exact_renames() {
        let before =
            DirectoryManifest::from_entries(vec![file("old", 1), file("same", 7)]).unwrap();
        let after = DirectoryManifest::from_entries(vec![
            file("new", 1),
            file("same", 7),
            file("changed", 9),
        ])
        .unwrap();
        let diff = before.diff(&after).unwrap();
        assert_eq!(diff.renamed.len(), 1);
        assert_eq!(diff.renamed[0].from.path, "old");
        assert_eq!(diff.renamed[0].to.path, "new");
        assert_eq!(
            diff.unchanged
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["same"]
        );
        assert_eq!(
            diff.added
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["changed"]
        );
    }

    #[test]
    fn symlink_targets_are_data_and_never_followed() {
        let manifest = DirectoryManifest::from_entries(vec![DirectoryEntry::symlink(
            "link",
            b"../../outside".to_vec(),
        )])
        .unwrap();
        assert_eq!(manifest.entries[0].kind, EntryKind::Symlink);
        assert_eq!(
            manifest.entries[0].symlink_target.as_deref(),
            Some(&b"../../outside"[..])
        );
    }
}
