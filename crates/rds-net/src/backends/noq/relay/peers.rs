//! Bounded identity ownership for the relay's synthetic address namespace.
use iroh::EndpointId;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerRegistrationError {
    #[error("relay peer table is full of active registrations")]
    Capacity,
    #[error("relay synthetic address is owned by another identity")]
    Collision,
    #[error("relay peer registration reference limit reached")]
    ReferenceLimit,
}

/// Keeps one peer mapping pinned while its connection or caller needs it.
/// Dropping the last lease makes the entry eligible for bounded retirement.
#[must_use = "retain the lease for the lifetime of relay I/O"]
pub struct PeerLease {
    registry: Arc<PeerRegistry>,
    address: SocketAddr,
    peer: EndpointId,
}
impl std::fmt::Debug for PeerLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerLease")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}
impl Drop for PeerLease {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = state.entries.get_mut(&self.address)
            && entry.peer == self.peer
            && entry.pins > 0
        {
            entry.pins -= 1;
            if entry.pins == 0 {
                entry.expires = Instant::now() + self.registry.grace;
            }
        }
    }
}

struct Entry {
    peer: EndpointId,
    pins: usize,
    expires: Instant,
}
struct State {
    entries: HashMap<SocketAddr, Entry>,
    capacity: usize,
    grace: Duration,
}

