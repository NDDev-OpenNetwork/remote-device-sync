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
//! Metrics are exposed only through the separate authenticated admin listener.
//! ```
//!
//! Security posture: membership is configured separately from reachability.
//! Signatures and revision ordering are checked before a write spends its
//! identity budget. Known identities retain one protected renewal per minute;
//! new admissions, extra writes and policy updates have separate budgets.
//! Connection/body/worker bounds limit pre-authentication work; availability
//! under arbitrary network flooding is not promised.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use tokio::net::TcpListener;
use tokio::sync::watch;

mod tasks;
use tasks::TaskGroup;

use crate::http::{self, Request, Response};
use crate::registry::{SignedRegistry, valid_name};
use crate::revocations::SignedRevocations;
use crate::{authority::Authority, clock::Reading, policy::PolicyStore};

use crate::{
    DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, Enrollment, RecordStore,
    admission::Limiter,
};

/// Tunables for [`serve`].
#[derive(Debug, Clone)]
pub struct Limits {
    /// Minimum spacing between new admitted mutations for one identity.
    /// Exact retries and rejected revisions do not spend this budget.
    pub put_min_interval: Duration,
    /// Shared extra writes per minute, after known-device protected renewal.
    pub put_per_minute: u32,
    /// New identity admissions per minute, separate from remembered renewals.
    pub admissions_per_minute: u32,
    /// Maximum new mutations by one verified identity per minute, charged first.
    pub writer_per_minute: u32,
    /// Each policy role has its own verified-new-revision write budget.
    pub policy_per_minute: u32,
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
            admissions_per_minute: 600,
            writer_per_minute: 120,
            policy_per_minute: 60,
            conn_timeout: Duration::from_secs(10),
            max_conns: 1024,
            max_workers: 16,
        }
    }
}

/// Runtime configuration for [`serve`].
#[derive(Default)]
pub struct ServiceConfig {
    /// Independent publisher allowlist. Empty by default: no endpoint access.
    pub enrollment: Enrollment,
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

impl ServiceConfig {
    /// Explicit open enrollment for isolated synthetic fixtures only.
    pub fn open_ephemeral() -> Self {
        Self {
            enrollment: Enrollment::unrestricted_for_tests(),
            ..Self::default()
        }
    }
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
    connections_rejected: AtomicU64,
    workers_rejected: AtomicU64,
    requests: AtomicU64,
}

/// Aggregate counters only; clones retain no store, policy or service tasks.
#[derive(Clone)]
pub struct DirectoryMetrics {
    counters: Arc<Metrics>,
    connections: std::sync::Weak<TaskGroup>,
    workers: std::sync::Weak<TaskGroup>,
    maintenance: std::sync::Weak<TaskGroup>,
}

impl DirectoryMetrics {
    /// Independent in-memory observations. Task gauges include completed
    /// handles not yet reaped; a busy/expired group is explicitly unknown.
    pub fn snapshot(&self) -> BTreeMap<&'static str, u64> {
        let m = &self.counters;
        let mut values = BTreeMap::from([
            (
                "rds_directory_puts_ok_total",
                m.puts_ok.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_puts_rejected_total",
                m.puts_rejected.load(Ordering::Relaxed),
            ),
            ("rds_directory_gets_total", m.gets.load(Ordering::Relaxed)),
            (
                "rds_directory_deletes_total",
                m.deletes.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_name_lookups_total",
                m.name_lookups.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_registry_puts_total",
                m.registry_puts.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_revocations_puts_total",
                m.revocations_puts.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_requests_bad_total",
                m.requests_bad.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_requests_total",
                m.requests.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_writes_rate_limited_total",
                m.writes_rate_limited.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_gc_retired_total",
                m.gc_retired.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_gc_failures_total",
                m.gc_failures.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_connections_rejected_total",
                m.connections_rejected.load(Ordering::Relaxed),
            ),
            (
                "rds_directory_workers_rejected_total",
                m.workers_rejected.load(Ordering::Relaxed),
            ),
        ]);
        for (group, known, tasks, limit) in [
            (
                &self.connections,
                "rds_directory_connections_known",
                "rds_directory_connection_tasks",
                "rds_directory_connection_limit",
            ),
            (
                &self.workers,
                "rds_directory_workers_known",
                "rds_directory_worker_tasks",
                "rds_directory_worker_limit",
            ),
            (
                &self.maintenance,
                "rds_directory_maintenance_known",
                "rds_directory_maintenance_tasks",
                "rds_directory_maintenance_limit",
            ),
        ] {
            let observed = group.upgrade().and_then(|group| group.snapshot());
            values.insert(known, u64::from(observed.is_some()));
            if let Some((count, max)) = observed {
                values.insert(tasks, count as u64);
                values.insert(limit, max as u64);
            }
        }
        values
    }
}

