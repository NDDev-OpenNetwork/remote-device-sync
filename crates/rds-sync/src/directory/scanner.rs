//! Confined directory scanner implementation.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;

use super::{
    DirectoryEntry, DirectoryManifest, MAX_DIRECTORY_DATA_BYTES, MAX_DIRECTORY_DEPTH,
    MAX_DIRECTORY_ENTRIES, MAX_DIRECTORY_PATH_BYTES, MAX_SYMLINK_TARGET_BYTES,
};
use crate::{SyncError, confined::Directory, journal::STATE_DIR};

/// Scan a directory using held directory descriptors and no-following
/// operations. The function is blocking filesystem work; async callers
/// should run it on a blocking executor. Symlink targets are recorded as
/// bytes and never traversed; special files are rejected.
pub fn scan_path(path: &Path) -> Result<DirectoryManifest, SyncError> {
    scan_path_cancellable(path, &|| false)
}

/// Cancellable, bounded directory scan. `stop` is checked between entries and
/// while hashing each regular file. A mutation observed during a scan fails
/// closed so callers can retry from a fresh snapshot.
pub fn scan_path_cancellable(
    path: &Path,
    stop: &dyn Fn() -> bool,
) -> Result<DirectoryManifest, SyncError> {
    let root = Directory::open_root(path, false)?;
    let root_metadata = root.metadata()?;
    let root_stamp = metadata_stamp(&root_metadata);
    if !root_metadata.is_dir() {
        return Err(SyncError::Manifest("sync root is not a directory".into()));
    }
    let mut scanner = Scanner {
        entries: Vec::new(),
        visited: BTreeSet::from([(root_stamp.0, root_stamp.1)]),
        stop,
        metadata_bytes: 0,
    };
    scanner.scan_dir(&root, "", 0)?;
    DirectoryManifest::from_entries(scanner.entries)
}

struct Scanner<'a> {
    entries: Vec<DirectoryEntry>,
    visited: BTreeSet<(u64, u64)>,
    stop: &'a dyn Fn() -> bool,
    metadata_bytes: usize,
}

impl Scanner<'_> {
    fn scan_dir(&mut self, dir: &Directory, prefix: &str, depth: usize) -> Result<(), SyncError> {
        self.check_stop()?;
        if depth > MAX_DIRECTORY_DEPTH {
            return Err(SyncError::Manifest(
                "directory nesting limit exceeded".into(),
            ));
        }
        let before = metadata_stamp(&dir.metadata()?);
        let mut names_seen = 0usize;
        let mut children = dir
            .children_checked(|_| {
                if (self.stop)() {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                }
                names_seen = names_seen.saturating_add(1);
                if names_seen > MAX_DIRECTORY_ENTRIES {
                    return Err(std::io::Error::other("directory entry limit exceeded"));
                }
                Ok(())
            })
            .map_err(SyncError::Io)?;
        children.sort_by(|a, b| os_bytes(a).cmp(os_bytes(b)));
        for name in children {
            self.check_stop()?;
            if self.entries.len() >= MAX_DIRECTORY_ENTRIES {
                return Err(SyncError::Manifest("directory manifest too large".into()));
            }
            // The root's journal is private implementation state, never user
            // data. The reserved namespace is rejected at every other depth,
            // matching the wire path validator.
            if name.eq_ignore_ascii_case(OsStr::new(STATE_DIR)) {
                if prefix.is_empty() {
                    continue;
                }
                return Err(SyncError::Manifest(
                    "directory tree enters the reserved sync journal namespace".into(),
                ));
            }
            let path = join_path(prefix, &name)?;
            let stat = dir.entry_stat(&name)?;
            match Directory::file_type(&stat) {
                rustix::fs::FileType::RegularFile => self.scan_file(dir, &name, path, &stat)?,
                rustix::fs::FileType::Directory => {
                    self.scan_directory(dir, &name, path, depth + 1, &stat)?
                }
                rustix::fs::FileType::Symlink => {
                    let target = dir.read_link_target(&name, MAX_SYMLINK_TARGET_BYTES)?;
                    self.push(DirectoryEntry::symlink(path, target))?;
                    ensure_entry_unchanged(dir, &name, &stat)?;
                }
                _ => {
                    return Err(SyncError::Manifest(format!(
                        "unsupported filesystem entry: {path:?}"
                    )));
                }
            }
        }
        if metadata_stamp(&dir.metadata()?) != before {
            return Err(SyncError::Manifest(
                "directory changed while it was being scanned".into(),
            ));
        }
        Ok(())
    }

    fn scan_file(
        &mut self,
        parent: &Directory,
        name: &OsStr,
        path: String,
        entry_before: &rustix::fs::Stat,
    ) -> Result<(), SyncError> {
        let file = parent.read_file(name)?;
        let file_before = metadata_stamp(&file.metadata()?);
        let manifest = crate::manifest_of_reader_cancellable(&file, self.stop)?;
        let after = metadata_stamp(&file.metadata()?);
        if file_before != after {
            return Err(SyncError::Manifest(format!(
                "file changed while it was being scanned: {path:?}"
            )));
        }
        ensure_entry_unchanged(parent, name, entry_before)?;
        self.push(DirectoryEntry::file(path, manifest.size, manifest.root))
    }

    fn scan_directory(
        &mut self,
        parent: &Directory,
        name: &OsStr,
        path: String,
        depth: usize,
        entry_before: &rustix::fs::Stat,
    ) -> Result<(), SyncError> {
        let child = parent.child(name, false)?;
        let child_stat = child.raw_stat()?;
        if !Directory::same_entry(entry_before, &child_stat) {
            return Err(SyncError::Manifest(format!(
                "directory changed while it was being opened: {path:?}"
            )));
        }
        let stamp = metadata_stamp(&child.metadata()?);
        if !self.visited.insert((stamp.0, stamp.1)) {
            return Err(SyncError::Manifest(
                "directory inode visited more than once".into(),
            ));
        }
        self.push(DirectoryEntry::directory(path.clone()))?;
        self.scan_dir(&child, &path, depth)?;
        ensure_entry_unchanged(parent, name, entry_before)
    }

    fn push(&mut self, entry: DirectoryEntry) -> Result<(), SyncError> {
        if self.entries.len() >= MAX_DIRECTORY_ENTRIES {
            return Err(SyncError::Manifest("directory manifest too large".into()));
        }
        let bytes = entry.path.len() + entry.symlink_target.as_ref().map_or(0, Vec::len);
        self.metadata_bytes = self.metadata_bytes.saturating_add(bytes);
        if self.metadata_bytes > MAX_DIRECTORY_DATA_BYTES {
            return Err(SyncError::Manifest(
                "directory metadata byte budget exceeded".into(),
            ));
        }
        self.entries.push(entry);
        Ok(())
    }

    fn check_stop(&self) -> Result<(), SyncError> {
        if (self.stop)() {
            return Err(SyncError::Io(std::io::Error::from(
                std::io::ErrorKind::Interrupted,
            )));
        }
        Ok(())
    }
}

