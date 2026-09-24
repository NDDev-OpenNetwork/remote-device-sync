//! Owned relay server (WS2).
//!
//! One mutually-authenticated QUIC connection per endpoint (ALPN
//! [`proto::RELAY_ALPN`]). Endpoint identity is the TLS-verified
//! `EndpointId` — registration is the act of attaching; there is no
//! key material in the protocol to spoof.
//!
//! Datagrams carry `[32B dst_key][payload]` client→relay and are
//! re-emitted as `[32B src_key][payload]` to the destination's own
//! connection. Unknown destinations and malformed frames are dropped;
//! per-source token buckets bound flood cost.
//!
//! [`Relay::drain`] broadcasts bounded, framed notices and refuses new
//! registrations. Existing tunnels stay usable through the grace period.
//! Automatic warm replacement and interruption-free migration remain separate
//! work; a notice alone cannot keep a relay-only session alive after shutdown.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use iroh::EndpointId;
use rds_net::backends::noq as rds_noq;
use rds_net::relay_control::{read_control, write_control};
use rds_net::{EndpointConfig, SendStream};
use tokio::task::{JoinHandle, JoinSet};
use tracing::debug;

use crate::proto::{self, RelayControl};

/// Per-source datagram rate limit: steady bytes/sec and burst allowance.
/// Forwarded frames are outer QUIC packets; the cap only bounds abuse —
/// real paths migrate off the relay once direct paths exist.
const RATE_BYTES_PER_SEC: f64 = 64.0 * 1024.0 * 1024.0;
const RATE_BURST: f64 = 4.0 * 1024.0 * 1024.0;
/// How many recent peers to notify on disconnect.
const MAX_RECENT_PEERS: usize = 64;
/// Grace between `Drain` broadcast and closing the listener.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// A connection that never opens its control stream and registers
/// parks a task otherwise — bounded, like the agent's stream hello.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_NOTICE_TASKS: usize = 16;

/// A running owned relay. Dropping it leaves tasks detached; call
/// [`Relay::close`] for a clean stop or [`Relay::drain`] for a graceful
/// handover.
pub struct Relay {
    endpoint: rds_noq::Endpoint,
    state: std::sync::Arc<State>,
    accept_task: JoinHandle<()>,
}

/// Live counters, cheap to snapshot for health/metrics.
#[derive(Debug, Default)]
pub struct Stats {
    /// Datagrams forwarded.
    pub forwarded: AtomicU64,
    /// Datagrams dropped (unknown dst, malformed, rate limited).
    pub dropped: AtomicU64,
    /// Bytes forwarded.
    pub bytes: AtomicU64,
}

struct ConnSlot {
    conn: rds_noq::Connection,
    /// Control stream writer for `Drain`/`PeerGone`/liveness.
    ctrl: tokio::sync::Mutex<SendStream>,
    /// Token bucket for source rate limiting.
    bucket: Mutex<Bucket>,
}

/// Per-source token bucket bounding datagram flood cost.
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn new() -> Self {
        Self {
            tokens: RATE_BURST,
            last: Instant::now(),
        }
    }

    /// Refill to `now`, then take `bytes` if affordable.
    fn take(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens
            + now.duration_since(self.last).as_secs_f64() * RATE_BYTES_PER_SEC)
            .min(RATE_BURST);
        self.last = now;
        if self.tokens >= bytes as f64 {
            self.tokens -= bytes as f64;
            true
        } else {
            false
        }
    }
}

struct State {
    conns: Mutex<HashMap<EndpointId, std::sync::Arc<ConnSlot>>>,
    /// dst → sources that recently forwarded to it; drives `PeerGone`.
    recent: Mutex<HashMap<EndpointId, HashSet<EndpointId>>>,
    allow: Option<HashSet<EndpointId>>,
    draining: AtomicBool,
    stats: Stats,
}

impl Relay {
    /// Bound socket address of the relay endpoint.
    pub fn local_addr(&self) -> SocketAddr {
        self.endpoint.local_addr()
    }

