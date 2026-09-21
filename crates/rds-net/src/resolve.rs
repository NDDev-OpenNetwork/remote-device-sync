//! Target resolution (WS3 D3): `rds <cmd> <target>` accepts
//!
//! 1. a **ticket** — full `EndpointAddr` inline, no lookup needed;
//! 2. a **bare endpoint key** — answered by the directory when
//!    `--server` is set, else an addr-less `EndpointAddr` (dialable
//!    only if the peer was reachable out-of-band);
//! 3. a **device name** — resolved to a key through the estate-signed
//!    registry served by the directory, then the record fetch.
//!
//! The directory is only a cache of self-certifying records: the
//! resolved `EndpointAddr` is authenticated by the record signature,
//! and the QUIC handshake re-authenticates the key anyway. A hostile
//! or stale directory yields a failed dial, never a wrong peer.

use anyhow::{Context, bail};

use iroh::{EndpointAddr, EndpointId, TransportAddr};
use rds_discovery::client::Client as DirectoryClient;
use rds_discovery::{EndpointKey, EndpointRecord};

use crate::parse_target;

/// Resolve `target` to an [`EndpointAddr`].
///
/// `directory` is the discovery service address (the `--server` flag).
/// Names require it; bare keys use it when present.
pub async fn resolve_target(
    directory: Option<DirectoryClient>,
    target: &str,
) -> anyhow::Result<EndpointAddr> {
    // Tickets and bare keys are handled by the sync parser first.
    if let Ok(addr) = parse_target(target) {
        if !addr.addrs.is_empty() || directory.is_none() {
            return Ok(addr);
        }
        // Bare key + directory: fetch the published record.
        return fetch_record(&directory.unwrap(), addr.id).await;
    }
    // Device name → registry lookup → record fetch.
    let client = directory.context(format!(
        "{target:?} is not a ticket or endpoint key; name resolution needs --server"
    ))?;
    let key: EndpointKey = client
        .resolve_name(target)
        .await
        .with_context(|| format!("resolve name {target:?}"))?;
    let id = EndpointId::from_bytes(&key.0).context("registry returned a bad key")?;
    fetch_record(&client, id).await
}

async fn fetch_record(client: &DirectoryClient, id: EndpointId) -> anyhow::Result<EndpointAddr> {
    let key = EndpointKey(*id.as_bytes());
    let record: EndpointRecord = client
        .fetch(&key)
        .await
        .with_context(|| format!("fetch record for {id}"))?;
    let payload = record
        .verify_fresh()
        .context("stored record failed verification")?;
    anyhow::ensure!(
        payload.key.0 == *id.as_bytes(),
        "directory returned a record for the wrong key"
    );
    let mut addrs = std::collections::BTreeSet::new();
    for addr in payload.addrs {
        addrs.insert(TransportAddr::Ip(addr));
    }
    for url in payload.relay_urls {
        let url: crate::RelayUrl = url.parse().context("record carries a bad relay URL")?;
        addrs.insert(TransportAddr::Relay(url));
    }
    if addrs.is_empty() {
        bail!("record for {id} advertises no addresses");
    }
    Ok(EndpointAddr { id, addrs })
}
