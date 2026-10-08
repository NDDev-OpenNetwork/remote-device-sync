//! Directory contract regressions independent of filesystem mutation services.

use rds_sync::{
    directory::{
        DeletePolicy, DirectoryEntry, DirectoryManifest, DirectorySnapshotAssembler,
        DirectorySnapshotHeader, DirectorySnapshotPart, EntryKind, MAX_DIRECTORY_PART_BYTES,
        ReconcileOperation, ReconcilePlan, ScanLimits, conflicts, scan_path, scan_path_with_limits,
    },
    journal::Journal,
    manifest_of,
};
use std::{fs, path::PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        // Keep Unix-domain fixture paths below macOS SUN_LEN even when the
        // user's TMPDIR is a long /var/folders path.
        let path =
            PathBuf::from("/tmp").join(format!("rds-dir-contract-{:032x}", rand::random::<u128>()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn file(path: &str, byte: u8) -> DirectoryEntry {
    DirectoryEntry::file(path, 1, *blake3::hash(&[byte]).as_bytes())
}
fn manifest(entries: Vec<DirectoryEntry>) -> DirectoryManifest {
    DirectoryManifest::from_entries(entries).unwrap()
}

#[test]
fn nested_journal_created_by_file_transfer_is_excluded_from_scans() {
    let scratch = Scratch::new();
    let bytes = b"committed user content";
    let data = manifest_of(bytes);
    let mut journal = Journal::open(&scratch.0, "nested/file", &data).unwrap();
    for index in journal.need() {
        let chunk = data.chunks[index as usize];
        journal
            .store(
                index,
                &bytes[chunk.offset as usize..chunk.offset as usize + chunk.len as usize],
            )
            .unwrap();
    }
    journal.assemble().unwrap();
    let scan = scan_path(&scratch.0).unwrap();
    assert_eq!(
        scan.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["nested", "nested/file"]
    );
    assert_eq!(scan.entries[1].content, Some(data.root));
    assert_eq!(scan.entries[1].size, data.size);
}

#[test]
fn scan_budgets_apply_across_directories_and_count_symlink_data() {
    let scratch = Scratch::new();
    fs::create_dir(scratch.0.join("a")).unwrap();
    fs::create_dir(scratch.0.join("b")).unwrap();
    fs::write(scratch.0.join("a/x"), b"x").unwrap();
    fs::write(scratch.0.join("b/x"), b"x").unwrap();
    let exact = ScanLimits {
        max_entries: 4,
        max_metadata_bytes: 8,
        max_depth: 2,
    };
    assert_eq!(
        scan_path_with_limits(&scratch.0, exact, &|| false)
            .unwrap()
            .entries
            .len(),
        4
    );
    for limits in [
        ScanLimits {
            max_entries: 3,
            ..exact
        },
        ScanLimits {
            max_metadata_bytes: 7,
            ..exact
        },
        ScanLimits {
            max_depth: 1,
            ..exact
        },
    ] {
        assert!(scan_path_with_limits(&scratch.0, limits, &|| false).is_err());
    }
    std::os::unix::fs::symlink("outside", scratch.0.join("link")).unwrap();
    let limits = ScanLimits {
        max_entries: 5,
        max_metadata_bytes: 19,
        ..exact
    };
    assert_eq!(
        scan_path_with_limits(&scratch.0, limits, &|| false)
            .unwrap()
            .entries
            .len(),
        5
    );
    assert!(
        scan_path_with_limits(
            &scratch.0,
            ScanLimits {
                max_metadata_bytes: 18,
                ..limits
            },
            &|| false
        )
        .is_err()
    );
}

#[test]
fn special_files_invalid_names_and_root_links_fail_without_blocking() {
    let scratch = Scratch::new();
    let socket = std::os::unix::net::UnixListener::bind(scratch.0.join("socket")).unwrap();
    assert!(scan_path(&scratch.0).is_err());
    drop(socket);
    fs::remove_file(scratch.0.join("socket")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(scratch.0.join("fifo"))
            .status()
            .unwrap()
            .success()
    );
    assert!(scan_path(&scratch.0).is_err());
    fs::remove_file(scratch.0.join("fifo")).unwrap();
    fs::write(scratch.0.join("noncanonical\\name"), b"x").unwrap();
    assert!(scan_path(&scratch.0).is_err());
    fs::remove_file(scratch.0.join("noncanonical\\name")).unwrap();
    // Linux filesystems support arbitrary filename bytes; APFS rejects this
    // fixture at creation. Invalid UTF-8 parsing is also tested without I/O.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStringExt;
        fs::write(
            scratch.0.join(std::ffi::OsString::from_vec(vec![0xff])),
            b"x",
        )
        .unwrap();
        assert!(scan_path(&scratch.0).is_err());
    }
    let link = Scratch::new();
    std::os::unix::fs::symlink(&scratch.0, link.0.join("root")).unwrap();
    assert!(scan_path(&link.0.join("root")).is_err());
}

#[test]
fn wire_byte_limit_splits_long_symlinks_and_borrowed_layout_matches_owned() {
    let entries = (0..35)
        .map(|index| {
            DirectoryEntry::symlink(
                format!("link-{index:03}-{}", "x".repeat(470)),
                vec![b'y'; 4096],
            )
        })
        .collect();
    let original = manifest(entries);
    let header = DirectorySnapshotHeader::from_manifest(&original).unwrap();
    let mut receiver = DirectorySnapshotAssembler::new(header).unwrap();
    let mut parts = 0;
    for part in original.snapshot_part_iter().unwrap() {
        let part = part.unwrap();
        let bytes = postcard::to_stdvec(&part).unwrap();
        assert_eq!(part.encoded_len().unwrap(), bytes.len());
        assert!(bytes.len() <= MAX_DIRECTORY_PART_BYTES);
        assert_eq!(bytes, postcard::to_stdvec(&part.to_owned()).unwrap());
        let decoded: DirectorySnapshotPart = postcard::from_bytes(&bytes).unwrap();
        receiver.push_part(decoded).unwrap();
        parts += 1;
    }
    assert!(
        parts > 3,
        "byte ceiling, rather than count, must split these entries"
    );
    assert_eq!(receiver.finish().unwrap(), original);
}

#[test]
fn rejected_wire_part_cannot_corrupt_accumulated_receiver_state() {
    let original = manifest(vec![file("a", 1), file("b", 2)]);
    let mut receiver =
        DirectorySnapshotAssembler::new(DirectorySnapshotHeader::from_manifest(&original).unwrap())
            .unwrap();
    receiver
        .push_part(DirectorySnapshotPart::new(vec![file("a", 1)]).unwrap())
        .unwrap();
    assert!(
        receiver
            .push_part(DirectorySnapshotPart::new(vec![file("very-long", 3)]).unwrap())
            .is_err()
    );
    receiver
        .push_part(DirectorySnapshotPart::new(vec![file("b", 2)]).unwrap())
        .unwrap();
    assert_eq!(receiver.finish().unwrap(), original);
}

#[test]
fn directory_delete_conflicts_with_remote_descendant_addition_in_both_directions() {
    let base = manifest(vec![DirectoryEntry::directory("dir")]);
    let deleted = manifest(Vec::new());
    let added = manifest(vec![DirectoryEntry::directory("dir"), file("dir/new", 8)]);
    for (local, remote) in [(&deleted, &added), (&added, &deleted)] {
        let records = conflicts(&base, local, remote).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.path.as_str())
                .collect::<Vec<_>>(),
            ["dir"]
        );
    }
    assert!(conflicts(&base, &added, &added).unwrap().is_empty());
}

