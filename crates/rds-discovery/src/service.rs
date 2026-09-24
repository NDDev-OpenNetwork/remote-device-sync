//! The discovery directory HTTP service.
//!
//! Routes (one request per connection, `Connection: close`):
//!
//! ```text
//! PUT    /v1/records            publish a signed EndpointRecord
//! GET    /v1/records/{key}      fetch the stored record JSON
//! DELETE /v1/records/{key}      body: signed DeleteRequest
//! GET    /v1/names/{name}       individually signed name binding
//! PUT    /v1/registry           replace the registry snapshot
//! GET    /v1/revocations        estate-signed grant denylist snapshot
//! PUT    /v1/revocations        replace the denylist snapshot
//! GET    /v1/health             liveness
//! GET    /v1/metrics            prometheus text counters (loopback only)
//! ```
//!
//! Security posture: every write is signature-verified before it
//! touches the store; every signature-verifying write (`PUT` records,
//! `PUT` registry, `DELETE` records) is globally rate-limited before
//! verification, and `PUT` records are additionally paced per key;
//! records are self-certifying so a compromised directory can at worst
//! withhold updates, never forge reachability.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use tokio::net::TcpListener;

use crate::http::{self, Request, Response};
use crate::registry::{SignedRegistry, valid_name};
use crate::revocations::SignedRevocations;
use crate::{authority::Authority, clock::Reading, policy::PolicyStore};

use crate::{DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, RecordStore};

/// Tunables for [`serve`].
#[derive(Debug, Clone)]
pub struct Limits {
    /// Minimum spacing between accepted PUTs for one endpoint key.
    /// Default 0 disables it: a publisher can only ever write its own
    /// slot (the signature binds the key), so per-key pacing mostly
    /// obstructs legitimate announce republishes. Deployments that
    /// want it anyway can set a non-zero interval.
    pub put_min_interval: Duration,
    /// Maximum signature-verifying write requests globally per minute
    /// — this is the real abuse bound: it caps JSON parse +
    /// signature-verification CPU an unauthenticated peer can burn.
    /// Applies to `PUT /v1/records`, `PUT /v1/registry` and
    /// `DELETE /v1/records/{key}`, checked before any parsing.
    pub put_per_minute: u32,
    /// Absolute per-connection timeout, including TLS handshake and response.
    pub conn_timeout: Duration,
    /// Maximum concurrently held connections. Without a bound a SYN
    /// flood spends one task + one FD + one 8 KiB read buffer each —
    /// cheap per connection but unbounded in count. Excess connections
    /// are accepted and dropped immediately (the peer sees a close).
    pub max_conns: usize,
    /// Blocking signature/storage jobs, including jobs whose request timed out.
    pub max_workers: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            put_min_interval: Duration::ZERO,
            put_per_minute: 600,
            conn_timeout: Duration::from_secs(10),
            max_conns: 1024,
            max_workers: 16,
        }
    }
}

/// Runtime configuration for [`serve`].
#[derive(Default)]
pub struct ServiceConfig {
    /// When set, this listener accepts only TLS; there is no plaintext fallback.
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// Verifying key the estate signs registry snapshots with. Without
    /// it the name API is disabled: lookups 404 and registry PUTs 401.
    pub registry_key: Option<VerifyingKey>,
    /// Snapshot loaded at startup (verified against `registry_key`).
    pub registry: Option<SignedRegistry>,
    /// Pre-opened durable authority state. Omitting it explicitly uses memory.
    pub policy: Option<PolicyStore>,
    pub limits: Limits,
}

#[derive(Default)]
struct Metrics {
    puts_ok: AtomicU64,
    puts_rejected: AtomicU64,
    gets: AtomicU64,
    deletes: AtomicU64,
    name_lookups: AtomicU64,
    registry_puts: AtomicU64,
    revocations_puts: AtomicU64,
    requests_bad: AtomicU64,
    writes_rate_limited: AtomicU64,
    gc_retired: AtomicU64,
    gc_failures: AtomicU64,
    /// Per-writer PUT counts keyed by an anonymized id —
    /// `blake3(endpoint_key)[..8]` hex — so the scrape shows
    /// per-endpoint accounting without disclosing public keys.
    /// Bounded; writers past the cap fold into `other`.
    endpoint_puts: Mutex<HashMap<String, u64>>,
}

