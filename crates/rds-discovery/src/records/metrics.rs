use crate::observation::Observer;

/// A coherent observation of a completed owner operation. Counts describe
/// stored content, including records awaiting expiry collection, not reachability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordSnapshot {
    pub healthy: bool,
    pub durable: bool,
    /// Last acknowledged database/anchor generation; absent for memory stores.
    pub generation: Option<u64>,
    pub records: u64,
    /// All retained identities, including delete tombstones and expiry floors.
    pub identities: u64,
    pub capacity: u64,
}

/// Weak metadata-only observer. It owns no database, file lock or task.
#[derive(Clone)]
pub struct RecordMetrics(pub(super) Observer<RecordSnapshot>);

impl RecordMetrics {
    /// Unknown while the owner is working, the publication lock is busy or
    /// poisoned, or the owner is gone. Never performs I/O or takes a store lock.
    /// When unhealthy, counts describe the last acknowledged state only.
    pub fn snapshot(&self) -> Option<RecordSnapshot> {
        self.0.snapshot()
    }
}
