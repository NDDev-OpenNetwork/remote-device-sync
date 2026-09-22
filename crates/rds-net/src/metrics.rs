//! Transport metrics: a lightweight counter facade every backend feeds
//! and any owner (agent, bench harness, rds-server) can scrape.
//!
//! The model is deliberately small: an endpoint owns one [`Registry`]
//! of atomics; a [`ConnSampler`] diffs a connection's cumulative
//! `path_stats()` into it so relay-vs-direct accounting is exact even
//! when paths migrate. QNT attempt/success counters are driven by the
//! noq policy driver — on iroh they stay zero (`paths_seen{via=direct}`
//! appearing after a relay-only start is the equivalent signal).
//!
//! Export: [`Registry::render_prometheus`] emits the standard text
//! exposition format behind the `metrics` feature — no prometheus
//! dependency, just the counter names the bench reports cite.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{Connection, PathStats};

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
    /// Gauge: last-sampled selected-path RTT, microseconds.
    rtt_us: AtomicU64,
    /// Gauge: last-sampled selected-path congestion window, bytes.
    cwnd_bytes: AtomicU64,
    /// Gauge: live paths across sampled connections.
    live_paths: AtomicU64,
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
    /// connection closes.
    pub fn sampler(&self, conn: Connection) -> ConnSampler {
        self.inner
            .active_connections
            .fetch_add(1, Ordering::Relaxed);
        ConnSampler {
            registry: self.clone(),
            conn,
            seen: HashMap::new(),
            last_live: 0,
        }
    }

    /// Counter snapshot keyed by exposition name — what bench reports
    /// embed and `render_prometheus` serializes.
    pub fn snapshot(&self) -> BTreeMap<&'static str, u64> {
        let c = &*self.inner;
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
            ("rds_net_rtt_us", c.rtt_us.load(Ordering::Relaxed)),
            ("rds_net_cwnd_bytes", c.cwnd_bytes.load(Ordering::Relaxed)),
            ("rds_net_live_paths", c.live_paths.load(Ordering::Relaxed)),
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
/// and emits a final sample so nothing is lost at teardown.
pub struct ConnSampler {
    registry: Registry,
    conn: Connection,
    /// path_id → (sent, lost, sent_bytes, recv_bytes, congestion_events)
    /// at the last sample.
    seen: HashMap<u64, (u64, u64, u64, u64, u64)>,
    last_live: u64,
}

impl ConnSampler {
    /// Fold current `path_stats` into the registry. Safe to call any
    /// number of times — only deltas count.
    pub fn sample(&mut self) {
        let paths = self.conn.path_stats();
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
        if let Some(sel) = paths.iter().find(|p| p.selected).or(paths.first()) {
            self.registry
                .inner
                .rtt_us
                .store(sel.rtt.as_micros() as u64, Ordering::Relaxed);
            self.registry
                .inner
                .cwnd_bytes
                .store(sel.cwnd, Ordering::Relaxed);
        }
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

    /// Sample every `interval` until the connection closes, with a
    /// final sample at teardown. Intended to run as a per-connection
    /// task beside the service loop.
    pub async fn run(mut self, interval: Duration) {
        while !self.conn.is_closed() {
            self.sample();
            tokio::time::sleep(interval).await;
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
