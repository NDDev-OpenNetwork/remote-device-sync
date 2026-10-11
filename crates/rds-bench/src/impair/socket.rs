//! Impairment as an [`AsyncUdpSocket`] decorator.
//!
//! The in-line [`Proxy`](super::Proxy) is the right tool for black-box
//! scenarios, but a QUIC stack that learns peer addresses in-band
//! (iroh holepunching, noq QNT) can migrate around it — the datagrams
//! simply stop crossing the proxy. Wrapped around a real socket
//! instead, impairment sits *underneath* the transport: every datagram
//! the endpoint emits is delayed, jittered or dropped no matter which
//! remote address the connection selects.
//!
//! Impairment applies to outbound datagrams only; wrap both endpoints'
//! sockets for a symmetric impaired link.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use noq::udp::{RecvMeta, Transmit};
use noq::{AsyncUdpSocket, UdpSender};
use tokio::task::JoinHandle;

use super::{Counters, Impairment, Pipe, ProxyStats, Queued, Rng};

/// Shared enqueue side: one seeded schedule per socket, so all senders
/// draw loss/jitter from a single deterministic stream.
struct Schedule {
    /// Live impairment config — [`StatsHandle::set_impairment`] swaps it
    /// mid-run so recovery scenarios can impose loss on an established
    /// connection rather than only from t=0.
    cfg: Arc<Mutex<Impairment>>,
    rng: Mutex<Rng>,
    seq: AtomicU64,
}

#[derive(Default)]
struct Timing {
    max_queue_depth: AtomicU64,
    max_release_lateness_us: AtomicU64,
    max_inner_send_us: AtomicU64,
}

/// Passive maxima from the impairment pump, distinct from sampled link delay.
/// Rate-limit waits contribute to release lateness; inner socket waits are
/// measured separately. Maxima may describe different packets and must not be
/// added. These observations do not measure the QUIC receiver's scheduling.
#[derive(Debug, Clone, Copy, Default)]
pub struct SocketTimingStats {
    pub max_queue_depth: u64,
    pub max_release_lateness_us: u64,
    pub max_inner_send_us: u64,
}

/// An [`AsyncUdpSocket`] that impairs every outbound datagram.
///
/// `poll_send` enqueues the datagram with a sampled release time and
/// reports success immediately; a pump task releases it to the inner
/// socket once the delay expires, or drops it per the loss schedule —
/// identical semantics to [`super::Proxy`], same seeded [`Rng`].
pub struct ImpairingSocket {
    inner: Box<dyn AsyncUdpSocket>,
    pipe: Arc<Pipe>,
    schedule: Arc<Schedule>,
    counters: Arc<Counters>,
    timing: Arc<Timing>,
    pump: JoinHandle<()>,
}

/// Live counters for an [`ImpairingSocket`]; cloneable so it survives
/// boxing the socket into the endpoint.
#[derive(Clone)]
pub struct StatsHandle {
    counters: Arc<Counters>,
    cfg: Arc<Mutex<Impairment>>,
    timing: Arc<Timing>,
}

impl StatsHandle {
    /// Datagrams actually released vs dropped so far — the proof a test
    /// needs that impairment engaged (`dropped > 0` when `loss > 0`).
    pub fn get(&self) -> ProxyStats {
        ProxyStats {
            forwarded: self.counters.forwarded.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            bytes: self.counters.bytes.load(Ordering::Relaxed),
        }
    }

    /// Bounded metadata only; no packet payload or new timer/task is retained.
    pub fn timing(&self) -> SocketTimingStats {
        SocketTimingStats {
            max_queue_depth: self.timing.max_queue_depth.load(Ordering::Relaxed),
            max_release_lateness_us: self.timing.max_release_lateness_us.load(Ordering::Relaxed),
            max_inner_send_us: self.timing.max_inner_send_us.load(Ordering::Relaxed),
        }
    }

    /// Replace the live impairment. Takes effect on the next datagram —
    /// already-queued packets keep their sampled fate.
    pub fn set_impairment(&self, cfg: Impairment) {
        *self.cfg.lock().unwrap() = cfg;
    }
}

impl ImpairingSocket {
    /// Wrap `inner` with `cfg` impairment. Spawns the pump on the
    /// current tokio runtime; returns the socket plus its stats handle.
    pub fn wrap(inner: Box<dyn AsyncUdpSocket>, cfg: Impairment) -> (Self, StatsHandle) {
        let pipe = Arc::new(Pipe::new());
        let counters = Arc::new(Counters::default());
        let timing = Arc::new(Timing::default());
        let schedule = Arc::new(Schedule {
            cfg: Arc::new(Mutex::new(cfg)),
            rng: Mutex::new(Rng(cfg.seed)),
            seq: AtomicU64::new(0),
        });
        let pump = tokio::spawn(dispatch(
            inner.create_sender(),
            pipe.clone(),
            schedule.clone(),
            counters.clone(),
            timing.clone(),
        ));
        let stats = StatsHandle {
            counters: counters.clone(),
            cfg: schedule.cfg.clone(),
            timing: timing.clone(),
        };
        (
            Self {
                inner,
                pipe,
                schedule,
                counters,
                timing,
                pump,
            },
            stats,
        )
    }
}

impl fmt::Debug for ImpairingSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImpairingSocket")
            .field("local_addr", &self.inner.local_addr().ok())
            .finish()
    }
}

