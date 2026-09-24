//! Atomic grant admission and connection-owned authorization resources.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rds_core::grant::{self, GrantId, VerifiedGrant};
use rds_core::{HelloAck, write_frame};
use rds_net::Connection;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{AgentPolicy, lock};

const AUTHZ_REPLY_TIMEOUT: Duration = Duration::from_secs(15);

enum State {
    Open,
    Pending,
    Authorizing(Option<GrantLease>),
    Granted(GrantLease),
    Closed,
}

/// Reservation released on every error, cancellation and teardown path.
struct GrantSlot {
    id: GrantId,
    active: Arc<Mutex<HashSet<GrantId>>>,
}

impl GrantSlot {
    fn claim(policy: &AgentPolicy, id: GrantId) -> Result<Self, &'static str> {
        if !lock(&policy.active_grants).insert(id) {
            return Err("grant already in use");
        }
        Ok(Self {
            id,
            active: policy.active_grants.clone(),
        })
    }
}

impl Drop for GrantSlot {
    fn drop(&mut self) {
        lock(&self.active).remove(&self.id);
    }
}

struct GrantLease {
    grant: Arc<VerifiedGrant>,
    watcher: JoinHandle<()>,
    _slot: GrantSlot,
}

impl Drop for GrantLease {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

/// One mutex owns the state, reservation and watchdog together. No await
/// occurs under it; a concurrent Authz can never overwrite a live lease.
pub(crate) struct ConnAuthz {
    state: Mutex<State>,
    sync_busy: std::sync::atomic::AtomicBool,
}

impl ConnAuthz {
    pub(crate) fn new(required: bool) -> Self {
        Self {
            state: Mutex::new(if required {
                State::Pending
            } else {
                State::Open
            }),
            sync_busy: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn begin(&self) -> Result<(), &'static str> {
        let mut state = lock(&self.state);
        match *state {
            State::Pending => {
                *state = State::Authorizing(None);
                Ok(())
            }
            State::Open => Err("agent does not require grants"),
            State::Authorizing(_) | State::Granted(_) => Err("already authorizing or authorized"),
            State::Closed => Err("connection closed"),
        }
    }

    fn install(
        &self,
        conn: &Connection,
        policy: &AgentPolicy,
        verified: VerifiedGrant,
    ) -> Result<(), &'static str> {
        let mut state = lock(&self.state);
        if !matches!(*state, State::Authorizing(None)) || conn.is_closed() {
            return Err("authorization interrupted");
        }
        // Subscribe before checking the current value. An update after
        // this read is either seen by the watchdog's initial check or
        // wakes changed(); there is no check/subscribe gap.
        let denied = policy.denylist.subscribe();
        if denied.borrow().contains(&verified.id) {
            return Err("grant revoked");
        }
        if !verified.live_at(grant::now_unix()) {
            return Err("grant expired");
        }
        let slot = GrantSlot::claim(policy, verified.id)?;
        let grant = Arc::new(verified);
        let watcher = tokio::spawn(watch_grant(conn.clone(), grant.clone(), denied));
        *state = State::Authorizing(Some(GrantLease {
            grant,
            watcher,
            _slot: slot,
        }));
        Ok(())
    }

    fn commit(&self, policy: &AgentPolicy, conn: &Connection) -> Result<(), &'static str> {
        let mut state = lock(&self.state);
        match std::mem::replace(&mut *state, State::Closed) {
            State::Authorizing(Some(lease))
                if !conn.is_closed()
                    && lease.grant.live_at(grant::now_unix())
                    && !policy.denied().contains(&lease.grant.id) =>
            {
                *state = State::Granted(lease);
                Ok(())
            }
            _ => Err("authorization interrupted"),
        }
    }

    fn close(&self) {
        // Drop the lease outside the state lock: teardown releases the
        // replay slot and aborts the watchdog even if stream tasks live on.
        let previous = std::mem::replace(&mut *lock(&self.state), State::Closed);
        drop(previous);
    }

    pub(crate) fn scope(
        &self,
        policy: &AgentPolicy,
    ) -> Result<Option<Arc<VerifiedGrant>>, ScopeError> {
        match &*lock(&self.state) {
            State::Open => Ok(None),
            State::Pending | State::Authorizing(_) => Err(ScopeError::Pending),
            State::Closed => Err(ScopeError::Closed),
            State::Granted(lease) => {
                if !lease.grant.live_at(grant::now_unix()) {
                    return Err(ScopeError::Expired);
                }
                if policy.denied().contains(&lease.grant.id) {
                    return Err(ScopeError::Revoked);
                }
                Ok(Some(lease.grant.clone()))
            }
        }
    }

