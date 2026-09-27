//! Capability grants — the WS4 authorization token.
//!
//! A grant is a signed statement by a trusted *issuer* (the estate's
//! authorization key) that a *subject* endpoint may use a bounded set of
//! services for a bounded time. It is self-certifying: the agent verifies
//! the Ed25519 signature, issuer membership, subject match, destination
//! audience, protocol version, validity window and TTL — none of it depends
//! on trusting the channel the grant arrived over.
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

/// Grant payload format, independently versioned inside the Authz envelope.
/// Version 3 adds the tenant/policy-revision claims and the `sync_paths`
/// constraint; version-2 payloads still verify with all claims absent so
/// estate-minted grants stay valid across the cutover. A deployment that
/// requires the binding enforces it through policy — version-2 grants simply
/// cannot satisfy a pinned tenant or revision floor.
pub const GRANT_VERSION: u16 = 3;
/// Oldest payload version still accepted at verify time.
pub const GRANT_VERSION_MIN: u16 = 2;
pub const MAX_PAYLOAD_LEN: usize = 4096;
const DOMAIN_V2: &[u8] = b"rds/capability-grant/v2\0";
const DOMAIN_V3: &[u8] = b"rds/capability-grant/v3\0";
const ID_DOMAIN: &[u8] = b"rds/grant-session/v2\0";

/// Hard bounds on grant collections, checked after decode so a hostile
/// grant cannot force oversized allocations into scope checks.
pub const MAX_SERVICES: usize = 32;
pub const MAX_TCP_PORTS: usize = 128;
pub const MAX_DISPLAYS: usize = 32;
/// Bound on a tenant identifier carried by a version-3 grant.
pub const MAX_TENANT_LEN: usize = 64;
/// Bound on the number of `sync_paths` scope entries.
pub const MAX_SYNC_PATHS: usize = 64;
/// Bound on one `sync_paths` entry — same cap as a protocol rel_path.
pub const MAX_SCOPE_PATH_LEN: usize = 512;

/// Stable session identifier: domain-separated BLAKE3 of issuer, subject,
/// destination and nonce. Revocation and concurrent-replay checks cover every
/// renewal revision; a renewed token cannot evade an existing revocation.
pub type GrantId = [u8; 32];

/// A grant as it travels the wire and rests on disk: the signed payload
/// bytes plus the issuer's signature over them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// Postcard-encoded [`GrantPayload`].
    pub payload: Vec<u8>,
    /// Ed25519 signature over the protocol domain and `payload`, made by `payload.issuer`.
    pub signature: Vec<u8>,
}

/// The signed portion of a [`Grant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantPayload {
    pub version: u16,
    /// Positive issuer revision. Renewal increases it without changing scope.
    pub revision: u64,
    /// Issuer's Ed25519 verifying key — must be a member of the agent's
    /// trusted issuer set.
    pub issuer: [u8; 32],
    /// Subject's endpoint key — must equal the connecting peer's
    /// QUIC-authenticated identity.
    pub subject: [u8; 32],
    /// Controlled device endpoint key, checked against the serving endpoint.
    pub audience: [u8; 32],
    /// Issuer-chosen cryptographically random 128-bit nonce identifying one
    /// authorization session across renewal revisions. Never reuse for a new session.
    pub nonce: [u8; 16],
    /// Services this grant unlocks.
    pub services: Vec<ServiceKind>,
    /// Unix seconds: grant invalid strictly before this time (minus
    /// [`SKEW_SECS`] tolerance).
    pub not_before: u64,
    /// Unix seconds: grant invalid at or after this time.
    pub expires_at: u64,
    /// Optional per-service constraints.
    pub constraints: GrantConstraints,
    /// Tenant the issuer binds this grant to (v3+). `None` = unscoped;
    /// agents that pin a tenant refuse unscoped and mismatched grants.
    pub tenant: Option<String>,
    /// Estate policy revision the issuer minted under (v3+). Agents may
    /// enforce a floor so policy changes invalidate older grants.
    pub policy_revision: Option<u64>,
}

/// Frozen version-2 wire layout, kept only for decode. New grants are
/// always issued at [`GRANT_VERSION`].
#[derive(Debug, Serialize, Deserialize)]
struct GrantPayloadV2 {
    version: u16,
    revision: u64,
    issuer: [u8; 32],
    subject: [u8; 32],
    audience: [u8; 32],
    nonce: [u8; 16],
    services: Vec<ServiceKind>,
    not_before: u64,
    expires_at: u64,
    constraints: GrantConstraintsV2,
}

/// Version-2 [`GrantConstraints`] — no `sync_paths` field.
#[derive(Debug, Serialize, Deserialize)]
struct GrantConstraintsV2 {
    max_bps: Option<u64>,
    tcp_ports: Option<Vec<u16>>,
    displays: Option<Vec<u32>>,
}