#[test]
fn forged_plan_with_correct_revision_cannot_change_unrelated_content() {
    let destination = manifest(vec![file("a", 1)]);
    let source = manifest(vec![file("a", 2)]);
    let mut plan = ReconcilePlan::one_way(&source, &destination, DeletePolicy::Delete).unwrap();
    plan.check_inputs(&source, &destination).unwrap();
    assert_eq!(plan.project_destination(&destination).unwrap(), source);
    plan.operations = vec![ReconcileOperation::Put {
        entry: file("evil", 3),
    }];
    assert!(
        plan.verify().is_ok(),
        "structural validation alone is insufficient"
    );
    assert!(plan.check_inputs(&source, &destination).is_err());
    plan.operations = vec![ReconcileOperation::Replace {
        before: file("a", 9),
        after: file("a", 2),
    }];
    assert!(plan.verify().is_ok());
    assert!(plan.check_destination(&destination).is_err());
}

#[test]
fn keep_rename_retains_previous_path_and_deletes_are_children_first() {
    let source = manifest(vec![file("new", 1)]);
    let destination = manifest(vec![file("old", 1)]);
    let plan = ReconcilePlan::one_way(&source, &destination, DeletePolicy::Keep).unwrap();
    let projected = plan.project_destination(&destination).unwrap();
    assert_eq!(
        projected
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["new", "old"]
    );
    let nested = manifest(vec![
        DirectoryEntry::directory("a"),
        DirectoryEntry::directory("a/b"),
        file("a/b/c", 4),
    ]);
    let empty = manifest(Vec::new());
    let plan = ReconcilePlan::one_way(&empty, &nested, DeletePolicy::Delete).unwrap();
    assert_eq!(
        plan.operations
            .iter()
            .map(|op| match op {
                ReconcileOperation::Delete { tombstone } => tombstone.path.as_str(),
                _ => panic!("delete-only fixture"),
            })
            .collect::<Vec<_>>(),
        ["a/b/c", "a/b", "a"]
    );
    assert_eq!(plan.project_destination(&nested).unwrap(), empty);
    assert_eq!(nested.entries[0].kind, EntryKind::Directory);
}

