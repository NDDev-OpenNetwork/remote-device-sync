//! Directory client: publish, fetch, delete, name resolution.
//!
//! One TCP connection per request with a single overall timeout —
//! the directory is a control-plane dependency, and a hanging lookup
//! must never wedge an announce loop or a CLI resolve.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::{SigningKey, VerifyingKey};
use tokio::net::TcpStream;

use crate::http::{self, Response};
use crate::registry::{NameBindingPayload, SignedNameBinding, SignedRegistry, valid_name};
use crate::{DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord};

/// Client for one directory address.
#[derive(Debug, Clone)]
pub struct Client {
    addr: SocketAddr,
    timeout: Duration,
    registry_key: Option<VerifyingKey>,
    // Shared by cloned clients. Durable anti-rollback state is W1.4;
    // this bounded cache prevents regression within one client lifetime.
    seen_names: Arc<Mutex<BTreeMap<String, NameBindingPayload>>>,
}

impl Client {
    /// Default timeout is 3s — generous for a LAN control plane, tight
    /// enough that an unreachable directory fails fast.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            timeout: Duration::from_secs(3),
            registry_key: None,
            seen_names: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn with_timeout(addr: SocketAddr, timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::new(addr)
        }
    }

    /// Trust anchor provisioned independently of the directory response.
    /// Reconfiguration starts a separate authority/cache namespace.
    pub fn with_registry_key(mut self, key: VerifyingKey) -> Self {
        self.registry_key = Some(key);
        self.seen_names = Arc::new(Mutex::new(BTreeMap::new()));
        self
    }

    /// Base32 configuration form, shared by applications without duplicating
    /// key parsing or accepting trust keys from network responses.
    pub fn with_registry_key_base32(self, key: &str) -> Result<Self, DiscoveryError> {
        let key: EndpointKey = key.parse()?;
        let key = VerifyingKey::from_bytes(&key.0)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        Ok(self.with_registry_key(key))
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

    /// Fetch an untrusted record envelope. The caller must verify signature,
    /// expected key and freshness before consuming its addresses.
    pub async fn fetch(&self, key: &EndpointKey) -> Result<EndpointRecord, DiscoveryError> {
        let resp = self
            .request("GET", &format!("/v1/records/{key}"), &[])
            .await?;
        let resp = self.expect(resp, &[200])?;
        serde_json::from_slice(&resp.body).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))
    }

    /// Resolve a device name to an [`EndpointKey`] via the estate
    /// registry (`GET /v1/names/{name}`). Requires an independently configured
    /// registry key; unsigned/legacy responses never select a peer identity.
    pub async fn resolve_name(&self, name: &str) -> Result<EndpointKey, DiscoveryError> {
        if !valid_name(name) {
            return Err(DiscoveryError::InvalidRecord("invalid device name".into()));
        }
        let key = self.registry_key.as_ref().ok_or_else(|| {
            DiscoveryError::InvalidRecord("name resolution requires a trusted registry key".into())
        })?;
        let resp = self
            .request("GET", &format!("/v1/names/{name}"), &[])
            .await?;
        let resp = self.expect(resp, &[200])?;
        let answer: SignedNameBinding = serde_json::from_slice(&resp.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let binding = answer.verify(key, name, crate::now_unix()?)?;
        let mut seen = self
            .seen_names
            .lock()
            .map_err(|_| DiscoveryError::Store("name cache poisoned".into()))?;
        if let Some(previous) = seen.get(name) {
            if binding.issued_at < previous.issued_at
                || (binding.issued_at == previous.issued_at && binding != *previous)
            {
                return Err(DiscoveryError::Stale);
            }
        } else if seen.len() >= 1024 {
            return Err(DiscoveryError::Store("name freshness cache full".into()));
        }
        let endpoint = binding.key;
        seen.insert(name.into(), binding);
        Ok(endpoint)
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

    /// Fetch the current grant-revocation snapshot (`GET /v1/revocations`).
    /// `None` when the estate has not published one yet.
    pub async fn fetch_revocations(
        &self,
    ) -> Result<Option<crate::revocations::SignedRevocations>, DiscoveryError> {
        let resp = self.request("GET", "/v1/revocations", &[]).await?;
        if resp.status == 404 {
            return Ok(None);
        }
        let resp = self.expect(resp, &[200])?;
        serde_json::from_slice(&resp.body)
            .map(Some)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))
    }

    /// Replace the denylist snapshot (`PUT /v1/revocations`).
    pub async fn update_revocations(
        &self,
        snap: &crate::revocations::SignedRevocations,
    ) -> Result<(), DiscoveryError> {
        let body =
            serde_json::to_vec(snap).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let resp = self.request("PUT", "/v1/revocations", &body).await?;
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