impl From<GrantPayloadV2> for GrantPayload {
    fn from(old: GrantPayloadV2) -> Self {
        Self {
            version: old.version,
            revision: old.revision,
            issuer: old.issuer,
            subject: old.subject,
            audience: old.audience,
            nonce: old.nonce,
            services: old.services,
            not_before: old.not_before,
            expires_at: old.expires_at,
            constraints: GrantConstraints {
                max_bps: old.constraints.max_bps,
                tcp_ports: old.constraints.tcp_ports,
                displays: old.constraints.displays,
                sync_paths: None,
            },
            tenant: None,
            policy_revision: None,
        }
    }
}

/// Narrowing constraints inside a grant. `None` = unconstrained by the
/// grant (the agent's local policy still applies).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantConstraints {
    /// Ceiling for media bitrate steering (bits per second).
    pub max_bps: Option<u64>,
    /// Allowed TCP target ports for `TcpConnect`. `Some` restricts to
    /// the listed ports; the agent's own target allowlist still applies.
    pub tcp_ports: Option<Vec<u16>>,
    /// Allowed display indices for `Desktop`. `Some` restricts.
    pub displays: Option<Vec<u32>>,
    /// Allowed relative sync paths (v3+). `Some` restricts `Sync` reads and
    /// writes to the listed subtrees; each entry is a normalized relative
    /// path under the agent's sync root.
    pub sync_paths: Option<Vec<String>>,
}

impl GrantPayload {
    fn id(&self) -> GrantId {
        let mut hash = blake3::Hasher::new();
        hash.update(ID_DOMAIN);
        hash.update(&self.issuer);
        hash.update(&self.subject);
        hash.update(&self.audience);
        hash.update(&self.nonce);
        *hash.finalize().as_bytes()
    }
}

/// A grant that passed [`Grant::verify`]: parsed payload plus its id.
#[derive(Debug, Clone)]
pub struct VerifiedGrant {
    /// The verified signed fields.
    pub payload: GrantPayload,
    /// Stable revocation and concurrent-replay key across lease revisions.
    pub id: GrantId,
}

impl VerifiedGrant {
    /// Check a verified replacement for this live authorization. `false` is an
    /// exact idempotent retry: it must retain the original monotonic deadline.
    /// `true` extends the lease. Scope changes require a new authorization.
    pub fn permits_renewal(&self, next: &Self) -> Result<bool, GrantError> {
        if self.payload == next.payload {
            return Ok(false);
        }
        if self.id != next.id
            || self.payload.services != next.payload.services
            || self.payload.constraints != next.payload.constraints
            || self.payload.tenant != next.payload.tenant
            || self.payload.policy_revision != next.payload.policy_revision
            || next.payload.revision <= self.payload.revision
            || next.payload.not_before < self.payload.not_before
            || next.payload.expires_at <= self.payload.expires_at
        {
            return Err(GrantError::InvalidRenewal);
        }
        Ok(true)
    }

    /// Whether `service` is inside the grant's scope.
    pub fn permits(&self, service: ServiceKind) -> bool {
        self.payload.services.contains(&service)
    }

    /// Stream admission; narrow capabilities never imply a broad grant.
    /// Per-operation checks must still run inside sync and desktop handlers.
    pub fn permits_service(&self, service: ServiceKind) -> bool {
        match service {
            ServiceKind::Sync => self.permits_sync_read() || self.permits_sync_write(),
            ServiceKind::Desktop => self.permits_desktop_view(),
            other => self.permits(other),
        }
    }

    pub fn permits_sync_read(&self) -> bool {
        self.permits(ServiceKind::Sync) || self.permits(ServiceKind::SyncRead)
    }

    pub fn permits_sync_write(&self) -> bool {
        self.permits(ServiceKind::Sync) || self.permits(ServiceKind::SyncWrite)
    }

    pub fn permits_desktop_view(&self) -> bool {
        self.permits(ServiceKind::Desktop) || self.permits(ServiceKind::DesktopView)
    }

