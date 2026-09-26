//! Tagged uni-stream routing with explicit ownership and bounded pending work.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rds_core::UniHello;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use crate::{ConnectionInner, RecvStream};

const QUEUE_DEPTH: usize = 128;
const MAX_PENDING: usize = 64;
const MAX_ROUTES: usize = 32;
const TAG_TIMEOUT: Duration = Duration::from_secs(10);

/// Local implementation budgets, not negotiated wire capabilities.
#[derive(Clone, Copy, Debug)]
pub struct UniRoutingStats {
    /// Live registered inboxes, including distinct file-transfer IDs.
    pub routes: usize,
    /// Maximum live inboxes on one connection.
    pub max_routes: usize,
    /// Tasks reading a tag or waiting to hand a stream to its inbox.
    pub pending: usize,
    /// Maximum simultaneous tag/queue-handoff tasks for this connection.
    pub max_pending: usize,
    /// Buffered streams allowed in each service-kind inbox.
    pub queue_depth: usize,
}

/// An inbox is an I/O owner: it can outlive the facade Connection and keeps
/// routing alive until dropped. The task itself holds only a weak owner.
pub struct UniStreams {
    kind: UniHello,
    rx: mpsc::Receiver<RecvStream>,
    // Drop the receiver before releasing ownership, waking blocked senders.
    _owner: Arc<Demux>,
}

impl UniStreams {
    pub fn kind(&self) -> UniHello {
        self.kind
    }

    /// Next stream, or None when the connection and queued streams end.
    pub async fn recv(&mut self) -> Option<RecvStream> {
        self.rx.recv().await
    }
}

impl Drop for UniStreams {
    fn drop(&mut self) {
        self.rx.close();
        let mut state = self._owner.state.lock().unwrap_or_else(|p| p.into_inner());
        // A new claim may already have replaced this closed channel.
        if state
            .routes
            .get(&self.kind)
            .is_some_and(|tx| tx.is_closed())
        {
            state.routes.remove(&self.kind);
        }
    }
}

#[derive(Default)]
pub(super) struct Demux {
    state: Mutex<State>,
    pending: Arc<AtomicUsize>,
}

#[derive(Default)]
struct State {
    routes: HashMap<UniHello, mpsc::Sender<RecvStream>>,
    task: Option<JoinHandle<()>>,
}

impl Demux {
    pub fn claim(
        self: &Arc<Self>,
        conn: &ConnectionInner,
        kind: UniHello,
    ) -> anyhow::Result<UniStreams> {
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        if !conn.is_closed() {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.routes.retain(|_, tx| !tx.is_closed());
            if state.routes.get(&kind).is_some_and(|s| !s.is_closed()) {
                anyhow::bail!("uni stream kind {kind:?} already claimed");
            }
            if state.routes.len() >= MAX_ROUTES {
                anyhow::bail!("uni route capacity reached");
            }
            state.routes.insert(kind, tx);
            if state.task.is_none() {
                state.task = Some(tokio::spawn(run(
                    conn.clone(),
                    Arc::downgrade(self),
                    self.pending.clone(),
                )));
            }
        }
        // A claim on a closed connection returns an already-ended inbox.
        Ok(UniStreams {
            kind,
            rx,
            _owner: self.clone(),
        })
    }

    pub fn stats(&self) -> UniRoutingStats {
        UniRoutingStats {
            routes: self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .routes
                .len(),
            max_routes: MAX_ROUTES,
            pending: self.pending.load(Ordering::Relaxed),
            max_pending: MAX_PENDING,
            queue_depth: QUEUE_DEPTH,
        }
    }
}

impl Drop for Demux {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|p| p.into_inner());
        if let Some(task) = state.task.take() {
            // Dropping the driver's JoinSet aborts all remaining route tasks.
            // No task owns an Arc<Demux> across an await, so this is reachable.
            task.abort();
        }
    }
}

struct Lifetime(Weak<Demux>);

impl Drop for Lifetime {
    fn drop(&mut self) {
        if let Some(owner) = self.0.upgrade() {
            let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
            state.routes.clear();
            state.task = None;
        }
    }
}

struct Pending(Arc<AtomicUsize>);

