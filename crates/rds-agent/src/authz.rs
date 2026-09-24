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
    watcher: Option<JoinHandle<()>>,
    _slot: GrantSlot,
}

impl Drop for GrantLease {
    fn drop(&mut self) {
        if let Some(watcher) = &self.watcher {
            watcher.abort();
        }
    }
}

/// One mutex owns the state, reservation and watchdog together. No await
/// occurs under it; a concurrent Authz can never overwrite a live lease.
pub(crate) struct ConnAuthz {
    state: Mutex<State>,
    changed: watch::Sender<()>,
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
            changed: watch::channel(()).0,
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
        if !denied.borrow().fresh() {
            return Err("revocation policy unavailable or stale");
        }
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
            watcher: Some(watcher),
            _slot: slot,
        }));
        Ok(())
    }

    fn commit(&self, policy: &AgentPolicy, conn: &Connection) -> Result<(), &'static str> {
        let mut state = lock(&self.state);
        let result = match std::mem::replace(&mut *state, State::Closed) {
            State::Authorizing(Some(lease))
                if !conn.is_closed()
                    && lease.grant.live_at(grant::now_unix())
                    && policy.revocations_permit(&lease.grant.id) =>
            {
                *state = State::Granted(lease);
                Ok(())
            }
            _ => Err("authorization interrupted"),
        };
        drop(state);
        self.changed.send_replace(());
        result
    }

    fn close(&self) {
        // Drop the lease outside the state lock: teardown releases the
        // replay slot and aborts the watchdog even if stream tasks live on.
        let previous = std::mem::replace(&mut *lock(&self.state), State::Closed);
        drop(previous);
        self.changed.send_replace(());
    }

    /// Normal teardown joins the watchdog. The synchronous close/drop path
    /// still aborts it when the owning service future is itself canceled.
    pub(crate) async fn close_and_wait(&self) {
        let previous = std::mem::replace(&mut *lock(&self.state), State::Closed);
        self.changed.send_replace(());
        let watcher = match previous {
            State::Authorizing(Some(mut lease)) | State::Granted(mut lease) => lease.watcher.take(),
            _ => None,
        };
        // The lease and replay reservation have already dropped before await.
        if let Some(watcher) = watcher {
            watcher.abort();
            if let Err(error) = watcher.await
                && !error.is_cancelled()
            {
                tracing::debug!(%error, "grant watchdog ended");
            }
        }
    }

    pub(crate) fn scope(
        &self,
        policy: &AgentPolicy,
    ) -> Result<Option<Arc<VerifiedGrant>>, ScopeError> {
        match &*lock(&self.state) {
            State::Open => Ok(None),
            State::Pending => Err(ScopeError::Pending),
            State::Authorizing(_) => Err(ScopeError::Authorizing),
            State::Closed => Err(ScopeError::Closed),
            State::Granted(lease) => {
                if !lease.grant.live_at(grant::now_unix()) {
                    return Err(ScopeError::Expired);
                }
                let revocations = policy.denylist.borrow();
                if !revocations.fresh() {
                    return Err(ScopeError::PolicyStale);
                }
                if revocations.contains(&lease.grant.id) {
                    return Err(ScopeError::Revoked);
                }
                Ok(Some(lease.grant.clone()))
            }
        }
    }

    /// The peer may observe the successful ACK before the authorization task
    /// resumes to commit. Wait for that transaction without granting early or
    /// rejecting a correctly sequenced service on another QUIC stream.
    pub(crate) async fn service_scope(
        &self,
        policy: &AgentPolicy,
    ) -> Result<Option<Arc<VerifiedGrant>>, ScopeError> {
        // Subscribe before reading the state so commit/close cannot be missed.
        let mut changed = self.changed.subscribe();
        tokio::time::timeout(AUTHZ_REPLY_TIMEOUT, async {
            loop {
                match self.scope(policy) {
                    Err(ScopeError::Authorizing) => {
                        if changed.changed().await.is_err() {
                            return Err(ScopeError::Closed);
                        }
                    }
                    result => return result,
                }
            }
        })
        .await
        .unwrap_or(Err(ScopeError::AuthorizationTimeout))
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
    Authorizing,
    AuthorizationTimeout,
    Closed,
    Expired,
    Revoked,
    PolicyStale,
}

