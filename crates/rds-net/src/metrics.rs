//! Transport metrics: a lightweight counter facade every backend feeds
//! and any owner (agent, bench harness, rds-server) can scrape.
//!
//! The model is deliberately small: an endpoint owns one [`Registry`]
//! of atomics; a [`ConnSampler`] diffs a connection's cumulative
//! observed path counters into direct/relay buckets. Sampling can miss short
//! paths and final increments after retirement; these are observed totals,
//! not lossless accounting. Noq coverage and event loss are explicit. QNT attempt/success counters are driven by the
//! noq policy driver — on iroh they stay zero (`paths_seen{via=direct}`
//! appearing after a relay-only start is the equivalent signal).
//!
//! Export: [`Registry::render_prometheus`] emits the standard text
//! exposition format behind the `metrics` feature — no prometheus
//! dependency, just the counter names the bench reports cite.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{Connection, PathStats, PathStatsCoverage};

/// Shared counter set for one endpoint. Clone to share; every clone
/// writes the same counters.
#[derive(Clone, Default)]
pub struct Registry {
    inner: Arc<Counters>,
}

#[derive(Default)]
struct Counters {
    connections_opened: AtomicU64,
    connections_accepted: AtomicU64,
    datagrams_sent_direct: AtomicU64,
    datagrams_sent_relay: AtomicU64,
    datagrams_lost_direct: AtomicU64,
    datagrams_lost_relay: AtomicU64,
    bytes_sent_direct: AtomicU64,
    bytes_sent_relay: AtomicU64,
    bytes_recv_direct: AtomicU64,
    bytes_recv_relay: AtomicU64,
    congestion_events: AtomicU64,
    paths_seen_direct: AtomicU64,
    paths_seen_relay: AtomicU64,
    qnt_attempts: AtomicU64,
    qnt_success: AtomicU64,
    /// Gauge: connections with a live sampler.
    active_connections: AtomicU64,
    /// Last-sampled selected-path RTT/cwnd and validity are one observation.
    /// A scrape must not combine different samplers' values.
    selected_path: Mutex<Option<SelectedPathSample>>,
    /// Gauge: live paths across sampled connections.
    live_paths: AtomicU64,
    /// Connections sampled through an event-driven (not full snapshot) view.
    policy_observed_connections: AtomicU64,
    /// Policy views with lost events or a stopped observer.
    degraded_path_observers: AtomicU64,
    path_events_lost: AtomicU64,
}

struct SelectedPathSample {
    owner: Arc<()>,
    rtt_us: u64,
    cwnd_bytes: u64,
}

