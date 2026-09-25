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
//! per-source token buckets bound admitted parsing/forwarding work.
//!
//! [`Relay::drain`] broadcasts bounded, framed notices and refuses new
//! registrations. Existing tunnels stay usable through the grace period.
//! Automatic warm replacement and interruption-free migration remain separate
//! work; a notice alone cannot keep a relay-only session alive after shutdown.

use std::collections::{HashMap, HashSet};
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use iroh::EndpointId;
use rds_net::backends::noq as rds_noq;
use rds_net::relay_control::{read_control, write_control};
use rds_net::{EndpointConfig, SendStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::debug;

use crate::proto::{self, RelayControl};

/// Per-source admission units: larger frames cost their byte length; every
/// frame has a minimum cost so tiny/malformed input cannot buy free parsing.
/// This bounds admitted application work, not QUIC ingress/decryption cost.
const RATE_UNITS_PER_SEC: f64 = 64.0 * 1024.0 * 1024.0;
const RATE_BURST_UNITS: f64 = 4.0 * 1024.0 * 1024.0;
const MIN_FRAME_CHARGE: usize = 1024;
/// How many recent peers to notify on disconnect.
const MAX_RECENT_PEERS: usize = 64;
/// Grace between `Drain` broadcast and closing the listener.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// A connection that never opens its control stream and registers
/// parks a task otherwise — bounded, like the agent's stream hello.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_NOTICE_WRITES: usize = 16;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-server application admission budget, including handshakes, registration
/// and final notification cleanup. Underlying QUIC buffers are separate.
#[derive(Debug, Clone, Copy)]
pub struct ServerLimits {
    pub max_connections: NonZeroU16,
}
impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_connections: NonZeroU16::new(256).expect("positive limit"),
        }
    }
}

/// Individual concurrent snapshots of application admission and history.
#[derive(Debug, Clone, Copy)]
pub struct LifecycleStats {
    pub connection_limit: usize,
    pub active_connections: usize,
    pub rejected_connections: u64,
    pub history_entries: usize,
    pub history_edges: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shutdown {
    Running,
    Drain,
    Stop,
}

/// A running owned relay. Drop requests shutdown and closes attached tunnels;
/// [`Relay::close`] also joins the server runner and its child tasks. Use
/// [`Relay::drain`] to preserve a bounded grace period before shutdown.
pub struct Relay {
    endpoint: rds_noq::Endpoint,
    state: std::sync::Arc<State>,
    shutdown: watch::Sender<Shutdown>,
    // Keep the handle stored while awaiting: canceling close must not lose
    // ownership or prevent another caller from joining the same runner.
    accept_task: tokio::sync::Mutex<Runner>,
    connections: ConnectionTasks,
}

type ConnectionTasks = std::sync::Arc<tokio::sync::Mutex<JoinSet<()>>>;

struct Runner {
    task: Option<JoinHandle<()>>,
    outcome: Option<Result<(), ShutdownError>>,
}

impl Runner {
    async fn wait(&mut self) {
        if let Some(task) = self.task.as_mut() {
            let outcome = task
                .await
                .map_err(|error| ShutdownError(std::sync::Arc::new(error)));
            // No await between consuming the handle and retaining its result.
            self.task = None;
            self.outcome = Some(outcome);
        }
    }

    fn result(&self) -> Result<(), ShutdownError> {
        // Construction starts with a handle, and wait saves its outcome.
        self.outcome.clone().expect("runner outcome retained")
    }
}

/// The owned relay runner failed; cleanup still joins its retained children.
#[derive(Debug, Clone)]
pub struct ShutdownError(std::sync::Arc<tokio::task::JoinError>);

impl std::fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "owned relay runner failed: {}", self.0)
    }
}
impl std::error::Error for ShutdownError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
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

