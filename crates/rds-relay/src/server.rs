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
//! [`Relay::drain`] asks clients to migrate: `Drain` is sent on every
//! control stream and new registrations are refused, so a replacement
//! relay can take over without dropping live paths abruptly.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use iroh::EndpointId;
use rds_net::backends::noq as rds_noq;
use rds_net::{EndpointConfig, SendStream};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;
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
        let msg = postcard::to_stdvec(&RelayControl::Drain).unwrap_or_default();
        let conns: Vec<std::sync::Arc<ConnSlot>> =
            self.state.conns.lock().unwrap().values().cloned().collect();
        for slot in conns {
            let msg = msg.clone();
            tokio::spawn(async move {
                let mut ctrl = slot.ctrl.lock().await;
                let _ = ctrl.write_all(&msg).await;
            });
        }
        tokio::time::sleep(DRAIN_GRACE).await;
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

    // Control stream: first bidi the client opens — bounded so a
    // connection that never registers costs seconds, not a parked task.
    let (mut ctrl_send, mut ctrl_recv) = tokio::time::timeout(REGISTER_TIMEOUT, conn.accept_bi())
        .await
        .context("control stream never opened")??;
    let hello = tokio::time::timeout(REGISTER_TIMEOUT, read_control(&mut ctrl_recv))
        .await
        .context("register read timed out")??;
    if !matches!(hello, RelayControl::Register) {
        conn.close(0u32.into(), b"expected register");
        bail!("{id} did not register");
    }
    write_control(&mut ctrl_send, &RelayControl::Registered).await?;

    let slot = std::sync::Arc::new(ConnSlot {
        conn: conn.clone(),
        ctrl: tokio::sync::Mutex::new(ctrl_send),
        bucket: Mutex::new(Bucket::new()),
    });
    // A second attachment for the same id replaces the first — a client
    // that re-registers invalidates its stale connection.
    if let Some(old) = state.conns.lock().unwrap().insert(id, slot.clone()) {
        old.conn.close(0u32.into(), b"replaced");
    }
    debug!(%id, "endpoint attached");

    // Control-reader: liveness and protocol errors only; forwarding
    // happens on datagrams below.
    let ctrl_slot = slot.clone();
    let ctrl_task = tokio::spawn(async move {
        loop {
            match read_control(&mut ctrl_recv).await {
                Ok(RelayControl::Ping { seq }) => {
                    let mut w = ctrl_slot.ctrl.lock().await;
                    if write_control(&mut w, &RelayControl::Pong { seq })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });

    // Datagram forward loop — ends when the connection does.
    let res = forward_loop(&conn, &state).await;

    ctrl_task.abort();
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

        let dst_slot = state.conns.lock().unwrap().get(&dst).cloned();
        let Some(slot) = dst_slot else {
            debug!(%src, %dst, "relay drop: unknown destination");
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };

        // Record the flow so `PeerGone` reaches the right senders.
        {
            let mut recent = state.recent.lock().unwrap();
            let peers = recent.entry(dst).or_default();
            if peers.len() < MAX_RECENT_PEERS {
                peers.insert(src);
            }
        }

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
    {
        let mut conns = state.conns.lock().unwrap();
        if conns
            .get(&id)
            .is_some_and(|s| std::sync::Arc::ptr_eq(s, slot))
        {
            conns.remove(&id);
        }
    }
    let peers = state.recent.lock().unwrap().remove(&id).unwrap_or_default();
    let msg = postcard::to_stdvec(&RelayControl::PeerGone {
        peer: *id.as_bytes(),
    })
    .unwrap_or_default();
    for peer in peers {
        let Some(slot) = state.conns.lock().unwrap().get(&peer).cloned() else {
            continue;
        };
        let msg = msg.clone();
        tokio::spawn(async move {
            let mut ctrl = slot.ctrl.lock().await;
            let _ = ctrl.write_all(&msg).await;
        });
    }
    debug!(%id, "endpoint detached");
}

/// Read one length-prefixed control frame (`u32 len` + postcard body).
async fn read_control(recv: &mut rds_net::RecvStream) -> anyhow::Result<RelayControl> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let n = u32::from_be_bytes(len) as usize;
    if n > 4096 {
        bail!("control frame too large: {n}");
    }
    let mut buf = vec![0u8; n];
    recv.read_exact(&mut buf).await?;
    Ok(postcard::from_bytes(&buf)?)
}

/// Write one length-prefixed control frame.
async fn write_control(send: &mut SendStream, msg: &RelayControl) -> anyhow::Result<()> {
    let body = postcard::to_stdvec(msg).context("encode control")?;
    send.write_all(&(body.len() as u32).to_be_bytes()).await?;
    send.write_all(&body).await?;
    send.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