    /// The relay's own endpoint identity — what clients dial.
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// The relay's advertised endpoint address (identity + socket).
    pub fn endpoint_addr(&self) -> iroh::EndpointAddr {
        self.endpoint.addr()
    }

    /// Live counter snapshot.
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.state.stats.forwarded.load(Ordering::Relaxed),
            self.state.stats.dropped.load(Ordering::Relaxed),
            self.state.stats.bytes.load(Ordering::Relaxed),
        )
    }

    /// Attached endpoints right now.
    pub fn endpoints(&self) -> usize {
        self.state.conns.lock().unwrap().len()
    }

    /// Graceful drain: tell every client to migrate, refuse new
    /// registrations, then close after [`DRAIN_GRACE`].
    pub async fn drain(&self) {
        if self.state.draining.swap(true, Ordering::SeqCst) {
            return;
        }
        let deadline = tokio::time::Instant::now() + DRAIN_GRACE;
        let conns: Vec<std::sync::Arc<ConnSlot>> =
            self.state.conns.lock().unwrap().values().cloned().collect();
        let _ = tokio::time::timeout_at(deadline, notify_all(conns, RelayControl::Drain)).await;
        tokio::time::sleep_until(deadline).await;
        self.close().await;
    }

    /// Stop the relay: close all connections and the listener.
    pub async fn close(&self) {
        for (_, slot) in self.state.conns.lock().unwrap().drain() {
            slot.conn.close(0u32.into(), b"relay closed");
        }
        self.endpoint.close().await;
        self.accept_task.abort();
    }
}

/// Bind and run an owned relay server.
///
/// `allow` restricts which endpoint ids may attach (empty = open relay).
/// The relay sees only `EndpointId`s and byte counts — payload is
/// end-to-end encrypted QUIC it cannot read.
pub async fn serve(config: EndpointConfig, allow: Vec<EndpointId>) -> anyhow::Result<Relay> {
    let mut config = config;
    config.alpns = vec![proto::RELAY_ALPN.to_vec()];
    let endpoint = rds_noq::bind_endpoint(config).await?;

    let state = std::sync::Arc::new(State {
        conns: Mutex::new(HashMap::new()),
        recent: Mutex::new(HashMap::new()),
        allow: (!allow.is_empty()).then(|| allow.into_iter().collect()),
        draining: AtomicBool::new(false),
        stats: Stats::default(),
    });

    let accept_task = tokio::spawn(accept_loop(endpoint.clone(), state.clone()));
    Ok(Relay {
        endpoint,
        state,
        accept_task,
    })
}

async fn accept_loop(endpoint: rds_noq::Endpoint, state: std::sync::Arc<State>) {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    if let Err(e) = serve_conn(conn, state).await {
                        debug!("relay conn ended: {e:#}");
                    }
                }
                Err(e) => debug!("relay handshake failed: {e:#}"),
            }
        });
    }
}