pub(super) struct PeerRegistry {
    state: Mutex<State>,
    grace: Duration,
}
impl PeerRegistry {
    pub fn new(capacity: usize, grace: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                entries: HashMap::new(),
                capacity,
                grace,
            }),
            grace,
        })
    }
    pub fn acquire(self: &Arc<Self>, peer: EndpointId) -> Result<PeerLease, PeerRegistrationError> {
        let address = self.state.lock().unwrap_or_else(|p| p.into_inner()).admit(
            peer,
            true,
            Instant::now(),
        )?;
        Ok(PeerLease {
            registry: self.clone(),
            address,
            peer,
        })
    }
    pub fn observe(&self, peer: EndpointId) -> Result<(), PeerRegistrationError> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admit(peer, false, Instant::now())
            .map(|_| ())
    }
    pub fn get(&self, address: SocketAddr) -> Option<EndpointId> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(super::super::candidates::canonical(address), Instant::now())
    }
    /// Storage occupancy, including bounded grace entries, and pinned peers.
    pub fn occupancy(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (
            state.entries.len(),
            state
                .entries
                .values()
                .filter(|entry| entry.pins > 0)
                .count(),
        )
    }
}
impl State {
    fn admit(
        &mut self,
        peer: EndpointId,
        pin: bool,
        now: Instant,
    ) -> Result<SocketAddr, PeerRegistrationError> {
        let address = super::synthetic_for(&peer);
        if self
            .entries
            .get(&address)
            .is_some_and(|entry| entry.pins == 0 && entry.expires <= now)
        {
            self.entries.remove(&address);
        }
        if let Some(entry) = self.entries.get_mut(&address) {
            if entry.peer != peer {
                return Err(PeerRegistrationError::Collision);
            }
            if pin {
                entry.pins = entry
                    .pins
                    .checked_add(1)
                    .ok_or(PeerRegistrationError::ReferenceLimit)?;
            }
            entry.expires = now + self.grace;
            return Ok(address);
        }
        if self.entries.len() == self.capacity {
            // O(capacity) work occurs only on new-peer pressure, not on every
            // datagram from an already known peer.
            self.entries
                .retain(|_, entry| entry.pins > 0 || entry.expires > now);
            if self.entries.len() == self.capacity {
                let oldest = self
                    .entries
                    .iter()
                    .filter(|(_, entry)| entry.pins == 0)
                    .min_by_key(|(_, entry)| entry.expires)
                    .map(|(address, _)| *address);
                let Some(oldest) = oldest else {
                    return Err(PeerRegistrationError::Capacity);
                };
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            address,
            Entry {
                peer,
                pins: usize::from(pin),
                expires: now + self.grace,
            },
        );
        Ok(address)
    }
    fn get(&mut self, address: SocketAddr, now: Instant) -> Option<EndpointId> {
        if self
            .entries
            .get(&address)
            .is_some_and(|entry| entry.pins == 0 && entry.expires <= now)
        {
            self.entries.remove(&address);
            return None;
        }
        let entry = self.entries.get_mut(&address)?;
        entry.expires = now + self.grace;
        Some(entry.peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer(seed: u8) -> EndpointId {
        crate::SecretKey::from_bytes(&[seed; 32]).public()
    }
    fn state(capacity: usize) -> State {
        State {
            entries: HashMap::new(),
            capacity,
            grace: Duration::from_secs(30),
        }
    }
    #[test]
    fn active_registrations_cannot_be_evicted_by_pressure() {
        let now = Instant::now();
        let mut table = state(2);
        table.admit(peer(1), true, now).unwrap();
        table.admit(peer(2), true, now).unwrap();
        assert_eq!(
            table.admit(peer(3), true, now),
            Err(PeerRegistrationError::Capacity)
        );
        assert_eq!(
            table.admit(peer(3), false, now),
            Err(PeerRegistrationError::Capacity)
        );
        assert_eq!(table.entries.len(), 2);
        assert_eq!(
            table.get(super::super::synthetic_for(&peer(1)), now),
            Some(peer(1))
        );
    }
    #[test]
    fn only_unpinned_entries_are_evicted_and_expire() {
        let now = Instant::now();
        let mut table = state(2);
        let pinned = table.admit(peer(1), true, now).unwrap();
        let learned = table.admit(peer(2), false, now).unwrap();
        let successor = table
            .admit(peer(3), false, now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(table.get(learned, now), None);
        assert_eq!(
            table.get(pinned, now + Duration::from_secs(100)),
            Some(peer(1))
        );
        assert_eq!(table.get(successor, now + Duration::from_secs(31)), None);
    }
    #[test]
    fn concurrent_leases_release_one_reference_and_leave_bounded_grace() {
        let table = PeerRegistry::new(1, Duration::from_secs(30));
        let first = table.acquire(peer(1)).unwrap();
        let second = table.acquire(peer(1)).unwrap();
        assert_eq!(table.occupancy(), (1, 1));
        drop(first);
        assert_eq!(
            table.acquire(peer(2)).unwrap_err(),
            PeerRegistrationError::Capacity
        );
        assert_eq!(table.occupancy(), (1, 1));
        drop(second);
        assert_eq!(table.occupancy(), (1, 0));
        assert_eq!(
            table.get(super::super::synthetic_for(&peer(1))),
            Some(peer(1))
        );
        let replacement = table.acquire(peer(2)).unwrap();
        assert_eq!(table.occupancy(), (1, 1));
        drop(replacement);
    }
    #[test]
    fn learned_peer_can_be_promoted_to_a_protected_owner() {
        let table = PeerRegistry::new(1, Duration::from_secs(30));
        table.observe(peer(1)).unwrap();
        let lease = table.acquire(peer(1)).unwrap();
        assert_eq!(table.observe(peer(2)), Err(PeerRegistrationError::Capacity));
        assert_eq!(
            table.get(super::super::synthetic_for(&peer(1))),
            Some(peer(1))
        );
        drop(lease);
        table.observe(peer(2)).unwrap();
        assert_eq!(table.occupancy(), (1, 0));
    }
    #[test]
    fn real_synthetic_collision_preserves_the_original_identity() {
        let key = |index: u32| {
            let mut bytes = [123; 32];
            bytes[..4].copy_from_slice(&index.to_le_bytes());
            crate::SecretKey::from_bytes(&bytes).public()
        };
        let a = key(153039);
        let b = key(167304);
        assert_ne!(a, b);
        assert_eq!(
            super::super::synthetic_for(&a),
            super::super::synthetic_for(&b)
        );
        let table = PeerRegistry::new(2, Duration::from_secs(30));
        let _lease = table.acquire(a).unwrap();
        assert_eq!(
            table.acquire(b).unwrap_err(),
            PeerRegistrationError::Collision
        );
        assert_eq!(table.observe(b), Err(PeerRegistrationError::Collision));
        assert_eq!(table.get(super::super::synthetic_for(&a)), Some(a));
    }
}
