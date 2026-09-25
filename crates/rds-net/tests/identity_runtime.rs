//! Cooperative runtime exclusion, independent processes and crash recovery.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rds_net::{KeyStoreError, acquire_key, load_or_create_key};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "rds-owner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn owner_excludes_same_inode_but_allows_readers_and_other_keys() {
    let root = Scratch::new();
    let path = root.0.join("endpoint.key");
    let owner = acquire_key(&path).unwrap();
    let id = owner.secret_key().public();
    assert!(matches!(acquire_key(&path), Err(KeyStoreError::InUse)));
    assert_eq!(load_or_create_key(&path).unwrap().public(), id);
    let other = acquire_key(&root.0.join("other.key")).unwrap();
    assert_ne!(other.secret_key().public(), id);
    symlink(&root.0, root.0.join("alias")).unwrap();
    assert!(matches!(
        acquire_key(&root.0.join("alias/endpoint.key")),
        Err(KeyStoreError::InUse)
    ));
    drop(owner);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    let owner = acquire_key(&path).unwrap();
    assert_eq!(owner.secret_key().public(), id);
    assert!(matches!(acquire_key(&path), Err(KeyStoreError::InUse)));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o400
    );
    drop(owner);
    assert_eq!(acquire_key(&path).unwrap().secret_key().public(), id);
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn independent_creators_have_one_runtime_owner_and_sigkill_releases_it() {
    let root = Scratch::new();
    let path = root.0.join("endpoint.key");
    let mut children = Vec::new();
    for index in 0..8 {
        children.push(Process(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "runtime_child", "--nocapture"])
                .env("RDS_OWNER_TEST_KEY", &path)
                .env(
                    "RDS_OWNER_TEST_RESULT",
                    root.0.join(format!("result-{index}")),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while !(0..8).all(|i| root.0.join(format!("result-{i}")).exists()) {
        assert!(
            Instant::now() < deadline,
            "child did not finish acquisition"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut winners = Vec::new();
    for index in 0..8 {
        match fs::read(root.0.join(format!("result-{index}")))
            .unwrap()
            .as_slice()
        {
            b"owner" => winners.push(index),
            b"in-use" => {}
            other => panic!("unexpected acquisition result: {other:?}"),
        }
    }
    assert_eq!(winners.len(), 1);
    let id = load_or_create_key(&path).unwrap().public();
    assert!(matches!(acquire_key(&path), Err(KeyStoreError::InUse)));
    let winner = &mut children[winners[0]].0;
    winner.kill().unwrap();
    assert!(!winner.wait().unwrap().success());
    assert_eq!(acquire_key(&path).unwrap().secret_key().public(), id);
}

#[test]
fn runtime_child() {
    let Some(path) = std::env::var_os("RDS_OWNER_TEST_KEY") else {
        return;
    };
    let result = PathBuf::from(std::env::var_os("RDS_OWNER_TEST_RESULT").unwrap());
    let owner = acquire_key(&PathBuf::from(path));
    let status = match &owner {
        Ok(_) => b"owner".as_slice(),
        Err(KeyStoreError::InUse) => b"in-use".as_slice(),
        Err(_) => b"unexpected".as_slice(),
    };
    // Publish status atomically, so the parent never observes a short write.
    let pending = result.with_extension("pending");
    fs::write(&pending, status).unwrap();
    fs::rename(pending, result).unwrap();
    if owner.is_ok() {
        use std::io::Read;
        let _ = std::io::stdin().read(&mut [0u8]);
    }
    drop(owner);
}
