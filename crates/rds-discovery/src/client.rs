//! Directory client: publish, fetch, delete, name resolution.
//!
//! One TCP connection per request with a single overall timeout —
//! the directory is a control-plane dependency, and a hanging lookup
//! must never wedge an announce loop or a CLI resolve.

use std::net::SocketAddr;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use tokio::net::TcpStream;

use crate::http::{self, Response};
use crate::registry::SignedRegistry;
use crate::{DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord};

/// Client for one directory address.
#[derive(Debug, Clone)]
pub struct Client {
    addr: SocketAddr,
    timeout: Duration,
}

impl Client {
    /// Default timeout is 3s — generous for a LAN control plane, tight
    /// enough that an unreachable directory fails fast.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            timeout: Duration::from_secs(3),
        }
    }

    pub fn with_timeout(addr: SocketAddr, timeout: Duration) -> Self {
        Self { addr, timeout }
    }

    /// Directory address this client talks to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Publish a signed record (`PUT /v1/records`).
    pub async fn publish(&self, record: &EndpointRecord) -> Result<(), DiscoveryError> {
        let body =
            serde_json::to_vec(record).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let resp = self.request("PUT", "/v1/records", &body).await?;
        self.expect(resp, &[200]).map(|_| ())
    }

    /// Fetch the stored record for `key` and verify its signature.
    /// Expiry is left to the caller via `verify`/`verify_fresh`.
    pub async fn fetch(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        let resp = self
            .request("GET", &format!("/v1/records/{key}"), &[])
            .await?;
        let resp = self.expect(resp, &[200])?;
        serde_json::from_slice(&resp.body).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))
    }

    /// Resolve a device name to an [`EndpointKey`] via the estate
    /// registry (`GET /v1/names/{name}`).
    pub async fn resolve_name(&self, name: &str) -> Result<EndpointKey, DiscoveryError> {
        let resp = self
            .request("GET", &format!("/v1/names/{name}"), &[])
            .await?;
        let resp = self.expect(resp, &[200])?;
        #[derive(serde::Deserialize)]
        struct NameAnswer {
            key: String,
        }
        let answer: NameAnswer = serde_json::from_slice(&resp.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        answer.key.parse()
    }

    /// Delete `key`'s record with a signed tombstone (`DELETE`).
    pub async fn remove(
        &self,
        key: &EndpointKey,
        signing: &SigningKey,
    ) -> Result<(), DiscoveryError> {
        let tomb = DeleteRequest::new(signing)?;
        debug_assert_eq!(
            tomb.verify()?.key,
            *key,
            "tombstone key must match the delete path"
        );
        let body =
            serde_json::to_vec(&tomb).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let resp = self
            .request("DELETE", &format!("/v1/records/{key}"), &body)
            .await?;
        self.expect(resp, &[200]).map(|_| ())
    }

    /// Replace the registry snapshot (`PUT /v1/registry`).
    pub async fn update_registry(&self, snap: &SignedRegistry) -> Result<(), DiscoveryError> {
        let body =
            serde_json::to_vec(snap).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let resp = self.request("PUT", "/v1/registry", &body).await?;
        self.expect(resp, &[200]).map(|_| ())
    }

    /// `GET /v1/health`.
    pub async fn health(&self) -> Result<(), DiscoveryError> {
        let resp = self.request("GET", "/v1/health", &[]).await?;
        self.expect(resp, &[200]).map(|_| ())
    }

    /// `GET /v1/metrics` — raw prometheus text.
    pub async fn metrics(&self) -> Result<String, DiscoveryError> {
        let resp = self.request("GET", "/v1/metrics", &[]).await?;
        let resp = self.expect(resp, &[200])?;
        String::from_utf8(resp.body).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<Response, DiscoveryError> {
        let addr = self.addr;
        tokio::time::timeout(self.timeout, async move {
            let mut sock = TcpStream::connect(addr)
                .await
                .map_err(|e| DiscoveryError::Unreachable(e.to_string()))?;
            http::write_request(&mut sock, method, path, body).await?;
            http::read_response(&mut sock).await
        })
        .await
        .map_err(|_| DiscoveryError::Unreachable("request timed out".into()))?
    }

    fn expect(&self, resp: Response, ok: &[u16]) -> Result<Response, DiscoveryError> {
        if ok.contains(&resp.status) {
            return Ok(resp);
        }
        let message = serde_json::from_slice::<serde_json::Value>(&resp.body)
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| String::from_utf8_lossy(&resp.body).into_owned());
        Err(match resp.status {
            429 => DiscoveryError::RateLimited,
            status => DiscoveryError::Http { status, message },
        })
    }
}
