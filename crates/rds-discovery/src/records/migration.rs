//! Offline v2 import. The source is never opened by the database engine.
//! All intermediate state stays in a private sibling audit directory; only a
//! verified complete catalog can be renamed into the requested destination.
use super::*;
use redb::{ReadableTableMetadata, TableHandle};
use rustix::fs::{AtFlags, RenameFlags, mkdirat, renameat_with, statat, unlinkat};
use std::{
    ffi::OsStr,
    io::Write,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
};

const V2_RECORDS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("records-v2");
const V2_META: TableDefinition<'_, u8, &[u8]> = TableDefinition::new("metadata-v2");

// Exact historical 0dea767 schema. Never reinterpret the v1 timestamp counter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Metadata {
    format: u16,
    database: [u8; 16],
    generation: u64,
    live: u64,
    identities: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Anchor {
    metadata: V2Metadata,
    digest: [u8; 32],
}

/// Local integrity receipt, not a signed GDS inventory or deployment approval.
/// The durable JSON receipt describes preparation; success from `migrate_v2`
/// additionally means the destination rename and both parent syncs completed.
#[derive(Debug, Serialize, Deserialize)]
pub struct MigrationReceipt {
    pub receipt_version: u16,
    pub source_format: u16,
    pub destination_format: u16,
    pub source_database: [u8; 16],
    pub source_generation: u64,
    pub source_anchor_generation: u64,
    pub source_database_bytes: u64,
    pub source_database_digest: [u8; 32],
    pub source_anchor_digest: [u8; 32],
    pub destination_database: [u8; 16],
    pub destination_generation: u64,
    pub imported_identities: u64,
    pub imported_records: u64,
    pub imported_deletions: u64,
    /// Basename relative to the source/destination's shared parent.
    pub audit_directory: String,
    pub destination_directory: String,
}

/// Convert an initialized format-2 catalog into a NEW sibling directory.
/// Requires stopped source ownership. Source and destination must have the same
/// existing trusted parent; symlinks at either final component are refused.
/// Every imported envelope becomes a retired revision floor, never a new lease.
///
/// Runs blocking I/O. On any error, retain all artifacts for inspection; never
/// delete an existing destination or blindly retry a possibly completed rename.
pub fn migrate_v2(source: &Path, destination: &Path) -> Result<MigrationReceipt, DiscoveryError> {
    migrate(source, destination, &mut |_| Ok(()))
}

