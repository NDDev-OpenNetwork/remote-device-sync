//! Opt-in, authenticated loopback-only metrics listener, separate from every
//! public product listener. No proxy headers can grant access. See the admin
//! contract in docs/observability.md for deployment and snapshot semantics.

use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

mod http;
mod snapshot;
#[cfg(unix)]
mod token;
pub use snapshot::Snapshot;
#[cfg(unix)]
pub use token::Token;

#[cfg(not(unix))]
pub struct Token;
#[cfg(not(unix))]
impl Token {
    pub fn load(_: &std::path::Path) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }
    pub fn create(_: &std::path::Path) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn accepts(&self, _: &[u8]) -> bool {
        false
    }
}

const MAX_CONNECTIONS: usize = 16;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
type Source = Arc<dyn Fn() -> Snapshot + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("admin listener requires an explicit loopback IP and a private token file")]
    Configuration,
    #[error(
        "admin token requires a private, singly linked, owner-readable regular file owned by this user and containing 64 lowercase hex characters"
    )]
    TokenFile,
    #[error("secure admin token files are unsupported on this platform")]
    Unsupported,
    #[error("admin listener I/O failed")]
    Io(#[from] std::io::Error),
    #[error("admin worker failed")]
    Worker(#[source] tokio::task::JoinError),
    #[error("admin service failed")]
    Service(#[source] Arc<Error>),
}

/// Binds before product identities/catalogs are initialized, then starts only
/// after all metric sources exist. Dropping a prepared listener releases it.
pub struct Prepared {
    listener: TcpListener,
    token: Token,
    addr: SocketAddr,
}

impl Prepared {
    pub async fn bind(addr: SocketAddr, token: Token) -> Result<Self, Error> {
        if !addr.ip().is_loopback() {
            return Err(Error::Configuration);
        }
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        Ok(Self {
            listener,
            token,
            addr,
        })
    }
}

/// Shared daemon flags. Neither flag enables anything alone; credentials are
/// loaded once and are never read from a command-line value or logged.
#[cfg(feature = "cli")]
#[derive(clap::Args, Default)]
pub struct Args {
    /// Separate metrics listener; only explicit loopback IPs are accepted.
    #[arg(long, requires = "admin_token_file")]
    admin_addr: Option<SocketAddr>,
    /// Private 64-hex bearer token file (create with `rds admin-token --file`).
    #[arg(long, requires = "admin_addr")]
    admin_token_file: Option<std::path::PathBuf>,
}

#[cfg(feature = "cli")]
impl Args {
    pub async fn bind(self) -> Result<Option<Prepared>, Error> {
        match (self.admin_addr, self.admin_token_file) {
            (None, None) => Ok(None),
            (Some(addr), Some(path)) if addr.ip().is_loopback() => {
                let token = tokio::task::spawn_blocking(move || Token::load(&path))
                    .await
                    .map_err(Error::Worker)??;
                Prepared::bind(addr, token).await.map(Some)
            }
            _ => Err(Error::Configuration),
        }
    }
}

#[derive(Default)]
struct Counters {
    active: AtomicU64,
    accepted: AtomicU64,
    rejected: AtomicU64,
    requests: AtomicU64,
    scrapes: AtomicU64,
    unauthorized: AtomicU64,
    malformed: AtomicU64,
    timeouts: AtomicU64,
    io_errors: AtomicU64,
    snapshot_errors: AtomicU64,
}
impl Counters {
    fn snapshot(&self) -> Snapshot {
        Snapshot::from([
            (
                "rds_admin_connections_active",
                self.active.load(Ordering::Relaxed),
            ),
            ("rds_admin_connections_limit", MAX_CONNECTIONS as u64),
            (
                "rds_admin_connections_accepted_total",
                self.accepted.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_connections_rejected_total",
                self.rejected.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_requests_total",
                self.requests.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_scrapes_total",
                self.scrapes.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_unauthorized_total",
                self.unauthorized.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_malformed_total",
                self.malformed.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_timeouts_total",
                self.timeouts.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_io_errors_total",
                self.io_errors.load(Ordering::Relaxed),
            ),
            (
                "rds_admin_snapshot_errors_total",
                self.snapshot_errors.load(Ordering::Relaxed),
            ),
        ])
    }
}
struct Active(Arc<Counters>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Own until shutdown. An optional disabled listener has no tasks or sockets.
/// Drop aborts the runner and its owned request set; explicit close joins them.
/// Canceled/repeated observers preserve the runner result for subsequent close.
pub struct Server {
    addr: Option<SocketAddr>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<Result<(), Error>>>,
    outcome: Option<Result<(), Arc<Error>>>,
    counters: Arc<Counters>,
}

impl Server {
    pub fn start(
        prepared: Option<Prepared>,
        source: impl Fn() -> Snapshot + Send + Sync + 'static,
    ) -> Self {
        let counters = Arc::new(Counters::default());
        let (stop, receiver) = watch::channel(false);
        let addr = prepared.as_ref().map(|p| p.addr);
        let task = prepared.map(|prepared| {
            tokio::spawn(run(prepared, Arc::new(source), counters.clone(), receiver))
        });
        Self {
            addr,
            stop,
            task,
            outcome: None,
            counters,
        }
    }
    pub fn addr(&self) -> Option<SocketAddr> {
        self.addr
    }
    pub fn snapshot(&self) -> Snapshot {
        self.counters.snapshot()
    }

    async fn join(&mut self) -> Result<(), Error> {
        if let Some(task) = self.task.as_mut() {
            let result = match task.await {
                Ok(result) => result,
                Err(error) => Err(Error::Worker(error)),
            };
            self.outcome = Some(result.map_err(Arc::new));
            self.task = None;
        }
        self.outcome
            .as_ref()
            .cloned()
            .unwrap_or(Ok(()))
            .map_err(Error::Service)
    }
    pub async fn stopped(&mut self) -> Result<(), Error> {
        if self.addr.is_none() {
            return std::future::pending().await;
        }
        self.join().await
    }
    pub async fn close(&mut self) -> Result<(), Error> {
        self.stop.send_replace(true);
        self.join().await
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn run(
    prepared: Prepared,
    source: Source,
    counters: Arc<Counters>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let Prepared {
        listener, token, ..
    } = prepared;
    let token = Arc::new(token);
    let mut requests = JoinSet::new();
    let result = loop {
        // Observe the initial value as well: close may precede the first poll.
        if *stop.borrow() {
            break Ok(());
        }
        tokio::select! {
            biased;
            _ = stop.changed() => break Ok(()),
            completed = requests.join_next(), if !requests.is_empty() => {
                if let Some(Err(error)) = completed { break Err(Error::Worker(error)); }
            }
            accepted = listener.accept() => {
                let (mut socket, peer) = match accepted { Ok(pair) => pair, Err(error) => break Err(error.into()) };
                if requests.len() >= MAX_CONNECTIONS || !peer.ip().is_loopback() {
                    counters.rejected.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                counters.accepted.fetch_add(1, Ordering::Relaxed);
                counters.active.fetch_add(1, Ordering::Relaxed);
                let active = Active(counters.clone());
                let token = token.clone();
                let source = source.clone();
                requests.spawn(async move {
                    let result = tokio::time::timeout(REQUEST_TIMEOUT, http::serve(&mut socket, &token, &source, &active.0)).await;
                    match result {
                        Err(_) => { active.0.timeouts.fetch_add(1, Ordering::Relaxed); }
                        Ok(Err(_)) => { active.0.io_errors.fetch_add(1, Ordering::Relaxed); }
                        Ok(Ok(())) => {}
                    }
                });
            }
        }
    };
    drop(listener);
    requests.shutdown().await;
    result
}

#[cfg(test)]
mod tests;