impl Drop for ImpairingSocket {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

impl AsyncUdpSocket for ImpairingSocket {
    fn create_sender(&self) -> Pin<Box<dyn UdpSender>> {
        Box::pin(ImpairingSender {
            pipe: self.pipe.clone(),
            schedule: self.schedule.clone(),
            counters: self.counters.clone(),
            timing: self.timing.clone(),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_recv(cx, bufs, meta)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_receive_segments(&self) -> NonZeroUsize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

/// Send half: loss and release-time sampling happen at enqueue so the
/// schedule is seed-stable; the pump owns actual socket writes.
struct ImpairingSender {
    pipe: Arc<Pipe>,
    schedule: Arc<Schedule>,
    counters: Arc<Counters>,
    timing: Arc<Timing>,
}

impl fmt::Debug for ImpairingSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImpairingSender").finish()
    }
}

impl UdpSender for ImpairingSender {
    fn poll_send(
        self: Pin<&mut Self>,
        transmit: &Transmit<'_>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let sched = &self.schedule;
        let cfg = *sched.cfg.lock().unwrap();
        let mut rng = sched.rng.lock().unwrap();
        if cfg.loss > 0.0 && rng.next_f64() < cfg.loss {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Ok(()));
        }
        let extra = if cfg.jitter_ms > 0 {
            Duration::from_millis((rng.next_f64() * cfg.jitter_ms as f64) as u64)
        } else {
            Duration::ZERO
        };
        drop(rng);
        let seq = sched.seq.fetch_add(1, Ordering::Relaxed);
        let mut heap = self.pipe.heap.lock().unwrap();
        heap.push(Queued {
            release: Instant::now() + Duration::from_millis(cfg.delay_ms) + extra,
            seq,
            dest: transmit.destination,
            data: transmit.contents.to_vec(),
        });
        let depth = heap.len() as u64;
        drop(heap);
        self.timing
            .max_queue_depth
            .fetch_max(depth, Ordering::Relaxed);
        self.pipe.notify.notify_one();
        Poll::Ready(Ok(()))
    }

    /// Single datagram per transmit: the impairment model is
    /// per-datagram, so segmentation offload stays off.
    fn max_transmit_segments(&self) -> NonZeroUsize {
        NonZeroUsize::MIN
    }
}

/// Drain the delay heap into the inner socket. Mirrors the proxy's
/// dispatcher: earliest release first, byte-serial rate pacing.
async fn dispatch(
    sender: Pin<Box<dyn UdpSender>>,
    pipe: Arc<Pipe>,
    schedule: Arc<Schedule>,
    counters: Arc<Counters>,
    timing: Arc<Timing>,
) {
    let mut sender = sender;
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
                let _ = tokio::time::timeout(wait, pipe.notify.notified()).await;
            }
            continue;
        };
        let cfg = *schedule.cfg.lock().unwrap();
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
        let transmit = Transmit {
            destination: q.dest,
            ecn: None,
            contents: &q.data,
            segment_size: None,
            src_ip: None,
        };
        let started = Instant::now();
        timing.max_release_lateness_us.fetch_max(
            micros(started.saturating_duration_since(q.release)),
            Ordering::Relaxed,
        );
        let sent = std::future::poll_fn(|cx| sender.as_mut().poll_send(&transmit, cx)).await;
        timing
            .max_inner_send_us
            .fetch_max(micros(started.elapsed()), Ordering::Relaxed);
        if sent.is_ok() {
            counters.forwarded.fetch_add(1, Ordering::Relaxed);
            counters
                .bytes
                .fetch_add(q.data.len() as u64, Ordering::Relaxed);
        }
    }
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timing_observes_the_pump_without_changing_drop_or_payload_delivery() {
        let sink = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let inner = noq::Runtime::wrap_udp_socket(
            &noq::TokioRuntime,
            std::net::UdpSocket::bind("127.0.0.1:0").unwrap(),
        )
        .unwrap();
        let (socket, stats) = ImpairingSocket::wrap(
            inner,
            Impairment {
                loss: 1.0,
                ..Default::default()
            },
        );
        let mut sender = socket.create_sender();
        for byte in 0..5 {
            let contents = [byte];
            let transmit = Transmit {
                destination: sink.local_addr().unwrap(),
                ecn: None,
                contents: &contents,
                segment_size: None,
                src_ip: None,
            };
            std::future::poll_fn(|cx| sender.as_mut().poll_send(&transmit, cx))
                .await
                .unwrap();
        }
        assert_eq!(stats.get().dropped, 5);
        assert_eq!(stats.get().forwarded, 0);
        assert_eq!(stats.timing().max_queue_depth, 0);

        stats.set_impairment(Impairment {
            delay_ms: 25,
            ..Default::default()
        });
        for byte in 0..5 {
            let contents = [byte];
            let transmit = Transmit {
                destination: sink.local_addr().unwrap(),
                ecn: None,
                contents: &contents,
                segment_size: None,
                src_ip: None,
            };
            std::future::poll_fn(|cx| sender.as_mut().poll_send(&transmit, cx))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            for byte in 0..5 {
                let mut contents = [0; 2];
                let (size, _) = sink.recv_from(&mut contents).await.unwrap();
                assert_eq!(&contents[..size], &[byte]);
            }
            while stats.get().forwarded != 5 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(stats.get().dropped, 5);
        assert!((1..=5).contains(&stats.timing().max_queue_depth));
    }
}
