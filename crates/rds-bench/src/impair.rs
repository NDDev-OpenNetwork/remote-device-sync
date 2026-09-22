//! Deterministic UDP impairment proxy.
//!
//! One proxy sits between a client endpoint and an agent endpoint and
//! applies per-datagram loss, fixed delay, uniform jitter and a rate cap
//! to traffic in both directions. Randomness comes from a seeded PRNG so
//! a scenario is reproducible: same seed, same drop schedule.
//!
//! The proxy assumes one client; the last address that sent a datagram
//! toward the upstream is remembered as the downstream destination.

use std::collections::BinaryHeap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Link parameters applied to every datagram crossing the proxy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Impairment {
    /// Per-datagram drop probability, both directions, `0.0..=1.0`.
    pub loss: f64,
    /// Fixed one-way delay.
    pub delay_ms: u64,
    /// Uniform extra delay sampled from `0..=jitter_ms`.
    pub jitter_ms: u64,
    /// Throughput cap in megabits per second. `None` is unbounded.
    pub rate_mbps: Option<f64>,
    /// PRNG seed controlling the drop/jitter schedule.
    pub seed: u64,
}

impl Default for Impairment {
    fn default() -> Self {
        Self {
            loss: 0.0,
            delay_ms: 0,
            jitter_ms: 0,
            rate_mbps: None,
            seed: 1,
        }
    }
}

impl Impairment {
    /// A clean link: proxy still works, it just adds nothing.
    pub fn clean() -> Self {
        Self::default()
    }

    /// `tc netem`-style preset: 5% loss, 30 ms jitter on a 50 ms base.
    pub fn lossy() -> Self {
        Self {
            loss: 0.05,
            delay_ms: 50,
            jitter_ms: 30,
            rate_mbps: None,
            seed: 1,
        }
    }

    /// True when the link drops or delays nothing.
    pub fn is_clean(&self) -> bool {
        self.loss == 0.0 && self.delay_ms == 0 && self.jitter_ms == 0 && self.rate_mbps.is_none()
    }

    /// One-line spec for reports, e.g. `loss=5% delay=50ms jitter=30ms rate=10Mbps seed=7`.
    pub fn describe(&self) -> String {
        format!(
            "loss={:.1}% delay={}ms jitter={}ms rate={} seed={}",
            self.loss * 100.0,
            self.delay_ms,
            self.jitter_ms,
            self.rate_mbps
                .map(|r| format!("{r}Mbps"))
                .unwrap_or_else(|| "uncapped".into()),
            self.seed
        )
    }
}

/// SplitMix64: tiny deterministic PRNG, sufficient for drop schedules.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A datagram waiting for its release time.
struct Queued {
    release: Instant,
    seq: u64,
    dest: SocketAddr,
    data: Vec<u8>,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.release == other.release && self.seq == other.seq
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    /// Inverted: the heap pops the *earliest* release first.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .release
            .cmp(&self.release)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

/// Live proxy: bind address plus counters for the report.
pub struct Proxy {
    /// Address clients should use instead of the real upstream.
    pub listen: SocketAddr,
    /// Upstream address the proxy forwards to.
    pub upstream: SocketAddr,
    counters: Arc<Counters>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Counters {
    forwarded: AtomicU64,
    dropped: AtomicU64,
    bytes: AtomicU64,
}

/// Snapshot of what the proxy did — reported so a scenario can prove
/// impairment actually engaged (dropped > 0 when loss > 0).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ProxyStats {
    pub forwarded: u64,
    pub dropped: u64,
    pub bytes: u64,
}

impl Proxy {
    /// Stats so far.
    pub fn stats(&self) -> ProxyStats {
        ProxyStats {
            forwarded: self.counters.forwarded.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            bytes: self.counters.bytes.load(Ordering::Relaxed),
        }
    }

    /// Stop the proxy.
    pub fn stop(self) {
        self.task.abort();
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Spawn a proxy forwarding to `upstream` with `cfg` applied.
pub async fn spawn(upstream: SocketAddr, cfg: Impairment) -> std::io::Result<Proxy> {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
    let listen = sock.local_addr()?;
    let counters = Arc::new(Counters::default());
    let task = {
        let sock = sock.clone();
        let counters = counters.clone();
        tokio::spawn(run(sock, upstream, cfg, counters))
    };
    Ok(Proxy {
        listen,
        upstream,
        counters,
        task,
    })
}

/// Socket-level impairment (`transport-noq` only): an `AsyncUdpSocket`
/// decorator applying the same model underneath QUIC.
#[cfg(feature = "transport-noq")]
mod socket;
#[cfg(feature = "transport-noq")]
pub use socket::{ImpairingSocket, StatsHandle};

/// Delay queue + sender shared between the receive loop and dispatcher.
struct Pipe {
    heap: Mutex<BinaryHeap<Queued>>,
    notify: Notify,
    /// Serialization point for the rate limiter: the next instant the
    /// token bucket permits a byte on the wire.
    pace_cursor: Mutex<Instant>,
}

impl Pipe {
    fn new() -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::new()),
            notify: Notify::new(),
            pace_cursor: Mutex::new(Instant::now()),
        }
    }
}

