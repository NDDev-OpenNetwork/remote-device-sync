//! In-process directory TLS. Trust is explicit; there is no insecure verifier.

use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

use crate::DiscoveryError;

fn config_error(error: impl std::fmt::Display) -> DiscoveryError {
    DiscoveryError::Configuration(format!("TLS: {error}"))
}

fn certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, DiscoveryError> {
    if pem.len() > 1024 * 1024 {
        return Err(config_error("certificate bundle exceeds 1 MiB"));
    }
    let certs: Vec<_> = CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(config_error)?;
    if certs.is_empty() {
        return Err(config_error("empty certificate bundle"));
    }
    Ok(certs)
}

/// Serve HTTPS with a PEM certificate chain and matching PEM private key.
/// Only HTTP/1.1 is negotiated. Certificates are loaded at startup; rotation
/// currently requires restarting the directory.
pub fn server_config_from_pem(
    chain: &[u8],
    key: &[u8],
) -> Result<Arc<ServerConfig>, DiscoveryError> {
    if key.len() > 64 * 1024 {
        return Err(config_error("private key exceeds 64 KiB"));
    }
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(config_error)?
            .with_no_client_auth()
            .with_single_cert(
                certificates(chain)?,
                PrivateKeyDer::from_pem_slice(key).map_err(config_error)?,
            )
            .map_err(config_error)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub(crate) fn client_config(ca: Option<&[u8]>) -> Result<Arc<ClientConfig>, DiscoveryError> {
    let mut roots = RootCertStore::empty();
    if let Some(pem) = ca {
        // A private bundle replaces public roots, so private deployments do
        // not silently trust an unrelated public authority too.
        for cert in certificates(pem)? {
            roots.add(cert).map_err(config_error)?;
        }
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(config_error)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
