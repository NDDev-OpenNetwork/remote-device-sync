//! Directory client: publish, fetch, delete, name resolution.
//!
//! One HTTP(S) exchange per request with a single overall timeout —
//! the directory is a control-plane dependency, and a hanging lookup
//! must never wedge an announce loop or a CLI resolve.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use rustls_pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_rustls::TlsConnector;

use crate::http::{self, Response};
use crate::registry::{SignedNameBinding, SignedRegistry, valid_name};
use crate::{DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord};
use crate::{
    authority::{Authority, SignedRotation},
    clock::Reading,
    policy::PolicyStore,
};

/// Client for one directory address.
#[derive(Debug, Clone)]
pub struct Client {
    host: String,
    port: u16,
    authority: String,
    tls: Option<Arc<rustls::ClientConfig>>,
    timeout: Duration,
    name_trust: Option<NameTrust>,
}

impl Client {
    /// Default timeout is 3s — generous for a LAN control plane, tight
    /// enough that an unreachable directory fails fast.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            host: addr.ip().to_string(),
            port: addr.port(),
            authority: addr.to_string(),
            tls: None,
            timeout: Duration::from_secs(3),
            name_trust: None,
        }
    }

    /// HTTPS/HTTP origin with a DNS name or IP literal, or a legacy plaintext
    /// socket address. Credentials, paths, query and fragment are refused.
    /// HTTPS verifies public WebPKI roots and the configured hostname/IP SAN.
    pub fn from_endpoint(endpoint: &str) -> Result<Self, DiscoveryError> {
        if let Ok(addr) = endpoint.parse::<SocketAddr>() {
            return Ok(Self::new(addr));
        }
        let invalid = || {
            DiscoveryError::Configuration(
                "expected http(s)://host[:port] without credentials, path, query or fragment"
                    .into(),
            )
        };
        if endpoint.len() > 2048
            || endpoint
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
            || !(endpoint.starts_with("https://") || endpoint.starts_with("http://"))
        {
            return Err(invalid());
        }
        let remainder = endpoint.split_once("://").ok_or_else(invalid)?.1;
        let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
        if authority_end == 0
            || remainder[..authority_end].contains('@')
            || !matches!(&remainder[authority_end..], "" | "/")
        {
            return Err(invalid());
        }
        let url = url::Url::parse(endpoint).map_err(|_| invalid())?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        let host = match url.host().ok_or_else(invalid)? {
            url::Host::Domain(name) => name.to_owned(),
            url::Host::Ipv4(ip) => ip.to_string(),
            url::Host::Ipv6(ip) => ip.to_string(),
        };
        let port = url
            .port_or_known_default()
            .filter(|port| *port != 0)
            .ok_or_else(invalid)?;
        let authority = match url.host().ok_or_else(invalid)? {
            url::Host::Ipv6(_) => format!("[{host}]:{port}"),
            _ => format!("{host}:{port}"),
        };
        let tls = if url.scheme() == "https" {
            ServerName::try_from(host.clone()).map_err(|_| invalid())?;
            Some(crate::tls::client_config(None)?)
        } else {
            None
        };
        Ok(Self {
            host,
            port,
            authority,
            tls,
            timeout: Duration::from_secs(3),
            name_trust: None,
        })
    }

    /// Use only this PEM CA bundle for HTTPS. Refused on a plaintext endpoint.
    pub fn with_ca_pem(mut self, pem: &[u8]) -> Result<Self, DiscoveryError> {
        if self.tls.is_none() {
            return Err(DiscoveryError::Configuration(
                "CA bundle requires HTTPS".into(),
            ));
        }
        self.tls = Some(crate::tls::client_config(Some(pem))?);
        Ok(self)
    }

    /// Absolute DNS + TCP + TLS + HTTP deadline, including all dial candidates.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_timeout(addr: SocketAddr, timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::new(addr)
        }
    }

    /// Trust anchor provisioned independently of the directory response.
    /// Ephemeral embedding/test cache; production callers use `with_registry_store`.
    /// Reconfiguration starts a separate authority/cache namespace.
    pub fn with_registry_key(mut self, key: VerifyingKey) -> Self {
        // The strongly typed key and fixed positive epoch cannot fail validation.
        let authority = Authority::new(&key, 1).expect("fixed positive authority epoch");
        let store = PolicyStore::memory(authority).expect("validated authority");
        self.name_trust = Some(NameTrust::Memory(Arc::new(Mutex::new(store))));
        self
    }

    /// Durable name trust. Each lookup opens a short exclusive transaction so
    /// concurrent CLI processes share one high-water mark. No volatile fallback.
    pub fn with_registry_store(
        mut self,
        authority: Authority,
        path: PathBuf,
        rotations: Vec<SignedRotation>,
    ) -> Result<Self, DiscoveryError> {
        authority.verifying_key()?;
        if rotations.len() > 16 {
            return Err(DiscoveryError::Configuration(
                "too many rotation receipts".into(),
            ));
        }
        self.name_trust = Some(NameTrust::Persistent {
            authority,
            path,
            rotations,
        });
        Ok(self)
    }

    /// Base32 configuration form, shared by applications without duplicating
    /// key parsing or accepting trust keys from network responses.
    pub fn with_registry_key_base32(self, key: &str) -> Result<Self, DiscoveryError> {
        let key: EndpointKey = key.parse()?;
        let key = VerifyingKey::from_bytes(&key.0)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        Ok(self.with_registry_key(key))
    }

    /// Literal address, if configured with an IP. DNS is resolved per request.
    pub fn addr(&self) -> Option<SocketAddr> {
        self.host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, self.port))
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
        let trust = self.name_trust.as_ref().ok_or_else(|| {
            DiscoveryError::InvalidRecord("name resolution requires a trusted registry key".into())
        })?;
        let resp = self
            .request("GET", &format!("/v1/names/{name}"), &[])
            .await?;
        let resp = self.expect(resp, &[200])?;
        let answer: SignedNameBinding = serde_json::from_slice(&resp.body)
            .map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
        let name = name.to_owned();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let trust = trust.clone();
                let answer = answer.clone();
                let name = name.clone();
                let result = tokio::task::spawn_blocking(move || trust.accept(answer, &name))
                    .await
                    .map_err(|e| DiscoveryError::Store(e.to_string()))?;
                match result {
                    Err(DiscoveryError::Busy) => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    other => return other,
                }
            }
        })
        .await
        .map_err(|_| DiscoveryError::Unreachable("policy acceptance timed out".into()))?
    }

    /// Send an already signed, revisioned deletion. Revision allocation belongs
    /// to the publisher; retries must preserve the exact mutation.
    pub async fn remove(&self, tomb: &DeleteRequest) -> Result<(), DiscoveryError> {
        let key = tomb.verify_fresh()?.key;
        let body =
            serde_json::to_vec(tomb).map_err(|e| DiscoveryError::InvalidRecord(e.to_string()))?;
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
        tokio::time::timeout(self.timeout, async move {
            let mut sock = self.connect().await?;
            sock.set_nodelay(true)
                .map_err(|e| DiscoveryError::Unreachable(e.to_string()))?;
            if let Some(config) = &self.tls {
                let name = ServerName::try_from(self.host.clone())
                    .map_err(|e| DiscoveryError::Configuration(e.to_string()))?;
                let mut tls = TlsConnector::from(config.clone())
                    .connect(name, sock)
                    .await
                    .map_err(|e| DiscoveryError::Unreachable(format!("TLS handshake: {e}")))?;
                http::write_request_with_host(&mut tls, &self.authority, method, path, body)
                    .await?;
                http::read_response(&mut tls).await
            } else {
                http::write_request_with_host(&mut sock, &self.authority, method, path, body)
                    .await?;
                http::read_response(&mut sock).await
            }
        })
        .await
        .map_err(|_| DiscoveryError::Unreachable("request timed out".into()))?
    }

    async fn connect(&self) -> Result<TcpStream, DiscoveryError> {
        let unreachable = |e: std::io::Error| DiscoveryError::Unreachable(e.to_string());
        if let Some(addr) = self.addr() {
            return TcpStream::connect(addr).await.map_err(unreachable);
        }
        let addresses = tokio::net::lookup_host((self.host.as_str(), self.port))
            .await
            .map_err(unreachable)?;
        // Bound sockets/tasks and stagger candidates. A dead first address
        // must not consume the entire request deadline before IPv4/IPv6 fallback.
        let mut attempts = JoinSet::new();
        for (i, addr) in addresses.take(16).enumerate() {
            attempts.spawn(async move {
                if i != 0 {
                    tokio::time::sleep(Duration::from_millis(i as u64 * 100)).await;
                }
                TcpStream::connect(addr).await
            });
        }
        let mut last = "DNS returned no addresses".to_string();
        while let Some(result) = attempts.join_next().await {
            match result {
                Ok(Ok(sock)) => return Ok(sock), // JoinSet drops/aborts losers.
                Ok(Err(e)) => last = e.to_string(),
                Err(e) => last = e.to_string(),
            }
        }
        Err(DiscoveryError::Unreachable(last))
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

#[derive(Clone)]
enum NameTrust {
    Memory(Arc<Mutex<PolicyStore>>),
    Persistent {
        authority: Authority,
        path: PathBuf,
        rotations: Vec<SignedRotation>,
    },
}

impl std::fmt::Debug for NameTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Memory(_) => "MemoryNameTrust",
            Self::Persistent { .. } => "PersistentNameTrust",
        })
    }
}

impl NameTrust {
    fn accept(&self, proof: SignedNameBinding, name: &str) -> Result<EndpointKey, DiscoveryError> {
        let now = Reading::now()?;
        let binding = match self {
            Self::Memory(store) => {
                let mut store = store
                    .lock()
                    .map_err(|_| DiscoveryError::Store("name state poisoned".into()))?;
                let binding = store.accept_name(&proof, name, now)?;
                store.check_name_lease(Reading::now()?)?;
                binding
            }
            Self::Persistent {
                authority,
                path,
                rotations,
            } => {
                let mut store = PolicyStore::open(path, *authority, now)?;
                for receipt in rotations {
                    store.apply_rotation(receipt, now)?;
                }
                let binding = store.accept_name(&proof, name, now)?;
                store.check_name_lease(Reading::now()?)?;
                binding
            }
        };
        Ok(binding.key)
    }
}
