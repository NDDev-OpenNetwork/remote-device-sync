//! Capability grants — the WS4 authorization token.
//!
//! A grant is a signed statement by a trusted *issuer* (the estate's
//! authorization key) that a *subject* endpoint may use a bounded set of
//! services for a bounded time. It is self-certifying: the agent verifies
//! the Ed25519 signature, issuer membership, subject match, validity
//! window and TTL — none of it depends on trusting the channel the grant
//! arrived over.
//!
//! Wire placement: the grant is the payload of [`crate::StreamHello::Authz`],
//! which must be the first frame on the first stream a client opens. The
//! agent rejects every service stream that arrives while no grant has
//! been verified — streams raced ahead of authorization are refused, not
//! queued.
//!
//! Revocation is two-layered: grants are short-lived (bounded by
//! `max_ttl` at verify time), and a denylist of [`GrantId`]s — fed by the
//! estate's signed revocation snapshot — drops both new attempts and
//! live connections.
//!
//! Clock skew: `not_before` tolerates [`SKEW_SECS`] of future-dating to
//! absorb issuer/agent clock drift; `expires_at` is strict — an expired
//! grant is dead even inside the skew window.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ServiceKind;

/// How far into the future `not_before` may sit before the grant is
/// rejected — the documented clock-skew tolerance.
pub const SKEW_SECS: u64 = 30;

/// Hard bounds on grant collections, checked after decode so a hostile
/// grant cannot force oversized allocations into scope checks.
pub const MAX_SERVICES: usize = 32;
pub const MAX_TCP_PORTS: usize = 128;
pub const MAX_DISPLAYS: usize = 32;

/// Stable identifier of a grant: the BLAKE3 digest of its signed
/// payload. Revocation lists and the replay guard key on this — the
/// same signed bytes always map to the same id.
pub type GrantId = [u8; 32];

/// A grant as it travels the wire and rests on disk: the signed payload
/// bytes plus the issuer's signature over them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// Postcard-encoded [`GrantPayload`].
    pub payload: Vec<u8>,
    /// Ed25519 signature over `payload`, made by `payload.issuer`.
    pub signature: Vec<u8>,
}

/// The signed portion of a [`Grant`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantPayload {
    /// Issuer's Ed25519 verifying key — must be a member of the agent's
    /// trusted issuer set.
    pub issuer: [u8; 32],
    /// Subject's endpoint key — must equal the connecting peer's
    /// QUIC-authenticated identity.
    pub subject: [u8; 32],
    /// Issuer-chosen uniqueness nonce; combined with the payload hash it
    /// makes every minted grant a distinct [`GrantId`].
    pub nonce: u64,
    /// Services this grant unlocks.
    pub services: Vec<ServiceKind>,
    /// Unix seconds: grant invalid strictly before this time (minus
    /// [`SKEW_SECS`] tolerance).
    pub not_before: u64,
    /// Unix seconds: grant invalid at or after this time.
    pub expires_at: u64,
    /// Optional per-service constraints.
    pub constraints: GrantConstraints,
}

/// Narrowing constraints inside a grant. `None` = unconstrained by the
/// grant (the agent's local policy still applies).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantConstraints {
    /// Ceiling for media bitrate steering (bits per second).
    pub max_bps: Option<u64>,
    /// Allowed TCP target ports for `TcpConnect`. `Some` restricts to
    /// the listed ports; the agent's own target allowlist still applies.
    pub tcp_ports: Option<Vec<u16>>,
    /// Allowed display indices for `Desktop`. `Some` restricts.
    pub displays: Option<Vec<u32>>,
}

/// A grant that passed [`Grant::verify`]: parsed payload plus its id.
#[derive(Debug, Clone)]
pub struct VerifiedGrant {
    /// The verified signed fields.
    pub payload: GrantPayload,
    /// `blake3(payload)` — revocation and replay key.
    pub id: GrantId,
}

impl VerifiedGrant {
    /// Whether `service` is inside the grant's scope.
    pub fn permits(&self, service: ServiceKind) -> bool {
        self.payload.services.contains(&service)
    }

    /// Whether a TCP target port is inside `constraints.tcp_ports`
    /// (`None` means the grant does not constrain ports).
    pub fn permits_port(&self, port: u16) -> bool {
        match &self.payload.constraints.tcp_ports {
            Some(ports) => ports.contains(&port),
            None => true,
        }
    }

    /// Whether a display index is inside `constraints.displays`.
    pub fn permits_display(&self, display: u32) -> bool {
        match &self.payload.constraints.displays {
            Some(displays) => displays.contains(&display),
            None => true,
        }
    }

