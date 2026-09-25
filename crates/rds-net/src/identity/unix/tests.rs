use super::*;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const PHASES: [Phase; 7] = [
    Phase::AfterMarkerCreate,
    Phase::AfterStagingCreate,
    Phase::BeforeWrite,
    Phase::AfterWrite,
    Phase::AfterFileSync,
    Phase::AfterPublish,
    Phase::AfterDirectorySync,
];

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rds-key-transaction-{}-{}-{:x}",
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
    fn lock(&self) -> PathBuf {
        self.0.join(LOCK_NAME)
    }
    fn pending(&self) -> PathBuf {
        self.0.join(PENDING_NAME)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn write_private(path: &Path, data: &[u8]) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(data)
        .unwrap();
}

fn wait_child(child: &mut Child) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("owned identity test child exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn child(t: &Temp, mode: &str, value: usize) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "identity::unix::tests::child_entrypoint",
            "--nocapture",
        ])
        .env("RDS_KEY_TEST_ROOT", &t.0)
        .env("RDS_KEY_TEST_MODE", mode)
        .env("RDS_KEY_TEST_VALUE", value.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

#[test]
fn child_entrypoint() {
    // Only this test-binary function reads injection environment variables.
    // Production key loading has no environment-controlled fault behavior.
    let Some(root) = std::env::var_os("RDS_KEY_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let value: usize = std::env::var("RDS_KEY_TEST_VALUE")
        .unwrap()
        .parse()
        .unwrap();
    match std::env::var("RDS_KEY_TEST_MODE").unwrap().as_str() {
        "exit" | "exit-strict" => {
            if std::env::var("RDS_KEY_TEST_MODE").unwrap() == "exit-strict" {
                rustix::process::umask(Mode::from_raw_mode(0o777));
            }
            transaction(&root.join("endpoint.key"), |phase| {
                if phase == PHASES[value] {
                    std::process::exit(86);
                }
                Ok(())
            })
            .unwrap();
            panic!("selected crash checkpoint did not execute");
        }
        "create" => {
            let start = Instant::now();
            while !root.join("go").exists() {
                assert!(start.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(2));
            }
            let result = transaction(&root.join("endpoint.key"), |phase| {
                // Hold the winner's transaction open so other independent
                // processes contend while a partial seed could be visible.
                if phase == Phase::BeforeWrite {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(())
            });
            let key = match result {
                Ok(key) => key,
                // The public API deliberately bounds lock wait. The parent
                // retries this one defined transient result after contention.
                Err(KeyStoreError::Busy) => std::process::exit(75),
                Err(error) => panic!("identity child failed: {error}"),
            };
            write_private(
                &root.join(format!("result-{value}")),
                key.public().to_string().as_bytes(),
            );
        }
        "umask" => {
            let path = root.join("endpoint.key");
            if value == 1 {
                drop(Transaction::open(&path).unwrap());
            }
            // This process-global setting changes only in the isolated child.
            let previous = rustix::process::umask(Mode::from_raw_mode(0o777));
            let result = load_or_create(&path);
            rustix::process::umask(previous);
            let key = result.unwrap();
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
            assert_eq!(
                fs::metadata(root.join(LOCK_NAME)).unwrap().mode() & 0o7777,
                0o600
            );
            assert_eq!(load_or_create(&path).unwrap().public(), key.public());
            assert!(!root.join(PENDING_NAME).exists());
        }
        "umask-parent" => {
            let parent = root.join("new-parent");
            let path = parent.join("endpoint.key");
            let previous = rustix::process::umask(Mode::from_raw_mode(0o777));
            let result = load_or_create(&path);
            rustix::process::umask(previous);
            if rustix::process::geteuid().is_root() {
                // Root can open its mode000 directory; success must still
                // return a readable seed that is preserved on reload.
                assert_eq!(
                    load_or_create(&path).unwrap().public(),
                    result.unwrap().public()
                );
            } else {
                assert!(result.is_err());
                assert!(!parent.exists(), "failed empty creation was left behind");
                load_or_create(&path).unwrap();
            }
        }
        _ => panic!("invalid child fixture mode"),
    }
}

#[test]
fn independent_process_creators_return_one_committed_identity() {
    let t = Temp::new();
    let mut children: Vec<_> = (0..8).map(|i| child(&t, "create", i)).collect();
    write_private(&t.0.join("go"), b"");
    let statuses: Vec<_> = children.iter_mut().map(wait_child).collect();
    assert!(
        statuses.iter().any(std::process::ExitStatus::success),
        "no creator succeeded: {statuses:?}"
    );
    for (index, status) in statuses.into_iter().enumerate() {
        if !status.success() {
            assert_eq!(status.code(), Some(75), "unexpected child failure");
            assert!(wait_child(&mut child(&t, "create", index)).success());
        }
    }
    let expected = load_or_create(&t.key()).unwrap().public().to_string();
    for i in 0..8 {
        assert_eq!(
            fs::read_to_string(t.0.join(format!("result-{i}"))).unwrap(),
            expected
        );
    }
    assert!(!t.pending().exists());
}

#[test]
fn restrictive_umask_never_returns_an_unreadable_identity_or_poisoned_lock() {
    for existing_lock in [0, 1] {
        let t = Temp::new();
        assert!(wait_child(&mut child(&t, "umask", existing_lock)).success());
    }
    let t = Temp::new();
    assert!(wait_child(&mut child(&t, "umask-parent", 0)).success());
}

#[test]
fn abrupt_exit_before_permissions_recovers_under_the_parent_inode_lock() {
    for (index, phase) in PHASES.into_iter().enumerate() {
        let t = Temp::new();
        assert_eq!(
            wait_child(&mut child(&t, "exit-strict", index)).code(),
            Some(86)
        );
        validate_recovery(&t, phase);
    }
}

#[test]
fn oversized_pending_state_is_refused_without_deletion() {
    let t = Temp::new();
    drop(Transaction::open(&t.key()).unwrap());
    write_private(&t.pending(), &[0x5a; 33]);
    assert!(matches!(
        load_or_create(&t.key()),
        Err(KeyStoreError::InvalidState)
    ));
    assert_eq!(fs::read(t.pending()).unwrap(), [0x5a; 33]);
    assert!(!t.key().exists());
}

fn validate_recovery(t: &Temp, phase: Phase) {
    let published = matches!(phase, Phase::AfterPublish | Phase::AfterDirectorySync);
    assert_eq!(t.key().exists(), published, "{phase:?}");
    let before = published.then(|| fs::read(t.key()).unwrap());
    let key = load_or_create(&t.key()).unwrap();
    if let Some(before) = before {
        assert_eq!(before, key.to_bytes());
    }
    assert_eq!(load_or_create(&t.key()).unwrap().public(), key.public());
    assert_eq!(fs::metadata(t.key()).unwrap().nlink(), 1);
    assert!(!t.pending().exists());
    assert_eq!(fs::read(t.lock()).unwrap(), MARKER);
}

#[test]
fn returned_errors_preserve_published_identity_and_clean_owned_staging() {
    for selected in PHASES {
        let t = Temp::new();
        let result = transaction(&t.key(), |phase| {
            if phase == selected {
                Err(io::Error::other("injected transaction failure").into())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err(), "{selected:?}");
        assert!(!t.pending().exists(), "{selected:?}");
        validate_recovery(&t, selected);
    }
}

#[test]
fn abrupt_exit_at_each_boundary_recovers_without_replacing_a_published_key() {
    for (index, phase) in PHASES.into_iter().enumerate() {
        let t = Temp::new();
        assert_eq!(wait_child(&mut child(&t, "exit", index)).code(), Some(86));
        validate_recovery(&t, phase);
    }
}

#[test]
fn marker_prefix_recovery_is_limited_to_empty_unpublished_state() {
    let t = Temp::new();
    write_private(&t.lock(), &MARKER[..7]);
    load_or_create(&t.key()).unwrap();
    assert_eq!(fs::read(t.lock()).unwrap(), MARKER);

    let t = Temp::new();
    write_private(&t.lock(), b"unrecognized owner");
    assert!(matches!(
        load_or_create(&t.key()),
        Err(KeyStoreError::InvalidState)
    ));
    assert_eq!(fs::read(t.lock()).unwrap(), b"unrecognized owner");
    assert!(!t.key().exists());

    let t = Temp::new();
    write_private(&t.pending(), b"unclaimed data");
    assert!(matches!(
        load_or_create(&t.key()),
        Err(KeyStoreError::InvalidState)
    ));
    assert_eq!(fs::read(t.pending()).unwrap(), b"unclaimed data");
    assert!(!t.key().exists());
}

#[test]
fn pending_symlink_and_unrelated_hardlink_are_left_untouched() {
    let t = Temp::new();
    drop(Transaction::open(&t.key()).unwrap());
    let other = t.0.join("other");
    write_private(&other, b"unrelated private data");
    symlink(&other, t.pending()).unwrap();
    assert!(load_or_create(&t.key()).is_err());
    assert!(
        fs::symlink_metadata(t.pending())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fs::remove_file(t.pending()).unwrap();
    fs::hard_link(&other, t.pending()).unwrap();
    assert!(matches!(
        load_or_create(&t.key()),
        Err(KeyStoreError::UnsafeFile)
    ));
    assert_eq!(fs::read(&other).unwrap(), b"unrelated private data");
    assert!(t.pending().exists());
}

#[test]
fn directory_lock_bounds_contention_and_keeps_a_stable_inode() {
    let t = Temp::new();
    let held = Transaction::open(&t.key()).unwrap();
    let inode = fs::metadata(&t.0).unwrap().ino();
    let start = Instant::now();
    assert!(matches!(
        load_or_create(&t.0.join("another.key")),
        Err(KeyStoreError::Busy)
    ));
    assert!(start.elapsed() >= LOCK_WAIT);
    let inherited_alias = held.directory.try_clone().unwrap();
    drop(held);
    load_or_create(&t.key()).unwrap();
    assert_eq!(fs::metadata(&t.0).unwrap().ino(), inode);
    drop(inherited_alias);
}

#[test]
fn long_key_name_and_trusted_parent_symlink_work() {
    let t = Temp::new();
    let directory = t.0.join("real");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    symlink(&directory, t.0.join("alias")).unwrap();
    let name = "k".repeat(255);
    let key = load_or_create(&t.0.join("alias").join(&name)).unwrap();
    assert_eq!(
        load_or_create(&directory.join(name)).unwrap().public(),
        key.public()
    );
}

#[test]
fn unsafe_parent_and_reserved_key_names_fail_without_seed_creation() {
    let t = Temp::new();
    fs::set_permissions(&t.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        load_or_create(&t.key()),
        Err(KeyStoreError::UnsafeParent)
    ));
    assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
    assert_eq!(fs::metadata(&t.0).unwrap().mode() & 0o777, 0o777);
    fs::set_permissions(&t.0, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        load_or_create(&t.0.join(".rds-key-reserved")),
        Err(KeyStoreError::InvalidPath)
    ));
}

#[test]
fn missing_parent_hierarchy_is_created_privately() {
    let t = Temp::new();
    let key = t.0.join("one/two/endpoint.key");
    load_or_create(&key).unwrap();
    for directory in [t.0.join("one"), t.0.join("one/two")] {
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
    }
}

#[test]
fn legacy_creator_wins_without_overwrite_or_staging_leak() {
    let t = Temp::new();
    let winner = SecretKey::generate();
    let key = transaction(&t.key(), |phase| {
        if phase == Phase::AfterFileSync {
            write_private(&t.key(), &winner.to_bytes());
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(key.public(), winner.public());
    assert_eq!(fs::read(t.key()).unwrap(), winner.to_bytes());
    assert!(!t.pending().exists());

    let t = Temp::new();
    let error = transaction(&t.key(), |phase| {
        if phase == Phase::AfterFileSync {
            write_private(&t.key(), b"partial legacy write");
        }
        Ok(())
    });
    assert!(matches!(error, Err(KeyStoreError::InvalidLength)));
    assert_eq!(fs::read(t.key()).unwrap(), b"partial legacy write");
    assert!(!t.pending().exists());
}

#[test]
fn held_parent_directory_survives_a_path_replacement() {
    let t = Temp::new();
    let original = t.0.join("original");
    let moved = t.0.join("moved");
    fs::DirBuilder::new().mode(0o700).create(&original).unwrap();
    let other = SecretKey::generate();
    let key = transaction(&original.join("endpoint.key"), |phase| {
        if phase == Phase::BeforeWrite {
            fs::rename(&original, &moved).unwrap();
            fs::DirBuilder::new().mode(0o700).create(&original).unwrap();
            write_private(&original.join("endpoint.key"), &other.to_bytes());
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        fs::read(moved.join("endpoint.key")).unwrap(),
        key.to_bytes()
    );
    assert_eq!(
        fs::read(original.join("endpoint.key")).unwrap(),
        other.to_bytes()
    );
    assert!(!moved.join(PENDING_NAME).exists());
}

#[test]
fn inode_checks_reject_aliases_of_internal_transaction_files() {
    let t = Temp::new();
    let state = Transaction::open(&t.key()).unwrap();
    fs::hard_link(t.lock(), t.key()).unwrap();
    assert!(matches!(
        state.reject_internal_key_alias(),
        Err(KeyStoreError::InvalidPath)
    ));
    fs::remove_file(t.key()).unwrap();
    write_private(&t.pending(), &[0; 32]);
    fs::hard_link(t.pending(), t.key()).unwrap();
    assert!(matches!(
        state.reject_internal_key_alias(),
        Err(KeyStoreError::InvalidPath)
    ));
    assert_eq!(fs::read(t.lock()).unwrap(), MARKER);
}

#[test]
fn orphan_cleanup_is_independent_of_the_requested_key_name() {
    let t = Temp::new();
    assert_eq!(wait_child(&mut child(&t, "exit", 2)).code(), Some(86));
    assert!(t.pending().exists());
    let other = t.0.join("different.key");
    load_or_create(&other).unwrap();
    assert!(!t.pending().exists());
    assert!(!t.key().exists());
    assert_eq!(fs::read(other).unwrap().len(), 32);
}
