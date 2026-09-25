//! Public API checks use only private temporary fixture directories and keys.
#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rds_net::{SecretKey, load_or_create_key};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rds-identity-{}-{}-{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn key(&self) -> PathBuf {
        self.0.join("endpoint.key")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn creation_and_reload_preserve_raw_seed_and_private_mode() {
    let t = Temp::new();
    let key = load_or_create_key(&t.key()).unwrap();
    assert_eq!(fs::read(t.key()).unwrap(), key.to_bytes());
    assert_eq!(
        fs::metadata(t.key()).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    for _ in 0..3 {
        assert_eq!(load_or_create_key(&t.key()).unwrap().public(), key.public());
    }
}

#[test]
fn final_key_symlink_is_refused_without_touching_target() {
    let t = Temp::new();
    let target = t.0.join("unrelated.key");
    let seed = SecretKey::generate().to_bytes();
    write_private(&target, &seed);
    symlink(&target, t.key()).unwrap();
    assert!(
        load_or_create_key(&t.key()).is_err(),
        "final key symlink was followed"
    );
    assert_eq!(fs::read(&target).unwrap(), seed);
    assert!(
        fs::symlink_metadata(t.key())
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn nonprivate_seed_is_refused_without_rewriting_permissions_or_bytes() {
    let t = Temp::new();
    let seed = SecretKey::generate().to_bytes();
    write_private(&t.key(), &seed);
    fs::set_permissions(t.key(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        load_or_create_key(&t.key()).is_err(),
        "nonprivate key accepted"
    );
    assert_eq!(fs::read(t.key()).unwrap(), seed);
    assert_eq!(
        fs::metadata(t.key()).unwrap().permissions().mode() & 0o7777,
        0o644
    );
}

#[test]
fn malformed_seed_is_preserved_and_never_replaced() {
    for length in [0, 31, 33, 4096] {
        let t = Temp::new();
        let bytes = vec![0x5a; length];
        write_private(&t.key(), &bytes);
        assert!(load_or_create_key(&t.key()).is_err());
        assert_eq!(fs::read(t.key()).unwrap(), bytes);
    }
}

#[test]
fn hardlinked_seed_is_refused_without_modifying_either_name() {
    let t = Temp::new();
    let seed = SecretKey::generate().to_bytes();
    write_private(&t.key(), &seed);
    let other = t.0.join("other.key");
    fs::hard_link(t.key(), &other).unwrap();
    assert!(
        load_or_create_key(&t.key()).is_err(),
        "hardlinked key accepted"
    );
    assert_eq!(fs::read(t.key()).unwrap(), seed);
    assert_eq!(fs::read(other).unwrap(), seed);
}

#[test]
#[cfg(target_os = "linux")]
fn fifo_and_directory_are_refused_without_blocking_or_replacement() {
    let t = Temp::new();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        t.key(),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    let start = std::time::Instant::now();
    assert!(load_or_create_key(&t.key()).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(t.key()).unwrap().file_type().is_fifo());
    fs::remove_file(t.key()).unwrap();
    fs::create_dir(t.key()).unwrap();
    assert!(load_or_create_key(&t.key()).is_err());
    assert!(t.key().is_dir());
}

#[test]
fn socket_and_directory_are_refused_without_replacement() {
    let t = Temp::new();
    // Keep the socket suffix short for Darwin's smaller sockaddr_un limit.
    let path = t.0.join("s");
    let socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
    assert!(load_or_create_key(&path).is_err());
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
    drop(socket);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(load_or_create_key(&path).is_err());
    assert!(path.is_dir());
}

#[test]
fn private_read_only_key_is_accepted_without_permission_changes() {
    let t = Temp::new();
    let key = load_or_create_key(&t.key()).unwrap();
    fs::set_permissions(t.key(), fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(load_or_create_key(&t.key()).unwrap().public(), key.public());
    assert_eq!(
        fs::metadata(t.key()).unwrap().permissions().mode() & 0o7777,
        0o400
    );
}
