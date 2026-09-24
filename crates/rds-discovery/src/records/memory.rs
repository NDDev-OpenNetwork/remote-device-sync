use super::*;
use std::collections::BTreeMap;

/// Explicitly ephemeral store. At most 4096 identities are remembered, including
/// retired content. Restart loses replay history; production uses FileStore.
pub struct MemoryStore {
    inner: Mutex<Memory>,
    capacity: usize,
}
#[derive(Default)]
struct Memory {
    entries: BTreeMap<[u8; 32], Stored>,
    observed_wall: Duration,
    cursor: Option<[u8; 32]>,
}
impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Memory::default()),
            capacity: MAX_IDENTITIES,
        }
    }
}
impl MemoryStore {
    /// A smaller identity budget for embedded deployments. Retired identities
    /// keep their slots; collection cannot discard anti-replay history.
    pub fn with_capacity(identities: usize) -> Result<Self, DiscoveryError> {
        if identities == 0 || identities > MAX_IDENTITIES {
            return Err(DiscoveryError::Configuration(
                "record capacity must be 1..=4096 identities".into(),
            ));
        }
        Ok(Self {
            capacity: identities,
            ..Self::default()
        })
    }
    fn access(
        &self,
        key: EndpointKey,
        entry: Option<Entry>,
        now: Option<Reading>,
    ) -> Result<Option<EndpointRecord>, DiscoveryError> {
        let mut inner = self.inner.lock().map_err(error)?;
        let now = now.map(Ok).unwrap_or_else(Reading::now)?;
        observe(Duration::ZERO, &mut inner.observed_wall, now)?;
        let decision = decide(inner.entries.get(&key.0), entry, &key, now)?;
        if let Some(replacement) = decision.replacement {
            if !inner.entries.contains_key(&key.0) && inner.entries.len() >= self.capacity {
                return Err(error("record identity capacity exceeded"));
            }
            inner.entries.insert(key.0, replacement);
        }
        decision.result
    }
    fn collect_at(&self, now: Option<Reading>) -> Result<usize, DiscoveryError> {
        let mut inner = self.inner.lock().map_err(error)?;
        let now = now.map(Ok).unwrap_or_else(Reading::now)?;
        observe(Duration::ZERO, &mut inner.observed_wall, now)?;
        let start = inner
            .cursor
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
        let keys: Vec<_> = inner
            .entries
            .range((start, std::ops::Bound::Unbounded))
            .take(GC_BATCH)
            .map(|(key, _)| *key)
            .collect();
        inner.cursor = if keys.len() == GC_BATCH {
            keys.last().copied()
        } else {
            None
        };
        let mut retired = 0;
        for key in keys {
            if let Some(old) = inner.entries.get(&key)
                && let Some(floor) = old.retire(&EndpointKey(key), now)?
            {
                inner.entries.insert(key, floor);
                retired += 1;
            }
        }
        Ok(retired)
    }
}
impl RecordStore for MemoryStore {
    fn put(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        self.access(
            record.verify()?.key,
            Some(Entry::Record(record.clone())),
            None,
        )
        .map(|_| ())
    }
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        self.access(*key, None, None)?
            .ok_or(DiscoveryError::NotFound)
    }
    fn remove(&self, tomb: &DeleteRequest) -> Result<(), DiscoveryError> {
        self.access(tomb.verify()?.key, Some(Entry::Deleted(tomb.clone())), None)
            .map(|_| ())
    }
    fn collect_expired(&self) -> Result<usize, DiscoveryError> {
        self.collect_at(None)
    }
    fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.entries.values().filter(|entry| entry.live()).count())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::expiry_tests::{record, time};

    #[test]
    fn capacity_preserves_renewal_and_history_after_collection() {
        assert!(MemoryStore::with_capacity(0).is_err());
        assert!(MemoryStore::with_capacity(MAX_IDENTITIES + 1).is_err());
        let store = MemoryStore::with_capacity(1).unwrap();
        let first = record(100, 1, 1000);
        store
            .access(
                first.key,
                Some(Entry::Record(first.clone())),
                Some(time(1000, 10)),
            )
            .unwrap();
        assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), 1);
        let stranger = record(101, 1, 1300);
        assert!(
            store
                .access(
                    stranger.key,
                    Some(Entry::Record(stranger)),
                    Some(time(1300, 310))
                )
                .is_err()
        );
        let replay = record(100, 1, 1300);
        assert!(matches!(
            store.access(
                replay.key,
                Some(Entry::Record(replay)),
                Some(time(1300, 310))
            ),
            Err(DiscoveryError::Stale)
        ));
        let renewal = record(100, 2, 1300);
        store
            .access(
                renewal.key,
                Some(Entry::Record(renewal)),
                Some(time(1300, 310)),
            )
            .unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.inner.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn memory_expiry_and_clock_rules_match_disk() {
        let store = MemoryStore::default();
        let record = record(102, 1, 1000);
        store
            .access(
                record.key,
                Some(Entry::Record(record.clone())),
                Some(time(1000, 10)),
            )
            .unwrap();
        assert!(matches!(
            store.access(record.key, None, Some(time(1200, 310))),
            Err(DiscoveryError::Expired)
        ));
        assert!(matches!(
            store.access(
                record.key,
                Some(Entry::Record(record.clone())),
                Some(time(1200, 311))
            ),
            Err(DiscoveryError::Expired)
        ));
        assert!(matches!(
            store.access(record.key, None, Some(time(1199, 312))),
            Err(DiscoveryError::Store(_))
        ));
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn memory_collector_rotates_in_bounded_batches() {
        let store = MemoryStore::default();
        for seed in 0..=GC_BATCH as u8 {
            let record = record(seed, 1, 1000);
            store
                .access(
                    record.key,
                    Some(Entry::Record(record)),
                    Some(time(1000, 10)),
                )
                .unwrap();
        }
        assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), GC_BATCH);
        assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), 1);
        assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), 0);
        assert_eq!(store.inner.lock().unwrap().entries.len(), GC_BATCH + 1);
    }
}