    pub(crate) fn try_sync_slot(&self) -> bool {
        !self
            .sync_busy
            .swap(true, std::sync::atomic::Ordering::AcqRel)
    }

    pub(crate) fn release_sync_slot(&self) {
        self.sync_busy
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

pub(crate) enum ScopeError {
    Pending,
    Closed,
    Expired,
    Revoked,
}

impl ScopeError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Pending => "grant required: send Authz first",
            Self::Closed => "connection closed",
            Self::Expired => "grant expired",
            Self::Revoked => "grant revoked",
        }
    }

    pub(crate) fn terminal(&self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// Even aborting the connection service future runs authorization cleanup.
pub(crate) struct ConnectionLifetime {
    pub(crate) conn: Connection,
    pub(crate) authz: Arc<ConnAuthz>,
}

impl Drop for ConnectionLifetime {
    fn drop(&mut self) {
        self.authz.close();
        self.conn.close(0u32.into(), b"service ended");
    }
}

/// A canceled/failed ACK must not leave a usable grant or replay reservation.
struct Admission<'a> {
    conn: &'a Connection,
    authz: &'a ConnAuthz,
    committed: bool,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.authz.close();
            self.conn.close(2u32.into(), b"authorization failed");
        }
    }
}

pub(crate) async fn authorize(
    conn: &Connection,
    mut send: rds_net::SendStream,
    grant: grant::Grant,
    policy: &AgentPolicy,
    authz: &ConnAuthz,
) -> anyhow::Result<()> {
    if let Err(message) = authz.begin() {
        let _ = tokio::time::timeout(
            AUTHZ_REPLY_TIMEOUT,
            write_frame(
                &mut send,
                &HelloAck::Error {
                    message: message.into(),
                },
            ),
        )
        .await;
        anyhow::bail!(message);
    }
    let mut admission = Admission {
        conn,
        authz,
        committed: false,
    };
    let verified = grant.verify(
        &policy.issuers,
        conn.remote_id().as_bytes(),
        policy.grant_max_ttl,
        grant::now_unix(),
    )?;
    let id = verified.id;
    authz
        .install(conn, policy, verified)
        .map_err(anyhow::Error::msg)?;
    // Watcher and reservation are already owned while the reply is in
    // flight. Service admission remains pending until this completes.
    tokio::time::timeout(AUTHZ_REPLY_TIMEOUT, write_frame(&mut send, &HelloAck::Ok)).await??;
    send.finish()?;
    authz.commit(policy, conn).map_err(anyhow::Error::msg)?;
    admission.committed = true;
    tracing::info!(peer = %conn.remote_id(), grant = %blake3::Hash::from(id), "grant authorized");
    Ok(())
}

