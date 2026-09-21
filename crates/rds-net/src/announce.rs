//! Directory announce loop (WS3 D2).
//!
//! Publishes this endpoint's signed [`EndpointRecord`] on start, then
//! refreshes it at `ttl/3`, and re-publishes promptly when the
//! advertised address set changes (observed external addr, home relay,
//! new socket). Publish failures are logged and retried on the next
//! tick — announce must never silently die while the endpoint lives.

use std::time::{Duration, Instant};

use iroh::{EndpointAddr, SecretKey, TransportAddr};
use rds_discovery::client::Client as DirectoryClient;
use rds_discovery::{EndpointRecord, Service};

use crate::Endpoint;

/// Parameters for [`announce`].
#[derive(Debug, Clone)]
pub struct AnnounceConfig {
    /// Signing key matching the endpoint's identity.
    pub key: SecretKey,
    /// Directory to publish into.
    pub directory: DirectoryClient,
    /// Services this endpoint serves.
    pub services: Vec<Service>,
    /// Record TTL; the loop refreshes at `ttl/3`.
    pub ttl: Duration,
}

/// Advertised reachability: direct socket addrs + relay urls.
type Advertised = (Vec<std::net::SocketAddr>, Vec<String>);

/// Running announce task. `Drop` stops publishing; already-published
/// records expire naturally.
pub struct Announce {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Announce {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start the announce loop. The first publish happens inside the
/// returned task; the loop re-checks the advertised address set at
/// `min(1s, ttl/6)` so observed-addr changes publish promptly.
pub fn announce(endpoint: Endpoint, config: AnnounceConfig) -> Announce {
    let task = tokio::spawn(async move {
        let signing = ed25519_dalek::SigningKey::from_bytes(&config.key.to_bytes());
        let refresh = (config.ttl / 3).max(Duration::from_secs(1));
        let poll = (config.ttl / 6).clamp(Duration::from_millis(250), Duration::from_secs(5));
        let mut last: Option<(Advertised, Instant)> = None;
        loop {
            let current = split_addrs(&endpoint.addr());
            let due = last.as_ref().is_none_or(|(_, t)| t.elapsed() >= refresh);
            let changed = last.as_ref().is_none_or(|(prev, _)| *prev != current);
            if due || changed {
                match EndpointRecord::publish(
                    &signing,
                    current.0.clone(),
                    current.1.clone(),
                    config.services.clone(),
                    config.ttl,
                ) {
                    Ok(record) => match config.directory.publish(&record).await {
                        Ok(()) => {
                            last = Some((current, Instant::now()));
                            tracing::debug!("endpoint record published");
                        }
                        Err(e) => tracing::warn!("directory publish failed: {e}"),
                    },
                    Err(e) => tracing::warn!("record build failed: {e}"),
                }
            }
            tokio::time::sleep(poll).await;
        }
    });
    Announce { task }
}

/// Split an [`EndpointAddr`] into direct socket addrs + relay urls.
fn split_addrs(addr: &EndpointAddr) -> Advertised {
    let mut addrs = Vec::new();
    let mut relays = Vec::new();
    for t in &addr.addrs {
        match t {
            TransportAddr::Ip(sock) => addrs.push(*sock),
            TransportAddr::Relay(url) => relays.push(url.to_string()),
            _ => {}
        }
    }
    (addrs, relays)
}
