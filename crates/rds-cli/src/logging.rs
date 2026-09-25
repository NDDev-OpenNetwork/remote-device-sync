//! A new, private, size-capped file for one CLI process. Never append to or
//! replace an existing path. Remote stdout/stderr never enter this writer.
use rustix::fs::{Mode, OFlags, openat};
use std::{
    fs::File,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

const MAX_BYTES: usize = 8 * 1024 * 1024;

pub struct PrivateLog {
    file: File,
    written: usize,
}

impl PrivateLog {
    pub fn create(path: &Path) -> io::Result<Self> {
        let denied = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "log requires an absolute path in an existing private directory without symlink components",
            )
        };
        if !path.is_absolute() {
            return Err(denied());
        }
        let parts: Vec<_> = path.components().collect();
        if parts.len() < 3 {
            return Err(denied());
        }
        let mut directory = File::open("/")?;
        let uid = rustix::process::geteuid().as_raw();
        for (i, part) in parts.iter().enumerate().skip(1).take(parts.len() - 2) {
            let Component::Normal(name) = part else {
                return Err(denied());
            };
            directory = File::from(openat(
                &directory,
                *name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            let metadata = directory.metadata()?;
            let mode = metadata.mode();
            let owner = metadata.uid();
            if i == parts.len() - 2 {
                if owner != uid || mode & 0o777 != 0o700 {
                    return Err(denied());
                }
            } else if (owner != 0 && owner != uid)
                || (mode & 0o022 != 0 && !(owner == 0 && mode & 0o1000 != 0))
            {
                return Err(denied());
            }
        }
        let Some(Component::Normal(name)) = parts.last() else {
            return Err(denied());
        };
        let file = File::from(openat(
            &directory,
            *name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        Ok(Self { file, written: 0 })
    }
}

impl Write for PrivateLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.written {
            return Err(io::Error::other(
                "per-process log file reached its 8 MiB limit",
            ));
        }
        let count = self.file.write(bytes)?;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, symlink};

    #[test]
    fn log_is_private_capped_and_never_replaces_existing_entries() {
        let root = Path::new("/tmp")
            .canonicalize()
            .unwrap()
            .join(format!("rds-log-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let path = root.join("log");
        let mut writer = PrivateLog::create(&path).unwrap();
        writer.write_all(&vec![b'x'; MAX_BYTES]).unwrap();
        assert!(writer.write_all(b"one byte too far").is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_BYTES as u64);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(PrivateLog::create(&path).is_err());
        let link = root.join("link");
        symlink(&path, &link).unwrap();
        assert!(PrivateLog::create(&link).is_err());
        let alias = root.join("alias");
        symlink(&root, &alias).unwrap();
        assert!(PrivateLog::create(&alias.join("other")).is_err());
        assert!(!root.join("other").exists());
        assert!(PrivateLog::create(Path::new("relative.jsonl")).is_err());
    }
}