impl Registry {
    pub fn connection_opened(&self) {
        self.inner
            .connections_opened
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_accepted(&self) {
        self.inner
            .connections_accepted
            .fetch_add(1, Ordering::Relaxed);
    }

    /// A NAT-traversal attempt: a round initiated or a path opened to a
    /// QNT-learned candidate (noq backend only — iroh does not expose
    /// its hole-punch attempts).
    pub fn qnt_attempt(&self) {
        self.inner.qnt_attempts.fetch_add(1, Ordering::Relaxed);
    }

    /// A QNT-learned path reached Established.
    pub fn qnt_success(&self) {
        self.inner.qnt_success.fetch_add(1, Ordering::Relaxed);
    }

    /// Per-connection tracker folding cumulative `path_stats` into
    /// these counters. One per connection; `run` ends when the
    /// connection closes. The sampler retains only a weak observation handle;
    /// passing the last connection handle here does not keep I/O alive.
    pub fn sampler(&self, conn: Connection) -> ConnSampler {
        self.inner
            .active_connections
            .fetch_add(1, Ordering::Relaxed);
        ConnSampler {
            registry: self.clone(),
            conn: crate::observation::Observer::new(&conn),
            sample_owner: Arc::new(()),
            seen: HashMap::new(),
            last_live: 0,
            last_policy: 0,
            last_degraded: 0,
            last_lost_events: 0,
        }
    }

    /// Counter snapshot keyed by exposition name — what bench reports
    /// embed and `render_prometheus` serializes. A busy selected-path observation
    /// is unknown for this scrape; export never waits for its sampler's lock.
    pub fn snapshot(&self) -> BTreeMap<&'static str, u64> {
        let c = &*self.inner;
        let selected = c
            .selected_path
            .try_lock()
            .ok()
            .and_then(|sample| sample.as_ref().map(|s| (s.rtt_us, s.cwnd_bytes)));
        BTreeMap::from([
            (
                "rds_net_connections_opened_total",
                c.connections_opened.load(Ordering::Relaxed),
            ),
            (
                "rds_net_connections_accepted_total",
                c.connections_accepted.load(Ordering::Relaxed),
            ),
            (
                "rds_net_datagrams_sent_total{via=\"direct\"}",
                c.datagrams_sent_direct.load(Ordering::Relaxed),
            ),
            (
                "rds_net_datagrams_sent_total{via=\"relay\"}",
                c.datagrams_sent_relay.load(Ordering::Relaxed),
            ),
            (
                "rds_net_datagrams_lost_total{via=\"direct\"}",
                c.datagrams_lost_direct.load(Ordering::Relaxed),
            ),
            (
                "rds_net_datagrams_lost_total{via=\"relay\"}",
                c.datagrams_lost_relay.load(Ordering::Relaxed),
            ),
            (
                "rds_net_bytes_sent_total{via=\"direct\"}",
                c.bytes_sent_direct.load(Ordering::Relaxed),
            ),
            (
                "rds_net_bytes_sent_total{via=\"relay\"}",
                c.bytes_sent_relay.load(Ordering::Relaxed),
            ),
            (
                "rds_net_bytes_received_total{via=\"direct\"}",
                c.bytes_recv_direct.load(Ordering::Relaxed),
            ),
            (
                "rds_net_bytes_received_total{via=\"relay\"}",
                c.bytes_recv_relay.load(Ordering::Relaxed),
            ),
            (
                "rds_net_congestion_events_total",
                c.congestion_events.load(Ordering::Relaxed),
            ),
            (
                "rds_net_paths_seen_total{via=\"direct\"}",
                c.paths_seen_direct.load(Ordering::Relaxed),
            ),
            (
                "rds_net_paths_seen_total{via=\"relay\"}",
                c.paths_seen_relay.load(Ordering::Relaxed),
            ),
            (
                "rds_net_qnt_attempts_total",
                c.qnt_attempts.load(Ordering::Relaxed),
            ),
            (
                "rds_net_qnt_success_total",
                c.qnt_success.load(Ordering::Relaxed),
            ),
            (
                "rds_net_active_connections",
                c.active_connections.load(Ordering::Relaxed),
            ),
            ("rds_net_rtt_us", selected.map_or(0, |s| s.0)),
            ("rds_net_cwnd_bytes", selected.map_or(0, |s| s.1)),
            ("rds_net_live_paths", c.live_paths.load(Ordering::Relaxed)),
            (
                "rds_net_policy_observed_connections",
                c.policy_observed_connections.load(Ordering::Relaxed),
            ),
            (
                "rds_net_degraded_path_observers",
                c.degraded_path_observers.load(Ordering::Relaxed),
            ),
            (
                "rds_net_path_events_lost_total",
                c.path_events_lost.load(Ordering::Relaxed),
            ),
            ("rds_net_selected_path_known", u64::from(selected.is_some())),
        ])
    }

    /// Prometheus text exposition (`metrics` feature — the counters
    /// themselves are always live; only the scrape format is gated).
    #[cfg(feature = "metrics")]
    pub fn render_prometheus(&self) -> String {
        const GAUGES: &[&str] = &[
            "rds_net_active_connections",
            "rds_net_rtt_us",
            "rds_net_cwnd_bytes",
            "rds_net_live_paths",
            "rds_net_policy_observed_connections",
            "rds_net_degraded_path_observers",
            "rds_net_selected_path_known",
        ];
        let mut out = String::new();
        for (name, value) in self.snapshot() {
            let base = name.split(['{', '=']).next().unwrap_or(name);
            let kind = if GAUGES.contains(&base) {
                "gauge"
            } else {
                "counter"
            };
            out.push_str(&format!("# TYPE {base} {kind}\n{name} {value}\n"));
        }
        out
    }
}