#[test]
fn seeded_mirror_plans_project_to_exact_source_without_mutating_inputs() {
    for seed in 0..40u32 {
        let source = manifest(
            (0..96)
                .filter(|index| (index + seed) % 3 != 0)
                .map(|index| {
                    file(
                        &format!("file-{index:03}"),
                        ((index * 13 + seed) % 251) as u8,
                    )
                })
                .collect(),
        );
        let destination = manifest(
            (0..96)
                .filter(|index| (index + seed) % 5 != 0)
                .map(|index| {
                    file(
                        &format!("file-{index:03}"),
                        ((index * 7 + seed) % 251) as u8,
                    )
                })
                .collect(),
        );
        let plan = ReconcilePlan::one_way(&source, &destination, DeletePolicy::Delete).unwrap();
        plan.check_inputs(&source, &destination).unwrap();
        assert_eq!(
            plan.project_destination(&destination).unwrap(),
            source,
            "seed {seed}"
        );
    }
}

#[test]
fn scan_observes_content_mutation_after_the_file_was_opened() {
    // Cancellation callbacks are an injectable observation point. A one-file
    // scan invokes them at directory entry, enumeration, file entry and read.
    // Mutate at the first read, after the open-file metadata was captured.
    let scratch = Scratch::new();
    fs::write(scratch.0.join("file"), vec![b'x'; 1024]).unwrap();
    let calls = std::cell::Cell::new(0);
    let result = scan_path_with_limits(&scratch.0, ScanLimits::default(), &|| {
        let next = calls.get() + 1;
        calls.set(next);
        if next == 4 {
            fs::write(scratch.0.join("file"), b"changed").unwrap();
        }
        false
    });
    assert!(result.is_err());
    assert!(calls.get() >= 4, "fixture never reached the mutation point");
}