    /// The grant's bitrate ceiling, if any.
    pub fn max_bps(&self) -> Option<u64> {
        self.payload.constraints.max_bps
    }

    /// Whether the grant is still valid at `now` (unix seconds).
    pub fn live_at(&self, now: u64) -> bool {
        now + SKEW_SECS >= self.payload.not_before && now < self.payload.expires_at
    }
}

#[derive(Debug, Error)]
pub enum GrantError {
    #[error("grant malformed: {0}")]
    Malformed(String),
    #[error("grant issuer not trusted")]
    UntrustedIssuer,
    #[error("grant signature invalid")]
    BadSignature,
    #[error("grant subject does not match the connecting peer")]
    WrongSubject,
    #[error("grant not valid yet")]
    NotYetValid,
    #[error("grant expired")]
    Expired,
    #[error("grant TTL {0}s exceeds the agent maximum")]
    TtlExceeded(u64),
    #[error("grant exceeds collection bounds")]
    Oversized,
}

impl Grant {
    /// Mint a grant: `subject` may use `services` for `ttl` from now.
    pub fn issue(
        issuer: &SigningKey,
        subject: [u8; 32],
        services: Vec<ServiceKind>,
        ttl: Duration,
        constraints: GrantConstraints,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
            .unwrap_or(0);
        Self::issue_at(
            issuer,
            GrantPayload {
                issuer: issuer.verifying_key().to_bytes(),
                subject,
                nonce,
                services,
                not_before: now,
                expires_at: now + ttl.as_secs(),
                constraints,
            },
        )
    }

    /// Sign an already-built payload — the deterministic path for tests.
    pub fn issue_at(issuer: &SigningKey, payload: GrantPayload) -> Self {
        let bytes = postcard::to_stdvec(&payload).expect("grant payload encodes");
        let signature = issuer.sign(&bytes);
        Self {
            payload: bytes,
            signature: signature.to_bytes().to_vec(),
        }
    }

    /// This grant's [`GrantId`] — computable without verifying.
    pub fn id(&self) -> GrantId {
        *blake3::hash(&self.payload).as_bytes()
    }

    /// Full verification: decode, bounds, issuer trust, signature,
    /// subject match, validity window and TTL cap.
    ///
    /// `subject` is the peer's QUIC-authenticated endpoint key; `now` is
    /// unix seconds (parameterized for deterministic tests).
    pub fn verify(
        &self,
        issuers: &HashSet<[u8; 32]>,
        subject: &[u8; 32],
        max_ttl: Duration,
        now: u64,
    ) -> Result<VerifiedGrant, GrantError> {
        let payload: GrantPayload = postcard::from_bytes(&self.payload)
            .map_err(|e| GrantError::Malformed(e.to_string()))?;
        if payload.services.len() > MAX_SERVICES
            || payload
                .constraints
                .tcp_ports
                .as_ref()
                .is_some_and(|p| p.len() > MAX_TCP_PORTS)
            || payload
                .constraints
                .displays
                .as_ref()
                .is_some_and(|d| d.len() > MAX_DISPLAYS)
        {
            return Err(GrantError::Oversized);
        }
        if !issuers.contains(&payload.issuer) {
            return Err(GrantError::UntrustedIssuer);
        }
        let issuer_key = VerifyingKey::from_bytes(&payload.issuer)
            .map_err(|e| GrantError::Malformed(e.to_string()))?;
        let sig_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| GrantError::Malformed("signature is not 64 bytes".into()))?;
        issuer_key
            .verify_strict(&self.payload, &Signature::from_bytes(&sig_bytes))
            .map_err(|_| GrantError::BadSignature)?;
        if &payload.subject != subject {
            return Err(GrantError::WrongSubject);
        }
        if now + SKEW_SECS < payload.not_before {
            return Err(GrantError::NotYetValid);
        }
        if now >= payload.expires_at {
            return Err(GrantError::Expired);
        }
        let ttl = payload.expires_at.saturating_sub(payload.not_before);
        if ttl > max_ttl.as_secs() {
            return Err(GrantError::TtlExceeded(ttl));
        }
        Ok(VerifiedGrant {
            id: self.id(),
            payload,
        })
    }
}

