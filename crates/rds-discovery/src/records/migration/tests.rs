use super::*;
use crate::records::{expiry_tests::record, tests::Temp};
use ed25519_dalek::SigningKey;
use std::{os::unix::fs::symlink, path::PathBuf, sync::Arc};

const PHASES: [&str; 12] = [
    "before-copy",
    "after-copy-chunk",
    "after-copy",
    "after-validation",
    "before-database",
    "after-database",
    "after-anchor",
    "before-receipt",
    "after-receipt",
    "after-marker",
    "after-rename",
    "after-parent-sync",
];
fn now() -> u64 {
    crate::now_unix().unwrap()
}
fn metadata(entries: &[(EndpointKey, Entry)]) -> V2Metadata {
    V2Metadata {
        format: 2,
        database: [24; 16],
        generation: 31,
        live: entries
            .iter()
            .filter(|(_, e)| matches!(e, Entry::Record(_)))
            .count() as u64,
        identities: entries.len() as u64,
    }
}
fn write_anchor(path: &Path, metadata: &V2Metadata) {
    let anchor = V2Anchor {
        metadata: metadata.clone(),
        digest: *blake3::hash(&postcard::to_stdvec(metadata).unwrap()).as_bytes(),
    };
    std::fs::write(
        path.join("records.anchor"),
        serde_json::to_vec(&anchor).unwrap(),
    )
    .unwrap();
}
fn fixture(path: &Path, entries: &[(EndpointKey, Entry)]) {
    let anchor = AtomicFile::open_named(path, "records.anchor", "records.lock").unwrap();
    let db = database(new_file(anchor.directory(), "records.redb").unwrap()).unwrap();
    let mut txn = db.begin_write().unwrap();
    txn.set_durability(Durability::Immediate).unwrap();
    {
        let mut records = txn.open_table(V2_RECORDS).unwrap();
        for (key, entry) in entries {
            records
                .insert(
                    key.0.as_slice(),
                    postcard::to_stdvec(entry).unwrap().as_slice(),
                )
                .unwrap();
        }
        txn.open_table(V2_META)
            .unwrap()
            .insert(
                0,
                postcard::to_stdvec(&metadata(entries)).unwrap().as_slice(),
            )
            .unwrap();
    }
    txn.commit().unwrap();
    drop(db);
    write_anchor(path, &metadata(entries));
    anchor.seal().unwrap();
}
fn entries() -> Vec<(EndpointKey, Entry)> {
    let a = record(101, 7, now());
    let b = DeleteRequest::new(&SigningKey::from_bytes(&[102; 32]), 19).unwrap();
    let c = record(103, u64::MAX, 1000); // authentic but long expired
    vec![
        (a.key, Entry::Record(a)),
        (b.key, Entry::Deleted(b)),
        (c.key, Entry::Record(c)),
    ]
}
fn snapshot(path: &Path) -> Vec<blake3::Hash> {
    ["records.redb", "records.anchor", "records.lock"]
        .iter()
        .map(|name| blake3::hash(&std::fs::read(path.join(name)).unwrap()))
        .collect()
}
fn audit_path(tmp: &Temp) -> PathBuf {
    std::fs::read_dir(&tmp.0)
        .unwrap()
        .map(Result::unwrap)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".rds-migration-")
        })
        .unwrap()
        .path()
}
fn assert_floors(path: &Path, entries: &[(EndpointKey, Entry)]) {
    let store = FileStore::new(path).unwrap();
    assert_eq!(store.len(), 0);
    for (key, entry) in entries {
        match entry {
            Entry::Record(_) => assert!(matches!(store.get(key), Err(DiscoveryError::Expired))),
            Entry::Deleted(_) => assert!(matches!(store.get(key), Err(DiscoveryError::NotFound))),
        }
        let inner = store.inner.lock().unwrap();
        let read = inner.db.begin_read().unwrap();
        let table = read.open_table(RECORDS).unwrap();
        let raw = table.get(key.0.as_slice()).unwrap().unwrap();
        let Stored::Retired {
            revision,
            digest,
            deleted,
        } = decode(raw.value(), key).unwrap()
        else {
            panic!("rearmed lease")
        };
        assert_eq!(revision, entry.details(key).unwrap().0);
        assert_eq!(digest, entry.digest().unwrap());
        assert_eq!(deleted, matches!(entry, Entry::Deleted(_)));
    }
}