fn directory(parent: &File, name: &OsStr) -> Result<File, DiscoveryError> {
    Ok(File::from(
        openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?,
    ))
}
fn parent(path: &Path) -> Result<(File, &OsStr), DiscoveryError> {
    // Path::file_name normalizes trailing `/.`; accepting that spelling here
    // could select a different sibling from the one the operator requested.
    if matches!(
        path.as_os_str().as_bytes().rsplit(|b| *b == b'/').next(),
        None | Some(b"" | b"." | b"..")
    ) {
        return Err(error(
            "migration path must end in a literal directory basename",
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| error("expected a directory basename"))?;
    let path = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir = File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?,
    );
    Ok((dir, name))
}
fn same_inode(a: &File, b: &File) -> Result<bool, DiscoveryError> {
    let a = a.metadata().map_err(error)?;
    let b = b.metadata().map_err(error)?;
    Ok((a.dev(), a.ino()) == (b.dev(), b.ino()))
}
fn new_file(dir: &File, name: &str) -> Result<File, DiscoveryError> {
    Ok(File::from(
        openat(
            dir,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(error)?,
    ))
}
fn backend(file: File) -> BoundedFile {
    BoundedFile {
        file,
        #[cfg(test)]
        fault: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
    }
}
fn database(file: File) -> Result<Database, DiscoveryError> {
    file.try_lock().map_err(error)?;
    Database::builder()
        .set_cache_size(16 * 1024 * 1024)
        .create_with_backend(backend(file))
        .map_err(error)
}

fn migrate(
    source: &Path,
    destination: &Path,
    checkpoint: &mut dyn FnMut(&str) -> Result<(), DiscoveryError>,
) -> Result<MigrationReceipt, DiscoveryError> {
    let (parent, source_name) = parent(source)?;
    let (target_parent, target_name) = self::parent(destination)?;
    if !same_inode(&parent, &target_parent)? || source_name == target_name {
        return Err(error("migration requires distinct sibling directories"));
    }
    let target_name = target_name
        .to_str()
        .ok_or_else(|| error("destination basename must be UTF-8"))?;
    match statat(&parent, target_name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => {}
        Ok(_) => return Err(error("migration destination already exists")),
        Err(e) => return Err(error(e)),
    }
    let source_dir = directory(&parent, source_name)?;
    if source_dir.metadata().map_err(error)?.mode() & 0o7777 != 0o700 {
        return Err(error("migration source must already have mode 0700"));
    }
    let source = AtomicFile::from_directory(source_dir, "records.anchor", "records.lock", false)?;
    if !source.initialized()? {
        return Err(error("migration source is not initialized"));
    }
    for (index, entry) in rustix::fs::Dir::read_from(source.directory())
        .map_err(error)?
        .enumerate()
    {
        let entry = entry.map_err(error)?;
        let name = entry.file_name().to_bytes();
        if index > 128
            || (!matches!(
                name,
                b"." | b".." | b"records.lock" | b"records.anchor" | b"records.redb"
            ) && !(name.starts_with(b".policy-") && name.ends_with(b".tmp")))
        {
            return Err(error("unknown or excessive migration source entries"));
        }
    }
    let anchor_bytes = source
        .read()?
        .ok_or_else(|| error("migration source anchor missing"))?;
    let anchored: V2Anchor = serde_json::from_slice(&anchor_bytes).map_err(error)?;
    validate_metadata(&anchored.metadata)?;
    if anchored.digest
        != *blake3::hash(&postcard::to_stdvec(&anchored.metadata).map_err(error)?).as_bytes()
    {
        return Err(error("migration source anchor checksum mismatch"));
    }
    let source_file = File::from(
        openat(
            source.directory(),
            "records.redb",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(error)?,
    );
    regular(&source_file)?;
    let source_len = source_file.metadata().map_err(error)?.len();
    if source_len == 0 || source_len > MAX_DATABASE {
        return Err(error("migration source database empty or oversized"));
    }
    source_file.try_lock().map_err(error)?;
    let source_file = backend(source_file); // owned unlock, never given to redb

    let audit_name = format!(".rds-migration-{:032x}", rand::random::<u128>());
    mkdirat(&parent, audit_name.as_str(), Mode::RWXU).map_err(error)?;
    parent.sync_all().map_err(error)?;
    let audit = AtomicFile::from_directory(
        directory(&parent, OsStr::new(&audit_name))?,
        "receipt.json",
        "migration.lock",
        true,
    )?;
    let mut intent = new_file(audit.directory(), "intent.json")?;
    intent
        .write_all(
            &serde_json::to_vec_pretty(&serde_json::json!({
                "intent_version": 1,
                "source_directory_bytes": source_name.as_bytes(),
                "destination_directory": target_name,
                "source_database": anchored.metadata.database,
                "source_anchor_generation": anchored.metadata.generation,
                "source_database_bytes": source_len,
                "source_anchor_digest": blake3::hash(&anchor_bytes).as_bytes(),
            }))
            .map_err(error)?,
        )
        .map_err(error)?;
    intent.sync_all().map_err(error)?;
    audit.directory().sync_all().map_err(error)?;
    // A pending marker protects even an accidental attempt to open work state.
    mkdirat(audit.directory(), "staged", Mode::RWXU).map_err(error)?;
    let stage_dir = directory(audit.directory(), OsStr::new("staged"))?;
    let mut pending = new_file(&stage_dir, "migration.pending")?;
    pending
        .write_all(b"rds-record-migration/v1\n")
        .map_err(error)?;
    pending.sync_all().map_err(error)?;
    stage_dir.sync_all().map_err(error)?;
    audit.directory().sync_all().map_err(error)?;
    let stage = AtomicFile::from_directory(stage_dir, "records.anchor", "records.lock", true)?;
    checkpoint("before-copy")?;
    let copy = new_file(audit.directory(), "source.redb")?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut offset = 0;
    while offset < source_len {
        let size = (source_len - offset).min(buffer.len() as u64) as usize;
        source_file
            .file
            .read_exact_at(&mut buffer[..size], offset)
            .map_err(error)?;
        hasher.update(&buffer[..size]);
        copy.write_all_at(&buffer[..size], offset).map_err(error)?;
        offset += size as u64;
        checkpoint("after-copy-chunk")?;
    }
    copy.sync_all().map_err(error)?;
    audit.directory().sync_all().map_err(error)?;
    checkpoint("after-copy")?;
    let copied = database(copy)?; // recovery writes ONLY to the copy
    let (metadata, rows) = read_v2(&copied, &anchored.metadata)?;
    drop(copied);
    checkpoint("after-validation")?;
    let next = Metadata {
        format: 3,
        database: rand::random(),
        generation: 1,
        live: 0,
        identities: metadata.identities,
        wall_floor: Duration::ZERO,
    };
    let db = database(new_file(stage.directory(), "records.redb")?)?;
    let mut txn = db.begin_write().map_err(error)?;
    txn.set_durability(Durability::Immediate).map_err(error)?;
    {
        let mut table = txn.open_table(RECORDS).map_err(error)?;
        for (key, row) in rows {
            table
                .insert(
                    key.0.as_slice(),
                    postcard::to_stdvec(&row).map_err(error)?.as_slice(),
                )
                .map_err(error)?;
        }
        txn.open_table(META)
            .map_err(error)?
            .insert(0, postcard::to_stdvec(&next).map_err(error)?.as_slice())
            .map_err(error)?;
    }
    checkpoint("before-database")?;
    txn.commit().map_err(error)?;
    checkpoint("after-database")?;
    drop(db);
    stage.write(&encode_anchor(&next)?)?;
    stage.seal()?;
    checkpoint("after-anchor")?;
    // Reopen through the same protected owner; only this internal call can
    // tolerate migration.pending. Normal FileStore startup still refuses it.
    let validated = FileStore::open_owned(stage, true)?;
    let receipt = MigrationReceipt {
        receipt_version: 1,
        source_format: 2,
        destination_format: 3,
        source_database: metadata.database,
        source_generation: metadata.generation,
        source_anchor_generation: anchored.metadata.generation,
        source_database_bytes: source_len,
        source_database_digest: *hasher.finalize().as_bytes(),
        source_anchor_digest: *blake3::hash(&anchor_bytes).as_bytes(),
        destination_database: next.database,
        destination_generation: next.generation,
        imported_identities: metadata.identities,
        imported_records: metadata.live,
        imported_deletions: metadata.identities - metadata.live,
        audit_directory: audit_name,
        destination_directory: target_name.to_owned(),
    };
    checkpoint("before-receipt")?;
    audit.write(&serde_json::to_vec_pretty(&receipt).map_err(error)?)?;
    audit.seal()?;
    checkpoint("after-receipt")?;
    let inner = validated.inner.lock().map_err(error)?;
    unlinkat(
        inner.anchor.directory(),
        "migration.pending",
        AtFlags::empty(),
    )
    .map_err(error)?;
    inner.anchor.directory().sync_all().map_err(error)?;
    checkpoint("after-marker")?;
    // Never overwrite even an empty destination created during verification.
    renameat_with(
        audit.directory(),
        "staged",
        &parent,
        target_name,
        RenameFlags::NOREPLACE,
    )
    .map_err(error)?;
    checkpoint("after-rename")?;
    audit.directory().sync_all().map_err(error)?;
    parent.sync_all().map_err(error)?;
    checkpoint("after-parent-sync")?;
    // Keep both source and destination ownership through the last durable step.
    drop(inner);
    drop(validated);
    Ok(receipt)
}

fn validate_metadata(m: &V2Metadata) -> Result<(), DiscoveryError> {
    if m.format != 2
        || m.generation == 0
        || m.identities > MAX_IDENTITIES as u64
        || m.live > m.identities
    {
        return Err(error("invalid or unsupported migration metadata"));
    }
    Ok(())
}
fn read_v2(
    db: &Database,
    anchored: &V2Metadata,
) -> Result<(V2Metadata, Vec<(EndpointKey, Stored)>), DiscoveryError> {
    let read = db.begin_read().map_err(error)?;
    let mut tables = 0;
    for table in read.list_tables().map_err(error)? {
        tables += 1;
        if tables > 2 || !matches!(table.name(), "records-v2" | "metadata-v2") {
            return Err(error("unknown migration table"));
        }
    }
    if tables != 2 || read.list_multimap_tables().map_err(error)?.next().is_some() {
        return Err(error("unexpected migration tables"));
    }
    let meta = read.open_table(V2_META).map_err(error)?;
    if meta.len().map_err(error)? != 1 {
        return Err(error("unexpected migration metadata entries"));
    }
    let value = meta
        .get(0)
        .map_err(error)?
        .ok_or_else(|| error("migration metadata missing"))?;
    if value.value().len() > MAX_CELL {
        return Err(error("migration metadata oversized"));
    }
    let metadata: V2Metadata = exact(value.value())?;
    validate_metadata(&metadata)?;
    if metadata.database != anchored.database
        || metadata.generation < anchored.generation
        || metadata.generation > anchored.generation.saturating_add(1)
        || (metadata.generation == anchored.generation && metadata != *anchored)
    {
        return Err(error(
            "migration database rolled back or disagrees with anchor",
        ));
    }
    let table = read.open_table(V2_RECORDS).map_err(error)?;
    if table.len().map_err(error)? != metadata.identities {
        return Err(error("migration identity count mismatch"));
    }
    let mut rows = Vec::with_capacity(metadata.identities as usize);
    let mut live = 0;
    for item in table.iter().map_err(error)? {
        let (key, value) = item.map_err(error)?;
        if rows.len() == MAX_IDENTITIES || value.value().len() > MAX_CELL {
            return Err(error("migration catalog limit exceeded"));
        }
        let key = EndpointKey(key.value().try_into().map_err(error)?);
        let entry: Entry = exact(value.value())?;
        let (revision, _, _) = entry.details(&key)?;
        let deleted = matches!(entry, Entry::Deleted(_));
        live += u64::from(!deleted);
        rows.push((
            key,
            Stored::Retired {
                revision,
                digest: entry.digest()?,
                deleted,
            },
        ));
    }
    if rows.len() as u64 != metadata.identities || live != metadata.live {
        return Err(error("migration catalog counts disagree"));
    }
    Ok((metadata, rows))
}

#[cfg(test)]
mod tests;
