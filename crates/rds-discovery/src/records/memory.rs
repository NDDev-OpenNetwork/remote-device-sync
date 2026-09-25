use super::*;
use std::collections::BTreeMap;

/// Explicitly ephemeral store. At most 4096 identities are remembered, including
/// retired content. Restart loses replay history; production uses FileStore.
pub struct MemoryStore {
    inner: Mutex<Memory>,
    capacity: usize,
    observed: Published<RecordSnapshot>,
}
#[derive(Default)]
struct Memory {
    entries: BTreeMap<[u8; 32], Stored>,
    observed_wall: Duration,
    live: u64,
    cursor: Option<[u8; 32]>,
}
impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Memory::default()),
            capacity: MAX_IDENTITIES,
            observed: Published::new(RecordSnapshot {
                healthy: true,
                durable: false,
                generation: None,
                records: 0,
                identities: 0,
                capacity: MAX_IDENTITIES as u64,
            }),
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
        let store = Self {
            capacity: identities,
            ..Self::default()
        };
        store
            .observed
            .finish(|snapshot| snapshot.capacity = identities as u64);
        Ok(store)
    }
    fn access(
        &self,
        key: EndpointKey,
        entry: Option<Entry>,
        now: Option<Reading>,
    ) -> Result<Option<EndpointRecord>, DiscoveryError> {
        self.access_admitted(key, entry, now, &mut |_| Ok(()))
    }
    fn access_admitted(
        &self,
        key: EndpointKey,
        entry: Option<Entry>,
        now: Option<Reading>,
        admit: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<Option<EndpointRecord>, DiscoveryError> {
        self.observed_access(|inner| {
            let now = now.map(Ok).unwrap_or_else(Reading::now)?;
            observe(Duration::ZERO, &mut inner.observed_wall, now)?;
            let decision = decide(inner.entries.get(&key.0), entry, &key, now)?;
            if let Some(replacement) = decision.replacement {
                if !inner.entries.contains_key(&key.0) && inner.entries.len() >= self.capacity {
                    return Err(error("record identity capacity exceeded"));
                }
                if matches!(replacement, Stored::Active { .. }) {
                    admit(inner.entries.contains_key(&key.0))?;
                }
                inner.live -= u64::from(inner.entries.get(&key.0).is_some_and(Stored::live));
                inner.live += u64::from(replacement.live());
                inner.entries.insert(key.0, replacement);
            }
            decision.result
        })
    }
    fn collect_at(&self, now: Option<Reading>) -> Result<usize, DiscoveryError> {
        self.observed_access(|inner| {
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
                    inner.live -= u64::from(old.live());
                    inner.entries.insert(key, floor);
                    retired += 1;
                }
            }
            Ok(retired)
        })
    }

    fn observed_access<T>(
        &self,
        work: impl FnOnce(&mut Memory) -> Result<T, DiscoveryError>,
    ) -> Result<T, DiscoveryError> {
        let mut inner = self.inner.lock().map_err(|e| {
            self.observed.finish(|snapshot| snapshot.healthy = false);
            error(e)
        })?;
        self.observed.begin();
        let result = work(&mut inner);
        self.observed.finish(|snapshot| {
            snapshot.records = inner.live;
            snapshot.identities = inner.entries.len() as u64;
        });
        result
    }
}
impl RecordStore for MemoryStore {
    fn metrics(&self) -> Option<RecordMetrics> {
        Some(RecordMetrics(self.observed.observer()))
    }
    fn put_admitted(
        &self,
        record: &EndpointRecord,
        admit: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<(), DiscoveryError> {
        self.access_admitted(
            record.verify()?.key,
            Some(Entry::Record(record.clone())),
            None,
            admit,
        )
        .map(|_| ())
    }
    fn get(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        self.access(*key, None, None)?
            .ok_or(DiscoveryError::NotFound)
    }
    fn remove_admitted(
        &self,
        tomb: &DeleteRequest,
        admit: &mut dyn FnMut(bool) -> Result<(), DiscoveryError>,
    ) -> Result<(), DiscoveryError> {
        self.access_admitted(
            tomb.verify()?.key,
            Some(Entry::Deleted(tomb.clone())),
            None,
            admit,
        )
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
        let metrics = store.metrics().unwrap();
        assert_eq!(metrics.snapshot().unwrap().capacity, 1);
        let first = record(100, 1, 1000);
        store
            .access(
                first.key,
                Some(Entry::Record(first.clone())),
                Some(time(1000, 10)),
            )
            .unwrap();
        let held = store.inner.lock().unwrap();
        let published = metrics.snapshot().unwrap();
        assert!(!published.durable);
        assert_eq!(
            (
                published.generation,
                published.records,
                published.identities
            ),
            (None, 1, 1)
        );
        drop(held);
        assert_eq!(store.collect_at(Some(time(1300, 310))).unwrap(), 1);
        assert_eq!(metrics.snapshot().unwrap().records, 0);
        assert_eq!(metrics.snapshot().unwrap().identities, 1);
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
        assert_eq!(metrics.snapshot().unwrap().records, 1);
        drop(store);
        assert!(metrics.snapshot().is_none());
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