    pub fn permits_desktop_control(&self) -> bool {
        self.permits(ServiceKind::Desktop)
            || (self.permits(ServiceKind::DesktopView) && self.permits(ServiceKind::DesktopControl))
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

    /// Tenant the issuer bound this grant to, if any (v3+).
    pub fn tenant(&self) -> Option<&str> {
        self.payload.tenant.as_deref()
    }

    /// Estate policy revision the grant was minted under, if any (v3+).
    pub fn policy_revision(&self) -> Option<u64> {
        self.payload.policy_revision
    }

    /// Whether a normalized relative sync path is inside
    /// `constraints.sync_paths` — the path itself or a descendant of a listed
    /// subtree. `None` means the grant does not narrow the sync root.
    pub fn permits_sync_path(&self, rel: &std::path::Path) -> bool {
        match &self.payload.constraints.sync_paths {
            Some(paths) => paths
                .iter()
                .map(std::path::Path::new)
                .any(|scope| rel == scope || rel.starts_with(scope)),
            None => true,
        }
    }

    /// Whether the grant is still valid at `now` (unix seconds).
    pub fn live_at(&self, now: u64) -> bool {
        now.saturating_add(SKEW_SECS) >= self.payload.not_before && now < self.payload.expires_at
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
    #[error("grant destination does not match this device")]
    WrongAudience,
    #[error("grant tenant does not match the agent's pinned tenant")]
    TenantMismatch,
    #[error("grant policy revision is below the agent's required floor")]
    PolicyRevisionStale,
    #[error("unsupported grant version")]
    Version,
    #[error("invalid grant validity interval")]
    InvalidInterval,
    #[error("invalid grant revision")]
    InvalidRevision,
    #[error("renewal changes grant identity/scope or does not advance its lease")]
    InvalidRenewal,
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
    /// Mint a grant for one subject and destination. The issuer must supply a
    /// fresh cryptographically random 128-bit nonce; core owns no RNG or key store.
    /// Use `issue_at` when the issuer controls the clock explicitly.
    pub fn issue(
        issuer: &SigningKey,
        subject: [u8; 32],
        audience: [u8; 32],
        nonce: [u8; 16],
        services: Vec<ServiceKind>,
        ttl: Duration,
        constraints: GrantConstraints,
    ) -> Self {
        let now = now_unix();
        Self::issue_at(
            issuer,
            GrantPayload {
                version: GRANT_VERSION,
                revision: 1,
                issuer: issuer.verifying_key().to_bytes(),
                subject,
                audience,
                nonce,
                services,
                not_before: now,
                expires_at: now.saturating_add(ttl.as_secs()),
                constraints,
                tenant: None,
                policy_revision: None,
            },
        )
    }

    /// Sign an already-built payload — the deterministic path for tests.
    /// The signature domain follows `payload.version`.
    pub fn issue_at(issuer: &SigningKey, payload: GrantPayload) -> Self {
        let bytes = postcard::to_stdvec(&payload).expect("grant payload encodes");
        let signature = issuer.sign(&signature_message(payload.version, &bytes));
        Self {
            payload: bytes,
            signature: signature.to_bytes().to_vec(),
        }
    }

    /// Stable session revocation id; parsing is not signature verification.
    pub fn id(&self) -> Result<GrantId, GrantError> {
        Ok(self.decode()?.id())
    }

    /// Full verification: decode, bounds, issuer trust, signature,
    /// subject match, validity window and TTL cap.
    ///
    /// `subject` is the peer's QUIC-authenticated endpoint key; `now` is
    /// unix seconds (parameterized for deterministic tests). `audience` is the
    /// local serving endpoint identity, never a peer-supplied request field.
    pub fn verify(
        &self,
        issuers: &HashSet<[u8; 32]>,
        subject: &[u8; 32],
        audience: &[u8; 32],
        max_ttl: Duration,
        now: u64,
    ) -> Result<VerifiedGrant, GrantError> {
        let payload = self.decode()?;
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
            .verify_strict(
                &signature_message(payload.version, &self.payload),
                &Signature::from_bytes(&sig_bytes),
            )
            .map_err(|_| GrantError::BadSignature)?;
        if &payload.subject != subject {
            return Err(GrantError::WrongSubject);
        }
        if &payload.audience != audience {
            return Err(GrantError::WrongAudience);
        }
        if now.saturating_add(SKEW_SECS) < payload.not_before {
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
            id: payload.id(),
            payload,
        })
    }
    fn decode(&self) -> Result<GrantPayload, GrantError> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(GrantError::Oversized);
        }
        // The version is the payload's first field; postcard encodes `u16`
        // as a varint, so peeking decodes exactly the version bytes.
        let (version, _) = postcard::take_from_bytes::<u16>(&self.payload)
            .map_err(|e| GrantError::Malformed(e.to_string()))?;
        let payload = match version {
            v if v == GRANT_VERSION_MIN => {
                let (old, trailing): (GrantPayloadV2, _) = postcard::take_from_bytes(&self.payload)
                    .map_err(|e| GrantError::Malformed(e.to_string()))?;
                if !trailing.is_empty() {
                    return Err(GrantError::Malformed("trailing grant bytes".into()));
                }
                old.into()
            }
            v if v == GRANT_VERSION => {
                let (new, trailing): (GrantPayload, _) =
                    postcard::take_from_bytes(&self.payload)
                        .map_err(|e| GrantError::Malformed(e.to_string()))?;
                if !trailing.is_empty() {
                    return Err(GrantError::Malformed("trailing grant bytes".into()));
                }
                new
            }
            _ => return Err(GrantError::Version),
        };
        if payload.revision == 0 {
            return Err(GrantError::InvalidRevision);
        }
        if payload.expires_at <= payload.not_before {
            return Err(GrantError::InvalidInterval);
        }
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
            || payload
                .constraints
                .sync_paths
                .as_ref()
                .is_some_and(|s| s.len() > MAX_SYNC_PATHS)
            || payload
                .tenant
                .as_ref()
                .is_some_and(|t| t.len() > MAX_TENANT_LEN)
        {
            return Err(GrantError::Oversized);
        }
        if let Some(tenant) = &payload.tenant
            && (tenant.is_empty() || tenant.bytes().any(|b| b < 0x21 || b == 0x7f))
        {
            return Err(GrantError::Malformed(
                "tenant contains control or whitespace bytes".into(),
            ));
        }
        if let Some(paths) = &payload.constraints.sync_paths {
            for path in paths {
                check_scope_path(path)?;
            }
        }
        Ok(payload)
    }
}

