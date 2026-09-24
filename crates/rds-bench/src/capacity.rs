//! File-store qualification with synthetic publishers, never estate records.
//! Every run requires a new directory and preserves it for inspection.
use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder},
    net::{Ipv6Addr, SocketAddr},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use ed25519_dalek::SigningKey;
use rds_discovery::{
    DeleteRequest, DiscoveryError, EndpointRecord, FileStore, MAX_DIRECT_ADDRS,
    MAX_RECORD_DATABASE_BYTES, MAX_RECORD_IDENTITIES, MAX_RELAY_URL_BYTES, MAX_RELAY_URLS,
    RecordStore, Service,
};

use crate::report::{BenchMeta, BenchReport, Percentiles, git_sha, unix_ts};

struct Measurement {
    path: PathBuf,
    samples: Vec<u64>,
    observed_peak: u64,
    observed_allocated_peak: u64,
}
impl Measurement {
    fn new(root: &Path) -> Self {
        Self {
            path: root.join("records.redb"),
            samples: Vec::new(),
            observed_peak: 0,
            observed_allocated_peak: 0,
        }
    }
    fn observe(&mut self) -> anyhow::Result<()> {
        let metadata = fs::metadata(&self.path)?;
        ensure!(
            metadata.len() <= MAX_RECORD_DATABASE_BYTES,
            "database exceeded hard limit"
        );
        self.observed_peak = self.observed_peak.max(metadata.len());
        self.observed_allocated_peak = self.observed_allocated_peak.max(metadata.blocks() * 512);
        Ok(())
    }
    fn time(&mut self, op: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
        let started = Instant::now();
        op()?;
        self.samples.push(started.elapsed().as_nanos().try_into()?);
        self.observe()
    }
    fn finish(self, phase: &str, identities: usize) -> BenchReport {
        let count = self.samples.len() as u64;
        BenchReport {
            meta: BenchMeta {
                scenario: format!("directory-{phase}"),
                backend: "redb".into(),
                path: "local-file".into(),
                impairment: None,
                unix_ts: unix_ts(),
                git: git_sha(),
            },
            rtt: Percentiles::of(&self.samples),
            throughput_mib_s: None,
            attempts: Some((count, count)),
            metrics: BTreeMap::from([
                ("identities".into(), identities as u64),
                ("database_limit_bytes".into(), MAX_RECORD_DATABASE_BYTES),
                ("observed_database_peak_bytes".into(), self.observed_peak),
                ("observed_allocated_peak_bytes".into(), self.observed_allocated_peak),
            ]),
            notes: vec![
                format!("Build debug assertions: {}. Use the paired receipt for compiler/profile and working-tree provenance.", cfg!(debug_assertions)),
                "Storage-call latency including verification and durable commits; signing and post-call file statistics excluded. Reopen includes catalog validation. Not network RTT or a production SLO.".into(),
                "File sizes sampled after each completed call; no filesystem exhaustion, power loss, HTTP quota or concurrent-writer qualification is implied.".into(),
            ],
        }
    }
}

fn key(index: usize) -> SigningKey {
    let mut bytes = [0x43; 32];
    bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
    SigningKey::from_bytes(&bytes)
}

fn record(key: &SigningKey, revision: u64, large: bool) -> anyhow::Result<EndpointRecord> {
    let (addrs, relays, services) = if large {
        let addrs = (0..MAX_DIRECT_ADDRS)
            .map(|i| {
                SocketAddr::from((
                    Ipv6Addr::new(
                        0x2001, 0xdb8, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, i as u16,
                    ),
                    u16::MAX,
                ))
            })
            .collect();
        let relays = (0..MAX_RELAY_URLS)
            .map(|i| {
                // Legal zero-padded port spelling fills the raw locator bound
                // without inventing a DNS name longer than a real hostname.
                let prefix = format!("https://relay-{i}.invalid:");
                format!(
                    "{prefix}{}443/",
                    "0".repeat(MAX_RELAY_URL_BYTES - prefix.len() - 4)
                )
            })
            .collect();
        (
            addrs,
            relays,
            vec![
                Service::Ping,
                Service::Info,
                Service::TcpForward,
                Service::Desktop,
                Service::Audio,
                Service::Sync,
            ],
        )
    } else {
        (vec!["127.0.0.1:4000".parse()?], vec![], vec![Service::Ping])
    };
    Ok(EndpointRecord::publish(
        key,
        revision,
        addrs,
        relays,
        services,
        Duration::from_secs(3600),
    )?)
}

fn reject_new(store: &FileStore) -> anyhow::Result<()> {
    let extra = record(&key(MAX_RECORD_IDENTITIES), 1, true)?;
    let mut charged = false;
    let result = store.put_admitted(&extra, &mut |_| {
        charged = true;
        Ok(())
    });
    ensure!(
        matches!(result, Err(DiscoveryError::Store(ref message)) if message == "record identity capacity exceeded"),
        "full store returned unexpected admission result: {result:?}"
    );
    ensure!(!charged, "identity overflow charged admission");
    Ok(())
}