#[test]
fn records_deletions_expired_and_maximum_revisions_survive_without_rearming() {
    let tmp = Temp::new();
    let src = tmp.0.join("old");
    let dst = tmp.0.join("new");
    let entries = entries();
    fixture(&src, &entries);
    let before = snapshot(&src);
    let receipt = migrate_v2(&src, &dst).unwrap();
    assert_eq!(snapshot(&src), before);
    assert_eq!(receipt.imported_identities, 3);
    assert_eq!(receipt.imported_records, 2);
    assert_eq!(receipt.imported_deletions, 1);
    assert_eq!(receipt.source_database_digest, *before[0].as_bytes());
    assert!(
        tmp.0
            .join(&receipt.audit_directory)
            .join("receipt.json")
            .is_file()
    );
    assert_floors(&dst, &entries);
    let store = FileStore::new(&dst).unwrap();
    let Entry::Record(first) = &entries[0].1 else {
        unreachable!()
    };
    assert!(matches!(store.put(first), Err(DiscoveryError::Expired)));
    assert!(matches!(
        store.put(&record(101, 6, now())),
        Err(DiscoveryError::Stale)
    ));
    let next = record(101, 8, now());
    store.put(&next).unwrap();
    store.put(&next).unwrap();
    assert_eq!(store.get(&next.key).unwrap(), next);
    let Entry::Deleted(tomb) = &entries[1].1 else {
        unreachable!()
    };
    assert!(matches!(store.remove(tomb), Err(DiscoveryError::Expired)));
    assert!(matches!(
        store.put(&record(102, 18, now())),
        Err(DiscoveryError::Stale)
    ));
    store.put(&record(102, 20, now())).unwrap();
    drop(store);
    assert!(migrate_v2(&src, &dst).is_err());
    assert_eq!(snapshot(&src), before);
    assert_eq!(FileStore::new(&dst).unwrap().get(&next.key).unwrap(), next);
}

#[test]
fn initialized_empty_catalog_is_supported_but_missing_source_is_not_created() {
    let tmp = Temp::new();
    let src = tmp.0.join("old");
    let dst = tmp.0.join("new");
    assert!(migrate_v2(&src, &dst).is_err());
    assert!(!src.exists());
    fixture(&src, &[]);
    assert_eq!(migrate_v2(&src, &dst).unwrap().imported_identities, 0);
    assert!(FileStore::new(&dst).unwrap().is_empty());
}

#[test]
fn one_ahead_is_preserved_but_rollback_gaps_wrong_identity_and_format_are_refused() {
    for kind in [
        "one-ahead",
        "behind",
        "gap",
        "identity",
        "counts",
        "format1",
        "format3",
        "zero",
    ] {
        let tmp = Temp::new();
        let src = tmp.0.join("old");
        let dst = tmp.0.join("new");
        let rows = entries();
        fixture(&src, &rows);
        let mut m = metadata(&rows);
        match kind {
            "one-ahead" => m.generation -= 1,
            "behind" => m.generation += 1,
            "gap" => m.generation -= 2,
            "identity" => m.database = [25; 16],
            "counts" => m.live -= 1,
            "format1" => m.format = 1,
            "format3" => m.format = 3,
            "zero" => m.generation = 0,
            _ => unreachable!(),
        }
        write_anchor(&src, &m);
        let before = snapshot(&src);
        let result = migrate_v2(&src, &dst);
        assert_eq!(result.is_ok(), kind == "one-ahead", "{kind}: {result:?}");
        assert_eq!(snapshot(&src), before, "{kind}");
        if kind == "one-ahead" {
            assert_floors(&dst, &rows);
        } else {
            assert!(!dst.exists());
        }
    }
}