/// Anonymized per-endpoint label: a truncated BLAKE3 of the public
/// key. Stable per endpoint, useless for recovering the key.
fn writer_label(key: &EndpointKey) -> String {
    hex16(&blake3::hash(&key.0).as_bytes()[..8])
}

fn hex16(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Lock acquisition that survives a poisoned lock: every lock here
/// guards plain data (instants, counters, `Option` snapshots) whose
/// invariants a panic cannot tear, so one panicked holder must not
/// fail every request the directory serves from then on.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Cap on distinct writer labels before accounting folds into `other`
/// — the scrape stays bounded under a writer flood.
const MAX_WRITER_LABELS: usize = 4096;

/// Per-key PUT pacing + a global per-minute window over verifying writes.
struct RateLimiter {
    last_put: Mutex<HashMap<EndpointKey, Instant>>,
    window_start: Mutex<Instant>,
    window_count: AtomicU64,
}

impl RateLimiter {
    /// Global window over all PUT requests. Runs before parsing so it
    /// bounds the verification CPU an unauthenticated peer can burn.
    fn check_global(&self, limits: &Limits) -> bool {
        {
            let mut start = lock(&self.window_start);
            if start.elapsed() >= Duration::from_secs(60) {
                *start = Instant::now();
                self.window_count.store(0, Ordering::Relaxed);
            }
        }
        self.window_count.fetch_add(1, Ordering::Relaxed) < u64::from(limits.put_per_minute)
    }

    /// Per-key interval between accepted PUTs.
    fn check_key(&self, key: &EndpointKey, limits: &Limits) -> bool {
        let mut last = lock(&self.last_put);
        if let Some(t) = last.get(key)
            && t.elapsed() < limits.put_min_interval
        {
            return false;
        }
        last.retain(|_, t| t.elapsed() < limits.put_min_interval);
        last.insert(*key, Instant::now());
        true
    }
}

/// A running directory service. `Drop` aborts its listener and owned requests.
pub struct Directory {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Directory {
    /// Bound HTTP(S) address (port is concrete even when bound to `:0`).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct State {
    store: Arc<dyn RecordStore>,
    policy: Mutex<Option<PolicyStore>>,
    workers: Arc<tokio::sync::Semaphore>,
    limits: Limits,
    limiter: RateLimiter,
    metrics: Metrics,
}

/// Bind `addr` and serve the directory over `store` until the returned
/// [`Directory`] is dropped.
pub async fn serve(
    addr: SocketAddr,
    store: Arc<dyn RecordStore>,
    config: ServiceConfig,
) -> std::io::Result<Directory> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let policy = tokio::task::spawn_blocking(move || -> Result<_, DiscoveryError> {
        let mut policy = match config.policy {
            Some(policy) => {
                if config
                    .registry_key
                    .is_some_and(|k| policy.bootstrap().key.0 != k.to_bytes())
                {
                    return Err(DiscoveryError::Configuration(
                        "policy bootstrap key mismatch".into(),
                    ));
                }
                Some(policy)
            }
            None => config
                .registry_key
                .map(|key| PolicyStore::memory(Authority::new(&key, 1)?))
                .transpose()?,
        };
        if let Some(snap) = config.registry {
            policy
                .as_mut()
                .ok_or(DiscoveryError::BadSignature)?
                .bootstrap_registry(&snap, Reading::now()?)?;
        }
        Ok(policy)
    })
    .await
    .map_err(std::io::Error::other)?
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    let state = Arc::new(State {
        store,
        policy: Mutex::new(policy),
        workers: Arc::new(tokio::sync::Semaphore::new(config.limits.max_workers)),
        limits: config.limits,
        limiter: RateLimiter {
            last_put: Mutex::new(HashMap::new()),
            window_start: Mutex::new(Instant::now()),
            window_count: AtomicU64::new(0),
        },
        metrics: Metrics::default(),
    });
    let task = tokio::spawn({
        let state = state.clone();
        let tls = config.tls.map(tokio_rustls::TlsAcceptor::from);
        async move {
            let mut connections = tokio::task::JoinSet::new();
            // Separate single-job maintenance capacity: request saturation must
            // not indefinitely starve expiry. A started blocking job can finish
            // after drop, but no successor is scheduled without this owner.
            let mut maintenance = tokio::task::JoinSet::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let accepted = tokio::select! {
                    result = listener.accept() => result,
                    _ = connections.join_next(), if !connections.is_empty() => continue,
                    _ = tick.tick(), if maintenance.is_empty() => {
                        let store = state.store.clone();
                        maintenance.spawn_blocking(move || store.collect_expired());
                        continue;
                    },
                    result = maintenance.join_next(), if !maintenance.is_empty() => {
                        match result {
                            Some(Ok(Ok(count))) => { state.metrics.gc_retired.fetch_add(count as u64, Ordering::Relaxed); }
                            _ => { state.metrics.gc_failures.fetch_add(1, Ordering::Relaxed); }
                        }
                        continue;
                    },
                };
                let Ok((mut sock, peer)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                // Reap completed requests before admission. The owned set is
                // the budget, bounding completed handles as well as active
                // tasks. There is no await between admission and spawn.
                while connections.try_join_next().is_some() {}
                if connections.len() >= state.limits.max_conns {
                    continue;
                }
                let state = state.clone();
                let tls = tls.clone();
                connections.spawn(async move {
                    let _ = tokio::time::timeout(state.limits.conn_timeout, async {
                        if let Some(tls) = tls {
                            if let Ok(mut stream) = tls.accept(sock).await {
                                serve_request(&mut stream, &state, peer).await;
                            }
                        } else {
                            serve_request(&mut sock, &state, peer).await;
                        }
                    })
                    .await;
                });
            }
        }
    });
    Ok(Directory { addr: local, task })
}

async fn serve_request(
    stream: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
    state: &Arc<State>,
    peer: SocketAddr,
) {
    let response = match http::read_request(stream).await {
        Ok(Some(req)) => match state.workers.clone().try_acquire_owned() {
            Ok(permit) => {
                let state = state.clone();
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    route(&state, peer, &req)
                })
                .await
                .unwrap_or_else(|e| Response::error(500, &DiscoveryError::Store(e.to_string())))
            }
            Err(_) => Response::error(429, &DiscoveryError::RateLimited),
        },
        Ok(None) => return,
        Err(e) => {
            state.metrics.requests_bad.fetch_add(1, Ordering::Relaxed);
            let status = match &e {
                DiscoveryError::InvalidRecord(m) if m.contains("body too large") => 413,
                _ => 400,
            };
            Response::error(status, &e)
        }
    };
    let _ = http::write_response(stream, &response).await;
    let _ = tokio::io::AsyncWriteExt::shutdown(stream).await;
}