/// A running directory service. Drop seals admission and requests cleanup.
/// Use `close` to join requests and already-started blocking storage work.
/// Cleanup after Drop needs the Tokio executor to continue running.
pub struct Directory {
    addr: SocketAddr,
    metrics: Arc<Metrics>,
    shutdown: watch::Sender<bool>,
    connections: Arc<TaskGroup>,
    workers: Arc<TaskGroup>,
    maintenance: Arc<TaskGroup>,
    runner: tokio::sync::Mutex<Runner>,
}

struct Runner {
    task: Option<tokio::task::JoinHandle<()>>,
    outcome: Option<Result<(), String>>,
}

impl Runner {
    async fn wait(&mut self) {
        if let Some(task) = self.task.as_mut() {
            let result = task.await.map_err(|error| error.to_string());
            // Retain before any further await: repeated observation/cleanup
            // must never poll a consumed JoinHandle again.
            self.task = None;
            self.outcome = Some(result);
        }
    }

    fn result(&self) -> std::io::Result<()> {
        match self.outcome.as_ref() {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(std::io::Error::other(error.clone())),
            None => Err(std::io::Error::other("directory runner outcome missing")),
        }
    }
}

impl Directory {
    pub fn metrics(&self) -> DirectoryMetrics {
        DirectoryMetrics {
            counters: self.metrics.clone(),
            connections: Arc::downgrade(&self.connections),
            workers: Arc::downgrade(&self.workers),
            maintenance: Arc::downgrade(&self.maintenance),
        }
    }

    /// Bound HTTP(S) address, including the assigned port for a `:0` bind.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Observe runner termination without stopping it. Call close afterward
    /// to join retained request/storage work. Observation itself does not join
    /// failure-fallback disk jobs, so observe before closing to stop sibling
    /// services promptly. A concurrent close may hold the shared runner mutex
    /// during cleanup. Canceled/repeated observation preserves the result.
    pub async fn wait_stopped(&self) -> std::io::Result<()> {
        let mut runner = self.runner.lock().await;
        runner.wait().await;
        runner.result()
    }

    /// Seal admission and join service-owned work. Running blocking filesystem
    /// operations cannot be preempted; this waits for them to return. Canceling
    /// this waiter does not cancel cleanup or lose the runner's join handle.
    pub async fn close(&self) -> std::io::Result<()> {
        self.request_shutdown();
        let mut runner = self.runner.lock().await;
        runner.wait().await;
        // Also join connections and started jobs if the runner panicked before
        // normal cleanup. Only one waiter may poll each fallback JoinSet.
        tokio::join!(
            self.connections.drain(),
            self.workers.drain(),
            self.maintenance.drain()
        );
        runner.result()
    }