#[test]
fn malformed_catalog_and_anchor_are_refused_without_changing_source() {
    for kind in [
        "signature",
        "key",
        "trailing",
        "cell",
        "unknown-table",
        "multimap",
        "metadata-extra",
        "metadata-trailing",
        "identity-count",
        "live-count",
        "checksum",
        "oversized-file",
        "garbage-file",
        "unknown-file",
    ] {
        let tmp = Temp::new();
        let src = tmp.0.join("old");
        let dst = tmp.0.join("new");
        let rows = entries();
        fixture(&src, &rows);
        let db = Database::open(src.join("records.redb")).unwrap(); // synthetic fixture writer only
        let txn = db.begin_write().unwrap();
        match kind {
            "signature" | "key" | "trailing" | "cell" => {
                let mut entry = rows[0].1.clone();
                if kind == "signature" {
                    let Entry::Record(ref mut r) = entry else {
                        unreachable!()
                    };
                    r.signature[0] ^= 1;
                }
                if kind == "key" {
                    entry = rows[2].1.clone();
                }
                let mut bytes = postcard::to_stdvec(&entry).unwrap();
                if kind == "trailing" {
                    bytes.push(0);
                }
                if kind == "cell" {
                    bytes.resize(MAX_CELL + 1, 0);
                }
                txn.open_table(V2_RECORDS)
                    .unwrap()
                    .insert(rows[0].0.0.as_slice(), bytes.as_slice())
                    .unwrap();
            }
            "unknown-table" => {
                txn.open_table(TableDefinition::<u8, u8>::new("other"))
                    .unwrap();
            }
            "multimap" => {
                txn.open_multimap_table(redb::MultimapTableDefinition::<u8, u8>::new("other"))
                    .unwrap();
            }
            "metadata-extra" => {
                txn.open_table(V2_META)
                    .unwrap()
                    .insert(1, &[0][..])
                    .unwrap();
            }
            "metadata-trailing" | "identity-count" | "live-count" => {
                let mut m = metadata(&rows);
                if kind == "identity-count" {
                    m.identities += 1;
                }
                if kind == "live-count" {
                    m.live -= 1;
                }
                let mut bytes = postcard::to_stdvec(&m).unwrap();
                if kind == "metadata-trailing" {
                    bytes.push(0);
                }
                txn.open_table(V2_META)
                    .unwrap()
                    .insert(0, bytes.as_slice())
                    .unwrap();
                write_anchor(&src, &m);
            }
            _ => {}
        }
        txn.commit().unwrap();
        drop(db);
        match kind {
            "checksum" => {
                std::fs::write(src.join("records.anchor"), b"{}").unwrap();
            }
            "oversized-file" => {
                File::options()
                    .write(true)
                    .open(src.join("records.redb"))
                    .unwrap()
                    .set_len(MAX_DATABASE + 1)
                    .unwrap();
            }
            "garbage-file" => {
                std::fs::write(src.join("records.redb"), [42; 512]).unwrap();
            }
            "unknown-file" => {
                std::fs::write(src.join("legacy.json"), b"{}").unwrap();
            }
            _ => {}
        }
        let before = snapshot(&src);
        assert!(migrate_v2(&src, &dst).is_err(), "{kind}");
        assert_eq!(snapshot(&src), before, "{kind}");
        assert!(!dst.exists());
    }
}

#[test]
fn ownership_aliases_and_final_symlinks_are_refused() {
    let tmp = Temp::new();
    let src = tmp.0.join("old");
    let dst = tmp.0.join("new");
    fixture(&src, &entries());
    let before = snapshot(&src);
    let owner = AtomicFile::open_named(&src, "records.anchor", "records.lock").unwrap();
    assert!(matches!(migrate_v2(&src, &dst), Err(DiscoveryError::Busy)));
    drop(owner);
    let db_owner = File::open(src.join("records.redb")).unwrap();
    db_owner.try_lock().unwrap();
    assert!(migrate_v2(&src, &dst).is_err());
    drop(db_owner);
    assert!(migrate_v2(&src, &src).is_err());
    assert!(migrate_v2(&src, &src.join("nested")).is_err());
    assert!(migrate_v2(&src.join("."), &dst).is_err());
    assert!(migrate_v2(&src, &dst.join(".")).is_err());
    symlink(&src, &dst).unwrap();
    assert!(migrate_v2(&src, &dst).is_err());
    assert!(migrate_v2(&dst, &tmp.0.join("other")).is_err());
    std::fs::remove_file(&dst).unwrap();
    for name in ["records.redb", "records.anchor", "records.lock"] {
        let original = src.join(name);
        let held = tmp.0.join("held");
        std::fs::rename(&original, &held).unwrap();
        symlink(&held, &original).unwrap();
        assert!(migrate_v2(&src, &dst).is_err(), "{name}");
        std::fs::remove_file(&original).unwrap();
        std::fs::hard_link(&held, &original).unwrap();
        assert!(migrate_v2(&src, &dst).is_err(), "hardlink {name}");
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&held, &original).unwrap();
    }
    assert_eq!(snapshot(&src), before);
    assert!(!dst.exists());
}