async fn run(sock: Arc<UdpSocket>, upstream: SocketAddr, cfg: Impairment, counters: Arc<Counters>) {
    let pipe = Arc::new(Pipe::new());
    let dispatcher = tokio::spawn(dispatch(sock.clone(), pipe.clone(), cfg, counters.clone()));
    let mut rng = Rng(cfg.seed);
    let mut seq = 0u64;
    let mut client: Option<SocketAddr> = None;
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let Ok((n, src)) = sock.recv_from(&mut buf).await else {
            break;
        };
        // Demux: from upstream → destined for the last seen client;
        // anything else is (or becomes) the client.
        let dest = if src == upstream {
            match client {
                Some(c) => c,
                None => continue, // no client yet; drop
            }
        } else {
            client = Some(src);
            upstream
        };
        // Drop decision happens at enqueue so the schedule is seed-stable.
        if cfg.loss > 0.0 && rng.next_f64() < cfg.loss {
            counters.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let extra = if cfg.jitter_ms > 0 {
            Duration::from_millis((rng.next_f64() * cfg.jitter_ms as f64) as u64)
        } else {
            Duration::ZERO
        };
        seq += 1;
        pipe.heap.lock().unwrap().push(Queued {
            release: Instant::now() + Duration::from_millis(cfg.delay_ms) + extra,
            seq,
            dest,
            data: buf[..n].to_vec(),
        });
        pipe.notify.notify_one();
    }
    dispatcher.abort();
}

async fn dispatch(sock: Arc<UdpSocket>, pipe: Arc<Pipe>, cfg: Impairment, counters: Arc<Counters>) {
    loop {
        let (queued, wait) = {
            let mut heap = pipe.heap.lock().unwrap();
            match heap.pop() {
                Some(q) if q.release <= Instant::now() => (Some(q), Duration::ZERO),
                Some(q) => {
                    let wait = q.release - Instant::now();
                    heap.push(q);
                    (None, wait)
                }
                None => (None, Duration::MAX),
            }
        };
        let Some(q) = queued else {
            if wait == Duration::MAX {
                pipe.notify.notified().await;
            } else {
                // New items can arrive earlier than the current head.
                let _ = tokio::time::timeout(wait, pipe.notify.notified()).await;
            }
            continue;
        };
        // Rate pacing: byte-serial token bucket.
        if let Some(rate) = cfg.rate_mbps {
            let bytes_per_sec = rate * 1_000_000.0 / 8.0;
            let cost = Duration::from_secs_f64(q.data.len() as f64 / bytes_per_sec);
            let send_at = {
                let mut cursor = pipe.pace_cursor.lock().unwrap();
                let now = Instant::now();
                if *cursor < now {
                    *cursor = now;
                }
                let at = *cursor;
                *cursor += cost;
                at
            };
            tokio::time::sleep_until(send_at.into()).await;
        }
        if sock.send_to(&q.data, q.dest).await.is_ok() {
            counters.forwarded.fetch_add(1, Ordering::Relaxed);
            counters
                .bytes
                .fetch_add(q.data.len() as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clean_proxy_forwards() {
        let echo = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((n, src)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], src).await;
            }
        });

        let proxy = spawn(echo_addr, Impairment::clean()).await.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"ping", proxy.listen).await.unwrap();
        let mut buf = [0u8; 16];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"ping");
        assert_eq!(proxy.stats().forwarded, 2);
        proxy.stop();
    }

    #[tokio::test]
    async fn full_loss_drops_everything() {
        let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let proxy = spawn(
            sink.local_addr().unwrap(),
            Impairment {
                loss: 1.0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        for _ in 0..10 {
            client.send_to(b"x", proxy.listen).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let stats = proxy.stats();
        assert_eq!(stats.dropped, 10);
        assert_eq!(stats.forwarded, 0);
        proxy.stop();
    }

    #[tokio::test]
    async fn seeded_drop_schedule_is_reproducible() {
        // Same seed twice must produce identical drop counts for the
        // same packet stream — this is what makes scenarios replayable.
        async fn drops(seed: u64) -> u64 {
            let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let proxy = spawn(
                sink.local_addr().unwrap(),
                Impairment {
                    loss: 0.5,
                    seed,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            for _ in 0..100 {
                client.send_to(b"x", proxy.listen).await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let s = proxy.stats().dropped;
            proxy.stop();
            s
        }
        assert_eq!(drops(42).await, drops(42).await);
    }
}