fn route(state: &State, peer: SocketAddr, req: &Request) -> Response {
    let segments: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("PUT", ["v1", "records"]) => put_record(state, req),
        ("GET", ["v1", "records", key]) => get_record(state, key),
        ("DELETE", ["v1", "records", key]) => delete_record(state, key, req),
        ("GET", ["v1", "names", name]) => get_name(state, name),
        ("PUT", ["v1", "registry"]) => put_registry(state, req),
        ("GET", ["v1", "revocations"]) => get_revocations(state),
        ("PUT", ["v1", "revocations"]) => put_revocations(state, req),
        ("GET", ["v1", "health"]) => Response::json(200, serde_json::json!({ "ok": true })),
        // Per-endpoint counters reveal writer activity, so scrapes are
        // loopback-only; remote monitoring goes over SSH or a local
        // exporter rather than a public port.
        ("GET", ["v1", "metrics"]) if peer.ip().is_loopback() => metrics(state),
        (_, ["v1", ..]) => Response::text(404, "unknown route"),
        _ => Response::text(404, "unknown route"),
    }
}

/// Global-window rejection shared by every verifying write route.
fn rate_limited(state: &State) -> Response {
    state
        .metrics
        .writes_rate_limited
        .fetch_add(1, Ordering::Relaxed);
    Response::error(429, &DiscoveryError::RateLimited)
}