/// Current unix seconds; `verify` takes it explicitly for tests.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> SigningKey {
        SigningKey::from_bytes(&[42u8; 32])
    }

    fn issuers(key: &SigningKey) -> HashSet<[u8; 32]> {
        HashSet::from([key.verifying_key().to_bytes()])
    }

    #[test]
    fn issued_grant_verifies() {
        let key = issuer();
        let subject = [7u8; 32];
        let grant = Grant::issue(
            &key,
            subject,
            vec![ServiceKind::Ping, ServiceKind::Tcp],
            Duration::from_secs(300),
            GrantConstraints::default(),
        );
        let v = grant
            .verify(
                &issuers(&key),
                &subject,
                Duration::from_secs(600),
                now_unix(),
            )
            .expect("fresh grant verifies");
        assert!(v.permits(ServiceKind::Ping));
        assert!(!v.permits(ServiceKind::Desktop));
    }

    #[test]
    fn every_negative_path_rejects() {
        let key = issuer();
        let other = SigningKey::from_bytes(&[43u8; 32]);
        let subject = [7u8; 32];
        let grant = Grant::issue(
            &key,
            subject,
            vec![ServiceKind::Ping],
            Duration::from_secs(300),
            GrantConstraints::default(),
        );
        let now = now_unix();

        // Forged signature.
        let mut bad = grant.clone();
        bad.signature[0] ^= 1;
        assert!(matches!(
            bad.verify(&issuers(&key), &subject, Duration::from_secs(600), now),
            Err(GrantError::BadSignature)
        ));
        // Wrong issuer.
        assert!(matches!(
            grant.verify(&issuers(&other), &subject, Duration::from_secs(600), now),
            Err(GrantError::UntrustedIssuer)
        ));
        // Wrong subject.
        assert!(matches!(
            grant.verify(&issuers(&key), &[9u8; 32], Duration::from_secs(600), now),
            Err(GrantError::WrongSubject)
        ));
        // Expired.
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &subject,
                Duration::from_secs(600),
                now + 3600
            ),
            Err(GrantError::Expired)
        ));
        // TTL cap.
        assert!(matches!(
            grant.verify(&issuers(&key), &subject, Duration::from_secs(60), now),
            Err(GrantError::TtlExceeded(300))
        ));
        // Future-dated beyond skew tolerance.
        let future = Grant::issue_at(
            &key,
            GrantPayload {
                issuer: key.verifying_key().to_bytes(),
                subject,
                nonce: 1,
                services: vec![ServiceKind::Ping],
                not_before: now + 3600,
                expires_at: now + 7200,
                constraints: GrantConstraints::default(),
            },
        );
        assert!(matches!(
            future.verify(&issuers(&key), &subject, Duration::from_secs(7200), now),
            Err(GrantError::NotYetValid)
        ));
    }

    #[test]
    fn constraints_narrow_scope() {
        let key = issuer();
        let subject = [7u8; 32];
        let grant = Grant::issue(
            &key,
            subject,
            vec![ServiceKind::Tcp, ServiceKind::Desktop],
            Duration::from_secs(300),
            GrantConstraints {
                max_bps: Some(2_000_000),
                tcp_ports: Some(vec![22]),
                displays: Some(vec![0]),
            },
        );
        let v = grant
            .verify(
                &issuers(&key),
                &subject,
                Duration::from_secs(600),
                now_unix(),
            )
            .unwrap();
        assert!(v.permits_port(22));
        assert!(!v.permits_port(23));
        assert!(v.permits_display(0));
        assert!(!v.permits_display(1));
        assert_eq!(v.max_bps(), Some(2_000_000));
    }

    proptest::proptest! {
        /// Malformed grant bytes never panic — parse or verify fails
        /// cleanly on truncated payloads, garbage signatures and
        /// oversized collections.
        #[test]
        fn grant_decode_never_panics(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..=2048)) {
            let grant = Grant { payload: bytes.clone(), signature: bytes };
            let key = issuer();
            let _ = grant.verify(&issuers(&key), &[7u8; 32], Duration::from_secs(600), now_unix());
        }

        /// A real grant survives encode/decode; flipping any signed byte
        /// breaks verification.
        #[test]
        fn grant_roundtrip_and_corruption(flip in proptest::option::of(proptest::num::usize::ANY)) {
            let key = issuer();
            let subject = [7u8; 32];
            let grant = Grant::issue(
                &key,
                subject,
                vec![ServiceKind::Ping],
                Duration::from_secs(300),
                GrantConstraints::default(),
            );
            let now = now_unix();
            proptest::prop_assert!(grant.verify(&issuers(&key), &subject, Duration::from_secs(600), now).is_ok());
            if let Some(pos) = flip.filter(|p| *p < grant.payload.len()) {
                let mut bad = grant.clone();
                bad.payload[pos] ^= 1;
                proptest::prop_assert!(bad.verify(&issuers(&key), &subject, Duration::from_secs(600), now).is_err());
            }
        }
    }
}
