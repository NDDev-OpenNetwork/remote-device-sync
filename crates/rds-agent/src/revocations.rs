//! One observable revocation value owns both contents and freshness.

use crate::AgentPolicy;
use rds_core::grant::GrantId;
use rds_discovery::{
    DiscoveryError,
    client::Client,
    clock::{Lease, Reading},
    policy::PolicyStore,
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Default)]
pub struct RevocationPolicy {
    pub(crate) ids: Arc<HashSet<GrantId>>,
    lease: Option<Lease>,
    local: bool,
    owner: Option<Arc<()>>,
}

impl RevocationPolicy {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn contains(&self, id: &GrantId) -> bool {
        self.ids.contains(id)
    }
    pub fn fresh(&self) -> bool {
        self.local || Reading::now().is_ok_and(|now| self.lease.is_some_and(|l| l.valid_at(now)))
    }
}

impl AgentPolicy {
    /// Explicit local revocation authority for embedders. Managed CLI grant
    /// mode never calls this: it requires a durable estate feed.
    pub fn use_local_revocations(&self) {
        self.denylist.send_modify(|value| {
            let policy = Arc::make_mut(value);
            policy.local = true;
            policy.owner = None;
        });
    }

    /// Preserve known revoked ids while closing managed admission/live leases.
    pub fn require_managed_revocations(&self) {
        self.denylist.send_modify(|value| {
            let policy = Arc::make_mut(value);
            policy.local = false;
            policy.lease = None;
            policy.owner = None;
        });
    }

    fn claim_feed(&self, owner: &Arc<()>) -> Result<(), DiscoveryError> {
        let claimed = self.denylist.send_if_modified(|value| {
            if value.owner.is_some() {
                return false;
            }
            let policy = Arc::make_mut(value);
            policy.local = false;
            policy.lease = None;
            policy.owner = Some(owner.clone());
            true
        });
        if claimed {
            Ok(())
        } else {
            Err(DiscoveryError::Busy)
        }
    }

    fn update_feed(&self, owner: &Arc<()>, update: impl FnOnce(&mut RevocationPolicy)) {
        self.denylist.send_if_modified(|value| {
            if !value
                .owner
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, owner))
            {
                return false;
            }
            update(Arc::make_mut(value));
            true
        });
    }

    fn publish_revocations(&self, owner: &Arc<()>, ids: HashSet<GrantId>, lease: Lease) {
        self.update_feed(owner, |policy| {
            policy.ids = Arc::new(ids);
            policy.lease = Some(lease);
        });
    }

    pub(crate) fn revocations_permit(&self, id: &GrantId) -> bool {
        let value = self.denylist.borrow();
        value.fresh() && !value.contains(id)
    }
}

pub struct RevocationFeed {
    task: JoinHandle<()>,
    policy: Arc<AgentPolicy>,
    owner: Arc<()>,
}

impl Drop for RevocationFeed {
    fn drop(&mut self) {
        self.task.abort();
        self.policy.update_feed(&self.owner, |policy| {
            policy.lease = None;
            policy.owner = None;
        });
    }
}

/// Load the already verified cache before serving; poll signed snapshots and
/// commit before publication. Identical replies never extend the cached lease.
/// Disk jobs are serialized and never mutate runtime policy after cancellation.
pub fn watch_revocations(
    client: Client,
    store: PolicyStore,
    policy: Arc<AgentPolicy>,
    interval: Duration,
) -> Result<RevocationFeed, DiscoveryError> {
    if interval < Duration::from_secs(1) || interval > Duration::from_secs(60) {
        return Err(DiscoveryError::Configuration(
            "revocation interval must be 1..60 seconds".into(),
        ));
    }
    let now = Reading::now()?;
    let owner = Arc::new(());
    policy.claim_feed(&owner)?;
    if let Ok(Some((_, payload, lease))) = store.revocations(now) {
        policy.publish_revocations(&owner, payload.revoked.into_iter().collect(), lease);
    }
    let store = Arc::new(Mutex::new(store));
    let task_policy = policy.clone();
    let task_owner = owner.clone();
    let task = tokio::spawn(async move {
        loop {
            match client.fetch_revocations().await {
                Ok(Some(snapshot)) => {
                    let store = store.clone();
                    let outcome = tokio::task::spawn_blocking(move || {
                        let mut store = store
                            .lock()
                            .map_err(|_| (true, "policy store poisoned".to_owned()))?;
                        let now = Reading::now().map_err(|e| (true, e.to_string()))?;
                        if let Err(e) = store.accept_revocations(&snapshot, now) {
                            return Err((!store.is_healthy(), e.to_string()));
                        }
                        store
                            .revocations(Reading::now().map_err(|e| (true, e.to_string()))?)
                            .map_err(|e| (false, e.to_string()))
                    })
                    .await;
                    match outcome {
                        Ok(Ok(Some((_, payload, lease)))) => task_policy.publish_revocations(
                            &task_owner,
                            payload.revoked.into_iter().collect(),
                            lease,
                        ),
                        Ok(Err((fatal, error))) => {
                            if fatal {
                                task_policy.update_feed(&task_owner, |policy| policy.lease = None);
                                tracing::warn!(%error, "revocation persistence failed; restart required");
                                break;
                            }
                            tracing::debug!(%error, "revocation snapshot rejected");
                        }
                        Err(error) => {
                            task_policy.update_feed(&task_owner, |policy| policy.lease = None);
                            tracing::warn!(%error, "revocation worker failed");
                            break;
                        }
                        Ok(Ok(None)) => {}
                    }
                }
                Ok(None) => {} // Missing data cannot clear revocations or extend freshness.
                Err(error) => tracing::debug!(%error, "revocation fetch failed"),
            }
            tokio::time::sleep(interval).await;
        }
    });
    Ok(RevocationFeed {
        task,
        policy,
        owner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsolete_feed_cannot_publish_after_shutdown_or_invalidate_successor() {
        let policy = AgentPolicy::ssh_only(("127.0.0.1".into(), 9));
        let old = Arc::new(());
        policy.claim_feed(&old).unwrap();
        assert!(matches!(
            policy.claim_feed(&Arc::new(())),
            Err(DiscoveryError::Busy)
        ));
        let now = Reading::now().unwrap();
        let lease = Lease::new(now.wall.as_secs(), now.wall.as_secs() + 60, now).unwrap();
        policy.publish_revocations(&old, HashSet::new(), lease);
        assert!(policy.denylist.borrow().fresh());
        policy.update_feed(&old, |p| {
            p.lease = None;
            p.owner = None;
        });
        policy.publish_revocations(&old, HashSet::new(), lease);
        assert!(!policy.denylist.borrow().fresh());
        let new = Arc::new(());
        policy.claim_feed(&new).unwrap();
        policy.publish_revocations(&new, HashSet::from([[8; 32]]), lease);
        policy.publish_revocations(&old, HashSet::new(), lease);
        policy.update_feed(&old, |p| {
            p.lease = None;
            p.owner = None;
        });
        assert!(policy.denylist.borrow().fresh());
        assert!(policy.denied().contains(&[8; 32]));
    }
}