    fn request_shutdown(&self) {
        self.connections.seal();
        self.workers.seal();
        self.maintenance.seal();
        self.shutdown.send_replace(true);
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

struct State {
    store: Arc<dyn RecordStore>,
    policy: Mutex<Option<PolicyStore>>,
    workers: Arc<TaskGroup>,
    limits: Limits,
    limiter: Limiter,
    enrollment: Enrollment,
    metrics: Arc<Metrics>,
}

/// Bind `addr` and serve the directory over `store` until the returned
/// [`Directory`] is dropped.
pub async fn serve(
    addr: SocketAddr,
    store: Arc<dyn RecordStore>,
    config: ServiceConfig,
) -> std::io::Result<Directory> {
    if config.limits.max_workers == 0
        || config.limits.max_conns == 0
        || config.limits.conn_timeout.is_zero()
        || config.limits.writer_per_minute == 0
        || config.limits.put_min_interval > Duration::from_secs(60)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid directory capacity or timing limits",
        ));
    }
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
        workers: Arc::new(TaskGroup::new(config.limits.max_workers)),
        limits: config.limits,
        limiter: Limiter::default(),
        enrollment: config.enrollment,
        metrics: Arc::new(Metrics::default()),
    });
    let connections = Arc::new(TaskGroup::new(state.limits.max_conns));
    let maintenance = Arc::new(TaskGroup::new(1));
    let (shutdown, mut stop) = watch::channel(false);
    let task = tokio::spawn({
        let state = state.clone();
        let connections = connections.clone();
        let maintenance = maintenance.clone();
        let tls = config.tls.map(tokio_rustls::TlsAcceptor::from);
        async move {
            // Separate single-job maintenance capacity: request saturation must
            // not indefinitely starve expiry. Shutdown joins a started job.
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = stop.changed() => break,
                    _ = connections.join_next(), if !connections.is_empty() => continue,
                    _ = connections.changed() => continue,
                    _ = state.workers.join_next(), if !state.workers.is_empty() => continue,
                    _ = state.workers.changed() => continue,
                    result = maintenance.join_next(), if !maintenance.is_empty() => {
                        if let Some(Err(error)) = result
                            && !error.is_cancelled()
                        { state.metrics.gc_failures.fetch_add(1, Ordering::Relaxed); }
                        continue;
                    },
                    _ = maintenance.changed() => continue,
                    _ = tick.tick(), if maintenance.is_empty() => {
                        maintenance.collect(state.clone());
                        continue;
                    },
                    result = listener.accept() => result,
                };
                let Ok((mut sock, peer)) = accepted else {
                    tokio::select! {
                        biased;
                        _ = stop.changed() => break,
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                    }
                    continue;
                };
                // Reap completed requests before admission. The owned set is
                // the budget, bounding completed handles as well as active
                // tasks. There is no await between admission and spawn.
                let state = state.clone();
                let tls = tls.clone();
                let metrics = state.metrics.clone();
                if !connections.spawn(async move {
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
                }) {
                    metrics.connections_rejected.fetch_add(1, Ordering::Relaxed);
                }
            }
            drop(listener);
            state.workers.seal();
            connections.drain().await;
            tokio::join!(state.workers.drain(), maintenance.drain());
        }
    });
    Ok(Directory {
        addr: local,
        metrics: state.metrics.clone(),
        shutdown,
        connections,
        workers: state.workers.clone(),
        maintenance,
        runner: tokio::sync::Mutex::new(Runner {
            task: Some(task),
            outcome: None,
        }),
    })
}

