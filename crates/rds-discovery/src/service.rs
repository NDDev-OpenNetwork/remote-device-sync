//! The discovery directory HTTP service.
//!
//! Routes (one request per connection, `Connection: close`):
//!
//! ```text
//! PUT    /v1/records            publish a signed EndpointRecord
//! GET    /v1/records/{key}      fetch the stored record JSON
//! DELETE /v1/records/{key}      body: signed DeleteRequest
//! GET    /v1/names/{name}       estate registry lookup → {"key": ...}
//! PUT    /v1/registry           replace the registry snapshot
//! GET    /v1/health             liveness
//! GET    /v1/metrics            prometheus text counters
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
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use tokio::net::TcpListener;

use crate::http::{self, Request, Response};
use crate::registry::{RegistryPayload, SignedRegistry};
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
    /// Client-side and per-connection idle timeout.
    pub conn_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            put_min_interval: Duration::ZERO,
            put_per_minute: 600,
            conn_timeout: Duration::from_secs(10),
        }
    }
}

/// Runtime configuration for [`serve`].
#[derive(Default)]
pub struct ServiceConfig {
    /// Verifying key the estate signs registry snapshots with. Without
    /// it the name API is disabled: lookups 404 and registry PUTs 401.
    pub registry_key: Option<VerifyingKey>,
    /// Snapshot loaded at startup (verified against `registry_key`).
    pub registry: Option<SignedRegistry>,
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
    requests_bad: AtomicU64,
    writes_rate_limited: AtomicU64,
}

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
            let mut start = self.window_start.lock().unwrap();
            if start.elapsed() >= Duration::from_secs(60) {
                *start = Instant::now();
                self.window_count.store(0, Ordering::Relaxed);
            }
        }
        self.window_count.fetch_add(1, Ordering::Relaxed) < u64::from(limits.put_per_minute)
    }

    /// Per-key interval between accepted PUTs.
    fn check_key(&self, key: &EndpointKey, limits: &Limits) -> bool {
        let mut last = self.last_put.lock().unwrap();
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

/// A running directory service. `Drop` stops the accept task.
pub struct Directory {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Directory {
    /// Bound HTTP address (port is concrete even when bound to `:0`).
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
    registry: RwLock<Option<RegistryPayload>>,
    registry_key: Option<VerifyingKey>,
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
    let registry =
        match (&config.registry, &config.registry_key) {
            (Some(snap), Some(key)) => Some(snap.verify_fresh(key, None).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
            })?),
            _ => None,
        };
    let state = Arc::new(State {
        store,
        registry: RwLock::new(registry),
        registry_key: config.registry_key,
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
        async move {
            loop {
                let Ok((mut sock, _peer)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(state.limits.conn_timeout, async {
                        match http::read_request(&mut sock).await {
                            Ok(Some(req)) => {
                                let resp = route(&state, &req);
                                let _ = http::write_response(&mut sock, &resp).await;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                state.metrics.requests_bad.fetch_add(1, Ordering::Relaxed);
                                let status = match &e {
                                    DiscoveryError::InvalidRecord(m)
                                        if m.contains("body too large") =>
                                    {
                                        413
                                    }
                                    _ => 400,
                                };
                                let _ =
                                    http::write_response(&mut sock, &Response::error(status, &e))
                                        .await;
                            }
                        }
                    })
                    .await;
                });
            }
        }
    });
    Ok(Directory { addr: local, task })
}

fn route(state: &State, req: &Request) -> Response {
    let segments: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("PUT", ["v1", "records"]) => put_record(state, req),
        ("GET", ["v1", "records", key]) => get_record(state, key),
        ("DELETE", ["v1", "records", key]) => delete_record(state, key, req),
        ("GET", ["v1", "names", name]) => get_name(state, name),
        ("PUT", ["v1", "registry"]) => put_registry(state, req),
        ("GET", ["v1", "health"]) => Response::json(200, serde_json::json!({ "ok": true })),
        ("GET", ["v1", "metrics"]) => metrics(state),
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
    let del = match tomb.verify() {
        Ok(d) => d,
        Err(e) => return Response::error(401, &e),
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
    let registry = state.registry.read().unwrap();
    match registry.as_ref().and_then(|r| r.entries.get(name)) {
        Some(key) => Response::json(200, serde_json::json!({ "key": key.to_string() })),
        None => Response::error(404, &DiscoveryError::NotFound),
    }
}

fn put_registry(state: &State, req: &Request) -> Response {
    // Same global bound as PUTs: snapshot verify below is ed25519 work.
    if !state.limiter.check_global(&state.limits) {
        return rate_limited(state);
    }
    let Some(key) = &state.registry_key else {
        return Response::error(401, &DiscoveryError::BadSignature);
    };
    let snap: SignedRegistry = match serde_json::from_slice(&req.body) {
        Ok(s) => s,
        Err(e) => {
            return Response::error(400, &DiscoveryError::InvalidRecord(e.to_string()));
        }
    };
    let current = state.registry.read().unwrap();
    match snap.verify_fresh(key, current.as_ref()) {
        Ok(payload) => {
            drop(current);
            *state.registry.write().unwrap() = Some(payload);
            state.metrics.registry_puts.fetch_add(1, Ordering::Relaxed);
            Response::json(200, serde_json::json!({ "stored": true }))
        }
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn metrics(state: &State) -> Response {
    let m = &state.metrics;
    let body = format!(
        "rds_directory_records {}\n\
         rds_directory_puts_ok {}\n\
         rds_directory_puts_rejected {}\n\
         rds_directory_gets {}\n\
         rds_directory_deletes {}\n\
         rds_directory_name_lookups {}\n\
         rds_directory_registry_puts {}\n\
         rds_directory_requests_bad {}\n\
         rds_directory_writes_rate_limited {}\n",
        state.store.len(),
        m.puts_ok.load(Ordering::Relaxed),
        m.puts_rejected.load(Ordering::Relaxed),
        m.gets.load(Ordering::Relaxed),
        m.deletes.load(Ordering::Relaxed),
        m.name_lookups.load(Ordering::Relaxed),
        m.registry_puts.load(Ordering::Relaxed),
        m.requests_bad.load(Ordering::Relaxed),
        m.writes_rate_limited.load(Ordering::Relaxed),
    );
    Response::text(200, body)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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