impl Pending {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter.clone())
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn run(conn: ConnectionInner, owner: Weak<Demux>, pending: Arc<AtomicUsize>) {
    let _lifetime = Lifetime(owner.clone());
    let mut routes = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = conn.closed() => break,
            done = routes.join_next(), if !routes.is_empty() => {
                if let Some(Err(error)) = done {
                    tracing::debug!(%error, "uni route task ended");
                }
            }
            stream = conn.accept_uni(), if routes.len() < MAX_PENDING => {
                let Ok(stream) = stream else { break; };
                let owner = owner.clone();
                let pending = Pending::new(&pending);
                routes.spawn(async move {
                    let _pending = pending;
                    route(stream, owner).await;
                });
            }
        }
    }
    // Connection death cancels even tasks parked on a full service inbox.
    // Join before ending the inboxes; no late sender can reopen a route.
    routes.shutdown().await;
}

async fn route(mut stream: RecvStream, owner: Weak<Demux>) {
    let kind = match tokio::time::timeout(
        TAG_TIMEOUT,
        rds_core::read_frame::<_, UniHello>(&mut stream),
    )
    .await
    {
        Ok(Ok(kind)) => kind,
        Ok(Err(error)) => {
            tracing::debug!(%error, "uni stream dropped: unreadable tag");
            return;
        }
        Err(_) => {
            tracing::debug!("uni stream dropped: tag timeout");
            return;
        }
    };
    let tx = owner.upgrade().and_then(|owner| {
        owner
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .routes
            .get(&kind)
            .cloned()
    });
    if let Some(tx) = tx {
        // Keep backpressure for lossless sync. Only MAX_PENDING workers can
        // wait here; the transport's stream credit bounds further admissions.
        if tx.send(stream).await.is_err()
            && let Some(owner) = owner.upgrade()
        {
            let mut state = owner.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.routes.get(&kind).is_some_and(|t| t.same_channel(&tx)) {
                state.routes.remove(&kind);
            }
        }
    } else {
        tracing::debug!("uni {kind:?} stream dropped: no consumer");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Backend, EndpointConfig, bind_endpoint};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saturated_inbox_cancels_and_joins_pending_handoffs() {
        let backends = [
            Backend::Iroh,
            #[cfg(feature = "transport-noq")]
            Backend::Noq,
        ];
        for backend in backends {
            let mut phase = "bind client";
            let started = std::time::Instant::now();
            let mut observed = (0, 0);
            tokio::time::timeout(Duration::from_secs(5), async {
                let config = || EndpointConfig {
                    backend,
                    discovery: false,
                    bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
                    ..Default::default()
                };
                let a = bind_endpoint(config()).await.unwrap();
                phase = "bind server";
                let b = bind_endpoint(config()).await.unwrap();
                phase = "handshake";
                let (conn, peer) = tokio::join!(a.connect(b.addr(), rds_core::ALPN), async {
                    b.accept().await.unwrap().await
                });
                let conn = conn.unwrap();
                let peer = peer.unwrap();
                // Raise only the fixture's QUIC credit so the application
                // budget is tested beyond the transport's default 100 streams.
                match &conn.inner {
                    ConnectionInner::Iroh(raw) => raw.set_max_concurrent_uni_streams(512u32.into()),
                    #[cfg(feature = "transport-noq")]
                    ConnectionInner::Noq(raw) => {
                        raw.inner().set_max_concurrent_uni_streams(512u32.into())
                    }
                }
                let mut inbox = conn.uni_streams(UniHello::Sync).unwrap();
                let mut streams = Vec::new();
                phase = "send stream tags";
                for _ in 0..QUEUE_DEPTH + MAX_PENDING + 8 {
                    let mut send = peer.open_uni().await.unwrap();
                    rds_core::write_frame(&mut send, &UniHello::Sync)
                        .await
                        .unwrap();
                    send.write_all(b"pending body").await.unwrap();
                    streams.push(send);
                }
                phase = "saturate inbox and pending workers";
                loop {
                    observed = (inbox.rx.len(), conn.uni_routing_stats().pending);
                    if observed == (QUEUE_DEPTH, MAX_PENDING) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                phase = "cancel route workers";
                conn.close(0u32.into(), b"close with full inbox");
                while conn.uni_routing_stats().pending != 0 {
                    tokio::task::yield_now().await;
                }
                // Existing bounded inbox contents may drain after close; no
                // blocked sender survives to refill or keep the channel open.
                let mut drained = 0;
                phase = "drain closed inbox";
                while inbox.recv().await.is_some() {
                    drained += 1;
                }
                assert_eq!(drained, QUEUE_DEPTH);
                drop(streams);
                phase = "close client endpoint";
                a.close().await;
                phase = "close server endpoint";
                b.close().await;
            })
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{backend:?} full-inbox fixture timed out at {phase}; \
                     last queue/pending={observed:?}, elapsed={:?}: {error}",
                    started.elapsed()
                )
            });
        }
    }
}