/// One attached endpoint: control handshake, then the datagram loop.
async fn serve_conn(conn: rds_noq::Connection, state: std::sync::Arc<State>) -> anyhow::Result<()> {
    let id = conn.remote_id();

    if state.draining.load(Ordering::SeqCst) {
        conn.close(0u32.into(), b"draining");
        bail!("refused {id}: draining");
    }
    if let Some(allow) = &state.allow
        && !allow.contains(&id)
    {
        conn.close(0u32.into(), b"not allowed");
        bail!("refused {id}: not on allowlist");
    }

    let _lifetime = CloseConnection(Some(&conn));
    // One registration budget includes stream credit, read and ACK write.
    let (ctrl_send, mut ctrl_recv) = tokio::time::timeout(REGISTER_TIMEOUT, async {
        let (mut send, mut recv) = conn.accept_bi().await?;
        if !matches!(read_control(&mut recv).await?, RelayControl::Register) {
            bail!("expected relay registration");
        }
        write_control(&mut send, &RelayControl::Registered).await?;
        Ok::<_, anyhow::Error>((send, recv))
    })
    .await
    .context("relay registration timed out")??;

    let slot = std::sync::Arc::new(ConnSlot {
        conn: conn.clone(),
        ctrl: tokio::sync::Mutex::new(ctrl_send),
        bucket: Mutex::new(Bucket::new()),
    });
    // A second attachment for the same id replaces the first — a client
    // that re-registers invalidates its stale connection.
    {
        let mut conns = state.conns.lock().unwrap();
        // Serialize registration with the drain snapshot. An in-flight ACK
        // must not attach a new slot after the draining snapshot was taken.
        if state.draining.load(Ordering::SeqCst) {
            bail!("relay draining");
        }
        if let Some(old) = conns.insert(id, slot.clone()) {
            old.conn.close(0u32.into(), b"replaced");
        }
    }
    debug!(%id, "endpoint attached");

    // Control and forwarding share this connection future. Neither survives
    // the other or leaves a detached reader retaining the control writer.
    let res = tokio::select! {
        result = forward_loop(&conn, &state) => result,
        result = async {
            loop {
                match read_control(&mut ctrl_recv).await? {
                    RelayControl::Ping { seq } => {
                        if !write_notice(&slot, &RelayControl::Pong { seq }).await {
                            bail!("relay control reply failed");
                        }
                    }
                    RelayControl::Pong { .. } => {}
                    _ => bail!("unexpected client relay control"),
                }
            }
        } => result,
    };
    conn.close(0u32.into(), b"relay session ended");
    detach(id, &slot, &state).await;
    res
}