impl ScopeError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Pending => "grant required: send Authz first",
            Self::Authorizing => "authorization in progress",
            Self::AuthorizationTimeout => "authorization completion timed out",
            Self::Closed => "connection closed",
            Self::Expired => "grant expired",
            Self::Revoked => "grant revoked",
            Self::PolicyStale => "revocation policy unavailable or stale",
        }
    }

    pub(crate) fn terminal(&self) -> bool {
        !matches!(self, Self::Pending | Self::Authorizing)
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
    mut denied: watch::Receiver<Arc<crate::RevocationPolicy>>,
) {
    loop {
        // borrow_and_update also covers a revocation that happened before
        // this task's first poll, without losing a concurrent later update.
        let snapshot = denied.borrow_and_update().clone();
        if !snapshot.fresh() {
            conn.close(4u32.into(), b"revocation policy stale");
            return;
        }
        if snapshot.contains(&grant.id) {
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
        policy.use_local_revocations();
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
    async fn service_during_authorization_reply_waits_for_commit() {
        let (server, client, conn, peer) = connection_pair().await;
        let policy = Arc::new(AgentPolicy::ssh_only(("127.0.0.1".into(), 9)));
        let authz = Arc::new(ConnAuthz::new(true));
        authz.begin().unwrap();
        authz
            .install(&conn, &policy, verified(&policy, conn.remote_id()))
            .unwrap();
        // Hold the real admission state in the reply/commit window. A service
        // can arrive on another QUIC stream before that task runs commit().
        let (mut send, mut recv) = peer.open_bi().await.unwrap();
        write_frame(&mut send, &rds_core::StreamHello::Ping { nonce: 41 })
            .await
            .unwrap();
        let (reply, request) = conn.accept_bi().await.unwrap();
        let task = tokio::spawn(crate::serve_stream(
            conn.clone(),
            reply,
            request,
            policy.clone(),
            authz.clone(),
            false,
        ));
        let ack = rds_core::read_frame::<_, HelloAck>(&mut recv);
        tokio::pin!(ack);
        let early = tokio::time::timeout(Duration::from_millis(50), &mut ack).await;
        // Never authorize a service before commit, and never race a premature
        // Pending rejection against the success reply already sent to the peer.
        assert!(
            early.is_err(),
            "service replied before admission committed: {early:?}"
        );
        authz.commit(&policy, &conn).unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), ack)
                .await
                .unwrap()
                .unwrap(),
            HelloAck::Ok
        ));
        task.await.unwrap().unwrap();
        drop(ConnectionLifetime { conn, authz });
        client.close().await;
        server.close().await;
    }

    #[tokio::test]
    async fn waiting_service_wakes_on_aborted_or_revoked_admission() {
        for revoke in [false, true] {
            let (server, client, conn, _peer) = connection_pair().await;
            let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
            let authz = ConnAuthz::new(true);
            assert!(matches!(
                authz.service_scope(&policy).await,
                Err(ScopeError::Pending)
            ));
            authz.begin().unwrap();
            let grant = verified(&policy, conn.remote_id());
            let id = grant.id;
            authz.install(&conn, &policy, grant).unwrap();
            let waiting = authz.service_scope(&policy);
            tokio::pin!(waiting);
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut waiting)
                    .await
                    .is_err()
            );
            if revoke {
                policy.revoke(id);
                assert!(authz.commit(&policy, &conn).is_err());
            } else {
                authz.close();
            }
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(2), waiting)
                    .await
                    .unwrap(),
                Err(ScopeError::Closed)
            ));
            assert!(lock(&policy.active_grants).is_empty());
            conn.close(0u32.into(), b"done");
            client.close().await;
            server.close().await;
        }
    }

    #[tokio::test]
    async fn normal_teardown_joins_watchdog_and_releases_reservation() {
        let (server, client, conn, _peer) = connection_pair().await;
        let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        let authz = ConnAuthz::new(true);
        authz.begin().unwrap();
        authz
            .install(&conn, &policy, verified(&policy, conn.remote_id()))
            .unwrap();
        authz.commit(&policy, &conn).unwrap();
        let watcher = {
            let state = lock(&authz.state);
            let State::Granted(lease) = &*state else {
                panic!("missing committed grant");
            };
            lease.watcher.as_ref().unwrap().abort_handle()
        };
        assert!(!watcher.is_finished());
        authz.close_and_wait().await;
        assert!(watcher.is_finished());
        assert!(lock(&policy.active_grants).is_empty());
        assert!(matches!(
            authz.service_scope(&policy).await,
            Err(ScopeError::Closed)
        ));
        conn.close(0u32.into(), b"done");
        client.close().await;
        server.close().await;
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
        assert!(matches!(authz.scope(&policy), Err(ScopeError::Authorizing)));
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