fn put_record(state: &State, req: &Request) -> Response {
    // Global window first — bounds parse and signature-verification
    // CPU for unauthenticated peers, before any per-request work.
    if !state.limiter.check_global(&state.limits) {
        return rate_limited(state);
    }
    let record: EndpointRecord = match serde_json::from_slice(&req.body) {
        Ok(r) => r,
        Err(e) => {
            state.metrics.puts_rejected.fetch_add(1, Ordering::Relaxed);
            return Response::error(400, &DiscoveryError::InvalidRecord(e.to_string()));
        }
    };
    let payload = match record.verify_fresh() {
        Ok(p) => p,
        Err(e) => {
            state.metrics.puts_rejected.fetch_add(1, Ordering::Relaxed);
            return Response::error(status_for(&e), &e);
        }
    };
    if !state.limiter.check_key(&payload.key, &state.limits) {
        state.metrics.puts_rejected.fetch_add(1, Ordering::Relaxed);
        return Response::error(429, &DiscoveryError::RateLimited);
    }
    match state.store.put(&record) {
        Ok(()) => {
            state.metrics.puts_ok.fetch_add(1, Ordering::Relaxed);
            let mut per = lock(&state.metrics.endpoint_puts);
            let label = if per.len() >= MAX_WRITER_LABELS {
                "other".to_string()
            } else {
                writer_label(&payload.key)
            };
            *per.entry(label).or_insert(0) += 1;
            Response::json(200, serde_json::json!({ "stored": true }))
        }
        Err(e) => {
            state.metrics.puts_rejected.fetch_add(1, Ordering::Relaxed);
            Response::error(status_for(&e), &e)
        }
    }
}