/// Folds one connection's cumulative per-path counters into a
/// [`Registry`]. `sample()` is cheap (a `path_stats()` snapshot plus a
/// diff); `run()` samples on an interval until the connection closes
/// and attempts a final sample. Paths retired between samples (including
/// teardown) and their final increments can be missed.
pub struct ConnSampler {
    registry: Registry,
    conn: crate::observation::Observer,
    sample_owner: Arc<()>,
    /// path_id → (sent, lost, sent_bytes, recv_bytes, congestion_events)
    /// at the last sample.
    seen: HashMap<u64, (u64, u64, u64, u64, u64)>,
    last_live: u64,
    last_policy: u64,
    last_degraded: u64,
    last_lost_events: u64,
}

impl ConnSampler {
    /// Fold current `path_stats` into the registry. Safe to call any
    /// number of times — only deltas count.
    pub fn sample(&mut self) {
        let snapshot = self.conn.snapshot();
        self.observe_coverage(snapshot.coverage);
        let paths = snapshot.paths;
        // A retired path ID is never reused by either pinned backend. Keep
        // only live baselines, bounding storage by concurrent observed paths.
        self.seen
            .retain(|id, _| paths.iter().any(|path| path.path_id == *id));
        let mut live = 0u64;
        for p in &paths {
            live += 1;
            let new = !self.seen.contains_key(&p.path_id);
            let prev = self.seen.get(&p.path_id).copied().unwrap_or_default();
            if new {
                self.count_new_path(p);
            }
            self.add_delta(
                p,
                p.sent.saturating_sub(prev.0),
                p.lost.saturating_sub(prev.1),
                p.sent_bytes.saturating_sub(prev.2),
                p.recv_bytes.saturating_sub(prev.3),
                p.congestion_events.saturating_sub(prev.4),
            );
            self.seen.insert(
                p.path_id,
                (
                    p.sent,
                    p.lost,
                    p.sent_bytes,
                    p.recv_bytes,
                    p.congestion_events,
                ),
            );
        }
        let selected = paths.iter().find(|p| p.selected);
        *self
            .registry
            .inner
            .selected_path
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = selected.map(|sel| SelectedPathSample {
            owner: self.sample_owner.clone(),
            rtt_us: sel.rtt.as_micros().min(u64::MAX.into()) as u64,
            cwnd_bytes: sel.cwnd,
        });
        self.registry
            .inner
            .live_paths
            .fetch_sub(self.last_live, Ordering::Relaxed);
        self.registry
            .inner
            .live_paths
            .fetch_add(live, Ordering::Relaxed);
        self.last_live = live;
    }

    fn observe_coverage(&mut self, coverage: PathStatsCoverage) {
        let (policy, degraded, lost) = match coverage {
            PathStatsCoverage::BackendSnapshot => (0, 0, 0),
            PathStatsCoverage::PolicyObserved {
                lost_events,
                driver_running,
            } => (
                1,
                u64::from(lost_events != 0 || !driver_running),
                lost_events,
            ),
        };
        let counters = &self.registry.inner;
        counters
            .policy_observed_connections
            .fetch_sub(self.last_policy, Ordering::Relaxed);
        counters
            .policy_observed_connections
            .fetch_add(policy, Ordering::Relaxed);
        counters
            .degraded_path_observers
            .fetch_sub(self.last_degraded, Ordering::Relaxed);
        counters
            .degraded_path_observers
            .fetch_add(degraded, Ordering::Relaxed);
        counters.path_events_lost.fetch_add(
            lost.saturating_sub(self.last_lost_events),
            Ordering::Relaxed,
        );
        self.last_policy = policy;
        self.last_degraded = degraded;
        self.last_lost_events = lost;
    }

