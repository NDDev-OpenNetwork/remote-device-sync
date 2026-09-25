//! Safe descriptor-relative identity transactions on the supported Unix targets.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};
use std::time::{Duration, Instant};

use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, RenameFlags, Stat, mkdirat, openat, renameat_with, statat,
    unlinkat,
};

use super::{KeyOwner, KeyStoreError};
use crate::SecretKey;

const LOCK_NAME: &str = ".rds-key-transaction.lock";
const PENDING_NAME: &str = ".rds-key-transaction.pending";
const MARKER: &[u8] = b"rds-key-transaction/v1\n";
const PRIVATE: Mode = Mode::RUSR.union(Mode::WUSR);
const LOCK_WAIT: Duration = Duration::from_secs(2);
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AfterMarkerCreate,
    AfterStagingCreate,
    BeforeWrite,
    AfterWrite,
    AfterFileSync,
    AfterPublish,
    AfterDirectorySync,
}

pub(super) fn load_or_create(path: &Path) -> Result<SecretKey, KeyStoreError> {
    transaction(path, |_| Ok(()))
}

pub(super) fn acquire(path: &Path) -> Result<KeyOwner, KeyStoreError> {
    let (key, file) = transaction_file(path, |_| Ok(()))?;
    match file.try_lock() {
        Ok(()) => Ok(KeyOwner { key, file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(KeyStoreError::InUse),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn transaction(
    path: &Path,
    checkpoint: impl FnMut(Phase) -> Result<(), KeyStoreError>,
) -> Result<SecretKey, KeyStoreError> {
    transaction_file(path, checkpoint).map(|(key, _file)| key)
}

fn transaction_file(
    path: &Path,
    mut checkpoint: impl FnMut(Phase) -> Result<(), KeyStoreError>,
) -> Result<(SecretKey, File), KeyStoreError> {
    let state = Transaction::open_checked(path, &mut checkpoint)?;
    state.recover_pending()?;
    if let Some(key) = state.read_key()? {
        // Complete a publication whose creator exited before directory sync.
        state.directory.sync_all()?;
        return Ok(key);
    }

    let mut file = File::from(
        openat(
            &state.directory,
            PENDING_NAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            PRIVATE,
        )
        .map_err(io::Error::from)?,
    );
    let mut staged = Staged {
        state: &state,
        live: true,
    };
    checkpoint(Phase::AfterStagingCreate)?;
    // Creation modes are filtered by umask. Only this exclusively created
    // inode is ours to chmod; never publish a seed that cannot be reloaded.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    // Filesystem case/Unicode equivalence can bypass lexical reserved-name
    // checks. Never publish a key through an alias of our own staging file.
    state.reject_internal_key_alias()?;
    checkpoint(Phase::BeforeWrite)?;
    let key = SecretKey::generate();
    file.write_all(&key.to_bytes())?;
    checkpoint(Phase::AfterWrite)?;
    file.sync_all()?;
    checkpoint(Phase::AfterFileSync)?;
    let key = match renameat_with(
        &state.directory,
        PENDING_NAME,
        &state.directory,
        &state.key,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {
            staged.live = false;
            checkpoint(Phase::AfterPublish)?;
            (key, file)
        }
        // A legacy/noncooperating creator can win despite our advisory lock.
        // Never replace it; validate and reuse that key, or return its error.
        Err(rustix::io::Errno::EXIST) => state.read_key()?.ok_or(KeyStoreError::InvalidState)?,
        Err(e) => return Err(io::Error::from(e).into()),
    };
    if staged.live {
        staged.remove()?;
    }
    state.directory.sync_all()?;
    checkpoint(Phase::AfterDirectorySync)?;
    Ok(key)
}

struct Transaction {
    directory: LockedDirectory,
    lock: File,
    key: OsString,
}

struct LockedDirectory(File);

impl std::ops::Deref for LockedDirectory {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}

impl AsFd for LockedDirectory {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Drop for LockedDirectory {
    fn drop(&mut self) {
        // A fork may briefly inherit a descriptor before exec. Scope ownership
        // ends here, rather than when that incidental alias eventually closes.
        let _ = self.0.unlock();
    }
}

impl LockedDirectory {
    fn acquire(file: File) -> Result<Self, KeyStoreError> {
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT => {
                    // A synchronous startup transaction, called on a blocking
                    // worker by async binaries; never a core network driver.
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(std::fs::TryLockError::WouldBlock) => return Err(KeyStoreError::Busy),
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
}

impl Transaction {
    #[cfg(test)]
    fn open(path: &Path) -> Result<Self, KeyStoreError> {
        Self::open_checked(path, &mut |_| Ok(()))
    }

    fn open_checked(
        path: &Path,
        checkpoint: &mut impl FnMut(Phase) -> Result<(), KeyStoreError>,
    ) -> Result<Self, KeyStoreError> {
        let Some(Component::Normal(key)) = path.components().next_back() else {
            return Err(KeyStoreError::InvalidPath);
        };
        if key
            .as_bytes()
            .get(..9)
            .is_some_and(|s| s.eq_ignore_ascii_case(b".rds-key-"))
        {
            return Err(KeyStoreError::InvalidPath);
        }
        let directory =
            open_or_create_directory(nonempty(path.parent().unwrap_or(Path::new("."))))?;
        let meta = directory.metadata()?;
        if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
            return Err(KeyStoreError::UnsafeParent);
        }
        // The directory inode exists before any transaction metadata. This
        // lock also owns recovery after exit between CREATE and fd chmod,
        // without replacing an inode another creator could still have locked.
        let directory = LockedDirectory::acquire(directory)?;
        if let Some(meta) = stat_optional(&directory, LOCK_NAME)?
            && meta.st_size == 0
            && meta.st_mode & 0o7777 != 0o600
        {
            private_state_stat(&meta)?;
            if stat_optional(&directory, PENDING_NAME)?.is_some() {
                return Err(KeyStoreError::InvalidState);
            }
            unlinkat(&directory, LOCK_NAME, AtFlags::empty()).map_err(io::Error::from)?;
            directory.sync_all()?;
        }
        let lock_flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let lock = match openat(
            &directory,
            LOCK_NAME,
            lock_flags | OFlags::CREATE | OFlags::EXCL,
            PRIVATE,
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                checkpoint(Phase::AfterMarkerCreate)?;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                file
            }
            Err(rustix::io::Errno::EXIST) => File::from(
                openat(&directory, LOCK_NAME, lock_flags, Mode::empty())
                    .map_err(io::Error::from)?,
            ),
            Err(error) => return Err(io::Error::from(error).into()),
        };
        private_file(&lock)?;
        let mut state = Self {
            directory,
            lock,
            key: key.to_owned(),
        };
        state.seal_marker()?;
        Ok(state)
    }

    fn seal_marker(&mut self) -> Result<(), KeyStoreError> {
        self.lock.rewind()?;
        let mut marker = Vec::with_capacity(MARKER.len() + 1);
        (&self.lock)
            .take(MARKER.len() as u64 + 1)
            .read_to_end(&mut marker)?;
        if marker != MARKER {
            // A crash while initializing the lock can leave a prefix. No seed
            // is created before the entire marker and directory are synced.
            if !MARKER.starts_with(&marker)
                || stat_optional(&self.directory, PENDING_NAME)?.is_some()
            {
                return Err(KeyStoreError::InvalidState);
            }
            self.lock.rewind()?;
            self.lock.write_all(MARKER)?;
        }
        self.lock.sync_all()?;
        self.directory.sync_all()?;
        Ok(())
    }

    fn open_pending(&self) -> Result<Option<File>, KeyStoreError> {
        open_optional(&self.directory, OsStr::new(PENDING_NAME))
    }

    fn recover_pending(&self) -> Result<(), KeyStoreError> {
        let Some(meta) = stat_optional(&self.directory, PENDING_NAME)? else {
            return Ok(());
        };
        private_state_stat(&meta)?;
        if !(0..=32).contains(&meta.st_size) {
            return Err(KeyStoreError::InvalidState);
        }
        unlinkat(&self.directory, PENDING_NAME, AtFlags::empty()).map_err(io::Error::from)?;
        self.directory.sync_all()?;
        Ok(())
    }

    fn read_key(&self) -> Result<Option<(SecretKey, File)>, KeyStoreError> {
        let Some(file) = open_optional(&self.directory, &self.key)? else {
            return Ok(None);
        };
        self.reject_internal_alias(&file)?;
        private_file(&file)?;
        let mut bytes = Vec::with_capacity(33);
        (&file).take(33).read_to_end(&mut bytes)?;
        let seed: [u8; 32] = bytes.try_into().map_err(|_| KeyStoreError::InvalidLength)?;
        file.sync_all()?;
        Ok(Some((SecretKey::from_bytes(&seed), file)))
    }

    fn reject_internal_key_alias(&self) -> Result<(), KeyStoreError> {
        if let Some(file) = open_optional(&self.directory, &self.key)? {
            self.reject_internal_alias(&file)?;
        }
        Ok(())
    }

    fn reject_internal_alias(&self, file: &File) -> Result<(), KeyStoreError> {
        let candidate = file.metadata()?;
        let same = |other: std::fs::Metadata| {
            candidate.dev() == other.dev() && candidate.ino() == other.ino()
        };
        if same(self.lock.metadata()?)
            || self
                .open_pending()?
                .map(|p| p.metadata())
                .transpose()?
                .is_some_and(same)
        {
            return Err(KeyStoreError::InvalidPath);
        }
        Ok(())
    }
}

struct Staged<'a> {
    state: &'a Transaction,
    live: bool,
}

impl Staged<'_> {
    fn remove(&mut self) -> Result<(), KeyStoreError> {
        unlinkat(&self.state.directory, PENDING_NAME, AtFlags::empty()).map_err(io::Error::from)?;
        self.live = false;
        Ok(())
    }
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if self.live && self.remove().is_ok() {
            // Preserve the primary error. A failed cleanup is recovered under
            // this directory's lock at the next attempt, never by a prefix sweep.
            let _ = self.state.directory.sync_all();
        }
    }
}

fn private_file(file: &File) -> Result<(), KeyStoreError> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || !matches!(meta.mode() & 0o7777, 0o400 | 0o600)
        || meta.nlink() != 1
    {
        return Err(KeyStoreError::UnsafeFile);
    }
    Ok(())
}

fn stat_optional(directory: &File, name: &str) -> Result<Option<Stat>, KeyStoreError> {
    match statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(meta) => Ok(Some(meta)),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(io::Error::from(error).into()),
    }
}

