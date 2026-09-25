//! Policy-observed validated paths. Metadata never owns connection I/O.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{PathStats, PathStatsCoverage, PathStatsSnapshot};

pub(super) struct Telemetry(Mutex<State>);

struct State {
    paths: HashMap<noq::PathId, noq::WeakPathHandle>,
    selected: Option<noq::PathId>,
    lost_events: u64,
    running: bool,
}

impl Telemetry {
    /// Called after subscription, before returning the authenticated connection.
    pub fn new(conn: &noq::Connection) -> Arc<Self> {
        let mut paths = HashMap::new();
        let mut selected = None;
        if let Some(path) = conn.path(noq::PathId::ZERO)
            && let Ok(status) = path.status()
        {
            paths.insert(noq::PathId::ZERO, path.weak_handle());
            selected = (status == noq::PathStatus::Available).then_some(noq::PathId::ZERO);
        }
        Arc::new(Self(Mutex::new(State {
            paths,
            selected,
            lost_events: 0,
            running: true,
        })))
    }

    pub fn paths(&self) -> HashMap<noq::PathId, noq::WeakPathHandle> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .paths
            .clone()
    }

    pub fn publish(
        &self,
        paths: &HashMap<noq::PathId, noq::WeakPathHandle>,
        selected: Option<noq::PathId>,
    ) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.paths.clone_from(paths);
        state.selected = selected;
    }

    pub fn lagged(&self, count: u64) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.lost_events = state.lost_events.saturating_add(count);
        // Unknown paths may also be Available. Without reconciliation there
        // is no defensible selected-path estimate for pacing.
        state.selected = None;
    }

    pub fn snapshot(&self, closed: bool) -> PathStatsSnapshot {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let running = state.running && !closed;
        let mut paths = Vec::with_capacity(state.paths.len());
        let mut available = 0;
        if running {
            for (id, weak) in &state.paths {
                let Some(path) = weak.upgrade() else { continue };
                let Ok(status) = path.status() else { continue };
                available += usize::from(status == noq::PathStatus::Available);
                let Ok(remote) = path.remote_address() else {
                    continue;
                };
                let s = path.stats();
                paths.push(PathStats {
                    path_id: crate::path_id_u64(*id),
                    rtt: s.rtt,
                    cwnd: s.cwnd,
                    sent: s.udp_tx.datagrams,
                    lost: s.lost_packets,
                    sent_bytes: s.udp_tx.bytes,
                    recv_bytes: s.udp_rx.bytes,
                    congestion_events: s.congestion_events,
                    selected: state.lost_events == 0
                        && state.selected == Some(*id)
                        && status == noq::PathStatus::Available,
                    via_relay: super::relay::is_synthetic(remote),
                });
            }
        }
        // A status update can race this observation. If more than one known
        // path is Available, do not invent an exclusive pacing choice.
        if available != 1 {
            for path in &mut paths {
                path.selected = false;
            }
        }
        paths.sort_unstable_by_key(|path| path.path_id);
        PathStatsSnapshot {
            paths,
            coverage: PathStatsCoverage::PolicyObserved {
                lost_events: state.lost_events,
                driver_running: running,
            },
        }
    }

    pub fn guard(self: &Arc<Self>) -> ObserverGuard {
        ObserverGuard(self.clone())
    }
}

/// Constructed before spawning, so even cancellation before first poll marks
/// observation stopped. Neither this guard nor the metadata retain I/O.
pub(super) struct ObserverGuard(Arc<Telemetry>);

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
        state.running = false;
        state.selected = None;
        state.paths.clear();
    }
}