/// Move datagrams: decode dst key, rate-limit the source, forward.
async fn forward_loop(conn: &rds_noq::Connection, state: &State) -> anyhow::Result<()> {
    let src = conn.remote_id();
    loop {
        let frame = conn.read_datagram().await?;
        let Some((dst_raw, payload)) = proto::decode_frame(&frame) else {
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let Ok(dst) = EndpointId::from_bytes(&dst_raw) else {
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };

        if !rate_ok(state, &src, frame.len()) {
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let dst_slot = {
            let conns = state.conns.lock().unwrap();
            conns.get(&dst).cloned().inspect(|_| {
                // Use the same lock order as detach: a departed destination
                // cannot have its recent-flow entry recreated by a late send.
                let mut recent = state.recent.lock().unwrap();
                let peers = recent.entry(dst).or_default();
                if peers.len() < MAX_RECENT_PEERS {
                    peers.insert(src);
                }
            })
        };
        let Some(slot) = dst_slot else {
            debug!(%src, %dst, "relay drop: unknown destination");
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };

        tracing::trace!(%src, %dst, len = payload.len(), "relay forward");
        let out = proto::encode_forward(src.as_bytes(), payload);
        match slot.conn.send_datagram(out) {
            Ok(()) => {
                state.stats.forwarded.fetch_add(1, Ordering::Relaxed);
                state
                    .stats
                    .bytes
                    .fetch_add(payload.len() as u64, Ordering::Relaxed);
            }
            Err(_) => {
                state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Token-bucket admission for one source.
fn rate_ok(state: &State, src: &EndpointId, bytes: usize) -> bool {
    let Some(slot) = state.conns.lock().unwrap().get(src).cloned() else {
        return false;
    };
    slot.bucket.lock().unwrap().take(bytes)
}

/// Remove a detached endpoint — only if the stored slot is still the
/// one this connection owned — and notify peers that talked to it. A
/// re-registered replacement must not be evicted by the stale conn's
/// cleanup.
async fn detach(id: EndpointId, slot: &std::sync::Arc<ConnSlot>, state: &State) {
    let recipients = {
        let mut conns = state.conns.lock().unwrap();
        if !conns
            .get(&id)
            .is_some_and(|stored| std::sync::Arc::ptr_eq(stored, slot))
        {
            // A stale owner cannot erase successor history or emit PeerGone.
            return;
        }
        conns.remove(&id);
        let peers = state.recent.lock().unwrap().remove(&id).unwrap_or_default();
        peers
            .into_iter()
            .filter_map(|peer| conns.get(&peer).cloned())
            .collect()
    };
    notify_all(
        recipients,
        RelayControl::PeerGone {
            peer: *id.as_bytes(),
        },
    )
    .await;
    debug!(%id, "endpoint detached");
}

/// Close on cancellation/error so a partial control frame is never followed by
/// another writer's frame. A successful complete write disarms this guard.
struct CloseConnection<'a>(Option<&'a rds_noq::Connection>);
impl Drop for CloseConnection<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.0 {
            conn.close(0u32.into(), b"relay control ended");
        }
    }
}

async fn write_notice(slot: &ConnSlot, message: &RelayControl) -> bool {
    let mut pending = CloseConnection(Some(&slot.conn));
    let result = tokio::time::timeout(CONTROL_WRITE_TIMEOUT, async {
        let mut writer = slot.ctrl.lock().await;
        write_control(&mut *writer, message).await
    })
    .await;
    if matches!(result, Ok(Ok(()))) {
        pending.0 = None;
        true
    } else {
        false
    }
}

/// Bound concurrent notices and own their cancellation; no detached writes.
async fn notify_all(slots: Vec<std::sync::Arc<ConnSlot>>, message: RelayControl) {
    let mut remaining = slots.into_iter();
    let mut writes = JoinSet::new();
    loop {
        while writes.len() < MAX_NOTICE_TASKS {
            let Some(slot) = remaining.next() else {
                break;
            };
            let message = message.clone();
            writes.spawn(async move { write_notice(&slot, &message).await });
        }
        if writes.is_empty() {
            break;
        }
        if let Some(Err(error)) = writes.join_next().await {
            debug!(%error, "relay notice task ended");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn interrupted_control_write_closes_connection_even_while_waiting_for_lock() {
        for cancel_early in [false, true] {
            let config = || EndpointConfig {
                backend: rds_net::Backend::Noq,
                discovery: false,
                bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                ..Default::default()
            };
            let server = rds_noq::bind_endpoint(config()).await.unwrap();
            let client = rds_noq::bind_endpoint(config()).await.unwrap();
            let (outgoing, incoming) =
                tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
                    server.accept().await.unwrap().await
                });
            let peer = outgoing.unwrap();
            let conn = incoming.unwrap();
            let (send, _recv) = conn.open_bi().await.unwrap();
            let slot = ConnSlot {
                conn,
                ctrl: tokio::sync::Mutex::new(send),
                bucket: Mutex::new(Bucket::new()),
            };
            let held = slot.ctrl.lock().await;
            let budget = if cancel_early {
                Duration::from_millis(20)
            } else {
                Duration::from_secs(2)
            };
            let result =
                tokio::time::timeout(budget, write_notice(&slot, &RelayControl::Drain)).await;
            if cancel_early {
                assert!(result.is_err());
            } else {
                assert!(!result.unwrap());
            }
            tokio::time::timeout(Duration::from_secs(2), peer.inner().closed())
                .await
                .unwrap();
            drop(held);
            client.close().await;
            server.close().await;
        }
    }

    #[test]
    fn bucket_allows_burst_then_limits() {
        let mut bucket = Bucket::new();
        // One full burst is admitted.
        assert!(bucket.take(RATE_BURST as usize));
        // A second burst is not — the bucket is drained.
        assert!(!bucket.take(RATE_BURST as usize));
    }

    #[test]
    fn bucket_refills_at_steady_rate() {
        let mut bucket = Bucket::new();
        assert!(bucket.take(RATE_BURST as usize));
        std::thread::sleep(Duration::from_millis(10));
        // ~10ms of refill at RATE_BYTES_PER_SEC is admissible again.
        let refill = (RATE_BYTES_PER_SEC * 0.005) as usize;
        assert!(bucket.take(refill));
        // But never another full burst.
        assert!(!bucket.take(RATE_BURST as usize));
    }
}