fn private_state_stat(meta: &Stat) -> Result<(), KeyStoreError> {
    let mode = meta.st_mode & 0o7777;
    // An empty creation may precede fd chmod under a restrictive umask.
    // No non-owner access, hard link, special file or unreadable payload is
    // accepted. Metadata inspection does not need permission to read the file.
    if FileType::from_raw_mode(meta.st_mode) != FileType::RegularFile
        || meta.st_uid != rustix::process::geteuid().as_raw()
        || meta.st_nlink != 1
        || !(matches!(mode, 0o400 | 0o600) || (meta.st_size == 0 && mode & !0o600 == 0))
    {
        return Err(KeyStoreError::UnsafeFile);
    }
    Ok(())
}

fn open_optional(directory: &File, name: &OsStr) -> Result<Option<File>, KeyStoreError> {
    match openat(directory, name, READ_FLAGS, Mode::empty()) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(e) => Err(io::Error::from(e).into()),
    }
}

fn nonempty(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}

fn open_or_create_directory(path: &Path) -> Result<File, KeyStoreError> {
    match rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(File::from(fd)),
        Err(rustix::io::Errno::NOENT) => {
            let Some(name) = path.file_name() else {
                return Err(KeyStoreError::InvalidPath);
            };
            let parent =
                open_or_create_directory(nonempty(path.parent().unwrap_or(Path::new("."))))?;
            let created = match mkdirat(&parent, name, PRIVATE | Mode::XUSR) {
                Ok(()) => true,
                Err(rustix::io::Errno::EXIST) => false,
                Err(e) => return Err(io::Error::from(e).into()),
            };
            let child = match openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => File::from(fd),
                Err(error) => {
                    // Remove only our still-empty creation, never an existing
                    // directory or one containing another writer's data.
                    if created && unlinkat(&parent, name, AtFlags::REMOVEDIR).is_ok() {
                        let _ = parent.sync_all();
                    }
                    return Err(io::Error::from(error).into());
                }
            };
            parent.sync_all()?;
            Ok(child)
        }
        Err(e) => Err(io::Error::from(e).into()),
    }
}

#[cfg(test)]
mod tests;
