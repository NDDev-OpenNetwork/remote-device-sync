//! Endpoint-owned policy tasks. Completed tasks release storage immediately.

use std::sync::Mutex;

use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Default)]
pub(super) struct Drivers {
    tasks: TaskTracker,
    shutdown: CancellationToken,
    // Serialize task admission with shutdown: TaskTracker::close alone does
    // not prohibit new tasks, so it cannot provide this lifecycle boundary.
    admission: Mutex<()>,
}

impl Drivers {
    pub fn spawn(
        &self,
        conn: &noq::Connection,
        metrics: crate::metrics::Registry,
        local_addrs: Vec<std::net::SocketAddr>,
        candidates: Vec<std::net::SocketAddr>,
        peer_lease: Option<super::relay::PeerLease>,
    ) -> anyhow::Result<()> {
        let _guard = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        if self.tasks.is_closed() {
            conn.close(0u32.into(), b"endpoint closed");
            anyhow::bail!("endpoint closed before policy admission");
        }
        let weak = conn.weak_handle();
        let shutdown = self.shutdown.clone();
        // Subscribe before spawning, preserving the validation-event boundary.
        let policy = super::policy::connection_driver(
            weak.clone(),
            conn.nat_traversal_updates(),
            conn.path_events(),
            metrics,
            local_addrs,
            candidates,
        );
        self.tasks.spawn(async move {
            // Streams may outlive the Connection facade. The weak policy
            // lifetime, not facade drop, owns this metadata-only route lease.
            let _peer_lease = peer_lease;
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
        Ok(())
    }

    pub fn close_admission(&self) {
        let _guard = self.admission.lock().unwrap_or_else(|p| p.into_inner());
        self.tasks.close();
        self.shutdown.cancel();
    }

    pub async fn wait(&self) {
        self.tasks.wait().await;
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }
}