/// Per-source admitted datagram work. Charged before frame or key validation.
struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    fn new() -> Self {
        Self {
            tokens: RATE_BURST_UNITS,
            last: Instant::now(),
        }
    }

    /// Refill, then charge a raw frame regardless of whether it is valid.
    fn take(&mut self, bytes: usize) -> bool {
        self.take_at(bytes, Instant::now())
    }

    fn take_at(&mut self, bytes: usize, now: Instant) -> bool {
        self.tokens = (self.tokens
            + now.duration_since(self.last).as_secs_f64() * RATE_UNITS_PER_SEC)
            .min(RATE_BURST_UNITS);
        self.last = now;
        let charge = bytes.max(MIN_FRAME_CHARGE) as f64;
        if self.tokens >= charge {
            self.tokens -= charge;
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
    limits: ServerLimits,
    admission: std::sync::Arc<Semaphore>,
    rejected: AtomicU64,
}

impl State {
    fn stop(&self) {
        self.draining.store(true, Ordering::SeqCst);
        let mut conns = self.conns.lock().unwrap();
        for (_, slot) in conns.drain() {
            slot.conn.close(0u32.into(), b"relay closed");
        }
        self.recent.lock().unwrap().clear();
    }
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

    pub fn lifecycle_stats(&self) -> LifecycleStats {
        let history = self.state.recent.lock().unwrap();
        let limit = usize::from(self.state.limits.max_connections.get());
        LifecycleStats {
            connection_limit: limit,
            active_connections: limit - self.state.admission.available_permits(),
            rejected_connections: self.state.rejected.load(Ordering::Relaxed),
            history_entries: history.len(),
            history_edges: history.values().map(HashSet::len).sum(),
        }
    }

    /// Graceful drain: tell every client to migrate, refuse new
    /// registrations, then close after [`DRAIN_GRACE`].
    pub async fn drain(&self) -> Result<(), ShutdownError> {
        self.state.draining.store(true, Ordering::SeqCst);
        self.shutdown.send_if_modified(|phase| {
            if *phase == Shutdown::Running {
                *phase = Shutdown::Drain;
                true
            } else {
                false
            }
        });
        self.join_runner().await
    }

    /// Stop and join all server-owned tasks. Concurrent/repeated callers join
    /// the same runner. Cancellation retains ownership; the normal runner
    /// continues cleanup, and another caller can resume a failed runner's
    /// fallback join. Runner failure is returned after child cleanup.
    pub async fn close(&self) -> Result<(), ShutdownError> {
        self.request_shutdown();
        self.join_runner().await
    }

    /// Observe runner termination without requesting shutdown. Cancellation
    /// preserves its handle/result. After this returns, call close/drain to
    /// join retained children even when the runner failed before cleanup.
    pub async fn wait_stopped(&self) -> Result<(), ShutdownError> {
        let mut runner = self.accept_task.lock().await;
        runner.wait().await;
        runner.result()
    }

    async fn join_runner(&self) -> Result<(), ShutdownError> {
        let mut runner = self.accept_task.lock().await;
        runner.wait().await;
        self.state.stop();
        self.endpoint.close().await;
        let mut connections = self.connections.lock().await;
        finish_connections(&mut connections).await;
        self.state.stop();
        runner.result()
    }

    fn request_shutdown(&self) {
        self.state.stop();
        self.shutdown.send_replace(Shutdown::Stop);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

/// Bind and run an owned relay server.
///
/// `allow` restricts which endpoint ids may attach (empty = open relay).
/// The relay sees only `EndpointId`s and byte counts — payload is
/// end-to-end encrypted QUIC it cannot read.
pub async fn serve(config: EndpointConfig, allow: Vec<EndpointId>) -> anyhow::Result<Relay> {
    serve_with_limits(config, allow, ServerLimits::default()).await
}

/// Bind with an explicit positive application connection budget.
pub async fn serve_with_limits(
    config: EndpointConfig,
    allow: Vec<EndpointId>,
    limits: ServerLimits,
) -> anyhow::Result<Relay> {
    let mut config = config;
    config.alpns = vec![proto::RELAY_ALPN.to_vec()];
    let endpoint = rds_noq::bind_endpoint(config).await?;

    let state = std::sync::Arc::new(State {
        conns: Mutex::new(HashMap::new()),
        recent: Mutex::new(HashMap::new()),
        allow: (!allow.is_empty()).then(|| allow.into_iter().collect()),
        draining: AtomicBool::new(false),
        stats: Stats::default(),
        limits,
        admission: std::sync::Arc::new(Semaphore::new(usize::from(limits.max_connections.get()))),
        rejected: AtomicU64::new(0),
    });

    let (shutdown, receiver) = watch::channel(Shutdown::Running);
    let connections = std::sync::Arc::new(tokio::sync::Mutex::new(JoinSet::new()));
    let accept_task = tokio::spawn(accept_loop(
        endpoint.clone(),
        state.clone(),
        receiver,
        connections.clone(),
    ));
    Ok(Relay {
        endpoint,
        state,
        shutdown,
        accept_task: tokio::sync::Mutex::new(Runner {
            task: Some(accept_task),
            outcome: None,
        }),
        connections,
    })
}

async fn accept_loop(
    endpoint: rds_noq::Endpoint,
    state: std::sync::Arc<State>,
    mut shutdown: watch::Receiver<Shutdown>,
    connections: ConnectionTasks,
) {
    let mut connections = connections.lock().await;
    // The runner owns grace and notice I/O even if a drain caller is canceled.
    let mut drain: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(std::future::pending());
    loop {
        tokio::select! {
            biased;
            change = shutdown.changed() => {
                if change.is_err() { break; }
                match *shutdown.borrow_and_update() {
                    Shutdown::Stop => break,
                    Shutdown::Drain => {
                        let deadline = tokio::time::Instant::now() + DRAIN_GRACE;
                        let conns = state.conns.lock().unwrap().values().cloned().collect();
                        drain = Box::pin(async move {
                            let _ = tokio::time::timeout_at(deadline, notify_all(conns, RelayControl::Drain)).await;
                            tokio::time::sleep_until(deadline).await;
                        });
                    }
                    Shutdown::Running => {}
                }
            }
            _ = &mut drain => break,
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    debug!(%error, "relay connection task ended");
                }
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break; };
                if state.draining.load(Ordering::SeqCst) {
                    drop(incoming);
                    state.rejected.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let Ok(permit) = state.admission.clone().try_acquire_owned() else {
                    drop(incoming);
                    state.rejected.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let state = state.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming).await {
                        Ok(Ok(conn)) => {
                            if let Err(error) = serve_conn(conn, state).await {
                                debug!(%error, "relay connection ended");
                            }
                        }
                        Ok(Err(error)) => debug!(%error, "relay handshake failed"),
                        Err(_) => debug!("relay handshake timed out"),
                    }
                });
            }
        }
    }
    drop(drain);
    state.stop();
    endpoint.close().await;
    finish_connections(&mut connections).await;
    state.stop();
}

