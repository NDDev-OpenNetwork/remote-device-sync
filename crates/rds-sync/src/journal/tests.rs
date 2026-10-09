use super::*;
use crate::fault::{Action, arm};

const OLD: &[u8] = b"prior destination must remain intact";
const NEW: &[u8] = b"complete new verified destination";
const WRITE_POINTS: &[Point] = &[
    Point::Created,
    Point::PartialWrite,
    Point::Written,
    Point::FileSynced,
    Point::Renamed,
    Point::DestinationSynced,
];
const ASSEMBLY_POINTS: &[Point] = &[
    Point::Created,
    Point::PartialWrite,
    Point::Written,
    Point::FileSynced,
    Point::Renamed,
    Point::DestinationSynced,
    Point::SourceSynced,
    Point::PartRemoved,
    Point::MetaRemoved,
    Point::PartsRemoved,
    Point::JournalRemoved,
];

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rds-journal-fault-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("data.bin"), OLD).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn equal_content_does_not_share_parts_across_destinations() {
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let mut a = Journal::open(&root.0, "scope-a/file.bin", &manifest).unwrap();
    a.store(0, NEW).unwrap();
    drop(a);
    let b = Journal::open(&root.0, "scope-b/file.bin", &manifest).unwrap();
    assert_eq!(
        b.need(),
        [0],
        "another destination's bytes are not reusable"
    );
    assert!(b.assemble().is_err());
    assert!(!root.0.join("scope-b/file.bin").exists());
    let a = Journal::open(&root.0, "scope-a/file.bin", &manifest).unwrap();
    assert!(
        a.complete(),
        "the original destination must remain resumable"
    );
    a.assemble().unwrap();
    assert_eq!(std::fs::read(root.0.join("scope-a/file.bin")).unwrap(), NEW);
}

#[test]
fn verified_legacy_journal_resumes_only_its_bound_destination() {
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let state = root.0.join(STATE_DIR);
    plant_journal(
        &state,
        "data.bin",
        manifest.root,
        &[(&manifest.chunks[0].hash, NEW)],
    );
    let legacy = state.join(hex(&manifest.root));
    let original_meta = std::fs::read(legacy.join("meta")).unwrap();
    let b = Journal::open(&root.0, "other.bin", &manifest).unwrap();
    assert_eq!(b.need(), [0]);
    drop(b);
    assert_eq!(std::fs::read(legacy.join("meta")).unwrap(), original_meta);
    let a = Journal::open(&root.0, "./data.bin", &manifest).unwrap();
    assert!(a.complete());
    a.assemble().unwrap();
    assert!(
        !legacy.exists(),
        "cleanup must remove the selected legacy name"
    );
    assert_eq!(std::fs::read(root.0.join("data.bin")).unwrap(), NEW);
}

#[test]
fn unattributable_legacy_state_is_preserved_without_reuse() {
    for variant in [
        "missing",
        "torn",
        "wrong-path",
        "wrong-size",
        "wrong-root",
        "symlink",
    ] {
        let root = Scratch::new();
        let manifest = crate::manifest_of(NEW);
        let state = root.0.join(STATE_DIR);
        plant_journal(
            &state,
            "data.bin",
            manifest.root,
            &[(&manifest.chunks[0].hash, NEW)],
        );
        let legacy = state.join(hex(&manifest.root));
        match variant {
            "missing" => std::fs::remove_file(legacy.join("meta")).unwrap(),
            "torn" => std::fs::write(legacy.join("meta"), b"torn!").unwrap(),
            "symlink" => {
                std::fs::remove_file(legacy.join("meta")).unwrap();
                std::os::unix::fs::symlink(root.0.join("data.bin"), legacy.join("meta")).unwrap();
            }
            _ => {
                let dir = Directory::open_root(&legacy, false).unwrap();
                let mut meta = read_meta(&dir).unwrap().unwrap();
                match variant {
                    "wrong-path" => meta.rel_path = "other.bin".into(),
                    "wrong-size" => meta.size += 1,
                    "wrong-root" => meta.root[0] ^= 1,
                    _ => unreachable!(),
                }
                write_meta(&dir, &meta).unwrap();
            }
        }
        let opened = Journal::open(&root.0, "data.bin", &manifest).unwrap();
        assert_eq!(opened.need(), [0], "{variant}");
        assert!(
            legacy
                .join("parts")
                .join(hex(&manifest.chunks[0].hash))
                .exists(),
            "{variant}"
        );
    }
}