fn verify(store: &FileStore, expected: &[Option<EndpointRecord>]) -> anyhow::Result<()> {
    ensure!(
        store.len() == expected.iter().flatten().count(),
        "live count differs"
    );
    for (i, value) in expected.iter().enumerate() {
        let endpoint = rds_discovery::EndpointKey(key(i).verifying_key().to_bytes());
        match (value, store.get(&endpoint)) {
            (Some(expected), Ok(actual)) => ensure!(actual == *expected, "record {i} differs"),
            (None, Err(DiscoveryError::NotFound)) => {}
            (_, other) => anyhow::bail!("record {i} read failed: {other:?}"),
        }
    }
    Ok(())
}

/// Fill every identity slot, reject one more, then shrink/expand, delete,
/// reactivate and reopen the durable store. Failure stops the run and leaves
/// its database intact; no successful qualification report is fabricated.
pub fn run(root: &Path, rounds: u32) -> anyhow::Result<Vec<BenchReport>> {
    ensure!(
        (2..=64).contains(&rounds) && rounds.is_multiple_of(2),
        "rounds must be even, in 2..=64"
    );
    // create, not create_dir_all: never attach to or overwrite an existing run.
    DirBuilder::new()
        .mode(0o700)
        .create(root)
        .context("create fresh benchmark state directory")?;
    let mut store = FileStore::new(root)?;
    let keys: Vec<_> = (0..MAX_RECORD_IDENTITIES).map(key).collect();
    let mut expected = Vec::with_capacity(keys.len());
    let mut reports = Vec::new();
    let mut measurement = Measurement::new(root);
    let mut payload_range = (usize::MAX, 0usize);
    for signing_key in &keys {
        let value = record(signing_key, 1, true)?;
        payload_range.0 = payload_range.0.min(value.payload.len());
        payload_range.1 = payload_range.1.max(value.payload.len());
        measurement.time(|| Ok(store.put(&value)?))?;
        expected.push(Some(value));
        if expected.len().is_multiple_of(1024) {
            eprintln!(
                "directory capacity: filled {}/{}",
                expected.len(),
                keys.len()
            );
        }
    }
    reject_new(&store)?;
    let mut fill = measurement.finish("fill", keys.len());
    fill.notes.push(format!("Large fixture: {MAX_DIRECT_ADDRS} IPv6 candidates, {MAX_RELAY_URLS} raw {MAX_RELAY_URL_BYTES}-byte relay origins, all six services; signed payload {}..{} bytes. Addresses are synthetic and never dialed.", payload_range.0, payload_range.1));
    reports.push(fill);
    verify(&store, &expected)?;
    let mut reopen = Measurement::new(root);
    for round in 1..=rounds {
        let mut measurement = Measurement::new(root);
        for (i, signing_key) in keys.iter().enumerate() {
            let value = record(signing_key, u64::from(round) + 1, round.is_multiple_of(2))?;
            measurement.time(|| Ok(store.put(&value)?))?;
            expected[i] = Some(value);
            if (i + 1).is_multiple_of(1024) {
                eprintln!(
                    "directory capacity: renewal {round}/{rounds}, {}/{}",
                    i + 1,
                    keys.len()
                );
            }
        }
        reject_new(&store)?;
        reports.push(measurement.finish(&format!("renew-{round}"), keys.len()));
        drop(store);
        let started = Instant::now();
        store = FileStore::new(root)?;
        reopen
            .samples
            .push(started.elapsed().as_nanos().try_into()?);
        reopen.observe()?;
        verify(&store, &expected)?;
        eprintln!("directory capacity: renewal {round}/{rounds} and reopen passed");
    }
    let mut deletion = Measurement::new(root);
    eprintln!("directory capacity: deleting half of the identities");
    for (i, signing_key) in keys.iter().enumerate().step_by(2) {
        let tomb = DeleteRequest::new(signing_key, u64::from(rounds) + 2)?;
        deletion.time(|| Ok(store.remove(&tomb)?))?;
        expected[i] = None;
    }
    // A complete bounded catalog pass must not free retained identity slots.
    for _ in 0..=(MAX_RECORD_IDENTITIES / 64) {
        store.collect_expired()?;
    }
    reject_new(&store)?;
    verify(&store, &expected)?;
    reports.push(deletion.finish("delete-half", keys.len()));
    drop(store);
    let started = Instant::now();
    store = FileStore::new(root)?;
    reopen
        .samples
        .push(started.elapsed().as_nanos().try_into()?);
    reopen.observe()?;
    verify(&store, &expected)?;
    let mut reactivation = Measurement::new(root);
    eprintln!("directory capacity: reactivating deleted identities");
    for (i, signing_key) in keys.iter().enumerate().step_by(2) {
        let value = record(signing_key, u64::from(rounds) + 3, true)?;
        reactivation.time(|| Ok(store.put(&value)?))?;
        expected[i] = Some(value);
    }
    reject_new(&store)?;
    reports.push(reactivation.finish("reactivate-half", keys.len()));
    drop(store);
    let started = Instant::now();
    store = FileStore::new(root)?;
    reopen
        .samples
        .push(started.elapsed().as_nanos().try_into()?);
    reopen.observe()?;
    verify(&store, &expected)?;
    reports.push(reopen.finish("reopen", keys.len()));
    Ok(reports)
}