/// Lexical validation of a `sync_paths` scope entry — the same rules a
/// transfer-level `rel_path` must satisfy (relative, inside the root, no
/// NUL or `..`, nonempty after normalization). I/O confinement is still
/// proven by the journal's no-follow handles; this only proves the signed
/// scope list is well-formed.
fn check_scope_path(path: &str) -> Result<(), GrantError> {
    if path.is_empty() || path.len() > MAX_SCOPE_PATH_LEN {
        return Err(GrantError::Malformed(format!(
            "bad sync_paths entry {path:?}"
        )));
    }
    if path.contains('\0') {
        return Err(GrantError::Malformed(
            "sync_paths entry contains NUL".into(),
        ));
    }
    if path.starts_with(['/', '\\']) || path.as_bytes().get(1) == Some(&b':') {
        return Err(GrantError::Malformed(format!(
            "absolute sync_paths entry {path:?}"
        )));
    }
    let mut real_components = 0;
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                return Err(GrantError::Malformed(format!(
                    "traversal in sync_paths entry {path:?}"
                )));
            }
            _ => real_components += 1,
        }
    }
    if real_components == 0 {
        return Err(GrantError::Malformed(format!(
            "empty sync_paths entry {path:?}"
        )));
    }
    Ok(())
}

fn signature_message(version: u16, payload: &[u8]) -> Vec<u8> {
    let domain = match version {
        v if v == GRANT_VERSION_MIN => DOMAIN_V2,
        _ => DOMAIN_V3,
    };
    let mut message = Vec::with_capacity(domain.len() + payload.len());
    message.extend_from_slice(domain);
    message.extend_from_slice(payload);
    message
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

    #[test]
    fn directional_capabilities_preserve_legacy_scope_without_escalation() {
        use ServiceKind::*;
        let key = issuer();
        // Every combination, including control without view and empty scope.
        let capabilities = [
            Sync,
            SyncRead,
            SyncWrite,
            Desktop,
            DesktopView,
            DesktopControl,
        ];
        for mask in 0..(1 << capabilities.len()) {
            let services: Vec<_> = capabilities
                .iter()
                .enumerate()
                .filter_map(|(bit, kind)| (mask & (1 << bit) != 0).then_some(*kind))
                .collect();
            let has = |kind| services.contains(&kind);
            let signed = Grant::issue(
                &key,
                [7; 32],
                [8; 32],
                [1; 16],
                services.clone(),
                Duration::from_secs(300),
                Default::default(),
            );
            let grant = signed
                .verify(
                    &issuers(&key),
                    &[7; 32],
                    &[8; 32],
                    Duration::from_secs(600),
                    now_unix(),
                )
                .unwrap();
            assert_eq!(grant.permits_sync_read(), has(Sync) || has(SyncRead));
            assert_eq!(grant.permits_sync_write(), has(Sync) || has(SyncWrite));
            assert_eq!(
                grant.permits_service(Sync),
                has(Sync) || has(SyncRead) || has(SyncWrite)
            );
            assert_eq!(
                grant.permits_service(Desktop),
                has(Desktop) || has(DesktopView)
            );
            assert_eq!(
                grant.permits_desktop_control(),
                has(Desktop) || (has(DesktopView) && has(DesktopControl))
            );
            assert_eq!(grant.permits(Sync), has(Sync));
            assert_eq!(grant.permits(Desktop), has(Desktop));
            assert!(!grant.permits_service(Tcp));
            for kind in capabilities {
                let mut payload = grant.payload.clone();
                payload.revision += 1;
                payload.expires_at += 60;
                if has(kind) {
                    payload.services.retain(|s| *s != kind);
                } else {
                    payload.services.push(kind);
                }
                let next = Grant::issue_at(&key, payload)
                    .verify(
                        &issuers(&key),
                        &[7; 32],
                        &[8; 32],
                        Duration::from_secs(600),
                        now_unix(),
                    )
                    .unwrap();
                assert!(matches!(
                    grant.permits_renewal(&next),
                    Err(GrantError::InvalidRenewal)
                ));
            }
        }
    }

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
            [8; 32],
            [1; 16],
            vec![ServiceKind::Ping, ServiceKind::Tcp],
            Duration::from_secs(300),
            GrantConstraints::default(),
        );
        let v = grant
            .verify(
                &issuers(&key),
                &subject,
                &[8; 32],
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
            [8; 32],
            [1; 16],
            vec![ServiceKind::Ping],
            Duration::from_secs(300),
            GrantConstraints::default(),
        );
        let now = now_unix();

        // Forged signature.
        let mut bad = grant.clone();
        bad.signature[0] ^= 1;
        assert!(matches!(
            bad.verify(
                &issuers(&key),
                &subject,
                &[8; 32],
                Duration::from_secs(600),
                now
            ),
            Err(GrantError::BadSignature)
        ));
        // Wrong issuer.
        assert!(matches!(
            grant.verify(
                &issuers(&other),
                &subject,
                &[8; 32],
                Duration::from_secs(600),
                now
            ),
            Err(GrantError::UntrustedIssuer)
        ));
        // Wrong subject.
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &[9u8; 32],
                &[8; 32],
                Duration::from_secs(600),
                now
            ),
            Err(GrantError::WrongSubject)
        ));
        // Expired.
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &subject,
                &[8; 32],
                Duration::from_secs(600),
                now + 3600
            ),
            Err(GrantError::Expired)
        ));
        // TTL cap.
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &subject,
                &[8; 32],
                Duration::from_secs(60),
                now
            ),
            Err(GrantError::TtlExceeded(300))
        ));
        // Future-dated beyond skew tolerance.
        let future = Grant::issue_at(
            &key,
            GrantPayload {
                version: GRANT_VERSION,
                revision: 1,
                issuer: key.verifying_key().to_bytes(),
                subject,
                audience: [8; 32],
                nonce: [1; 16],
                services: vec![ServiceKind::Ping],
                not_before: now + 3600,
                expires_at: now + 7200,
                constraints: GrantConstraints::default(),
                tenant: None,
                policy_revision: None,
            },
        );
        assert!(matches!(
            future.verify(
                &issuers(&key),
                &subject,
                &[8; 32],
                Duration::from_secs(7200),
                now
            ),
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
            [8; 32],
            [1; 16],
            vec![ServiceKind::Tcp, ServiceKind::Desktop],
            Duration::from_secs(300),
            GrantConstraints {
                max_bps: Some(2_000_000),
                tcp_ports: Some(vec![22]),
                displays: Some(vec![0]),
                sync_paths: None,
            },
        );
        let v = grant
            .verify(
                &issuers(&key),
                &subject,
                &[8; 32],
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

    #[test]
    fn rejects_signature_without_protocol_domain() {
        let key = issuer();
        let mut grant = Grant::issue(
            &key,
            [7; 32],
            [8; 32],
            [1; 16],
            vec![ServiceKind::Ping],
            Duration::from_secs(60),
            GrantConstraints::default(),
        );
        grant.signature = key.sign(&grant.payload).to_bytes().to_vec();
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(60),
                now_unix()
            ),
            Err(GrantError::BadSignature)
        ));
    }

    #[test]
    fn rejects_inverted_signed_interval_inside_clock_skew() {
        let key = issuer();
        let grant = Grant::issue_at(
            &key,
            GrantPayload {
                version: GRANT_VERSION,
                revision: 1,
                issuer: key.verifying_key().to_bytes(),
                subject: [7; 32],
                audience: [8; 32],
                nonce: [1; 16],
                services: vec![ServiceKind::Ping],
                not_before: 105,
                expires_at: 103,
                constraints: GrantConstraints::default(),
                tenant: None,
                policy_revision: None,
            },
        );
        assert!(
            grant
                .verify(
                    &issuers(&key),
                    &[7; 32],
                    &[8; 32],
                    Duration::from_secs(60),
                    100
                )
                .is_err()
        );
    }

    #[test]
    fn audience_version_exact_payload_and_clock_boundaries() {
        let key = issuer();
        let payload = GrantPayload {
            version: GRANT_VERSION,
            revision: 1,
            issuer: key.verifying_key().to_bytes(),
            subject: [7; 32],
            audience: [8; 32],
            nonce: [1; 16],
            services: vec![ServiceKind::Ping],
            not_before: 100,
            expires_at: 160,
            constraints: GrantConstraints::default(),
            tenant: None,
            policy_revision: None,
        };
        let grant = Grant::issue_at(&key, payload.clone());
        let check = |grant: &Grant, audience: &[u8; 32], now| {
            grant.verify(
                &issuers(&key),
                &[7; 32],
                audience,
                Duration::from_secs(60),
                now,
            )
        };
        assert!(check(&grant, &[8; 32], 100).is_ok());
        assert!(matches!(
            check(&grant, &[9; 32], 100),
            Err(GrantError::WrongAudience)
        ));
        assert!(matches!(
            check(&grant, &[8; 32], 69),
            Err(GrantError::NotYetValid)
        ));
        assert!(check(&grant, &[8; 32], 70).is_ok());
        assert!(check(&grant, &[8; 32], 159).is_ok());
        assert!(matches!(
            check(&grant, &[8; 32], 160),
            Err(GrantError::Expired)
        ));
        assert!(matches!(
            check(&grant, &[8; 32], u64::MAX),
            Err(GrantError::Expired)
        ));
        assert!(!check(&grant, &[8; 32], 100).unwrap().live_at(u64::MAX));

        let mut changed = payload.clone();
        changed.version = 1;
        assert!(matches!(
            check(&Grant::issue_at(&key, changed), &[8; 32], 100),
            Err(GrantError::Version)
        ));
        let mut changed = payload;
        changed.expires_at = changed.not_before;
        assert!(matches!(
            check(&Grant::issue_at(&key, changed), &[8; 32], 99),
            Err(GrantError::InvalidInterval)
        ));

        let mut trailing = grant.clone();
        trailing.payload.push(0);
        trailing.signature = key
            .sign(&signature_message(GRANT_VERSION, &trailing.payload))
            .to_bytes()
            .to_vec();
        assert!(matches!(
            check(&trailing, &[8; 32], 100),
            Err(GrantError::Malformed(_))
        ));
        let huge = Grant {
            payload: vec![0; MAX_PAYLOAD_LEN + 1],
            signature: vec![0; 64],
        };
        assert!(matches!(
            check(&huge, &[8; 32], 100),
            Err(GrantError::Oversized)
        ));
        let mut long_signature = grant;
        long_signature.signature.push(0);
        assert!(matches!(
            check(&long_signature, &[8; 32], 100),
            Err(GrantError::Malformed(_))
        ));
    }

    #[test]
    fn renewal_keeps_revocation_identity_and_cannot_change_scope() {
        let key = issuer();
        let payload = GrantPayload {
            version: GRANT_VERSION,
            revision: 1,
            issuer: key.verifying_key().to_bytes(),
            subject: [7; 32],
            audience: [8; 32],
            nonce: [1; 16],
            services: vec![ServiceKind::Ping, ServiceKind::Tcp],
            not_before: 100,
            expires_at: 160,
            constraints: GrantConstraints {
                tcp_ports: Some(vec![22]),
                ..Default::default()
            },
            tenant: None,
            policy_revision: None,
        };
        let verify = |p: GrantPayload| {
            Grant::issue_at(&key, p)
                .verify(
                    &issuers(&key),
                    &[7; 32],
                    &[8; 32],
                    Duration::from_secs(60),
                    110,
                )
                .unwrap()
        };
        let initial = verify(payload.clone());
        assert!(!initial.permits_renewal(&initial).unwrap());
        let mut next = payload;
        next.revision = 2;
        next.not_before = 110;
        next.expires_at = 170;
        let renewed = verify(next.clone());
        assert_eq!(initial.id, renewed.id);
        assert!(initial.permits_renewal(&renewed).unwrap());
        assert!(renewed.permits_renewal(&initial).is_err());
        for case in 0..7 {
            let mut changed = next.clone();
            match case {
                0 => changed.nonce = [2; 16],
                1 => changed.services.push(ServiceKind::Info),
                2 => changed.services.retain(|s| *s != ServiceKind::Tcp),
                3 => changed.constraints.tcp_ports = None,
                4 => changed.constraints.tcp_ports = Some(vec![]),
                5 => changed.revision = 1,
                6 => changed.expires_at = 160,
                _ => unreachable!(),
            }
            assert!(
                initial.permits_renewal(&verify(changed)).is_err(),
                "case {case}"
            );
        }
        next.revision = 0;
        assert!(matches!(
            Grant::issue_at(&key, next).verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(60),
                110
            ),
            Err(GrantError::InvalidRevision)
        ));
    }

    proptest::proptest! {
        /// Malformed grant bytes never panic — parse or verify fails
        /// cleanly on truncated payloads, garbage signatures and
        /// oversized collections.
        #[test]
        fn grant_decode_never_panics(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..=2048)) {
            let grant = Grant { payload: bytes.clone(), signature: bytes };
            let key = issuer();
            let _ = grant.verify(&issuers(&key), &[7u8; 32], &[8; 32], Duration::from_secs(600), now_unix());
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
            [8; 32],
            [1; 16],
                vec![ServiceKind::Ping],
                Duration::from_secs(300),
                GrantConstraints::default(),
            );
            let now = now_unix();
            proptest::prop_assert!(grant.verify(&issuers(&key), &subject, &[8; 32], Duration::from_secs(600), now).is_ok());
            if let Some(pos) = flip.filter(|p| *p < grant.payload.len()) {
                let mut bad = grant.clone();
                bad.payload[pos] ^= 1;
                proptest::prop_assert!(bad.verify(&issuers(&key), &subject, &[8; 32], Duration::from_secs(600), now).is_err());
            }
        }
    }

    /// A v3 payload with every claim populated.
    fn claimed_payload(key: &SigningKey) -> GrantPayload {
        GrantPayload {
            version: GRANT_VERSION,
            revision: 1,
            issuer: key.verifying_key().to_bytes(),
            subject: [7; 32],
            audience: [8; 32],
            nonce: rand::random(),
            services: vec![ServiceKind::Sync],
            not_before: 100,
            expires_at: 160,
            constraints: GrantConstraints {
                sync_paths: Some(vec!["docs".into(), "media/2026".into()]),
                ..Default::default()
            },
            tenant: Some("tenant-a".into()),
            policy_revision: Some(7),
        }
    }

    #[test]
    fn v3_claims_verify_and_carry_scope() {
        let key = issuer();
        let grant = Grant::issue_at(&key, claimed_payload(&key));
        let verified = grant
            .verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100,
            )
            .expect("v3 grant verifies");
        assert_eq!(verified.tenant(), Some("tenant-a"));
        assert_eq!(verified.policy_revision(), Some(7));
        assert!(verified.permits_sync_path(std::path::Path::new("docs/a.txt")));
        assert!(verified.permits_sync_path(std::path::Path::new("media/2026/x")));
        assert!(verified.permits_sync_path(std::path::Path::new("docs")));
        // Prefix matching is component-wise: a shared name prefix does not
        // widen the scope.
        assert!(!verified.permits_sync_path(std::path::Path::new("docs-else/x")));
        assert!(!verified.permits_sync_path(std::path::Path::new("other/f")));

        // The same payload signed under the v2 domain must not verify.
        let mut cross = grant.clone();
        cross.signature = key
            .sign(&signature_message(GRANT_VERSION_MIN, &grant.payload))
            .to_bytes()
            .to_vec();
        assert!(matches!(
            cross.verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100
            ),
            Err(GrantError::BadSignature)
        ));
    }

    #[test]
    fn v2_grants_still_verify_with_absent_claims() {
        let key = issuer();
        // Hand-encode a genuine version-2 wire payload: pinned layout,
        // v2 signature domain — exactly what an estate issuer minted
        // before v3 produces.
        let old = GrantPayloadV2 {
            version: GRANT_VERSION_MIN,
            revision: 1,
            issuer: key.verifying_key().to_bytes(),
            subject: [7; 32],
            audience: [8; 32],
            nonce: rand::random(),
            services: vec![ServiceKind::Ping],
            not_before: 100,
            expires_at: 160,
            constraints: GrantConstraintsV2 {
                max_bps: None,
                tcp_ports: None,
                displays: None,
            },
        };
        let bytes = postcard::to_stdvec(&old).unwrap();
        let grant = Grant {
            signature: key
                .sign(&signature_message(GRANT_VERSION_MIN, &bytes))
                .to_bytes()
                .to_vec(),
            payload: bytes,
        };
        let verified = grant
            .verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100,
            )
            .expect("v2 grant verifies");
        assert_eq!(verified.payload.version, GRANT_VERSION_MIN);
        assert_eq!(verified.tenant(), None);
        assert_eq!(verified.policy_revision(), None);
        // No path constraint narrows a v2 grant.
        assert!(verified.permits_sync_path(std::path::Path::new("anything/at/all")));
    }

    #[test]
    fn v2_bytes_cannot_smuggle_v3_claims() {
        let key = issuer();
        // A v3-encoded payload with its version field rewritten to 2 must
        // fail — decode is layout-exact per version.
        let mut payload = postcard::to_stdvec(&claimed_payload(&key)).unwrap();
        assert_eq!(payload[0], GRANT_VERSION as u8);
        payload[0] = GRANT_VERSION_MIN as u8;
        let grant = Grant {
            signature: key
                .sign(&signature_message(GRANT_VERSION_MIN, &payload))
                .to_bytes()
                .to_vec(),
            payload,
        };
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100
            ),
            Err(GrantError::Malformed(_))
        ));
        // Unknown versions are refused outright.
        for version in [0u16, 1, 4, u16::MAX] {
            let mut payload = postcard::to_stdvec(&claimed_payload(&key)).unwrap();
            payload[0] = version as u8; // versions under 0x80 are one varint byte
            let grant = Grant {
                signature: key
                    .sign(&signature_message(version, &payload))
                    .to_bytes()
                    .to_vec(),
                payload,
            };
            assert!(matches!(
                grant.verify(
                    &issuers(&key),
                    &[7; 32],
                    &[8; 32],
                    Duration::from_secs(600),
                    100
                ),
                Err(GrantError::Version) | Err(GrantError::Malformed(_))
            ));
        }
    }

    #[test]
    fn v3_claim_bounds_are_enforced() {
        let key = issuer();
        let mut payload = claimed_payload(&key);
        payload.tenant = Some("x".repeat(MAX_TENANT_LEN + 1));
        let grant = Grant::issue_at(&key, payload);
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100
            ),
            Err(GrantError::Oversized)
        ));
        for bad in ["", "has space", "nul\0byte", "line\nbreak"] {
            let mut payload = claimed_payload(&key);
            payload.tenant = Some(bad.into());
            let grant = Grant::issue_at(&key, payload);
            assert!(
                matches!(
                    grant.verify(
                        &issuers(&key),
                        &[7; 32],
                        &[8; 32],
                        Duration::from_secs(600),
                        100
                    ),
                    Err(GrantError::Malformed(_))
                ),
                "tenant {bad:?}"
            );
        }
        let mut payload = claimed_payload(&key);
        payload.constraints.sync_paths =
            Some((0..MAX_SYNC_PATHS + 1).map(|i| format!("p{i}")).collect());
        let grant = Grant::issue_at(&key, payload);
        assert!(matches!(
            grant.verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100
            ),
            Err(GrantError::Oversized)
        ));
        for bad in [
            "/abs",
            "c:/win",
            "../escape",
            "a/../b",
            ".",
            ".\\..\\x",
            "nul\0",
        ] {
            let mut payload = claimed_payload(&key);
            payload.constraints.sync_paths = Some(vec![bad.into()]);
            let grant = Grant::issue_at(&key, payload);
            assert!(
                matches!(
                    grant.verify(
                        &issuers(&key),
                        &[7; 32],
                        &[8; 32],
                        Duration::from_secs(600),
                        100
                    ),
                    Err(GrantError::Malformed(_))
                ),
                "sync_paths entry {bad:?}"
            );
        }
        // Normalized-but-legal entries verify.
        let mut payload = claimed_payload(&key);
        payload.constraints.sync_paths = Some(vec!["a/./b".into(), "deep/sub".into()]);
        let grant = Grant::issue_at(&key, payload);
        assert!(
            grant
                .verify(
                    &issuers(&key),
                    &[7; 32],
                    &[8; 32],
                    Duration::from_secs(600),
                    100
                )
                .is_ok()
        );
    }

    #[test]
    fn renewal_cannot_change_v3_claims() {
        let key = issuer();
        let grant = Grant::issue_at(&key, claimed_payload(&key))
            .verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                100,
            )
            .unwrap();
        let mut next = grant.payload.clone();
        next.revision = 2;
        next.expires_at += 60;
        // Claim changes are scope changes and require fresh authorization.
        for claim in [
            |p: &mut GrantPayload| p.tenant = Some("tenant-b".into()),
            |p: &mut GrantPayload| p.tenant = None,
            |p: &mut GrantPayload| p.policy_revision = Some(8),
            |p: &mut GrantPayload| p.constraints.sync_paths = Some(vec!["other".into()]),
        ] {
            let mut tampered = next.clone();
            claim(&mut tampered);
            let renewed = Grant::issue_at(&key, tampered)
                .verify(
                    &issuers(&key),
                    &[7; 32],
                    &[8; 32],
                    Duration::from_secs(600),
                    110,
                )
                .unwrap();
            assert!(matches!(
                grant.permits_renewal(&renewed),
                Err(GrantError::InvalidRenewal)
            ));
        }
        // Identical claims with an advanced lease renew cleanly.
        let renewed = Grant::issue_at(&key, next)
            .verify(
                &issuers(&key),
                &[7; 32],
                &[8; 32],
                Duration::from_secs(600),
                110,
            )
            .unwrap();
        assert!(matches!(grant.permits_renewal(&renewed), Ok(true)));
    }
}
