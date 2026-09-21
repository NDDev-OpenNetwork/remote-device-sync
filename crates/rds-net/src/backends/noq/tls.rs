//! TLS configuration for the owned backend: raw public keys per [RFC 7250].
//!
//! The rds endpoint identity is an Ed25519 key pair. Instead of X.509
//! certificates the TLS layer transports the bare public key as a
//! `SubjectPublicKeyInfo` DER blob — the same scheme iroh uses, which makes
//! this backend wire-compatible with the shipping one on `rds/0`:
//!
//! - Server name encodes the expected peer `EndpointId`
//!   (`<base32>.iroh.invalid`); it is only used locally by the verifier and
//!   for session-ticket bucketing — SNI is disabled and never goes on the
//!   wire.
//! - The "certificate" presented by each side is the SPKI of its Ed25519
//!   key; the verifier checks it byte-for-byte against the expected id.
//!
//! [RFC 7250]: https://datatracker.ietf.org/doc/html/rfc7250

use std::sync::Arc;

use iroh::SecretKey;
use noq::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use noq::rustls;

/// TLS 1.3 only: QUIC requires it and nothing older is offered.
const PROTOCOL_VERSIONS: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

/// Server-name encoding of an `EndpointId`.
///
/// Format: `<BASE32_DNSSEC(id)>.iroh.invalid`. Base32 keeps us under the
/// 63-byte DNS label limit; `.invalid` (RFC 2606) can never resolve. The
/// name is used only locally — to carry the expected peer id into the
/// certificate verifier and to bucket 0-RTT session tickets per peer.
pub mod name {
    use data_encoding::BASE32_DNSSEC;
    use iroh::EndpointId;

    pub fn encode(endpoint_id: EndpointId) -> String {
        format!(
            "{}.iroh.invalid",
            BASE32_DNSSEC.encode(endpoint_id.as_bytes())
        )
    }

    pub fn decode(name: &str) -> Option<EndpointId> {
        let [base32_endpoint_id, "iroh", "invalid"] = name.split(".").collect::<Vec<_>>()[..]
        else {
            return None;
        };
        EndpointId::from_bytes(
            &BASE32_DNSSEC
                .decode(base32_endpoint_id.as_bytes())
                .ok()?
                .try_into()
                .ok()?,
        )
        .ok()
    }
}

/// Builds QUIC TLS configs for both roles from one identity.
///
/// The resolvers/verifiers are kept in `Arc`s because rustls compares
/// verifier identity across resumed sessions for 0-RTT.
#[derive(Debug)]
pub struct TlsConfig {
    cert_resolver: Arc<ResolveRawPublicKeyCert>,
    server_verifier: Arc<ServerCertificateVerifier>,
    client_verifier: Arc<ClientCertificateVerifier>,
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

/// Maximum TLS session tickets cached per endpoint for 0-RTT resumption.
const MAX_TLS_TICKETS: usize = 8 * 32;

impl TlsConfig {
    pub fn new(secret_key: SecretKey) -> Self {
        Self {
            cert_resolver: Arc::new(ResolveRawPublicKeyCert::new(&secret_key)),
            server_verifier: Arc::new(ServerCertificateVerifier),
            client_verifier: Arc::new(ClientCertificateVerifier),
            crypto_provider: Arc::new(rustls::crypto::ring::default_provider()),
        }
    }

    /// Client QUIC TLS config: verifies the server's raw public key against
    /// the `EndpointId` encoded in the server name, presents our own key as
    /// client certificate. `alpns` are the offered QUIC ALPN protocol ids.
    pub fn client_config(&self, alpns: Vec<Vec<u8>>) -> anyhow::Result<QuicClientConfig> {
        let mut crypto = rustls::ClientConfig::builder_with_provider(self.crypto_provider.clone())
            .with_protocol_versions(PROTOCOL_VERSIONS)?
            .dangerous()
            .with_custom_certificate_verifier(self.server_verifier.clone())
            .with_client_cert_resolver(self.cert_resolver.clone());

        crypto.resumption = rustls::client::Resumption::store(Arc::new(
            rustls::client::ClientSessionMemoryCache::new(MAX_TLS_TICKETS),
        ));
        crypto.enable_early_data = true;
        // The server name is used locally only; do not disclose the
        // peer id in ClientHello SNI.
        crypto.enable_sni = false;
        crypto.alpn_protocols = alpns;

        Ok(QuicClientConfig::try_from(crypto)?)
    }

