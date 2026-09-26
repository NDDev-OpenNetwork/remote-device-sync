//! Endpoint-owned policy tasks. Completed tasks release storage immediately.

use std::sync::{Arc, Mutex};

use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Default)]
pub(super) struct Drivers {
    /// Connection-scoped drivers — what `len()`/`active_path_drivers` counts.
    tasks: TaskTracker,
    /// Endpoint-scoped watchers (relay drain/unavailability fan-in) —
    /// they outlive any connection and are not path drivers.
    endpoint_tasks: TaskTracker,
    shutdown: CancellationToken,
    // Serialize task admission with shutdown: TaskTracker::close alone does
    // not prohibit new tasks, so it cannot provide this lifecycle boundary.
    admission: Mutex<()>,
    transport: Option<super::socket::Health>,
}

impl Drivers {
    pub fn new(transport: Option<super::socket::Health>) -> Self {
        Self {
            transport,
            ..Self::default()
        }
    }

    /// Park an endpoint-scoped watcher inside the same lifecycle: closed
    /// admission refuses new work, shutdown cancels, `wait` joins.
    pub fn spawn_endpoint(&self, task: impl Future<Output = ()> + Send + 'static) -> bool {
        let _guard = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        if self.endpoint_tasks.is_closed() {
            return false;
        }
        let shutdown = self.shutdown.clone();
        self.endpoint_tasks.spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {}
                _ = task => {}
            }
        });
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &self,
        conn: &noq::Connection,
        metrics: crate::metrics::Registry,
        local_addrs: Vec<std::net::SocketAddr>,
        candidates: Vec<std::net::SocketAddr>,
        peer_leases: Vec<super::relay::PeerLease>,
        relay_dead: tokio::sync::watch::Receiver<u64>,
        allow_direct: bool,
    ) -> anyhow::Result<Arc<super::telemetry::Telemetry>> {
        let _guard = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        if self.tasks.is_closed() {
            conn.close(0u32.into(), b"endpoint closed");
            anyhow::bail!("endpoint closed before policy admission");
        }
        if self
            .transport
            .as_ref()
            .is_some_and(super::socket::Health::all_failed)
        {
            conn.close(0u32.into(), b"all local transports failed");
            anyhow::bail!("all local transports failed before policy admission");
        }
        let weak = conn.weak_handle();
        let shutdown = self.shutdown.clone();
        // Subscribe before spawning, preserving the validation-event boundary.
        let events = conn.path_events();
        let telemetry = super::telemetry::Telemetry::new(conn);
        let observer = super::policy::Observer {
            events,
            telemetry: telemetry.clone(),
            transport: self.transport.clone(),
        };
        let guard = telemetry.guard();
        let policy = super::policy::connection_driver_observed(
            weak.clone(),
            conn.nat_traversal_updates(),
            observer,
            metrics,
            local_addrs,
            candidates,
            relay_dead,
            allow_direct,
        );
        self.tasks.spawn(async move {
            // Streams may outlive the Connection facade. The weak policy
            // lifetime, not facade drop, owns these metadata-only route leases.
            let _peer_leases = peer_leases;
            let _observer_guard = guard;
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    // Endpoint::close queues an event to noq's connection
                    // driver. After fatal sender I/O that driver may have
                    // stopped, so close state directly through a weak handle.
                    if let Some(conn) = weak.upgrade() {
                        conn.close(0u32.into(), b"endpoint closed");
                    }
                }
                _ = policy => {}
            }
        });
        Ok(telemetry)
    }

    pub fn close_admission(&self) {
        let _guard = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        self.tasks.close();
        self.endpoint_tasks.close();
        self.shutdown.cancel();
    }

    pub async fn wait(&self) {
        self.tasks.wait().await;
        self.endpoint_tasks.wait().await;
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }
}
