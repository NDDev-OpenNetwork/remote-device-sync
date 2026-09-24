//! Protected atomic policy state. The configured ancestors and OS identity are
//! trusted. Every descendant operation is relative to a pinned directory.

use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

use crate::DiscoveryError;

const MAX_STATE: usize = 2 * 1024 * 1024;

pub(crate) struct AtomicFile {
    directory: File,
    _lock: File,
    state_name: &'static str,
    #[cfg(test)]
    pub(crate) fault: Option<(Phase, bool)>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    BeforeWrite,
    AfterWrite,
    AfterFileSync,
    AfterRename,
    AfterDirectorySync,
}

fn store(e: impl std::fmt::Display) -> DiscoveryError {
    DiscoveryError::Store(e.to_string())
}

pub(crate) fn regular(file: &File) -> Result<(), DiscoveryError> {
    let meta = file.metadata().map_err(store)?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(store("policy state must be a singly linked regular file"));
    }
    Ok(())
}

impl AtomicFile {
    pub(crate) fn initialized(&self) -> Result<bool, DiscoveryError> {
        Ok(self._lock.metadata().map_err(store)?.len() != 0)
    }

    pub(crate) fn seal(&self) -> Result<(), DiscoveryError> {
        let mut lock = &self._lock;
        lock.write_all(b"rds-policy-state/v1\n").map_err(store)?;
        lock.sync_all().map_err(store)?;
        self.directory.sync_all().map_err(store)
    }
    /// Lifetime-exclusive ownership. CLI transactions open/release around one
    /// commit; long-running services own their state directory until shutdown.
    pub(crate) fn open(path: &Path) -> Result<Self, DiscoveryError> {
        Self::open_named(path, "policy.json", "policy.lock")
    }

    pub(crate) fn directory(&self) -> &File {
        &self.directory
    }

    pub(crate) fn open_named(
        path: &Path,
        state_name: &'static str,
        lock_name: &'static str,
    ) -> Result<Self, DiscoveryError> {
        std::fs::create_dir_all(path).map_err(store)?;
        let directory = File::from(
            rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(store)?,
        );
        if directory.metadata().map_err(store)?.permissions().mode() & 0o7777 != 0o700 {
            directory
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(store)?;
        }
        let lock = File::from(
            openat(
                &directory,
                lock_name,
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(store)?,
        );
        regular(&lock)?;
        lock.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => DiscoveryError::Busy,
            std::fs::TryLockError::Error(e) => store(e),
        })?;
        Ok(Self {
            directory,
            _lock: lock,
            state_name,
            #[cfg(test)]
            fault: None,
        })
    }

    #[cfg(test)]
    fn checkpoint(&self, phase: Phase) -> Result<(), DiscoveryError> {
        if let Some((selected, terminate)) = self.fault
            && selected == phase
        {
            if terminate {
                // Test child only: intentionally bypass destructors and cleanup.
                std::process::exit(86);
            }
            return Err(store(format!("injected failure at {phase:?}")));
        }
        Ok(())
    }

    pub(crate) fn read(&self) -> Result<Option<Vec<u8>>, DiscoveryError> {
        let fd = match openat(
            &self.directory,
            self.state_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(store(e)),
        };
        let file = File::from(fd);
        regular(&file)?;
        let mut bytes = Vec::new();
        file.take(MAX_STATE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(store)?;
        if bytes.len() > MAX_STATE {
            return Err(store("policy state exceeds limit"));
        }
        Ok(Some(bytes))
    }

    pub(crate) fn write(&self, bytes: &[u8]) -> Result<(), DiscoveryError> {
        if bytes.len() > MAX_STATE {
            return Err(store("policy state exceeds limit"));
        }
        let _ = self.read()?;
        let mut staging = None;
        for _ in 0..16 {
            let name = format!(".policy-{:032x}.tmp", rand::random::<u128>());
            match openat(
                &self.directory,
                &name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(fd) => {
                    staging = Some((name, File::from(fd)));
                    break;
                }
                Err(rustix::io::Errno::EXIST) => continue,
                Err(e) => return Err(store(e)),
            }
        }
        let (name, mut file) = staging.ok_or_else(|| store("cannot allocate policy staging"))?;
        let result = (|| {
            #[cfg(test)]
            self.checkpoint(Phase::BeforeWrite)?;
            file.write_all(bytes).map_err(store)?;
            #[cfg(test)]
            self.checkpoint(Phase::AfterWrite)?;
            file.sync_all().map_err(store)?;
            #[cfg(test)]
            self.checkpoint(Phase::AfterFileSync)?;
            renameat(&self.directory, &name, &self.directory, self.state_name).map_err(store)?;
            #[cfg(test)]
            self.checkpoint(Phase::AfterRename)?;
            self.directory.sync_all().map_err(store)?;
            #[cfg(test)]
            self.checkpoint(Phase::AfterDirectorySync)?;
            Ok(())
        })();
        // Only the exclusively created name is ours; unknown orphans stay put.
        let _ = unlinkat(&self.directory, &name, AtFlags::empty());
        result
    }
}
