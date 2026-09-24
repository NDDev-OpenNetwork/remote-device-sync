//! Transactional record/tombstone storage. The wire ordering currently remains
//! issued_at; explicit publisher revisions and quota/GC policy follow in W1.5.
use crate::{
    DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, RecordStore,
    persist::{AtomicFile, regular},
};
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use rustix::fs::{Mode, OFlags, openat};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io,
    os::unix::fs::FileExt,
    path::Path,
    sync::{Mutex, RwLock},
};

const RECORDS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("records-v1");
const META: TableDefinition<'_, u8, &[u8]> = TableDefinition::new("metadata-v1");
const MAX_CELL: usize = 256 * 1024;
const MAX_DATABASE: u64 = 256 * 1024 * 1024;
const MAX_IDENTITIES: usize = 4096;

#[cfg(test)]
mod tests;
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    BeforeDatabase,
    AfterDatabase,
    AfterAnchor,
}

fn error(e: impl std::fmt::Display) -> DiscoveryError {
    DiscoveryError::Store(e.to_string())
}

#[derive(Clone, Serialize, Deserialize)]
enum Entry {
    Record(EndpointRecord),
    Deleted(DeleteRequest),
}
impl Entry {
    fn verify(&self, key: &EndpointKey) -> Result<u64, DiscoveryError> {
        let (actual, issued) = match self {
            Self::Record(record) => {
                let p = record.verify()?;
                (p.key, p.issued_at)
            }
            Self::Deleted(tomb) => {
                let p = tomb.verify()?;
                (p.key, p.issued_at)
            }
        };
        if actual != *key {
            return Err(DiscoveryError::BadSignature);
        }
        Ok(issued)
    }
    fn record(&self) -> Result<EndpointRecord, DiscoveryError> {
        match self {
            Self::Record(record) => Ok(record.clone()),
            Self::Deleted(_) => Err(DiscoveryError::NotFound),
        }
    }
    fn live(&self) -> bool {
        matches!(self, Self::Record(_))
    }
}

fn check(old: Option<&Entry>, new: &Entry, key: &EndpointKey) -> Result<(), DiscoveryError> {
    let issued = new.verify(key)?;
    if let Some(old) = old {
        let previous = old.verify(key)?;
        if issued < previous || (issued == previous && new.live()) {
            return Err(DiscoveryError::Stale);
        }
    }
    Ok(())
}

fn decode(bytes: &[u8], key: &EndpointKey) -> Result<Entry, DiscoveryError> {
    if bytes.len() > MAX_CELL {
        return Err(error("stored record exceeds limit"));
    }
    let entry: Entry = postcard::from_bytes(bytes).map_err(error)?;
    entry.verify(key)?;
    Ok(entry)
}