fn ensure_entry_unchanged(
    parent: &Directory,
    name: &OsStr,
    before: &rustix::fs::Stat,
) -> Result<(), SyncError> {
    let after = parent.entry_stat(name)?;
    if !Directory::same_entry(before, &after) {
        return Err(SyncError::Manifest(
            "directory entry changed while it was being scanned".into(),
        ));
    }
    Ok(())
}

fn join_path(prefix: &str, name: &OsStr) -> Result<String, SyncError> {
    let name = name
        .to_str()
        .ok_or_else(|| SyncError::Manifest("filesystem entry name is not valid UTF-8".into()))?;
    let path = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    };
    if path.is_empty() || path.len() > MAX_DIRECTORY_PATH_BYTES || path.contains('\\') {
        return Err(SyncError::Manifest(
            "directory entry path exceeds the canonical limit".into(),
        ));
    }
    Ok(path)
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes()
}

#[cfg(not(unix))]
fn os_bytes(value: &OsStr) -> &[u8] {
    value.to_string_lossy().as_bytes()
}

type MetadataStamp = (u64, u64, u64, i64, i64, i64, i64);

#[cfg(unix)]
fn metadata_stamp(metadata: &std::fs::Metadata) -> MetadataStamp {
    use std::os::unix::fs::MetadataExt;
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

#[cfg(not(unix))]
fn metadata_stamp(metadata: &std::fs::Metadata) -> MetadataStamp {
    // The public module currently supports Unix targets only. Keep a
    // conservative fallback so the model remains buildable for tooling.
    (0, 0, metadata.len(), 0, 0, 0, 0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::directory::EntryKind;

    fn scratch(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("rds-directory-{name}-{}", rand::random::<u128>()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn scanner_is_sorted_confined_and_records_symlinks() {
        let root = scratch("scan");
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("z"), b"z").unwrap();
        fs::write(root.join("dir/a"), b"a").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../outside", root.join("link")).unwrap();
        fs::create_dir(root.join(STATE_DIR)).unwrap();
        fs::write(root.join(STATE_DIR).join("private"), b"secret").unwrap();

        let manifest = scan_path(&root).unwrap();
        assert_eq!(
            manifest
                .entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["dir", "dir/a", "link", "z"]
        );
        assert_eq!(manifest.entries[2].kind, EntryKind::Symlink);
        assert_eq!(
            manifest.entries[2].symlink_target.as_deref(),
            Some(&b"../outside"[..])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scanner_rejects_special_files_and_honors_cancellation() {
        let root = scratch("cancel");
        fs::write(root.join("file"), b"data").unwrap();
        let stop = AtomicBool::new(true);
        let error = scan_path_cancellable(&root, &|| stop.load(Ordering::Relaxed)).unwrap_err();
        assert!(
            matches!(error, SyncError::Io(error) if error.kind() == std::io::ErrorKind::Interrupted)
        );
        stop.store(false, Ordering::Relaxed);
        let manifest = scan_path_cancellable(&root, &|| stop.load(Ordering::Relaxed)).unwrap();
        assert_eq!(manifest.entries.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scanner_does_not_follow_symlinked_directories() {
        let root = scratch("nofollow");
        let outside = scratch("outside");
        fs::write(outside.join("secret"), b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let manifest = scan_path(&root).unwrap();
        assert_eq!(manifest.entries[0].kind, EntryKind::Symlink);
        assert_eq!(manifest.entries.len(), 1);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn scanner_rejects_reserved_namespace_below_the_root() {
        let root = scratch("nested-reserved");
        fs::create_dir(root.join("dir")).unwrap();
        fs::create_dir(root.join("dir").join(STATE_DIR)).unwrap();
        let error = scan_path(&root).unwrap_err();
        assert!(matches!(error, SyncError::Manifest(message) if message.contains("reserved")));
        fs::remove_dir_all(root).unwrap();
    }
}