    /// Sample every `interval` until the connection closes, with a
    /// final sample at teardown. Intended to run as a per-connection
    /// task beside the service loop. Closure wakes this wait immediately,
    /// independently of the sampling interval. No I/O handle spans the wait.
    pub async fn run(mut self, interval: Duration) {
        let closed = self.conn.closed();
        tokio::pin!(closed);
        loop {
            self.sample();
            tokio::select! {
                biased;
                _ = &mut closed => break,
                _ = tokio::time::sleep(interval) => {},
            }
        }
        self.sample();
    }

    fn count_new_path(&self, p: &PathStats) {
        let counter = if p.via_relay {
            &self.registry.inner.paths_seen_relay
        } else {
            &self.registry.inner.paths_seen_direct
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn add_delta(
        &self,
        p: &PathStats,
        sent: u64,
        lost: u64,
        sent_bytes: u64,
        recv_bytes: u64,
        cgt: u64,
    ) {
        let c = &self.registry.inner;
        c.congestion_events.fetch_add(cgt, Ordering::Relaxed);
        if p.via_relay {
            c.datagrams_sent_relay.fetch_add(sent, Ordering::Relaxed);
            c.datagrams_lost_relay.fetch_add(lost, Ordering::Relaxed);
            c.bytes_sent_relay.fetch_add(sent_bytes, Ordering::Relaxed);
            c.bytes_recv_relay.fetch_add(recv_bytes, Ordering::Relaxed);
        } else {
            c.datagrams_sent_direct.fetch_add(sent, Ordering::Relaxed);
            c.datagrams_lost_direct.fetch_add(lost, Ordering::Relaxed);
            c.bytes_sent_direct.fetch_add(sent_bytes, Ordering::Relaxed);
            c.bytes_recv_direct.fetch_add(recv_bytes, Ordering::Relaxed);
        }
    }
}

impl Drop for ConnSampler {
    fn drop(&mut self) {
        let mut selected = self
            .registry
            .inner
            .selected_path
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if selected
            .as_ref()
            .is_some_and(|sample| Arc::ptr_eq(&sample.owner, &self.sample_owner))
        {
            *selected = None;
        }
        drop(selected);
        self.registry
            .inner
            .policy_observed_connections
            .fetch_sub(self.last_policy, Ordering::Relaxed);
        self.registry
            .inner
            .degraded_path_observers
            .fetch_sub(self.last_degraded, Ordering::Relaxed);
        self.registry
            .inner
            .active_connections
            .fetch_sub(1, Ordering::Relaxed);
        self.registry
            .inner
            .live_paths
            .fetch_sub(self.last_live, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_selected_sample_does_not_block_scrapes_or_clear_the_observation() {
        let registry = Registry::default();
        registry.connection_opened();
        let mut held = registry.inner.selected_path.lock().unwrap();
        *held = Some(SelectedPathSample {
            owner: Arc::new(()),
            rtt_us: 25,
            cwnd_bytes: 64,
        });
        let source = registry.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || send.send(source.snapshot()).unwrap());
        let observed = receive.recv_timeout(Duration::from_secs(1));
        // Always release and join, including on the blocking baseline. A failed
        // regression must not leave a parked worker or hang the suite.
        drop(held);
        worker.join().unwrap();
        let values = observed.expect("snapshot waited for the transport observation lock");
        assert_eq!(values["rds_net_selected_path_known"], 0);
        assert_eq!(values["rds_net_rtt_us"], 0);
        assert_eq!(values["rds_net_cwnd_bytes"], 0);
        assert_eq!(values["rds_net_connections_opened_total"], 1);
        let values = registry.snapshot();
        assert_eq!(values["rds_net_selected_path_known"], 1);
        assert_eq!(values["rds_net_rtt_us"], 25);
        assert_eq!(values["rds_net_cwnd_bytes"], 64);
    }
}