/// Ephemeral record/tombstone store for tests and embedded deployments.
#[derive(Default)]
pub struct MemoryStore {
    entries: RwLock<HashMap<EndpointKey, Entry>>,
}
impl MemoryStore {
    fn apply(&self, key: EndpointKey, entry: Entry) -> Result<(), DiscoveryError> {
        let mut entries = self.entries.write().map_err(error)?;
        check(entries.get(&key), &entry, &key)?;
        entries.insert(key, entry);
        Ok(())
    }
}
impl RecordStore for MemoryStore {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        self.apply(record.verify()?.key, Entry::Record(record.clone()))
    }
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        self.entries
            .read()
            .map_err(error)?
            .get(key)
            .ok_or(DiscoveryError::NotFound)?
            .record()
    }
    fn remove(&self, tomb: &DeleteRequest) -> Result<(), DiscoveryError> {
        self.apply(tomb.verify()?.key, Entry::Deleted(tomb.clone()))
    }
    fn len(&self) -> usize {
        self.entries
            .read()
            .map(|entries| entries.values().filter(|entry| entry.live()).count())
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Metadata {
    format: u16,
    database: [u8; 16],
    generation: u64,
    live: u64,
    identities: u64,
}

#[derive(Serialize, Deserialize)]
struct Anchor {
    metadata: Metadata,
    digest: [u8; 32],
}
fn encode_anchor(metadata: &Metadata) -> Result<Vec<u8>, DiscoveryError> {
    let bytes = postcard::to_stdvec(metadata).map_err(error)?;
    serde_json::to_vec(&Anchor {
        metadata: metadata.clone(),
        digest: *blake3::hash(&bytes).as_bytes(),
    })
    .map_err(error)
}
fn decode_anchor(bytes: &[u8]) -> Result<Metadata, DiscoveryError> {
    let anchor: Anchor = serde_json::from_slice(bytes).map_err(error)?;
    let digest = *blake3::hash(&postcard::to_stdvec(&anchor.metadata).map_err(error)?).as_bytes();
    if digest != anchor.digest {
        return Err(error("record commit anchor checksum mismatch"));
    }
    Ok(anchor.metadata)
}

/// A file descriptor opened relative to the protected directory. redb never
/// opens a path. The strict OS file lock is held for this backend's lifetime;
/// unsupported file locking is an error, never an unsafe fallback.
#[derive(Debug)]
struct BoundedFile {
    file: File,
    #[cfg(test)]
    fault: std::sync::Arc<std::sync::atomic::AtomicU8>,
}
impl redb::StorageBackend for BoundedFile {
    fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
    fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
        bounds(offset, out.len())?;
        self.file.read_exact_at(out, offset)
    }
    fn set_len(&self, len: u64) -> io::Result<()> {
        if len > MAX_DATABASE {
            return Err(io::Error::other("record database capacity exceeded"));
        }
        self.file.set_len(len)
    }
    fn sync_data(&self) -> io::Result<()> {
        #[cfg(test)]
        if self
            .fault
            .compare_exchange(
                2,
                0,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            return Err(io::Error::other("injected database sync failure"));
        }
        self.file.sync_all()
    }
    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        bounds(offset, data.len())?;
        #[cfg(test)]
        if self
            .fault
            .compare_exchange(
                1,
                0,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            self.file.write_all_at(&data[..data.len() / 2], offset)?;
            return Err(io::Error::other("injected partial database write"));
        }
        self.file.write_all_at(data, offset)
    }
}
fn bounds(offset: u64, len: usize) -> io::Result<()> {
    if offset
        .checked_add(len as u64)
        .is_none_or(|end| end > MAX_DATABASE)
    {
        Err(io::Error::other("record database capacity exceeded"))
    } else {
        Ok(())
    }
}