    /// Server QUIC TLS config: requires a client raw public key (any key —
    /// the signature proof authenticates it) and presents ours.
    pub fn server_config(&self, alpns: Vec<Vec<u8>>) -> anyhow::Result<QuicServerConfig> {
        let mut crypto = rustls::ServerConfig::builder_with_provider(self.crypto_provider.clone())
            .with_protocol_versions(PROTOCOL_VERSIONS)?
            .with_client_cert_verifier(self.client_verifier.clone())
            .with_cert_resolver(self.cert_resolver.clone());

        // RFC 9001 §4.6.1: must be u32::MAX or 0 for QUIC.
        crypto.max_early_data_size = u32::MAX;
        crypto.alpn_protocols = alpns;

        Ok(QuicServerConfig::try_from(crypto)?)
    }
}

/// Resolves our Ed25519 key into a "certificate" for both TLS roles.
///
/// The certificate blob is the SPKI DER encoding of the public key; the
/// `only_raw_public_keys` markers make rustls negotiate RFC 7250.
#[derive(Debug)]
struct ResolveRawPublicKeyCert {
    key: Arc<rustls::sign::CertifiedKey>,
}

impl ResolveRawPublicKeyCert {
    fn new(secret_key: &SecretKey) -> Self {
        let signing_key = Arc::new(SecretKeySigner {
            key: secret_key.clone(),
        });
        let spki = signing_key.spki_public_key();
        let cert = rustls::pki_types::CertificateDer::from(spki.to_vec());
        let key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert], signing_key));
        Self { key }
    }
}

impl rustls::client::ResolvesClientCert for ResolveRawPublicKeyCert {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.key))
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }

    fn has_certs(&self) -> bool {
        true
    }
}

impl rustls::server::ResolvesServerCert for ResolveRawPublicKeyCert {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.key))
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

/// `rustls::sign::SigningKey` over an rds Ed25519 secret key.
#[derive(Debug, Clone)]
struct SecretKeySigner {
    key: SecretKey,
}

impl SecretKeySigner {
    fn spki_public_key(&self) -> rustls::pki_types::SubjectPublicKeyInfoDer<'static> {
        rustls::sign::public_key_to_spki(
            &rustls::pki_types::alg_id::ED25519,
            self.key.public().as_bytes(),
        )
    }
}

impl rustls::sign::SigningKey for SecretKeySigner {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn rustls::sign::Signer>> {
        offered
            .contains(&rustls::SignatureScheme::ED25519)
            .then(|| Box::new(self.clone()) as _)
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ED25519
    }

    fn public_key(&self) -> Option<rustls::pki_types::SubjectPublicKeyInfoDer<'_>> {
        Some(self.spki_public_key())
    }
}

impl rustls::sign::Signer for SecretKeySigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        Ok(self.key.sign(message).to_bytes().to_vec())
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ED25519
    }
}