#[test]
fn scoped_state_recovers_torn_metadata_without_rebinding_to_another_path() {
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let mut a = Journal::open(&root.0, "data.bin", &manifest).unwrap();
    a.store(0, NEW).unwrap();
    drop(a);
    let own = root
        .0
        .join(STATE_DIR)
        .join(scoped_id(Path::new("data.bin"), &manifest.root));
    std::fs::write(own.join("meta"), b"torn!").unwrap();
    let b = Journal::open(&root.0, "other.bin", &manifest).unwrap();
    assert_eq!(b.need(), [0]);
    drop(b);
    assert_eq!(std::fs::read(own.join("meta")).unwrap(), b"torn!");
    let a = Journal::open(&root.0, "data.bin", &manifest).unwrap();
    assert!(a.complete());
    a.assemble().unwrap();
    assert!(!own.exists());
}

#[test]
fn scoped_collection_removes_only_the_same_destination() {
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    for rel in ["data.bin", "other.bin"] {
        let mut journal = Journal::open(&root.0, rel, &manifest).unwrap();
        journal.store(0, NEW).unwrap();
    }
    let state = root.0.join(STATE_DIR);
    let own = state.join(scoped_id(Path::new("data.bin"), &manifest.root));
    let other = state.join(scoped_id(Path::new("other.bin"), &manifest.root));
    drop(Journal::open(&root.0, "data.bin", &crate::manifest_of(b"next")).unwrap());
    assert!(!own.exists());
    assert!(other.exists());
    assert!(
        Journal::open(&root.0, "other.bin", &manifest)
            .unwrap()
            .complete()
    );
}

#[test]
fn canceled_journal_open_does_not_create_state_or_destination_parents() {
    let root = Scratch::new();
    let dest = root.0.join("not-created");
    let result =
        Journal::open_cancellable(&dest, "nested/data.bin", &crate::manifest_of(NEW), &|| true);
    assert!(matches!(result, Err(SyncError::Io(ref e)) if e.kind() == io::ErrorKind::Interrupted));
    assert!(
        !dest.exists(),
        "pre-canceled preparation mutated the filesystem"
    );
}

#[test]
fn canceling_journal_open_preserves_resumability_and_releases_receive_locks() {
    use std::cell::Cell;
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let state = root.0.join(STATE_DIR);
    // Use a different destination parent to exercise both held receive locks.
    std::fs::create_dir(root.0.join("nested")).unwrap();
    std::fs::write(root.0.join("nested/data.bin"), OLD).unwrap();
    plant_journal(
        &state,
        "nested/data.bin",
        manifest.root,
        &[(&manifest.chunks[0].hash, NEW)],
    );
    let observations = Cell::new(0);
    let result = Journal::open_cancellable(&root.0, "nested/data.bin", &manifest, &|| {
        observations.set(observations.get() + 1);
        observations.get() >= 4
    });
    assert!(matches!(result, Err(SyncError::Io(ref e)) if e.kind() == io::ErrorKind::Interrupted));
    assert_eq!(std::fs::read(root.0.join("nested/data.bin")).unwrap(), OLD);
    let resumed = Journal::open(&root.0, "nested/data.bin", &manifest).unwrap();
    assert!(
        resumed.complete(),
        "verified parts must survive canceled preparation"
    );
}

