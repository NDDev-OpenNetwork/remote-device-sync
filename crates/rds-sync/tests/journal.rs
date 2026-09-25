//! Regression coverage for local reuse and assembly ownership.

use std::path::PathBuf;

use rds_sync::{Manifest, journal::Journal, manifest_of};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rds-journal-{name}-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn data() -> Vec<u8> {
    let mut state = 0x005D_50C6_u64;
    (0..2_000_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn fetch_missing(journal: &mut Journal, manifest: &Manifest, bytes: &[u8]) {
    for index in journal.need() {
        let chunk = manifest.chunks[index as usize];
        journal
            .store(
                index,
                &bytes[chunk.offset as usize..chunk.offset as usize + chunk.len as usize],
            )
            .unwrap();
    }
    assert!(journal.complete());
}

#[test]
fn partial_destination_reuse_assembles_after_source_mutation_and_restart() {
    let old = data();
    for variant in ["edit", "insert", "delete"] {
        let dir = Scratch::new(variant);
        let dest = dir.0.join("data.bin");
        std::fs::write(&dest, &old).unwrap();
        let mut new = old.clone();
        match variant {
            "edit" => new[1_000_000] ^= 0xff,
            "insert" => {
                new.splice(1_000_000..1_000_000, [7u8; 3_000]);
            }
            "delete" => {
                new.drain(1_000_000..1_003_000);
            }
            _ => unreachable!(),
        }
        let manifest = manifest_of(&new);
        let journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
        let needed = journal.need();
        assert!(!needed.is_empty(), "{variant} must require some data");
        assert!(needed.len() < journal.total() / 2, "{variant} lost reuse");
        assert_eq!(journal.fetched(), 0, "local reuse is not network traffic");
        // Advertised parts must survive the original file changing or vanishing.
        std::fs::write(&dest, b"changed while receiving").unwrap();
        drop(journal);
        std::fs::remove_file(&dest).unwrap();
        let mut resumed = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
        assert_eq!(resumed.need(), needed, "{variant} reuse was not persisted");
        fetch_missing(&mut resumed, &manifest, &new);
        assert_eq!(resumed.fetched(), needed.len() as u64);
        resumed.assemble().unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), new);
    }
}

#[test]
fn repeated_destination_chunks_survive_without_original_file() {
    let dir = Scratch::new("repeated");
    let old = vec![0u8; 2_000_000];
    let dest = dir.0.join("repeated.bin");
    std::fs::write(&dest, &old).unwrap();
    let mut new = old;
    new[1_000_000] = 1;
    let manifest = manifest_of(&new);
    assert!(manifest.chunks.windows(2).any(|c| c[0].hash == c[1].hash));
    let mut journal = Journal::open(&dir.0, "repeated.bin", &manifest).unwrap();
    assert!(journal.need().len() < journal.total());
    std::fs::remove_file(dest).unwrap();
    fetch_missing(&mut journal, &manifest, &new);
    let dest = journal.assemble().unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), new);
}

#[test]
fn assembly_preserves_user_staging_suffix_file() {
    let dir = Scratch::new("sibling");
    let sibling = dir.0.join("data.bin.rds-part");
    std::fs::write(&sibling, b"user-owned data").unwrap();
    let manifest = manifest_of(b"new destination");
    let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    fetch_missing(&mut journal, &manifest, b"new destination");
    journal.assemble().unwrap();
    assert_eq!(std::fs::read(sibling).unwrap(), b"user-owned data");
}

#[cfg(unix)]
#[test]
fn assembly_preserves_user_staging_suffix_links() {
    use std::os::unix::fs::symlink;
    for link in ["symlink", "hardlink"] {
        let dir = Scratch::new(link);
        let target = dir.0.join("user.txt");
        let sibling = dir.0.join("data.bin.rds-part");
        std::fs::write(&target, b"user-owned data").unwrap();
        if link == "symlink" {
            symlink(&target, &sibling).unwrap();
        } else {
            std::fs::hard_link(&target, &sibling).unwrap();
        }
        let manifest = manifest_of(b"new destination");
        let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
        fetch_missing(&mut journal, &manifest, b"new destination");
        journal.assemble().unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"user-owned data");
        assert_eq!(std::fs::read(&sibling).unwrap(), b"user-owned data");
        assert_eq!(sibling.is_symlink(), link == "symlink");
    }
}

#[test]
fn failed_assembly_keeps_destination_and_cleans_own_staging_file() {
    let dir = Scratch::new("failed-assembly");
    let dest = dir.0.join("data.bin");
    std::fs::write(&dest, b"old destination").unwrap();
    let mut manifest = manifest_of(b"new destination");
    manifest.root = *blake3::hash(b"inconsistent root").as_bytes();
    let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    fetch_missing(&mut journal, &manifest, b"new destination");
    assert!(journal.assemble().is_err());
    assert_eq!(std::fs::read(dest).unwrap(), b"old destination");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
}