async fn finish_connections(connections: &mut JoinSet<()>) {
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                debug!(%error, "relay connection task ended during shutdown");
            }
        }
    })
    .await
    .is_err()
    {
        connections.shutdown().await;
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
    let _registration = Registration {
        id,
        slot: slot.clone(),
        state: state.clone(),
    };
    debug!(%id, "endpoint attached");

    // Control and forwarding share this connection future. Neither survives
    // the other or leaves a detached reader retaining the control writer.
    let res = tokio::select! {
        result = forward_loop(&slot, &state) => result,
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

/// Move datagrams: charge raw input, look up an authenticated destination, forward.
async fn forward_loop(source: &std::sync::Arc<ConnSlot>, state: &State) -> anyhow::Result<()> {
    let conn = &source.conn;
    let src = conn.remote_id();
    let mut burst = 0usize;
    loop {
        if burst == 64 {
            tokio::task::yield_now().await;
            burst = 0;
        }
        burst += 1;
        let frame = conn.read_datagram().await?;
        // Even empty, short or invalid-key frames consume the source's work
        // budget. Exhaustion skips all frame parsing, key work and routing.
        if !source.bucket.lock().unwrap().take(frame.len()) {
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let Some((dst_raw, payload)) = proto::decode_frame(&frame) else {
            state.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let dst_slot = {
            let conns = state.conns.lock().unwrap();
            // A replaced/detached source cannot repopulate history after its
            // cleanup or borrow the successor's forwarding budget.
            if !conns
                .get(&src)
                .is_some_and(|current| std::sync::Arc::ptr_eq(current, source))
            {
                state.stats.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            // Keys in this table were validated by the attachment handshake.
            // EndpointId borrows its raw bytes for lookup: unknown/invalid keys
            // miss without curve decompression on each forwarded datagram.
            conns
                .get_key_value(&dst_raw)
                .map(|(dst, slot)| (*dst, slot.clone()))
                .inspect(|(dst, _)| {
                    // Use the same lock order as detach: a departed destination
                    // cannot have its recent-flow entry recreated by a late send.
                    let mut recent = state.recent.lock().unwrap();
                    let peers = recent.entry(*dst).or_default();
                    if peers.len() < MAX_RECENT_PEERS {
                        peers.insert(src);
                    }
                })
        };
        let Some((dst, slot)) = dst_slot else {
            debug!(%src, "relay drop: unknown destination");
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

/// Remove a detached endpoint — only if the stored slot is still the
/// one this connection owned — and notify peers that talked to it. A
/// re-registered replacement must not be evicted by the stale conn's
/// cleanup.
async fn detach(id: EndpointId, slot: &std::sync::Arc<ConnSlot>, state: &State) {
    let recipients = remove_registration(id, slot, state);
    notify_all(
        recipients,
        RelayControl::PeerGone {
            peer: *id.as_bytes(),
        },
    )
    .await;
    debug!(%id, "endpoint detached");
}

struct Registration {
    id: EndpointId,
    slot: std::sync::Arc<ConnSlot>,
    state: std::sync::Arc<State>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        // Cancellation cannot await notices, but must unlink owned state.
        remove_registration(self.id, &self.slot, &self.state);
    }
}

fn remove_registration(
    id: EndpointId,
    slot: &std::sync::Arc<ConnSlot>,
    state: &State,
) -> Vec<std::sync::Arc<ConnSlot>> {
    let mut conns = state.conns.lock().unwrap();
    if !conns
        .get(&id)
        .is_some_and(|stored| std::sync::Arc::ptr_eq(stored, slot))
    {
        // A stale owner cannot erase successor history or emit PeerGone.
        return Vec::new();
    }
    conns.remove(&id);
    let mut recent = state.recent.lock().unwrap();
    let peers = recent.remove(&id).unwrap_or_default();
    recent.retain(|_, sources| {
        sources.remove(&id);
        !sources.is_empty()
    });
    peers
        .into_iter()
        .filter_map(|peer| conns.get(&peer).cloned())
        .collect()
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

/// Poll at most 16 notice futures inline. Dropping this future destroys every
/// writer immediately; there are no separately spawned tasks to outlive it.
async fn notify_all(slots: Vec<std::sync::Arc<ConnSlot>>, message: RelayControl) {
    let mut remaining = slots.into_iter();
    let mut writes = Vec::new();
    loop {
        while writes.len() < MAX_NOTICE_WRITES {
            let Some(slot) = remaining.next() else {
                break;
            };
            let message = message.clone();
            writes.push(Box::pin(async move { write_notice(&slot, &message).await }));
        }
        if writes.is_empty() {
            break;
        }
        poll_fn(|cx| {
            let mut completed = false;
            for index in (0..writes.len()).rev() {
                if writes[index].as_mut().poll(cx).is_ready() {
                    drop(writes.swap_remove(index));
                    completed = true;
                }
            }
            if completed {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
    }
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod accounting_tests;

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
        let now = bucket.last;
        // One full burst is admitted.
        assert!(bucket.take_at(RATE_BURST_UNITS as usize, now));
        // A second burst is not — the bucket is drained.
        assert!(!bucket.take_at(RATE_BURST_UNITS as usize, now));
        assert_eq!(bucket.tokens, 0.0);
    }

    #[test]
    fn bucket_refills_at_steady_rate() {
        let mut bucket = Bucket::new();
        let now = bucket.last;
        assert!(bucket.take_at(RATE_BURST_UNITS as usize, now));
        let elapsed = Duration::from_millis(10);
        let refill = (RATE_UNITS_PER_SEC * elapsed.as_secs_f64()) as usize;
        assert!(bucket.take_at(refill, now + elapsed));
        // But never another full burst.
        assert!(!bucket.take_at(RATE_BURST_UNITS as usize, now + elapsed));
        // A long idle period cannot accumulate more than the burst allowance.
        assert!(bucket.take_at(RATE_BURST_UNITS as usize, now + Duration::from_secs(60)));
        assert!(!bucket.take_at(0, now + Duration::from_secs(60)));
    }

    #[test]
    fn empty_and_tiny_frames_have_a_positive_cost_and_a_finite_burst() {
        for size in [0, 1, 31, 32, MIN_FRAME_CHARGE - 1, MIN_FRAME_CHARGE] {
            let mut bucket = Bucket::new();
            let now = bucket.last;
            for _ in 0..RATE_BURST_UNITS as usize / MIN_FRAME_CHARGE {
                assert!(bucket.take_at(size, now));
            }
            assert!(!bucket.take_at(size, now));
            assert_eq!(bucket.tokens, 0.0);
        }
    }

    #[test]
    fn larger_frames_pay_their_actual_size_including_key_header() {
        let mut bucket = Bucket::new();
        let now = bucket.last;
        let size = proto::KEY_HEADER_LEN + proto::MAX_PAYLOAD;
        assert!(bucket.take_at(size, now));
        assert_eq!(bucket.tokens, RATE_BURST_UNITS - size as f64);
    }
}
