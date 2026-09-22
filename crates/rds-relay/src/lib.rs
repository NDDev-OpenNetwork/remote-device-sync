//! Relay server library for rds.
//!
//! Two generations live here:
//!
//! - [`iroh_server`] — the shipping relay: embedded `iroh-relay` with an
//!   endpoint allowlist. Forwards already-encrypted QUIC traffic; cannot
//!   read session content.
//! - [`proto`] — the owned relay protocol under design: datagram
//!   forwarding keyed by `EndpointId`, intended to run inside the
//!   `rds-server` composition on the GDS host.

pub mod proto;
#[cfg(feature = "owned-relay")]
pub mod server;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use iroh::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, AcmeConfig, CertConfig, ClientRequest, ConnectionId, RelayConfig,
    Server, ServerConfig, TlsConfig,
};

/// Admit only endpoint ids on the relay allowlist.
#[derive(Debug)]
struct AllowList(HashSet<EndpointId>);

impl AccessControl for AllowList {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        if self.0.contains(&request.endpoint_id()) {
            Access::Allow
        } else {
            Access::Deny {
                reason: Some("endpoint id not on relay allowlist".into()),
            }
        }
    }

    fn on_disconnect(&self, _endpoint_id: EndpointId, _connection_id: ConnectionId) {}
}

/// TLS mode for the relay's HTTPS listener.
///
/// When set, the relay protocol moves to `https_addr`; `addr` keeps
/// serving only the plaintext captive-portal probe. Endpoints then dial
/// `--relay https://<host>:<port>`.
#[derive(Debug)]
pub enum RelayTls {
    /// PEM certificate chain and private key files (e.g. certbot or an
    /// internal CA). Relayed payloads stay end-to-end encrypted either
    /// way; TLS here hides relay *usage* metadata from on-path observers.
    Manual {
        https_addr: SocketAddr,
        cert_pem: PathBuf,
        key_pem: PathBuf,
    },
    /// Let's Encrypt via in-process ACME (TLS-ALPN-01). Requires the
    /// HTTPS listener on port 443, reachable from the internet.
    LetsEncrypt {
        https_addr: SocketAddr,
        domains: Vec<String>,
        /// ACME contacts; emails need a `mailto:` prefix.
        contact: Vec<String>,
        /// Directory caching issued certificates across restarts.
        cache_dir: PathBuf,
        /// Use the LE staging directory (for tests; certs untrusted).
        staging: bool,
    },
}

/// Spawn the iroh relay on `addr`, restricted to `allow` when non-empty.
/// Returns the bound server; dropping it stops the relay.
pub async fn serve(
    addr: SocketAddr,
    allow: Vec<EndpointId>,
    tls: Option<RelayTls>,
) -> anyhow::Result<Server> {
    let mut relay_config = RelayConfig::new(addr);
    if !allow.is_empty() {
        relay_config.access = Arc::new(AllowList(allow.into_iter().collect()));
    }
    if let Some(tls) = tls {
        relay_config.tls = Some(tls_config(tls)?);
    }
    let mut config = ServerConfig::default();
    config.relay = Some(relay_config);
    Ok(Server::spawn(config).await?)
}

/// Resolve CLI-shaped TLS flags into a [`RelayTls`].
///
/// `None` when neither manual nor ACME flags are present. ACME requires a
/// cache directory: without one every restart re-issues certificates and
/// hits Let's Encrypt rate limits.
pub fn tls_from_flags(
    https_addr: SocketAddr,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    acme_domains: Vec<String>,
    acme_contact: Vec<String>,
    acme_cache: Option<PathBuf>,
    acme_staging: bool,
) -> anyhow::Result<Option<RelayTls>> {
    match (cert, key, acme_domains.is_empty()) {
        (Some(cert_pem), Some(key_pem), _) => Ok(Some(RelayTls::Manual {
            https_addr,
            cert_pem,
            key_pem,
        })),
        (None, None, false) => {
            let cache_dir = acme_cache
                .ok_or_else(|| anyhow::anyhow!("--tls-acme-cache is required for ACME"))?;
            Ok(Some(RelayTls::LetsEncrypt {
                https_addr,
                domains: acme_domains,
                contact: acme_contact,
                cache_dir,
                staging: acme_staging,
            }))
        }
        (None, None, true) => Ok(None),
        // clap `requires`/`conflicts_with` guard the halves.
        _ => anyhow::bail!("--tls-cert and --tls-key must be given together"),
    }
}

fn tls_config(tls: RelayTls) -> anyhow::Result<TlsConfig> {
    // ring: pure-Rust provider already in the tree; aws-lc-rs would add a
    // C build dependency for no gain here.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?;
    Ok(match tls {
        RelayTls::Manual {
            https_addr,
            cert_pem,
            key_pem,
        } => {
            use rustls_pki_types::pem::PemObject;
            let certs = rustls_pki_types::CertificateDer::pem_file_iter(&cert_pem)?
                .collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pki_types::PrivateKeyDer::from_pem_file(&key_pem)?;
            let server_config = builder.with_no_client_auth().with_single_cert(certs, key)?;
            TlsConfig::new(https_addr, CertConfig::Manual { server_config })
        }
        RelayTls::LetsEncrypt {
            https_addr,
            domains,
            contact,
            cache_dir,
            staging,
        } => {
            let acme = AcmeConfig::letsencrypt(!staging)
                .domains(domains)
                .contact(contact)
                .cache_path(cache_dir);
            TlsConfig::new(
                https_addr,
                CertConfig::LetsEncrypt {
                    acme_config: acme,
                    // iroh-relay injects the ACME cert resolver itself.
                    server_config_builder: builder.with_no_client_auth(),
                },
            )
        }
    })
}
