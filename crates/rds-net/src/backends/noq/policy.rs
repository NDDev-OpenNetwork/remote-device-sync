//! Path policy: candidate ordering and path opening.
//!
//! The first cut is deliberately conservative: deduplicate, cap the
//! candidate set at the multipath path limit, open the first candidate as
//! the primary path, then offer the rest via `open_path_ensure`. Relay
//! candidates are ignored until the relay transport lands (WS2); NAT
//! traversal rounds are driven by the connection driver task.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use iroh::{EndpointAddr, TransportAddr};

/// Maximum address candidates considered per connect.
///
/// Matches `max_concurrent_multipath_paths` on the transport config —
/// opening more paths than the peer accepts is wasted work.
pub const MAX_CANDIDATES: usize = 8;

/// Extract direct IP candidates from an advertised address.
///
/// Order is deterministic (sorted by addr) so benchmarks reproduce.
/// Relay addresses are skipped here; they enter through the socket mux
/// once `relay_link` lands.
pub fn ip_candidates(addr: &EndpointAddr) -> Vec<SocketAddr> {
    let set: BTreeSet<SocketAddr> = addr
        .addrs
        .iter()
        .filter_map(|a| match a {
            TransportAddr::Ip(sock) => Some(*sock),
            _ => None,
        })
        .collect();
    set.into_iter().take(MAX_CANDIDATES).collect()
}

/// Open additional paths for the remaining candidates.
///
/// `open_path_ensure` deduplicates against the path `connect` already
/// opened, so passing the primary address again is safe. Failures are
/// logged, not fatal: any established path keeps the connection alive.
pub fn open_extra_paths(conn: &noq::Connection, candidates: &[SocketAddr]) {
    for addr in candidates {
        let open = conn.open_path_ensure(
            noq::FourTuple::from_remote(*addr),
            noq::PathStatus::Available,
        );
        match open.path_id() {
            Some(id) => tracing::debug!(%addr, ?id, "opening extra path"),
            None => tracing::debug!(%addr, "extra path rejected"),
        }
    }
}