mod verify {
    use ed25519_dalek::pkcs8::DecodePublicKey;
    use iroh::EndpointId;
    use noq::rustls::pki_types::{
        InvalidSignature, SignatureVerificationAlgorithm, SubjectPublicKeyInfoDer,
    };
    use noq::rustls::{
        self, CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key},
        pki_types::CertificateDer as Certificate,
        server::danger::{ClientCertVerified, ClientCertVerifier},
    };

    /// Ed25519 signature verification over raw public keys.
    ///
    /// Verification goes through `ed25519_dalek` — the same primitive iroh
    /// uses, so signature semantics are identical across backends.
    #[derive(Debug)]
    struct Ed25519Dalek;

    impl SignatureVerificationAlgorithm for Ed25519Dalek {
        fn verify_signature(
            &self,
            public_key: &[u8],
            message: &[u8],
            signature: &[u8],
        ) -> Result<(), InvalidSignature> {
            let public_key =
                ed25519_dalek::VerifyingKey::try_from(public_key).map_err(|_| InvalidSignature)?;
            let signature =
                ed25519_dalek::Signature::try_from(signature).map_err(|_| InvalidSignature)?;
            public_key
                .verify_strict(message, &signature)
                .map_err(|_| InvalidSignature)
        }

        fn public_key_alg_id(&self) -> rustls::pki_types::AlgorithmIdentifier {
            rustls::pki_types::alg_id::ED25519
        }

        fn signature_alg_id(&self) -> rustls::pki_types::AlgorithmIdentifier {
            rustls::pki_types::alg_id::ED25519
        }

        fn fips(&self) -> bool {
            false
        }
    }

    const ED25519_DALEK: Ed25519Dalek = Ed25519Dalek;
    static SUPPORTED_SIG_ALGS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[&ED25519_DALEK],
        mapping: &[(SignatureScheme::ED25519, &[&ED25519_DALEK])],
    };

    /// Client-side verifier: the peer's raw public key must equal the
    /// `EndpointId` encoded in the server name.
    #[derive(Default, Debug)]
    pub struct ServerCertificateVerifier;

    impl ServerCertVerifier for ServerCertificateVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &Certificate<'_>,
            intermediates: &[Certificate<'_>],
            server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            let rustls::pki_types::ServerName::DnsName(dns_name) = server_name else {
                return Err(rustls::Error::UnsupportedNameType);
            };
            let Some(remote_peer_id) = super::name::decode(dns_name.as_ref()) else {
                return Err(rustls::Error::InvalidCertificate(
                    CertificateError::NotValidForName,
                ));
            };

            if !intermediates.is_empty() {
                return Err(rustls::Error::InvalidCertificate(
                    CertificateError::UnknownIssuer,
                ));
            }

            let end_entity_as_spki = SubjectPublicKeyInfoDer::from(end_entity.as_ref());
            let remote_public_spki = rustls::sign::public_key_to_spki(
                &rustls::pki_types::alg_id::ED25519,
                remote_peer_id.as_bytes(),
            );

            // The SPKI must match the expected peer id byte-for-byte: the
            // fixed prefix encodes the Ed25519 algorithm identifier, the
            // trailing 32 bytes are the key itself.
            if remote_public_spki != end_entity_as_spki {
                return Err(rustls::Error::InvalidCertificate(
                    CertificateError::UnknownIssuer,
                ));
            }

            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &Certificate<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Err(rustls::Error::PeerIncompatible(
                rustls::PeerIncompatible::Tls12NotOffered,
            ))
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &Certificate<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature_with_raw_key(
                message,
                &SubjectPublicKeyInfoDer::from(cert.as_ref()),
                dss,
                &SUPPORTED_SIG_ALGS,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            SUPPORTED_SIG_ALGS.supported_schemes()
        }

        fn requires_raw_public_keys(&self) -> bool {
            true
        }
    }

    /// Server-side verifier: require one raw public key, any key. The TLS
    /// handshake signature proves possession of the matching secret — the
    /// key itself is the peer's identity, checked by policy afterwards.
    #[derive(Default, Debug)]
    pub struct ClientCertificateVerifier;

    impl ClientCertVerifier for ClientCertificateVerifier {
        fn offer_client_auth(&self) -> bool {
            true
        }

        fn verify_client_cert(
            &self,
            _end_entity: &Certificate<'_>,
            intermediates: &[Certificate<'_>],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<ClientCertVerified, rustls::Error> {
            if !intermediates.is_empty() {
                return Err(rustls::Error::InvalidCertificate(
                    CertificateError::UnknownIssuer,
                ));
            }
            Ok(ClientCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &Certificate<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Err(rustls::Error::PeerIncompatible(
                rustls::PeerIncompatible::Tls12NotOffered,
            ))
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &Certificate<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature_with_raw_key(
                message,
                &SubjectPublicKeyInfoDer::from(cert.as_ref()),
                dss,
                &SUPPORTED_SIG_ALGS,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            SUPPORTED_SIG_ALGS.supported_schemes()
        }

        fn root_hint_subjects(&self) -> &[DistinguishedName] {
            &[]
        }

        fn requires_raw_public_keys(&self) -> bool {
            true
        }
    }

    /// Extract the peer `EndpointId` from a completed connection.
    ///
    /// `Connection::peer_identity` yields the peer certificate chain — for
    /// raw public keys a single SPKI DER blob whose last 32 bytes are the
    /// Ed25519 key.
    pub fn peer_endpoint_id(conn: &noq::Connection) -> Option<EndpointId> {
        let certs = conn
            .peer_identity()?
            .downcast::<Vec<Certificate<'static>>>()
            .ok()?;
        let [cert] = certs.as_slice() else {
            return None;
        };
        let verifying_key = ed25519_dalek::VerifyingKey::from_public_key_der(cert).ok()?;
        Some(EndpointId::from_verifying_key(verifying_key))
    }
}

pub use verify::{ClientCertificateVerifier, ServerCertificateVerifier, peer_endpoint_id};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_roundtrip() {
        let id = SecretKey::generate().public();
        assert_eq!(name::decode(&name::encode(id)), Some(id));
    }

    #[test]
    fn name_snapshot() {
        // Same encoding as iroh: interop depends on it staying identical.
        let key = SecretKey::from_bytes(&[0; 32]);
        assert_eq!(
            name::encode(key.public()),
            "7dl2ff6emqi2qol3l382krodedij45bn3nh479hqo14a32qpr8kg.iroh.invalid",
        );
    }

    #[test]
    fn configs_build() {
        let tls = TlsConfig::new(SecretKey::generate());
        tls.client_config(vec![b"rds/0".to_vec()]).unwrap();
        tls.server_config(vec![b"rds/0".to_vec()]).unwrap();
    }
}
