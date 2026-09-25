//! Credential input must not block on special files before contacting a manager.
#![cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::time::Duration;

#[tokio::test]
async fn grant_input_rejects_special_symlink_and_oversized_files_before_key_creation() {
    let root = std::path::Path::new("/tmp")
        .canonicalize()
        .unwrap()
        .join(format!("rds-gi-{}", std::process::id()));
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
    let socket = root.join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let large = root.join("oversized");
    std::fs::write(&large, vec![b' '; 65537]).unwrap();
    let link = root.join("symlink");
    symlink(&large, &link).unwrap();
    let invalid = root.join("invalid");
    std::fs::write(&invalid, b"not-json").unwrap();
    for file in [&socket, &root, &large, &link, &invalid] {
        assert_refused(file, &root).await;
    }
    assert_eq!(std::fs::read(&large).unwrap().len(), 65537);
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn fifo_grant_input_returns_without_waiting_for_a_writer() {
    let root = std::env::temp_dir().join(format!("rds-grant-fifo-{}", std::process::id()));
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
    let file = root.join("fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &file,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert_refused(&file, &root).await;
}

async fn assert_refused(file: &std::path::Path, root: &std::path::Path) {
    for session in [false, true] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rds"));
        command
            .kill_on_drop(true)
            .env("XDG_CONFIG_HOME", root.join("config"));
        if session {
            command
                .args(["session", "connect", "unused-peer", "--grant-file"])
                .arg(file);
        } else {
            command
                .arg("--grant")
                .arg(file)
                .args(["ping", "unused-peer"]);
        }
        let output = tokio::time::timeout(Duration::from_secs(3), command.output())
            .await
            .expect("grant input blocked before request")
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!root.join("config").exists());
    }
}
