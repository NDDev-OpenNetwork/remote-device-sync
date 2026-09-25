use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use rds_core::local::{ErrorCode, MAX_SESSIONS, Session, SessionId, Snapshot, Status};
use rds_net::{Connection, EndpointId, metrics::ConnSampler};
use tokio_util::sync::CancellationToken;

pub(super) type Shared = Arc<Mutex<State>>;

pub(super) struct Entry {
    pub peer: EndpointId,
    pub credential: Option<[u8; 32]>,
    pub cancel: CancellationToken,
    pub conn: Option<Connection>,
    pub sampler: Option<ConnSampler>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(conn) = &self.conn {
            conn.close(0u32.into(), b"local session ended");
        }
    }
}

pub(super) struct State {
    pub instance: SessionId,
    pub endpoint: String,
    pub generation: u64,
    pub selected: Option<SessionId>,
    pub entries: BTreeMap<SessionId, Entry>,
    pub stopped: bool,
}

impl State {
    pub fn new(endpoint: EndpointId) -> Shared {
        Arc::new(Mutex::new(Self {
            instance: SessionId(rand::random()),
            endpoint: endpoint.to_string(),
            generation: 0,
            selected: None,
            entries: BTreeMap::new(),
            stopped: false,
        }))
    }

    pub fn changed(&mut self) {
        // The process cannot perform 2^64 transitions in its lifetime.
        self.generation = self.generation.saturating_add(1);
    }

    pub fn prune(&mut self) {
        let before = self.entries.len();
        self.entries
            .retain(|_, e| !e.conn.as_ref().is_some_and(Connection::is_closed));
        if self.entries.len() != before {
            if self
                .selected
                .is_some_and(|id| !self.entries.contains_key(&id))
            {
                self.selected = None;
            }
            self.changed();
        }
    }

    pub fn remove(&mut self, id: SessionId) -> bool {
        if self.entries.remove(&id).is_none() {
            return false;
        }
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.changed();
        true
    }

    pub fn connection(&self, id: Option<SessionId>) -> Result<(SessionId, Connection), ErrorCode> {
        let id = id.or(self.selected).ok_or(ErrorCode::NoSelection)?;
        let entry = self.entries.get(&id).ok_or(ErrorCode::NotFound)?;
        let conn = entry.conn.as_ref().ok_or(ErrorCode::Busy)?;
        Ok((id, conn.clone()))
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            instance: self.instance,
            generation: self.generation,
            endpoint: self.endpoint.clone(),
            selected: self.selected,
            sessions: self
                .entries
                .iter()
                .map(|(id, entry)| Session {
                    id: *id,
                    peer: entry.peer.to_string(),
                    status: if entry.conn.is_some() {
                        Status::Connected
                    } else {
                        Status::Connecting
                    },
                })
                .collect(),
        }
    }
}

pub(super) fn lock(shared: &Shared) -> Result<MutexGuard<'_, State>, ErrorCode> {
    let mut state = shared.lock().map_err(|_| ErrorCode::Internal)?;
    if state.stopped {
        return Err(ErrorCode::Stopped);
    }
    state.prune();
    Ok(state)
}

/// The runner is the sole lifetime owner, independently of worker-held Arcs.
pub(super) struct Owner(pub Shared);
impl Drop for Owner {
    fn drop(&mut self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopped = true;
        state.entries.clear();
        state.selected = None;
        state.changed();
    }
}

/// Roll back a reserved/new connection on cancel, error or failed reply write.
pub(super) struct Reservation {
    pub shared: Shared,
    pub id: SessionId,
    pub cancel: CancellationToken,
    pub committed: bool,
}

pub(super) enum Reserved {
    Existing(SessionId),
    New(Reservation),
}

pub(super) fn reserve(
    shared: &Shared,
    peer: EndpointId,
    credential: Option<[u8; 32]>,
) -> Result<Reserved, ErrorCode> {
    let mut state = lock(shared)?;
    if let Some((id, entry)) = state.entries.iter().find(|(_, e)| e.peer == peer) {
        if entry.credential != credential {
            return Err(ErrorCode::CredentialConflict);
        }
        return if entry.conn.is_some() {
            Ok(Reserved::Existing(*id))
        } else {
            Err(ErrorCode::Busy)
        };
    }
    if state.entries.len() >= MAX_SESSIONS {
        return Err(ErrorCode::Capacity);
    }
    let id = loop {
        let candidate = SessionId(rand::random());
        if !state.entries.contains_key(&candidate) {
            break candidate;
        }
    };
    let cancel = CancellationToken::new();
    state.entries.insert(
        id,
        Entry {
            peer,
            credential,
            cancel: cancel.clone(),
            conn: None,
            sampler: None,
        },
    );
    state.changed();
    Ok(Reserved::New(Reservation {
        shared: shared.clone(),
        id,
        cancel,
        committed: false,
    }))
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.committed {
            self.shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(self.id);
        }
    }
}

/// Weak, nonblocking aggregate observation; never retains an endpoint/session.
pub struct Observer(Weak<Mutex<State>>);

impl Observer {
    pub(super) fn new(shared: &Shared) -> Self {
        Self(Arc::downgrade(shared))
    }

    pub fn snapshot(&self) -> rds_observe::admin::Snapshot {
        let mut result = BTreeMap::from([("rds_agent_local_manager_snapshot_available", 0)]);
        let Some(shared) = self.0.upgrade() else {
            return result;
        };
        let Ok(state) = shared.try_lock() else {
            return result;
        };
        if state.stopped {
            return result;
        }
        let connected = state
            .entries
            .values()
            .filter(|e| e.conn.as_ref().is_some_and(|c| !c.is_closed()))
            .count();
        let connecting = state.entries.values().filter(|e| e.conn.is_none()).count();
        result.extend([
            ("rds_agent_local_manager_snapshot_available", 1),
            ("rds_agent_local_manager_connected", connected as u64),
            ("rds_agent_local_manager_connecting", connecting as u64),
            ("rds_agent_local_manager_capacity", MAX_SESSIONS as u64),
        ]);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_counts_pending_and_dropped_reservations_release_slots() {
        let shared = State::new(rds_net::SecretKey::generate().public());
        let owner = Owner(shared.clone());
        let mut reservations = Vec::new();
        for _ in 0..MAX_SESSIONS {
            let Reserved::New(reservation) =
                reserve(&shared, rds_net::SecretKey::generate().public(), None).unwrap()
            else {
                panic!("new peer")
            };
            reservations.push(reservation);
        }
        assert!(matches!(
            reserve(&shared, rds_net::SecretKey::generate().public(), None),
            Err(ErrorCode::Capacity)
        ));
        let observer = Observer::new(&shared);
        assert_eq!(
            observer.snapshot()["rds_agent_local_manager_connecting"],
            MAX_SESSIONS as u64
        );
        let guard = lock(&shared).unwrap();
        assert_eq!(
            observer.snapshot()["rds_agent_local_manager_snapshot_available"],
            0
        );
        drop(guard);
        reservations.pop();
        assert_eq!(lock(&shared).unwrap().entries.len(), MAX_SESSIONS - 1);
        let Reserved::New(reservation) =
            reserve(&shared, rds_net::SecretKey::generate().public(), None).unwrap()
        else {
            panic!("new peer")
        };
        drop(owner);
        assert!(reservation.cancel.is_cancelled());
        assert!(matches!(lock(&shared), Err(ErrorCode::Stopped)));
        assert_eq!(
            observer.snapshot()["rds_agent_local_manager_snapshot_available"],
            0
        );
    }
}