async fn serve_request(
    stream: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
    state: &Arc<State>,
    peer: SocketAddr,
) {
    let response = match http::read_request(stream).await {
        Ok(Some(req)) => {
            state.metrics.requests.fetch_add(1, Ordering::Relaxed);
            let is_head = req.method == "HEAD";
            let mut response = match state.workers.request(state.clone(), peer, req) {
                Some(reply) => reply.await.unwrap_or_else(|_| {
                    Response::error(
                        500,
                        &DiscoveryError::Store("directory worker ended without a response".into()),
                    )
                }),
                None => {
                    state
                        .metrics
                        .workers_rejected
                        .fetch_add(1, Ordering::Relaxed);
                    Response::error(429, &DiscoveryError::RateLimited)
                }
            };
            // This also covers worker saturation/failure before routing HEAD.
            if is_head {
                response.body.clear();
            }
            response
        }
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

fn route(state: &State, _peer: SocketAddr, req: &Request) -> Response {
    let segments: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    match (req.method.as_str(), segments.as_slice()) {
        // HEAD has no representation in this API and must never return a body.
        ("HEAD", _) => Response::text(405, ""),
        ("PUT", ["v1", "records"]) => put_record(state, req),
        ("GET", ["v1", "records", key]) => get_record(state, key),
        ("DELETE", ["v1", "records", key]) => delete_record(state, key, req),
        ("GET", ["v1", "names", name]) => get_name(state, name),
        ("PUT", ["v1", "registry"]) => put_registry(state, req),
        ("GET", ["v1", "revocations"]) => get_revocations(state),
        ("PUT", ["v1", "revocations"]) => put_revocations(state, req),
        ("GET", ["v1", "health"]) => Response::json(200, serde_json::json!({ "ok": true })),
        // Metrics never share the public listener, including requests from a
        // loopback reverse proxy. The host owns authenticated admin export.
        (_, ["v1", ..]) => Response::text(404, "unknown route"),
        _ => Response::text(404, "unknown route"),
    }
}

/// Count quota refusals after signature and revision validation.
fn charge(state: &State, result: Result<(), DiscoveryError>) -> Result<(), DiscoveryError> {
    if matches!(result, Err(DiscoveryError::RateLimited)) {
        state
            .metrics
            .writes_rate_limited
            .fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn put_record(state: &State, req: &Request) -> Response {
    let record: EndpointRecord = match serde_json::from_slice(&req.body) {
        Ok(r) => r,
        Err(e) => {
            state.metrics.puts_rejected.fetch_add(1, Ordering::Relaxed);
            return Response::error(400, &DiscoveryError::InvalidRecord(e.to_string()));
        }
    };
    if !state.enrollment.allows(&record.key) {
        state.metrics.puts_rejected.fetch_add(1, Ordering::Relaxed);
        return Response::error(403, &DiscoveryError::NotEnrolled);
    }
    // The hint only filters unknown enrollment here. RecordStore must verify
    // signature, signed key agreement, lifetime and revision before admission.
    match state.store.put_admitted(&record, &mut |known| {
        charge(
            state,
            state.limiter.record(record.key, known, &state.limits),
        )
    }) {
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
    if !state.enrollment.allows(&key) {
        return Response::error(403, &DiscoveryError::NotEnrolled);
    }
    match state.store.get(&key) {
        Ok(record) => Response::json(200, record),
        Err(e) => Response::error(status_for(&e), &e),
    }
}

fn delete_record(state: &State, key: &str, req: &Request) -> Response {
    let key = match key.parse::<EndpointKey>() {
        Ok(k) => k,
        Err(e) => return Response::error(400, &e),
    };
    if !state.enrollment.allows(&key) {
        return Response::error(403, &DiscoveryError::NotEnrolled);
    }
    let tomb: DeleteRequest = match serde_json::from_slice(&req.body) {
        Ok(t) => t,
        Err(e) => {
            return Response::error(400, &DiscoveryError::InvalidRecord(e.to_string()));
        }
    };
    if tomb.key != key {
        return Response::error(401, &DiscoveryError::BadSignature);
    }
    state.metrics.deletes.fetch_add(1, Ordering::Relaxed);
    match state.store.remove_admitted(&tomb, &mut |known| {
        charge(state, state.limiter.record(key, known, &state.limits))
    }) {
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
    let result = with_policy(state, |policy| {
        let snap: SignedRegistry = serde_json::from_slice(&req.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        policy.accept_registry_admitted(&snap, Reading::now()?, || {
            charge(state, state.limiter.policy(false, &state.limits))
        })
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
    let result = with_policy(state, |policy| {
        let snap: SignedRevocations = serde_json::from_slice(&req.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        policy.accept_revocations_admitted(&snap, Reading::now()?, || {
            charge(state, state.limiter.policy(true, &state.limits))
        })
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

fn status_for(e: &DiscoveryError) -> u16 {
    match e {
        DiscoveryError::NotFound => 404,
        DiscoveryError::BadSignature => 401,
        DiscoveryError::Stale => 409,
        DiscoveryError::Expired => 410,
        DiscoveryError::RateLimited => 429,
        DiscoveryError::NotEnrolled => 403,
        DiscoveryError::InvalidRecord(_) => 400,
        _ => 500,
    }
}
