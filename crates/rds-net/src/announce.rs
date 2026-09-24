//! Directory announce loop (WS3 D2).
//!
//! Publishes this endpoint's signed [`EndpointRecord`] on start, then
//! refreshes it at `ttl/3`, and re-publishes promptly when the
//! advertised address set changes (observed external addr, home relay,
//! new socket). Publish failures are logged and retried on the next
//! tick. Fatal local history or permanent protocol errors reach `Announce::wait`
//! so the agent supervisor can close the endpoint and report failure.

use std::time::Duration;

use iroh::{EndpointAddr, TransportAddr};
use rds_discovery::client::Client as DirectoryClient;
use rds_discovery::publisher::{RecordDraft, RecordIssuer};
use rds_discovery::{DiscoveryError, EndpointRecord, MAX_RECORD_TTL, Service};

use crate::Endpoint;

/// Parameters for [`announce`].
pub struct AnnounceConfig {
    /// Single-owner issuer matching the endpoint; durable in production.
    pub issuer: RecordIssuer,
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
    task: tokio::task::JoinHandle<Result<(), DiscoveryError>>,
}

impl Announce {
    /// Observe fatal issuer/disk failures. Network outages keep retrying; local
    /// history failures stop publication and must reach the process supervisor.
    pub async fn wait(&mut self) -> Result<(), DiscoveryError> {
        (&mut self.task)
            .await
            .map_err(|e| DiscoveryError::Store(e.to_string()))?
    }
}

impl Drop for Announce {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start the announce loop. The first publish happens inside the
/// returned task; the loop re-checks the advertised address set at
/// `min(1s, ttl/6)` so observed-addr changes publish promptly.
pub fn announce(endpoint: Endpoint, config: AnnounceConfig) -> Result<Announce, DiscoveryError> {
    if config.issuer.key().0 != *endpoint.id().as_bytes() {
        return Err(DiscoveryError::BadSignature);
    }
    if config.ttl.as_secs() == 0
        || config.ttl.as_secs() > MAX_RECORD_TTL
        || config.ttl.subsec_nanos() != 0
    {
        return Err(DiscoveryError::Configuration(
            "record TTL must be 1..=3600 whole seconds".into(),
        ));
    }
    let task = tokio::spawn(async move {
        let AnnounceConfig {
            mut issuer,
            directory,
            services,
            ttl,
        } = config;
        let poll = (ttl / 6).clamp(Duration::from_millis(250), Duration::from_secs(1));
        let mut last: Option<EndpointRecord> = None;
        let mut renew = false;
        loop {
            let current = split_addrs(&endpoint.addr());
            let draft = RecordDraft {
                addrs: current.0,
                relay_urls: current.1,
                services: services.clone(),
                ttl,
            };
            // At most one disk job. Cancellation can let that job finish a
            // local commit, but the canceled task cannot publish its result.
            let (returned, result) = tokio::task::spawn_blocking(move || {
                let result = rds_discovery::now_unix().and_then(|now| {
                    if renew {
                        issuer.renew_record(draft, now)
                    } else {
                        issuer.record(draft, now)
                    }
                });
                (issuer, result)
            })
            .await
            .map_err(|e| DiscoveryError::Store(e.to_string()))?;
            issuer = returned;
            renew = false;
            let record = result?;
            if last.as_ref() != Some(&record) {
                match directory.publish(&record).await {
                    Ok(()) => {
                        last = Some(record);
                        tracing::debug!("endpoint record published");
                    }
                    Err(DiscoveryError::Http { status: 410, .. }) => {
                        renew = true;
                        tracing::debug!("directory lease expired; allocating a local successor");
                    }
                    Err(
                        e @ DiscoveryError::Http {
                            status: 400..=407 | 409 | 411..=428 | 430..=499,
                            ..
                        },
                    ) => return Err(e),
                    Err(e) => tracing::warn!("directory publish failed: {e}"),
                }
            }
            tokio::time::sleep(poll).await;
        }
    });
    Ok(Announce { task })
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
