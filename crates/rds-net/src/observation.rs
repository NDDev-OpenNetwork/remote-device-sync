//! Internal metadata observer. Never owns a facade, uni router or transport I/O.
use std::{future::Future, pin::Pin};

use crate::{Connection, ConnectionInner, PathStats, PathStatsCoverage, PathStatsSnapshot};

pub(crate) enum Observer {
    Iroh(iroh::endpoint::WeakConnectionHandle),
    #[cfg(feature = "transport-noq")]
    Noq(crate::backends::noq::ConnectionObserver),
}

impl Observer {
    pub fn new(connection: &Connection) -> Self {
        match &connection.inner {
            ConnectionInner::Iroh(c) => Self::Iroh(c.weak_handle()),
            #[cfg(feature = "transport-noq")]
            ConnectionInner::Noq(c) => Self::Noq(c.observer()),
        }
    }

    pub fn snapshot(&self) -> PathStatsSnapshot {
        match self {
            Self::Iroh(weak) => weak.upgrade().map_or_else(
                || PathStatsSnapshot {
                    paths: Vec::new(),
                    coverage: PathStatsCoverage::BackendSnapshot,
                },
                |conn| iroh_snapshot(&conn),
            ),
            #[cfg(feature = "transport-noq")]
            Self::Noq(observer) => observer.snapshot(),
        }
    }

    /// Register synchronously, retain no strong connection across the await.
    pub fn closed(&self) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        match self {
            Self::Iroh(weak) => {
                let closed = weak.closed();
                Box::pin(async move {
                    closed.await;
                })
            }
            #[cfg(feature = "transport-noq")]
            Self::Noq(observer) => Box::pin(observer.closed()),
        }
    }
}

pub(crate) fn iroh_snapshot(conn: &iroh::endpoint::Connection) -> PathStatsSnapshot {
    let paths = if conn.close_reason().is_some() {
        Vec::new()
    } else {
        conn.paths()
            .iter()
            .map(|path| {
                let stats = path.stats();
                PathStats {
                    path_id: crate::path_id_u64(path.id()),
                    rtt: stats.rtt,
                    cwnd: stats.cwnd,
                    sent: stats.udp_tx.datagrams,
                    lost: stats.lost_packets,
                    sent_bytes: stats.udp_tx.bytes,
                    recv_bytes: stats.udp_rx.bytes,
                    congestion_events: stats.congestion_events,
                    selected: path.is_selected(),
                    via_relay: path.is_relay(),
                }
            })
            .collect()
    };
    PathStatsSnapshot {
        paths,
        coverage: PathStatsCoverage::BackendSnapshot,
    }
}