fn get_record(state: &State, key: &str) -> Response {
    state.metrics.gets.fetch_add(1, Ordering::Relaxed);
    let key = match key.parse::<EndpointKey>() {
        Ok(k) => k,
        Err(e) => return Response::error(400, &e),
    };
    match state.store.get(&key) {
        Ok(record) => Response::json(200, record),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn delete_record(state: &State, key: &str, req: &Request) -> Response {
    // Same global bound as PUTs: the tombstone verify below is ed25519
    // work an unauthenticated peer could otherwise burn unbounded.
    if !state.limiter.check_global(&state.limits) {
        return rate_limited(state);
    }
    let key = match key.parse::<EndpointKey>() {
        Ok(k) => k,
        Err(e) => return Response::error(400, &e),
    };
    let tomb: DeleteRequest = match serde_json::from_slice(&req.body) {
        Ok(t) => t,
        Err(e) => {
            return Response::error(400, &DiscoveryError::InvalidRecord(e.to_string()));
        }
    };
    let del = match tomb.verify_fresh() {
        Ok(d) => d,
        Err(e) => return Response::error(status_for(&e), &e),
    };
    if del.key != key {
        return Response::error(401, &DiscoveryError::BadSignature);
    }
    state.metrics.deletes.fetch_add(1, Ordering::Relaxed);
    match state.store.remove(&tomb) {
        Ok(()) => Response::json(200, serde_json::json!({ "deleted": true })),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn get_name(state: &State, name: &str) -> Response {
    state.metrics.name_lookups.fetch_add(1, Ordering::Relaxed);
    if !valid_name(name) {
        return Response::error(
            400,
            &DiscoveryError::InvalidRecord("invalid device name".into()),
        );
    }
    let result = with_policy(state, |policy| policy.name(name, Reading::now()?));
    match result {
        Ok(proof) => Response::json(200, proof),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn with_policy<T>(
    state: &State,
    apply: impl FnOnce(&mut PolicyStore) -> Result<T, DiscoveryError>,
) -> Result<T, DiscoveryError> {
    let mut guard = state
        .policy
        .lock()
        .map_err(|_| DiscoveryError::Store("policy state poisoned".into()))?;
    apply(guard.as_mut().ok_or(DiscoveryError::NotFound)?)
}

fn put_registry(state: &State, req: &Request) -> Response {
    if !state.limiter.check_global(&state.limits) {
        return rate_limited(state);
    }
    let result = with_policy(state, |policy| {
        let snap: SignedRegistry = serde_json::from_slice(&req.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        policy.accept_registry(&snap, Reading::now()?)
    });
    match result {
        Ok(changed) => {
            if changed {
                state.metrics.registry_puts.fetch_add(1, Ordering::Relaxed);
            }
            Response::json(
                200,
                serde_json::json!({ "stored": true, "changed": changed }),
            )
        }
        Err(DiscoveryError::NotFound) => Response::error(401, &DiscoveryError::BadSignature),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn get_revocations(state: &State) -> Response {
    match with_policy(state, |policy| policy.revocations(Reading::now()?)) {
        Ok(Some((snap, _, _))) => Response::json(200, snap),
        Ok(None) => Response::error(404, &DiscoveryError::NotFound),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn put_revocations(state: &State, req: &Request) -> Response {
    if !state.limiter.check_global(&state.limits) {
        return rate_limited(state);
    }
    let result = with_policy(state, |policy| {
        let snap: SignedRevocations = serde_json::from_slice(&req.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        policy.accept_revocations(&snap, Reading::now()?)
    });
    match result {
        Ok(changed) => {
            if changed {
                state
                    .metrics
                    .revocations_puts
                    .fetch_add(1, Ordering::Relaxed);
            }
            Response::json(
                200,
                serde_json::json!({ "stored": true, "changed": changed }),
            )
        }
        Err(DiscoveryError::NotFound) => Response::error(401, &DiscoveryError::BadSignature),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn metrics(state: &State) -> Response {
    let m = &state.metrics;
    let mut body = format!(
        "rds_directory_records {}\n\
         rds_directory_puts_ok {}\n\
         rds_directory_puts_rejected {}\n\
         rds_directory_gets {}\n\
         rds_directory_deletes {}\n\
         rds_directory_name_lookups {}\n\
         rds_directory_registry_puts {}\n\
         rds_directory_revocations_puts {}\n\
         rds_directory_requests_bad {}\n\
         rds_directory_writes_rate_limited {}\n\
         rds_directory_gc_retired_total {}\n\
         rds_directory_gc_failures_total {}\n",
        state.store.len(),
        m.puts_ok.load(Ordering::Relaxed),
        m.puts_rejected.load(Ordering::Relaxed),
        m.gets.load(Ordering::Relaxed),
        m.deletes.load(Ordering::Relaxed),
        m.name_lookups.load(Ordering::Relaxed),
        m.registry_puts.load(Ordering::Relaxed),
        m.revocations_puts.load(Ordering::Relaxed),
        m.requests_bad.load(Ordering::Relaxed),
        m.writes_rate_limited.load(Ordering::Relaxed),
        m.gc_retired.load(Ordering::Relaxed),
        m.gc_failures.load(Ordering::Relaxed),
    );
    // Per-endpoint accounting: PUT counts by anonymized writer label
    // (blake3(key)[..8] — never the key itself; C7 security).
    let per = lock(&m.endpoint_puts);
    body.push_str(&format!("rds_directory_writers_distinct {}\n", per.len()));
    for (writer, count) in per.iter() {
        body.push_str(&format!(
            "rds_directory_endpoint_puts_total{{writer=\"{writer}\"}} {count}\n"
        ));
    }
    Response::text(200, body)
}

fn status_for(e: &DiscoveryError) -> u16 {
    match e {
        DiscoveryError::NotFound => 404,
        DiscoveryError::BadSignature => 401,
        DiscoveryError::Stale => 409,
        DiscoveryError::Expired => 410,
        DiscoveryError::RateLimited => 429,
        DiscoveryError::InvalidRecord(_) => 400,
        _ => 500,
    }
}