#[test]
fn canceled_collection_retains_metadata_and_unknown_entries_then_resumes() {
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let state_path = root.0.join(STATE_DIR);
    plant_journal(&state_path, "data.bin", manifest.root, &[]);
    let entry_path = state_path.join(hex(&manifest.root));
    let parts_path = entry_path.join("parts");
    for n in 0..32u8 {
        std::fs::write(parts_path.join(hex(&[n; 32])), [n]).unwrap();
    }
    let foreign = parts_path.join("not-owned");
    std::fs::write(&foreign, b"retain").unwrap();
    let state = Directory::open_root(&state_path, false).unwrap();
    let entry = state.child(hex(&manifest.root).as_ref(), false).unwrap();
    let stop = || std::fs::read_dir(&parts_path).unwrap().count() < 33;
    let result = collect_journal(
        &entry,
        &state,
        hex(&manifest.root).as_ref(),
        &stop,
        &mut CollectionBudget(MAX_COLLECTION_ENTRIES),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert!(
        entry_path.join("meta").exists(),
        "partial cleanup must retain attribution"
    );
    assert_eq!(std::fs::read_dir(&parts_path).unwrap().count(), 32);
    assert_eq!(std::fs::read(&foreign).unwrap(), b"retain");
    // Unknown residue refuses rmdir; it is never swept to finish cleanup.
    assert!(
        collect_journal(
            &entry,
            &state,
            hex(&manifest.root).as_ref(),
            &|| false,
            &mut CollectionBudget(MAX_COLLECTION_ENTRIES)
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(&parts_path).unwrap().count(), 1);
    assert!(entry_path.join("meta").exists());
}

#[test]
fn cleanup_work_limit_preserves_attribution_until_remaining_parts_are_removed() {
    let root = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let state_path = root.0.join(STATE_DIR);
    plant_journal(&state_path, "data.bin", manifest.root, &[]);
    let entry_path = state_path.join(hex(&manifest.root));
    let parts_path = entry_path.join("parts");
    for n in 0..32u8 {
        std::fs::write(parts_path.join(hex(&[n; 32])), [n]).unwrap();
    }
    let state = Directory::open_root(&state_path, false).unwrap();
    let entry = state.child(hex(&manifest.root).as_ref(), false).unwrap();
    let result = collect_journal(
        &entry,
        &state,
        hex(&manifest.root).as_ref(),
        &|| false,
        &mut CollectionBudget(8),
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
    assert_eq!(std::fs::read_dir(&parts_path).unwrap().count(), 24);
    assert!(entry_path.join("meta").exists());
    collect_journal(
        &entry,
        &state,
        hex(&manifest.root).as_ref(),
        &|| false,
        &mut CollectionBudget(32),
    )
    .unwrap();
    assert!(
        !entry_path.exists(),
        "a later bounded pass can finish known cleanup"
    );
}

#[test]
fn wide_foreign_catalog_does_not_block_new_journal_admission() {
    use std::cell::Cell;
    let root = Scratch::new();
    let state = root.0.join(STATE_DIR);
    std::fs::create_dir(&state).unwrap();
    for n in 0..MAX_COLLECTION_ENTRIES + 64 {
        std::fs::write(state.join(format!("foreign-{n}")), b"retain").unwrap();
    }
    let visited = Cell::new(0);
    let result = Journal::open_cancellable(&root.0, "data.bin", &crate::manifest_of(NEW), &|| {
        visited.set(visited.get() + 1);
        visited.get() > MAX_COLLECTION_ENTRIES + 32
    });
    assert!(
        result.is_ok(),
        "admission must defer the remaining foreign catalog"
    );
    assert_eq!(
        std::fs::read_dir(&state).unwrap().count(),
        MAX_COLLECTION_ENTRIES + 66
    );
    assert_eq!(std::fs::read(root.0.join("data.bin")).unwrap(), OLD);
}

#[test]
fn resumed_part_scan_can_stop_before_reading_the_next_chunk() {
    use std::cell::Cell;
    let root = Scratch::new();
    let data = vec![71u8; MAX_CHUNK as usize * 8];
    let manifest = crate::manifest_of(&data);
    assert!(manifest.chunks.len() > 2);
    let mut journal = Journal::open(&root.0, "data.bin", &manifest).unwrap();
    for (i, chunk) in manifest.chunks.iter().enumerate() {
        let offset = chunk.offset as usize;
        journal
            .store(i as u32, &data[offset..offset + chunk.len as usize])
            .unwrap();
    }
    let observed = Cell::new(0);
    let result = journal.rescan(&|| {
        observed.set(observed.get() + 1);
        observed.get() > 1
    });
    assert!(matches!(result, Err(SyncError::Io(ref e)) if e.kind() == io::ErrorKind::Interrupted));
    assert_eq!(journal.have.len(), 1);
    drop(journal);
    assert!(
        Journal::open(&root.0, "data.bin", &manifest)
            .unwrap()
            .complete()
    );
    assert_eq!(std::fs::read(root.0.join("data.bin")).unwrap(), OLD);
}

#[test]
fn destination_reuse_stops_after_one_verified_chunk_and_resumes() {
    let root = Scratch::new();
    let data = (0..MAX_CHUNK as usize * 8)
        .map(|i| 53 + (i / MAX_CHUNK as usize) as u8)
        .collect::<Vec<_>>();
    let manifest = crate::manifest_of(&data);
    let mut journal = Journal::open(&root.0, "data.bin", &manifest).unwrap();
    assert!(journal.have.is_empty());
    std::fs::write(root.0.join("data.bin"), &data).unwrap();
    let parts = root
        .0
        .join(STATE_DIR)
        .join(scoped_id(Path::new("data.bin"), &manifest.root))
        .join("parts");
    let result = journal
        .seed_from_destination(File::open(root.0.join("data.bin")).unwrap(), &|| {
            std::fs::read_dir(&parts).unwrap().next().is_some()
        });
    assert!(matches!(result, Err(SyncError::Io(ref e)) if e.kind() == io::ErrorKind::Interrupted));
    assert_eq!(std::fs::read_dir(&parts).unwrap().count(), 1);
    drop(journal);
    assert!(
        Journal::open(&root.0, "data.bin", &manifest)
            .unwrap()
            .complete()
    );
    assert_eq!(std::fs::read(root.0.join("data.bin")).unwrap(), data);
}

fn run(root: &Path, operation: &str, point: Point, action: Action) {
    let manifest = crate::manifest_of(NEW);
    let (armed, result) = match operation {
        "meta" => {
            let armed = arm(point, action);
            let result = Journal::open(root, "data.bin", &manifest).map(drop);
            (armed, result)
        }
        "part" => {
            let mut journal = Journal::open(root, "data.bin", &manifest).unwrap();
            let armed = arm(point, action);
            let result = journal.store(0, NEW).map(drop);
            (armed, result)
        }
        "assembly" => {
            let mut journal = Journal::open(root, "data.bin", &manifest).unwrap();
            journal.store(0, NEW).unwrap();
            let armed = arm(point, action);
            let result = journal.assemble().map(drop);
            (armed, result)
        }
        _ => panic!("unknown test operation"),
    };
    armed.assert_fired();
    let cleanup = matches!(
        point,
        Point::PartRemoved | Point::MetaRemoved | Point::PartsRemoved | Point::JournalRemoved
    );
    assert_eq!(result.is_ok(), cleanup, "{operation}/{point:?}: {result:?}");
}

fn verify_and_resume(root: &Path, operation: &str, point: Point) {
    let committed = operation == "assembly"
        && !matches!(
            point,
            Point::Created | Point::PartialWrite | Point::Written | Point::FileSynced
        );
    assert_eq!(
        std::fs::read(root.join("data.bin")).unwrap(),
        if committed { NEW } else { OLD }
    );
    let manifest = crate::manifest_of(NEW);
    let mut journal = Journal::open(root, "data.bin", &manifest).unwrap();
    let state = root.join(STATE_DIR);
    assert!(!state.join(ASSEMBLY).exists());
    let content = state.join(scoped_id(Path::new("data.bin"), &manifest.root));
    assert!(!content.join(PENDING).exists());
    assert!(!content.join("parts").join(PENDING).exists());
    if operation == "part" && matches!(point, Point::Renamed | Point::DestinationSynced) {
        assert!(
            journal.complete(),
            "published part must survive process exit"
        );
    }
    for index in journal.need() {
        journal.store(index, NEW).unwrap();
    }
    journal.assemble().unwrap();
    assert_eq!(std::fs::read(root.join("data.bin")).unwrap(), NEW);
    assert!(
        !content.exists(),
        "known completed journal must be collected"
    );
    assert_eq!(
        std::fs::read_dir(&state).unwrap().count(),
        1,
        "only persistent lock remains"
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        2,
        "no staging file may leak beside user data"
    );
    assert!(Journal::open(root, "data.bin", &manifest).is_ok());
}

#[test]
fn io_failures_at_each_commit_and_cleanup_boundary() {
    for errno in [rustix::io::Errno::NOSPC, rustix::io::Errno::ACCESS] {
        for (operation, points) in [
            ("meta", WRITE_POINTS),
            ("part", WRITE_POINTS),
            ("assembly", ASSEMBLY_POINTS),
        ] {
            for &point in points {
                let dir = Scratch::new();
                run(&dir.0, operation, point, Action::Error(errno));
                verify_and_resume(&dir.0, operation, point);
                println!("returned error {errno:?} {operation}/{point:?}: recovered");
            }
        }
    }
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("RDS_JOURNAL_FAULT_ROOT") else {
        return;
    };
    let operation = std::env::var("RDS_JOURNAL_FAULT_OPERATION").unwrap();
    let index: usize = std::env::var("RDS_JOURNAL_FAULT_POINT")
        .unwrap()
        .parse()
        .unwrap();
    let points = if operation == "assembly" {
        ASSEMBLY_POINTS
    } else {
        WRITE_POINTS
    };
    run(Path::new(&root), &operation, points[index], Action::Exit);
    panic!("process-exit fault was not reached");
}

#[test]
fn abrupt_exit_at_each_commit_and_cleanup_boundary() {
    for (operation, points) in [
        ("meta", WRITE_POINTS),
        ("part", WRITE_POINTS),
        ("assembly", ASSEMBLY_POINTS),
    ] {
        for (index, &point) in points.iter().enumerate() {
            let dir = Scratch::new();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "journal::tests::crash_child", "--nocapture"])
                .env("RDS_JOURNAL_FAULT_ROOT", &dir.0)
                .env("RDS_JOURNAL_FAULT_OPERATION", operation)
                .env("RDS_JOURNAL_FAULT_POINT", index.to_string())
                .output()
                .unwrap();
            assert_eq!(
                child.status.code(),
                Some(86),
                "{operation}/{point:?}: {} {}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            verify_and_resume(&dir.0, operation, point);
            println!("process exit {operation}/{point:?}: recovered");
        }
    }
}

#[test]
fn recovery_removes_only_reserved_regular_single_link_temporary_files() {
    use std::os::unix::fs::symlink;
    for variant in ["regular", "symlink", "hardlink", "directory"] {
        for name in ["assembly", "pending", "parts/pending"] {
            let dir = Scratch::new();
            let manifest = crate::manifest_of(NEW);
            drop(Journal::open(&dir.0, "data.bin", &manifest).unwrap());
            let state = dir.0.join(STATE_DIR);
            let content = state.join(scoped_id(Path::new("data.bin"), &manifest.root));
            let temp = if name == ASSEMBLY {
                state.join(name)
            } else {
                content.join(name)
            };
            let sentinel = dir.0.join("sentinel");
            std::fs::write(&sentinel, b"unrelated").unwrap();
            std::fs::write(content.join("unknown"), b"unrelated journal entry").unwrap();
            match variant {
                "regular" => std::fs::write(&temp, b"partial transaction").unwrap(),
                "symlink" => symlink(&sentinel, &temp).unwrap(),
                "hardlink" => std::fs::hard_link(&sentinel, &temp).unwrap(),
                "directory" => std::fs::create_dir(&temp).unwrap(),
                _ => unreachable!(),
            }
            let opened = Journal::open(&dir.0, "data.bin", &manifest);
            assert_eq!(opened.is_ok(), variant == "regular", "{variant}/{name}");
            if let Ok(mut journal) = opened {
                assert!(!temp.exists());
                journal.store(0, NEW).unwrap();
                journal.assemble().unwrap();
            }
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"unrelated");
            assert_eq!(
                std::fs::read(content.join("unknown")).unwrap(),
                b"unrelated journal entry"
            );
        }
    }
}

#[test]
fn canceled_assembly_never_installs_destination() {
    // W1.9: a stop raised between chunk writes abandons staging without
    // replacing the live destination; stored parts still complete the
    // journal, so a later un-canceled assembly installs the same content.
    let dir = Scratch::new();
    let manifest = crate::manifest_of(NEW);
    let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    journal.store(0, NEW).unwrap();
    let err = journal.assemble_cancellable(&|| true).unwrap_err();
    assert!(err.to_string().contains("canceled"), "{err}");
    assert_eq!(std::fs::read(dir.0.join("data.bin")).unwrap(), OLD);
    let journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
    assert!(journal.complete());
    journal.assemble().unwrap();
    assert_eq!(std::fs::read(dir.0.join("data.bin")).unwrap(), NEW);
}

#[test]
fn empty_and_final_chunk_cancellation_preserve_the_destination() {
    for bytes in [b"".as_slice(), NEW] {
        let dir = Scratch::new();
        let manifest = crate::manifest_of(bytes);
        let mut journal = Journal::open(&dir.0, "data.bin", &manifest).unwrap();
        for (index, chunk) in manifest.chunks.iter().enumerate() {
            let offset = chunk.offset as usize;
            journal
                .store(index as u32, &bytes[offset..offset + chunk.len as usize])
                .unwrap();
        }
        let checks = std::cell::Cell::new(0);
        let before_publish = manifest.chunks.len() + 1;
        let result = journal.assemble_cancellable(&|| {
            let previous = checks.get();
            checks.set(previous + 1);
            previous == before_publish
        });
        assert!(result.unwrap_err().to_string().contains("canceled"));
        assert_eq!(std::fs::read(dir.0.join("data.bin")).unwrap(), OLD);
        Journal::open(&dir.0, "data.bin", &manifest)
            .unwrap()
            .assemble()
            .unwrap();
        assert_eq!(std::fs::read(dir.0.join("data.bin")).unwrap(), bytes);
    }
}

/// Plant a well-formed journal directory without running `Journal::open`,
/// so several dead journals can coexist for the same destination.
fn plant_journal(state: &Path, rel: &str, root: ChunkHash, parts: &[(&ChunkHash, &[u8])]) {
    let dir = state.join(hex(&root));
    std::fs::create_dir_all(dir.join("parts")).unwrap();
    let meta = Meta {
        rel_path: rel.to_string(),
        size: parts.iter().map(|(_, d)| d.len() as u64).sum(),
        root,
    };
    let mut body = postcard::to_allocvec(&meta).unwrap();
    body.extend_from_slice(blake3::hash(&body).as_bytes());
    std::fs::write(dir.join("meta"), &body).unwrap();
    for (hash, data) in parts {
        std::fs::write(dir.join("parts").join(hex(hash)), data).unwrap();
    }
}

#[test]
fn superseded_journals_are_collected_and_foreign_entries_survive() {
    // W1.10: an offer carrying a different root makes the dead journal
    // for the same destination unreachable — a resume can only ever
    // open the offered root. Verified-meta superseded state is removed;
    // foreign, malformed, mislabeled and different-relation entries stay.
    const OTHER: &[u8] = b"other destination remains resumable";
    const NEXT: &[u8] = b"next verified destination content";
    let dir = Scratch::new();
    std::fs::write(dir.0.join("other.bin"), OTHER).unwrap();
    let state = dir.0.join(STATE_DIR);

    let ma = crate::manifest_of(NEW);
    plant_journal(&state, "data.bin", ma.root, &[(&ma.chunks[0].hash, NEW)]);
    let ma2 = crate::manifest_of(b"older superseded content");
    plant_journal(&state, "data.bin", ma2.root, &[]);
    let mo = crate::manifest_of(OTHER);
    plant_journal(&state, "other.bin", mo.root, &[(&mo.chunks[0].hash, OTHER)]);

    let state_a = state.join(hex(&ma.root));
    let state_a2 = state.join(hex(&ma2.root));
    let state_o = state.join(hex(&mo.root));
    std::fs::write(state_a.join("parts").join("foreign.bin"), b"foreign").unwrap();
    // Malformed, mislabeled and foreign siblings must never be swept.
    std::fs::create_dir(state.join("a".repeat(64))).unwrap();
    std::fs::create_dir(state.join("not-a-journal")).unwrap();
    let mislabeled = state.join("b".repeat(64));
    std::fs::create_dir(&mislabeled).unwrap();
    let mut body = postcard::to_allocvec(&Meta {
        rel_path: "data.bin".to_string(),
        size: 0,
        root: ma.root,
    })
    .unwrap();
    body.extend_from_slice(blake3::hash(&body).as_bytes());
    std::fs::write(mislabeled.join("meta"), &body).unwrap();

    let mb = crate::manifest_of(NEXT);
    drop(Journal::open(&dir.0, "data.bin", &mb).unwrap());

    assert!(
        state
            .join(scoped_id(Path::new("data.bin"), &mb.root))
            .exists(),
        "own journal must open"
    );
    assert!(!state_a2.exists(), "clean superseded journal collected");
    // Foreign residue keeps the directory; proven-owned names still go.
    assert!(!state_a.join("parts").join(hex(&ma.chunks[0].hash)).exists());
    assert!(state_a.join("parts").join("foreign.bin").exists());
    assert!(
        state_o.join("meta").exists(),
        "other destination stays resumable"
    );
    assert!(state.join("a".repeat(64)).exists());
    assert!(state.join("not-a-journal").exists());
    assert!(mislabeled.join("meta").exists(), "unattributable stays");
    // The surviving different-relation journal still resumes.
    let jo = Journal::open(&dir.0, "other.bin", &mo).unwrap();
    assert!(jo.complete());
}