async fn watch_grant(
    conn: Connection,
    grant: Arc<VerifiedGrant>,
    mut denied: watch::Receiver<Arc<HashSet<GrantId>>>,
) {
    loop {
        // borrow_and_update also covers a revocation that happened before
        // this task's first poll, without losing a concurrent later update.
        if denied.borrow_and_update().contains(&grant.id) {
            conn.close(4u32.into(), b"grant revoked");
            return;
        }
        let now = grant::now_unix();
        if !grant.live_at(now) {
            conn.close(3u32.into(), b"grant expired");
            return;
        }
        // Periodic wall-clock checks also handle system-clock changes.
        let wait = Duration::from_secs(grant.payload.expires_at.saturating_sub(now).min(1));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            changed = denied.changed() => {
                if changed.is_err() {
                    conn.close(4u32.into(), b"revocation policy unavailable");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn connection_pair() -> (rds_net::Endpoint, rds_net::Endpoint, Connection, Connection) {
        let config = rds_net::EndpointConfig {
            discovery: false,
            bind_addrs: vec!["127.0.0.1:0".parse().unwrap()],
            ..Default::default()
        };
        let server = rds_net::bind_endpoint(config.clone()).await.unwrap();
        let client = rds_net::bind_endpoint(config).await.unwrap();
        let (outgoing, incoming) =
            tokio::join!(client.connect(server.addr(), rds_core::ALPN), async {
                server.accept().await.unwrap().await.unwrap()
            });
        (server, client, incoming, outgoing.unwrap())
    }

    fn verified(policy: &AgentPolicy, subject: rds_net::EndpointId) -> VerifiedGrant {
        let issuer = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
        let grant = grant::Grant::issue(
            &issuer,
            *subject.as_bytes(),
            vec![rds_core::ServiceKind::Ping],
            Duration::from_secs(60),
            Default::default(),
        );
        grant
            .verify(
                &HashSet::from([issuer.verifying_key().to_bytes()]),
                subject.as_bytes(),
                policy.grant_max_ttl,
                grant::now_unix(),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn watchdog_checks_snapshot_before_waiting_for_changes() {
        let (server, client, conn, _peer) = connection_pair().await;
        let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        let grant = Arc::new(verified(&policy, conn.remote_id()));
        policy.revoke(grant.id);
        // A fresh subscriber considers this existing snapshot seen;
        // waiting only on changed() would miss the revocation forever.
        let receiver = policy.denylist.subscribe();
        tokio::time::timeout(
            Duration::from_secs(2),
            watch_grant(conn.clone(), grant, receiver),
        )
        .await
        .unwrap();
        assert!(conn.is_closed());
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn canceling_admission_releases_grant_and_never_reopens() {
        let (server, client, conn, _peer) = connection_pair().await;
        let policy = Arc::new(AgentPolicy::ssh_only(("127.0.0.1".into(), 9)));
        let authz = Arc::new(ConnAuthz::new(true));
        let (ready, started) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let conn = conn.clone();
            let policy = policy.clone();
            let authz = authz.clone();
            async move {
                authz.begin().unwrap();
                let _admission = Admission {
                    conn: &conn,
                    authz: &authz,
                    committed: false,
                };
                authz
                    .install(&conn, &policy, verified(&policy, conn.remote_id()))
                    .unwrap();
                ready.send(()).unwrap();
                std::future::pending::<()>().await;
            }
        });
        started.await.unwrap();
        assert_eq!(lock(&policy.active_grants).len(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(conn.is_closed());
        assert!(lock(&policy.active_grants).is_empty());
        assert!(authz.begin().is_err());
        assert!(authz.commit(&policy, &conn).is_err());
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn service_admission_rechecks_policy_and_connection_drop_releases_lease() {
        let (server, client, conn, _peer) = connection_pair().await;
        let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        let authz = Arc::new(ConnAuthz::new(true));
        authz.begin().unwrap();
        let grant = verified(&policy, conn.remote_id());
        let id = grant.id;
        authz.install(&conn, &policy, grant).unwrap();
        assert!(matches!(authz.scope(&policy), Err(ScopeError::Pending)));
        authz.commit(&policy, &conn).unwrap();
        assert!(authz.scope(&policy).is_ok());
        // No await: the watchdog has not run; service admission itself
        // must reject this revocation.
        policy.revoke(id);
        assert!(matches!(authz.scope(&policy), Err(ScopeError::Revoked)));
        policy.replace_denylist(HashSet::new());
        {
            let mut state = lock(&authz.state);
            let State::Granted(lease) = &mut *state else {
                panic!("missing grant")
            };
            Arc::make_mut(&mut lease.grant).payload.expires_at = 0;
        }
        assert!(matches!(authz.scope(&policy), Err(ScopeError::Expired)));
        drop(ConnectionLifetime {
            conn: conn.clone(),
            authz: authz.clone(),
        });
        assert!(lock(&policy.active_grants).is_empty());
        assert!(authz.begin().is_err());
        client.close().await;
        server.close().await;
    }

    #[test]
    fn concurrent_authz_has_one_owner_and_cannot_reopen_after_close() {
        let authz = Arc::new(ConnAuthz::new(true));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let successes = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let barrier = barrier.clone();
                let authz = authz.clone();
                let successes = &successes;
                scope.spawn(move || {
                    barrier.wait();
                    if authz.begin().is_ok() {
                        successes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(successes.load(std::sync::atomic::Ordering::Relaxed), 1);
        authz.close();
        assert!(authz.begin().is_err());
    }

    #[test]
    fn reservation_drop_releases_only_its_own_slot() {
        let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        let first = GrantSlot::claim(&policy, [1; 32]).unwrap();
        let second = GrantSlot::claim(&policy, [2; 32]).unwrap();
        assert!(GrantSlot::claim(&policy, [1; 32]).is_err());
        drop(first);
        assert_eq!(*lock(&policy.active_grants), HashSet::from([[2; 32]]));
        drop(second);
        assert!(lock(&policy.active_grants).is_empty());
    }
}