#[test]
fn destination_created_during_validation_is_never_overwritten() {
    let tmp = Temp::new();
    let src = tmp.0.join("old");
    let dst = tmp.0.join("new");
    let rows = entries();
    fixture(&src, &rows);
    let before = snapshot(&src);
    assert!(
        migrate(&src, &dst, &mut |phase| {
            if phase == "after-marker" {
                std::fs::create_dir(&dst).unwrap();
            }
            Ok(())
        })
        .is_err()
    );
    assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
    assert_eq!(snapshot(&src), before);
    assert_floors(&audit_path(&tmp).join("staged"), &rows);
}

fn verify_interruption(
    tmp: &Temp,
    phase: &str,
    before: &[blake3::Hash],
    rows: &[(EndpointKey, Entry)],
) {
    assert_eq!(snapshot(&tmp.0.join("old")), before, "{phase}");
    let destination = tmp.0.join("new");
    if matches!(phase, "after-rename" | "after-parent-sync") {
        assert_floors(&destination, rows);
        assert!(migrate_v2(&tmp.0.join("old"), &destination).is_err());
    } else {
        assert!(!destination.exists(), "{phase}");
        let stage = audit_path(tmp).join("staged");
        if phase == "after-marker" {
            assert_floors(&stage, rows);
        } else {
            assert!(FileStore::new(stage).is_err(), "{phase} exposed work state");
        }
    }
}
#[test]
fn failures_never_publish_an_empty_or_partial_replacement() {
    for phase in PHASES {
        let tmp = Temp::new();
        let rows = entries();
        fixture(&tmp.0.join("old"), &rows);
        let before = snapshot(&tmp.0.join("old"));
        assert!(
            migrate(&tmp.0.join("old"), &tmp.0.join("new"), &mut |at| {
                if at == phase {
                    Err(error("injected migration failure"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        verify_interruption(&tmp, phase, &before, &rows);
    }
}
#[test]
fn crash_importer() {
    let Ok(path) = std::env::var("RDS_TEST_MIGRATION_PATH") else {
        return;
    };
    let phase = std::env::var("RDS_TEST_MIGRATION_PHASE").unwrap();
    let path = PathBuf::from(path);
    migrate(&path.join("old"), &path.join("new"), &mut |at| {
        if at == phase {
            std::process::exit(86);
        }
        Ok(())
    })
    .unwrap();
    panic!("migration crash checkpoint was not reached");
}
#[test]
fn abrupt_process_exit_keeps_source_and_only_exposes_a_complete_catalog() {
    for phase in PHASES {
        let tmp = Temp::new();
        let rows = entries();
        fixture(&tmp.0.join("old"), &rows);
        let before = snapshot(&tmp.0.join("old"));
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "records::migration::tests::crash_importer"])
            .env("RDS_TEST_MIGRATION_PATH", &tmp.0)
            .env("RDS_TEST_MIGRATION_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(86),
            "{phase}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        verify_interruption(&tmp, phase, &before, &rows);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrated_http_publisher_commits_successor_retries_lost_reply_and_restarts() {
    use crate::{
        client::Client,
        publisher::{RecordDraft, RecordIssuer},
        service::{self, ServiceConfig},
    };
    let tmp = Temp::new();
    let src = tmp.0.join("old");
    let dst = tmp.0.join("new");
    let key = SigningKey::from_bytes(&[111; 32]);
    let mut issuer = RecordIssuer::open(&tmp.0.join("publisher"), key.clone(), now()).unwrap();
    let draft = || RecordDraft {
        addrs: vec!["127.0.0.1:4000".parse().unwrap()],
        relay_urls: vec![],
        services: vec![crate::Service::Ping],
        ttl: Duration::from_secs(300),
    };
    let old = issuer.record(draft(), now()).unwrap();
    fixture(&src, &[(old.key, Entry::Record(old.clone()))]);
    migrate_v2(&src, &dst).unwrap();
    let store = Arc::new(FileStore::new(&dst).unwrap());
    let directory = service::serve(
        "127.0.0.1:0".parse().unwrap(),
        store.clone(),
        ServiceConfig::open_ephemeral(),
    )
    .await
    .unwrap();
    let client = Client::new(directory.addr());
    assert!(matches!(
        client.publish(&old).await,
        Err(DiscoveryError::Http { status: 410, .. })
    ));
    let next = issuer.renew_record(draft(), now()).unwrap();
    assert_eq!(next.verify().unwrap().revision, 2);
    let mut lost_reply = tokio::net::TcpStream::connect(directory.addr())
        .await
        .unwrap();
    crate::http::write_request_with_host(
        &mut lost_reply,
        &directory.addr().to_string(),
        "PUT",
        "/v1/records",
        &serde_json::to_vec(&next).unwrap(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.get(&next.key).ok().as_ref() != Some(&next) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(lost_reply); // commit observed independently; never read the HTTP reply
    drop(issuer);
    let mut issuer = RecordIssuer::open(&tmp.0.join("publisher"), key, now()).unwrap();
    let retry = issuer.record(draft(), now()).unwrap();
    assert_eq!(retry, next);
    client.publish(&retry).await.unwrap();
    assert_eq!(client.fetch(&retry.key).await.unwrap(), retry);
    drop(directory);
    // Drop aborts directory children; their owned blocking jobs may finish
    // after cancellation. Wait for ownership teardown, not a fixed sleep.
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&store) != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(store);
    assert_eq!(
        FileStore::new(&dst).unwrap().get(&retry.key).unwrap(),
        retry
    );
}

#[test]
#[ignore = "explicit 4096-identity signed migration capacity qualification"]
fn full_capacity_migration_preserves_every_floor() {
    let tmp = Temp::new();
    let src = tmp.0.join("old");
    let dst = tmp.0.join("new");
    let mut rows = Vec::with_capacity(MAX_IDENTITIES);
    for index in 0..MAX_IDENTITIES {
        let mut seed = [0x51; 32];
        seed[..8].copy_from_slice(&(index as u64).to_le_bytes());
        let signer = SigningKey::from_bytes(&seed);
        let record = EndpointRecord::publish(
            &signer,
            index as u64 + 1,
            (1..=32)
                .map(|host| format!("[2001:db8::{host:x}]:4000").parse().unwrap())
                .collect(),
            (0..8)
                .map(|n| {
                    let host = format!("https://relay-{n}.invalid:");
                    format!(
                        "{host}{}443",
                        "0".repeat(crate::MAX_RELAY_URL_BYTES - host.len() - 3)
                    )
                })
                .collect(),
            vec![crate::Service::Ping],
            Duration::from_secs(3600),
        )
        .unwrap();
        rows.push((record.key, Entry::Record(record)));
    }
    fixture(&src, &rows);
    let before = snapshot(&src);
    let started = std::time::Instant::now();
    let receipt = migrate_v2(&src, &dst).unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    assert_eq!(receipt.imported_identities, MAX_IDENTITIES as u64);
    assert_eq!(snapshot(&src), before);
    assert_floors(&dst, &rows);
    let store = FileStore::new(&dst).unwrap();
    assert!(store.put(&record(230, 1, now())).is_err());
    drop(store);
    println!(
        "{}",
        serde_json::json!({
            "scenario": "format2-full-capacity-migration", "identities": MAX_IDENTITIES,
            "source_database_bytes": receipt.source_database_bytes,
            "destination_database_bytes": std::fs::metadata(dst.join("records.redb")).unwrap().len(),
            "migration_seconds": elapsed, "profile": "default cargo test dev (unoptimized)",
            "exact_floor_and_source_bytes_checks": "passed"
        })
    );
}