fn journal_path(dir: &Scratch, manifest: &Manifest) -> PathBuf {
    dir.0
        .join(".rds-sync")
        .join(blake3::Hash::from(manifest.root).to_hex().as_str())
}

#[test]
fn missing_part_cleans_staging_and_preserves_old_destination() {
    let dir = Scratch::new("missing-part");
    let dest = dir.0.join("data.bin");
    std::fs::write(&dest, b"old destination").unwrap();
    let manifest = manifest_of(b"new destination");
    let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    fetch_missing(&mut journal, &manifest, b"new destination");
    let part = journal_path(&dir, &manifest).join("parts").join(
        blake3::Hash::from(manifest.chunks[0].hash)
            .to_hex()
            .as_str(),
    );
    std::fs::remove_file(part).unwrap();
    assert!(journal.assemble().is_err());
    assert_eq!(std::fs::read(dest).unwrap(), b"old destination");
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn planted_journal_directory_and_part_links_are_refused() {
    use std::os::unix::fs::symlink;
    let bytes = b"one verified chunk";
    let manifest = manifest_of(bytes);
    for variant in ["state", "content", "parts", "part", "hardlink"] {
        let dir = Scratch::new(variant);
        let outside = Scratch::new("outside");
        let sentinel = outside.0.join("sentinel");
        std::fs::write(&sentinel, bytes).unwrap();
        let journal = journal_path(&dir, &manifest);
        match variant {
            "state" => symlink(&outside.0, dir.0.join(".rds-sync")).unwrap(),
            "content" => {
                std::fs::create_dir(dir.0.join(".rds-sync")).unwrap();
                symlink(&outside.0, &journal).unwrap();
            }
            "parts" => {
                std::fs::create_dir_all(&journal).unwrap();
                symlink(&outside.0, journal.join("parts")).unwrap();
            }
            "part" | "hardlink" => {
                std::fs::create_dir_all(journal.join("parts")).unwrap();
                let part = journal.join("parts").join(
                    blake3::Hash::from(manifest.chunks[0].hash)
                        .to_hex()
                        .as_str(),
                );
                if variant == "part" {
                    symlink(&sentinel, part).unwrap();
                } else {
                    std::fs::hard_link(&sentinel, part).unwrap();
                }
            }
            _ => unreachable!(),
        }
        assert!(
            Journal::open(&dir.0, "data.bin", &manifest).is_err(),
            "{variant} accepted"
        );
        assert_eq!(std::fs::read(sentinel).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn metadata_and_legacy_temp_links_never_write_their_targets() {
    use std::os::unix::fs::symlink;
    let dir = Scratch::new("metadata-links");
    let outside = Scratch::new("metadata-outside");
    let sentinel = outside.0.join("sentinel");
    std::fs::write(&sentinel, b"do not overwrite").unwrap();
    let manifest = manifest_of(b"new bytes");
    let journal_dir = journal_path(&dir, &manifest);
    std::fs::create_dir_all(journal_dir.join("parts")).unwrap();
    symlink(&sentinel, journal_dir.join("meta")).unwrap();
    symlink(&sentinel, journal_dir.join("meta.tmp")).unwrap();
    let part_temp = journal_dir.join("parts").join(format!(
        "{}.tmp",
        blake3::Hash::from(manifest.chunks[0].hash).to_hex()
    ));
    symlink(&sentinel, &part_temp).unwrap();
    let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    fetch_missing(&mut journal, &manifest, b"new bytes");
    journal.assemble().unwrap();
    assert_eq!(std::fs::read(sentinel).unwrap(), b"do not overwrite");
    assert!(journal_dir.join("meta.tmp").is_symlink());
    assert!(part_temp.is_symlink());
}

#[cfg(unix)]
#[test]
fn directory_substitution_after_open_keeps_io_on_pinned_handles() {
    use std::os::unix::fs::symlink;
    let dir = Scratch::new("substitution");
    let outside = Scratch::new("substitution-outside");
    let manifest = manifest_of(b"new bytes");
    let mut journal = Journal::open(&dir.0, "nested/data.bin", &manifest).unwrap();
    let state = journal_path(&dir, &manifest);
    std::fs::rename(state.join("parts"), state.join("parts-held")).unwrap();
    symlink(&outside.0, state.join("parts")).unwrap();
    std::fs::rename(dir.0.join("nested"), dir.0.join("nested-held")).unwrap();
    symlink(&outside.0, dir.0.join("nested")).unwrap();
    std::fs::rename(dir.0.join(".rds-sync"), dir.0.join(".rds-sync-held")).unwrap();
    symlink(&outside.0, dir.0.join(".rds-sync")).unwrap();
    fetch_missing(&mut journal, &manifest, b"new bytes");
    journal.assemble().unwrap();
    assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0);
    assert_eq!(
        std::fs::read(dir.0.join("nested-held/data.bin")).unwrap(),
        b"new bytes"
    );
}

#[test]
fn root_lock_prevents_cleanup_of_another_transfer() {
    let dir = Scratch::new("concurrent");
    let one = manifest_of(b"one");
    let two = manifest_of(b"two");
    let first = Journal::open(&dir.0, "one.bin", &one).unwrap();
    assert!(Journal::open(&dir.0, "one.bin", &one).is_err());
    assert!(Journal::open(&dir.0, "one.bin", &two).is_err());
    assert!(Journal::open(&dir.0, "alias.bin", &one).is_err());
    assert!(Journal::open(&dir.0, "two.bin", &two).is_err());
    drop(first);
    assert!(Journal::open(&dir.0, "one.bin", &one).is_ok());
}

#[test]
fn overlapping_roots_share_destination_ownership() {
    let dir = Scratch::new("overlapping-roots");
    let nested = dir.0.join("nested");
    let one = manifest_of(b"first transfer");
    let two = manifest_of(b"second transfer");
    let first = Journal::open(&dir.0, "nested/data.bin", &one).unwrap();
    assert!(Journal::open(&nested, "data.bin", &two).is_err());
    drop(first);
    let second = Journal::open(&nested, "data.bin", &two).unwrap();
    assert!(Journal::open(&dir.0, "nested/data.bin", &one).is_err());
    drop(second);
    assert!(Journal::open(&dir.0, "nested/data.bin", &one).is_ok());
}

#[test]
fn nested_private_state_is_never_a_sync_destination() {
    let dir = Scratch::new("nested-private-state");
    let manifest = manifest_of(b"do not replace state");
    for path in [
        "nested/.rds-sync/receive.lock",
        "nested/.RDS-SYNC/assembly",
        "nested/.rDs-SyNc",
    ] {
        assert!(Journal::open(&dir.0, path, &manifest).is_err(), "{path}");
    }
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn locks_survive_process_boundary() {
    let manifest = manifest_of(b"locked bytes");
    if let Some(path) = std::env::var_os("RDS_TEST_JOURNAL_LOCK_ROOT") {
        let available = std::env::var_os("RDS_TEST_JOURNAL_LOCK_AVAILABLE").is_some();
        assert_eq!(
            Journal::open(PathBuf::from(path).as_path(), "data.bin", &manifest).is_ok(),
            available
        );
        return;
    }
    let dir = Scratch::new("process-lock");
    let journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    let run_child = |available| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "locks_survive_process_boundary"])
            .env("RDS_TEST_JOURNAL_LOCK_ROOT", &dir.0)
            .env_remove("RDS_TEST_JOURNAL_LOCK_AVAILABLE");
        if available {
            command.env("RDS_TEST_JOURNAL_LOCK_AVAILABLE", "1");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    };
    run_child(false);
    drop(journal);
    run_child(true);
}

#[test]
fn invalid_path_is_rejected_by_journal_itself() {
    let dir = Scratch::new("invalid-path");
    let manifest = manifest_of(b"new bytes");
    for path in [
        "../escape",
        "/absolute",
        ".rds-sync/locks/bypass",
        ".RDS-SYNC/receive.lock",
        ".rDs-SyNc/meta",
        "",
    ] {
        assert!(Journal::open(&dir.0, path, &manifest).is_err());
    }
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn abrupt_process_exit_releases_lock_and_preserves_verified_parts() {
    let bytes = data();
    let manifest = manifest_of(&bytes);
    if let Some(path) = std::env::var_os("RDS_TEST_JOURNAL_CRASH_ROOT") {
        let path = PathBuf::from(path);
        let mut journal = Journal::open(&path, "data.bin", &manifest).unwrap();
        for (index, c) in manifest.chunks.iter().take(3).enumerate() {
            journal
                .store(
                    index as u32,
                    &bytes[c.offset as usize..c.offset as usize + c.len as usize],
                )
                .unwrap();
        }
        std::fs::write(path.join("ready"), b"parts committed").unwrap();
        loop {
            std::thread::park();
        }
    }
    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = Scratch::new("process-crash");
    std::fs::write(dir.0.join("data.bin"), b"old destination").unwrap();
    let mut child = KillOnDrop(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "abrupt_process_exit_releases_lock_and_preserves_verified_parts",
            ])
            .env("RDS_TEST_JOURNAL_CRASH_ROOT", &dir.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !dir.0.join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "writer exited before committing parts"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "writer did not commit parts in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(
        std::fs::read(dir.0.join("data.bin")).unwrap(),
        b"old destination"
    );
    let mut resumed = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    assert_eq!(resumed.need().len(), manifest.chunks.len() - 3);
    fetch_missing(&mut resumed, &manifest, &bytes);
    let dest = resumed.assemble().unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), bytes);
}