/// Protected embedded Rust database, including durable delete tombstones.
/// Legacy per-key JSON directories require explicit migration; they are never
/// ignored or overwritten. No database daemon or subprocess is used.
pub struct FileStore {
    inner: Mutex<Inner>,
}
struct Inner {
    db: Database,
    anchor: AtomicFile,
    metadata: Metadata,
    failed: bool,
    #[cfg(test)]
    fault: Option<(Phase, bool)>,
    #[cfg(test)]
    io_fault: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl FileStore {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> io::Result<Self> {
        Self::open(&dir.into()).map_err(io::Error::other)
    }
    fn open(path: &Path) -> Result<Self, DiscoveryError> {
        let anchor = AtomicFile::open_named(path, "records.anchor", "records.lock")?;
        // This namespace is exclusively ours. Refuse legacy/unrecognized
        // files instead of silently initializing over unrelated history.
        for (index, entry) in rustix::fs::Dir::read_from(anchor.directory())
            .map_err(error)?
            .enumerate()
        {
            if index > 128 {
                return Err(error("record state directory requires maintenance"));
            }
            let entry = entry.map_err(error)?;
            let name = entry.file_name().to_bytes();
            if !matches!(
                name,
                b"." | b".." | b"records.lock" | b"records.anchor" | b"records.redb"
            ) && !(name.starts_with(b".policy-") && name.ends_with(b".tmp"))
            {
                return Err(error(
                    "legacy or unknown record files require explicit migration",
                ));
            }
        }
        let stored_anchor = anchor
            .read()?
            .map(|bytes| decode_anchor(&bytes))
            .transpose()?;
        if stored_anchor.is_none() && anchor.initialized()? {
            return Err(error("record commit anchor missing"));
        }
        let mut flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        if stored_anchor.is_none() {
            flags |= OFlags::CREATE;
        }
        let file = File::from(
            openat(
                anchor.directory(),
                "records.redb",
                flags,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(error)?,
        );
        regular(&file)?;
        file.try_lock()
            .map_err(|e| error(format!("record database lock: {e}")))?;
        let len = file.metadata().map_err(error)?.len();
        if len > MAX_DATABASE || (len == 0 && stored_anchor.is_some()) {
            return Err(error("record database missing, empty or oversized"));
        }
        #[cfg(test)]
        let io_fault = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let db = Database::builder()
            .set_cache_size(16 * 1024 * 1024)
            .create_with_backend(BoundedFile {
                file,
                #[cfg(test)]
                fault: io_fault.clone(),
            })
            .map_err(error)?;
        let metadata = {
            let read = db.begin_read().map_err(error)?;
            match read.open_table(META) {
                Ok(table) => {
                    let value = table
                        .get(0)
                        .map_err(error)?
                        .ok_or_else(|| error("record metadata missing"))?;
                    postcard::from_bytes::<Metadata>(value.value()).map_err(error)?
                }
                Err(redb::TableError::TableDoesNotExist(_)) if stored_anchor.is_none() => {
                    if read.list_tables().map_err(error)?.next().is_some() {
                        return Err(error("record metadata absent from nonempty database"));
                    }
                    drop(read);
                    let metadata = Metadata {
                        format: 1,
                        database: rand::random(),
                        generation: 1,
                        live: 0,
                        identities: 0,
                    };
                    let mut txn = db.begin_write().map_err(error)?;
                    txn.set_durability(Durability::Immediate).map_err(error)?;
                    txn.open_table(RECORDS).map_err(error)?;
                    txn.open_table(META)
                        .map_err(error)?
                        .insert(0, postcard::to_stdvec(&metadata).map_err(error)?.as_slice())
                        .map_err(error)?;
                    txn.commit().map_err(error)?;
                    metadata
                }
                Err(e) => return Err(error(e)),
            }
        };
        if metadata.format != 1
            || metadata.generation == 0
            || metadata.identities > MAX_IDENTITIES as u64
        {
            return Err(error("invalid record metadata"));
        }
        if let Some(previous) = &stored_anchor {
            if previous.database != metadata.database
                || metadata.generation < previous.generation
                || metadata.generation > previous.generation.saturating_add(1)
                || (metadata.generation == previous.generation && *previous != metadata)
            {
                return Err(error(
                    "record database rolled back or disagrees with durable commit anchor",
                ));
            }
        } else if metadata.generation != 1 || metadata.identities != 0 {
            return Err(error("record database lacks its commit anchor"));
        }
        // Validate the entire bounded catalog on open. A repair must not turn
        // corrupt entries into missing history or serve an unverified record.
        let mut identities = 0;
        let mut live = 0;
        {
            let read = db.begin_read().map_err(error)?;
            let table = read.open_table(RECORDS).map_err(error)?;
            for entry in table.iter().map_err(error)? {
                let (key, value) = entry.map_err(error)?;
                identities += 1;
                if identities > MAX_IDENTITIES as u64 {
                    return Err(error("record identity capacity exceeded"));
                }
                let key = EndpointKey(key.value().try_into().map_err(error)?);
                live += u64::from(decode(value.value(), &key)?.live());
            }
        }
        if identities != metadata.identities || live != metadata.live {
            return Err(error("record metadata/catalog mismatch"));
        }
        if stored_anchor.as_ref() != Some(&metadata) {
            anchor.write(&encode_anchor(&metadata)?)?;
        }
        if !anchor.initialized()? {
            anchor.seal()?;
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                db,
                anchor,
                metadata,
                failed: false,
                #[cfg(test)]
                fault: None,
                #[cfg(test)]
                io_fault,
            }),
        })
    }
    fn apply(&self, key: EndpointKey, entry: Entry) -> Result<(), DiscoveryError> {
        let bytes = postcard::to_stdvec(&entry).map_err(error)?;
        if bytes.len() > MAX_CELL {
            return Err(error("record exceeds storage limit"));
        }
        let mut inner = self.inner.lock().map_err(error)?;
        inner.healthy()?;
        // Leave the instance closed on any unexpected storage/validation error,
        // including a failure before commit or after the database commit.
        inner.failed = true;
        let anchored = inner
            .anchor
            .read()?
            .ok_or_else(|| error("record commit anchor disappeared"))?;
        if decode_anchor(&anchored)? != inner.metadata {
            return Err(error(
                "record commit anchor changed during service lifetime",
            ));
        }
        let mut txn = inner.db.begin_write().map_err(error)?;
        txn.set_durability(Durability::Immediate).map_err(error)?;
        let mut next = inner.metadata.clone();
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or_else(|| error("record commit generation exhausted"))?;
        {
            let mut table = txn.open_table(RECORDS).map_err(error)?;
            let old = table
                .get(key.0.as_slice())
                .map_err(error)?
                .map(|old| decode(old.value(), &key))
                .transpose()?;
            match check(old.as_ref(), &entry, &key) {
                Err(DiscoveryError::Stale) => {
                    inner.failed = false;
                    return Err(DiscoveryError::Stale);
                }
                other => other?,
            }
            if let (Some(Entry::Deleted(old)), Entry::Deleted(new)) = (&old, &entry)
                && old.payload == new.payload
            {
                inner.failed = false;
                return Ok(());
            }
            if old.is_none() {
                if next.identities >= MAX_IDENTITIES as u64 {
                    inner.failed = false;
                    return Err(error("record identity capacity exceeded"));
                }
                next.identities += 1;
            }
            next.live = next
                .live
                .checked_sub(u64::from(old.as_ref().is_some_and(Entry::live)))
                .and_then(|count| count.checked_add(u64::from(entry.live())))
                .ok_or_else(|| error("record count inconsistent"))?;
            table
                .insert(key.0.as_slice(), bytes.as_slice())
                .map_err(error)?;
        }
        txn.open_table(META)
            .map_err(error)?
            .insert(0, postcard::to_stdvec(&next).map_err(error)?.as_slice())
            .map_err(error)?;
        // Keep the outer mutex through BOTH commits. Readers and subsequent
        // writers cannot observe/publish a generation before its anchor lands.
        #[cfg(test)]
        inner.checkpoint(Phase::BeforeDatabase)?;
        txn.commit().map_err(error)?;
        #[cfg(test)]
        inner.checkpoint(Phase::AfterDatabase)?;
        inner.anchor.write(&encode_anchor(&next)?)?;
        #[cfg(test)]
        inner.checkpoint(Phase::AfterAnchor)?;
        inner.metadata = next;
        inner.failed = false;
        Ok(())
    }
}
impl Inner {
    #[cfg(test)]
    fn checkpoint(&self, phase: Phase) -> Result<(), DiscoveryError> {
        if let Some((selected, terminate)) = self.fault
            && selected == phase
        {
            if terminate {
                std::process::exit(86);
            }
            return Err(error(format!("injected record failure at {phase:?}")));
        }
        Ok(())
    }
    fn healthy(&self) -> Result<(), DiscoveryError> {
        if self.failed {
            Err(error("record commit outcome uncertain; reopen required"))
        } else {
            Ok(())
        }
    }
}
impl RecordStore for FileStore {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        self.apply(record.verify()?.key, Entry::Record(record.clone()))
    }
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        let inner = self.inner.lock().map_err(error)?;
        inner.healthy()?;
        let txn = inner.db.begin_read().map_err(error)?;
        let table = txn.open_table(RECORDS).map_err(error)?;
        let value = table
            .get(key.0.as_slice())
            .map_err(error)?
            .ok_or(DiscoveryError::NotFound)?;
        decode(value.value(), key)?.record()
    }
    fn remove(&self, tomb: &DeleteRequest) -> Result<(), DiscoveryError> {
        self.apply(tomb.verify()?.key, Entry::Deleted(tomb.clone()))
    }
    fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.metadata.live as usize)
            .unwrap_or(0)
    }
}
