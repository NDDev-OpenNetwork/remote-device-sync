//! Durable acceptance of policy snapshots. Runtime publication follows commit.

use crate::{
    DiscoveryError,
    authority::{Authority, SignedRotation, SnapshotStamp, invalid},
    clock::{Lease, Reading},
    persist::AtomicFile,
    registry::{NameBindingPayload, SignedNameBinding, SignedRegistry},
    revocations::{RevocationPayload, SignedRevocations},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

const MAX_ROTATIONS: usize = 16;
const MAX_NAMES: usize = 1024;

#[cfg(test)]
mod tests;

/// Bounded configuration input; receipts are authenticated when applied.
pub fn read_rotations(paths: &[std::path::PathBuf]) -> Result<Vec<SignedRotation>, DiscoveryError> {
    use std::io::Read;
    if paths.len() > MAX_ROTATIONS {
        return Err(invalid("too many authority rotation receipts"));
    }
    paths
        .iter()
        .map(|path| {
            let file =
                std::fs::File::open(path).map_err(|e| DiscoveryError::Store(e.to_string()))?;
            let mut bytes = Vec::new();
            file.take(8193)
                .read_to_end(&mut bytes)
                .map_err(|e| DiscoveryError::Store(e.to_string()))?;
            if bytes.len() > 8192 {
                return Err(invalid("authority rotation receipt too large"));
            }
            serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Names {
    anchor: SignedNameBinding,
    hashes: BTreeMap<String, [u8; 32]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct State {
    format: u16,
    bootstrap: Authority,
    authority: Authority,
    rotations: Vec<SignedRotation>,
    wall_floor: u64,
    registry: Option<SignedRegistry>,
    revocations: Option<SignedRevocations>,
    names: Option<Names>,
    registry_lease: Option<Lease>,
    revocations_lease: Option<Lease>,
    names_lease: Option<Lease>,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    state: State,
    digest: [u8; 32],
}

/// One writer owns a protected directory. Use `memory` explicitly in tests or
/// ephemeral embedders. Production binaries use `open` and never fall back.
pub struct PolicyStore {
    state: State,
    disk: Option<AtomicFile>,
    failed: bool,
}

impl PolicyStore {
    pub fn memory(bootstrap: Authority) -> Result<Self, DiscoveryError> {
        bootstrap.verifying_key()?;
        Ok(Self {
            state: State {
                format: 1,
                bootstrap,
                authority: bootstrap,
                rotations: Vec::new(),
                wall_floor: 0,
                registry: None,
                revocations: None,
                names: None,
                registry_lease: None,
                revocations_lease: None,
                names_lease: None,
            },
            disk: None,
            failed: false,
        })
    }

    pub fn open(path: &Path, bootstrap: Authority, now: Reading) -> Result<Self, DiscoveryError> {
        let disk = AtomicFile::open(path)?;
        let mut store = Self::memory(bootstrap)?;
        match disk.read()? {
            Some(bytes) => {
                let envelope: Envelope =
                    serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))?;
                if digest(&envelope.state)? != envelope.digest {
                    return Err(invalid("policy state checksum mismatch"));
                }
                store.state = envelope.state;
                store.validate(bootstrap)?;
                store.check_clock(now)?;
            }
            None if disk.initialized()? => {
                return Err(invalid("committed policy state is missing"));
            }
            None => disk.write(&encode(&store.state)?)?,
        }
        // Read-only cache hits must not rewrite/sync the initialization marker.
        // The first seal syncs both the marker inode and its directory entry.
        if !disk.initialized()? {
            disk.seal()?;
        }
        store.disk = Some(disk);
        Ok(store)
    }

    fn validate(&self, bootstrap: Authority) -> Result<(), DiscoveryError> {
        if self.state.format != 1
            || self.state.bootstrap != bootstrap
            || self.state.rotations.len() > MAX_ROTATIONS
        {
            return Err(invalid(
                "policy state format or bootstrap authority mismatch",
            ));
        }
        let mut authority = bootstrap;
        for receipt in &self.state.rotations {
            authority = receipt.verify_committed(authority)?.next;
        }
        if self.state.authority != authority {
            return Err(invalid("policy authority lacks rotation continuity"));
        }
        let key = authority.verifying_key()?;
        if let Some(snap) = &self.state.registry {
            let payload = snap.verify(&key)?;
            self.check_stamp(payload.stamp)?;
            if !self
                .state
                .registry_lease
                .is_some_and(|l| l.matches_interval(payload.issued_at, payload.expires_at))
            {
                return Err(invalid("registry lease metadata mismatch"));
            }
        }
        if let Some(snap) = &self.state.revocations {
            let payload = snap.verify(&key)?;
            self.check_stamp(payload.stamp)?;
            if !self
                .state
                .revocations_lease
                .is_some_and(|l| l.matches_interval(payload.issued_at, payload.expires_at))
            {
                return Err(invalid("revocation lease metadata mismatch"));
            }
        }
        if let Some(names) = &self.state.names {
            let payload = names.anchor.verify_committed(&key)?;
            self.check_stamp(payload.stamp)?;
            if !self
                .state
                .names_lease
                .is_some_and(|l| l.matches_interval(payload.issued_at, payload.expires_at))
            {
                return Err(invalid("name lease metadata mismatch"));
            }
            if names.hashes.len() > MAX_NAMES
                || names.hashes.keys().any(|n| !crate::registry::valid_name(n))
                || names.hashes.get(&payload.name)
                    != Some(blake3::hash(&names.anchor.payload).as_bytes())
            {
                return Err(invalid("invalid name freshness state"));
            }
        }
        Ok(())
    }

    pub fn authority(&self) -> Authority {
        self.state.authority
    }

    pub fn bootstrap(&self) -> Authority {
        self.state.bootstrap
    }

    pub fn is_healthy(&self) -> bool {
        !self.failed
    }

    pub fn bootstrap_registry(
        &mut self,
        snap: &SignedRegistry,
        now: Reading,
    ) -> Result<(), DiscoveryError> {
        let key = self.authority().verifying_key()?;
        let incoming = snap.verify(&key)?;
        self.check_stamp(incoming.stamp)?;
        if let Some(previous) = &self.state.registry {
            let stored = previous.verify(&key)?;
            if stored.stamp.revision > incoming.stamp.revision {
                return Ok(());
            }
        }
        self.accept_registry(snap, now)?;
        Ok(())
    }

    fn check_stamp(&self, stamp: SnapshotStamp) -> Result<(), DiscoveryError> {
        if stamp.authority != self.state.authority {
            return Err(DiscoveryError::BadSignature);
        }
        Ok(())
    }

    fn check_clock(&self, now: Reading) -> Result<(), DiscoveryError> {
        if self.failed {
            return Err(DiscoveryError::Store(
                "policy commit outcome uncertain; reopen required".into(),
            ));
        }
        if now.wall.as_secs() < self.state.wall_floor {
            return Err(invalid("clock precedes committed policy time"));
        }
        Ok(())
    }

    fn commit(&mut self, state: State) -> Result<(), DiscoveryError> {
        if let Some(disk) = &self.disk {
            let result = (|| {
                if disk.read()?.is_none() {
                    return Err(invalid("committed policy state disappeared"));
                }
                disk.write(&encode(&state)?)
            })();
            if let Err(e) = result {
                self.failed = true;
                return Err(e);
            }
        }
        self.state = state;
        Ok(())
    }

    pub fn apply_rotation(
        &mut self,
        receipt: &SignedRotation,
        now: Reading,
    ) -> Result<(), DiscoveryError> {
        self.check_clock(now)?;
        if self.state.rotations.contains(receipt) {
            return Ok(());
        }
        if self.state.rotations.len() >= MAX_ROTATIONS {
            return Err(invalid("authority rotation chain full"));
        }
        let transition = receipt.verify_at(self.state.authority, now.wall.as_secs())?;
        let mut next = self.state.clone();
        next.authority = transition.next;
        next.rotations.push(receipt.clone());
        next.registry = None;
        next.revocations = None;
        next.names = None;
        next.registry_lease = None;
        next.revocations_lease = None;
        next.names_lease = None;
        next.wall_floor = now.wall.as_secs();
        self.commit(next)?;
        Ok(())
    }

    pub fn accept_revocations(
        &mut self,
        snap: &SignedRevocations,
        now: Reading,
    ) -> Result<bool, DiscoveryError> {
        self.check_clock(now)?;
        let key = self.state.authority.verifying_key()?;
        let payload = snap.verify_at(&key, now.wall.as_secs())?;
        self.check_stamp(payload.stamp)?;
        if let Some(previous) = &self.state.revocations {
            let old = previous.verify(&key)?;
            if old.stamp == payload.stamp && previous.payload == snap.payload {
                return Ok(false);
            }
            payload.stamp.newer_than(&old.stamp)?;
        }
        let lease = Lease::new(payload.issued_at, payload.expires_at, now)?;
        let mut next = self.state.clone();
        next.revocations = Some(snap.clone());
        next.wall_floor = now.wall.as_secs();
        next.revocations_lease = Some(lease);
        self.commit(next)?;
        Ok(true)
    }

    pub fn revocations(
        &self,
        now: Reading,
    ) -> Result<Option<(SignedRevocations, RevocationPayload, Lease)>, DiscoveryError> {
        self.check_clock(now)?;
        let Some(snap) = &self.state.revocations else {
            return Ok(None);
        };
        let lease = self
            .state
            .revocations_lease
            .filter(|l| l.valid_at(now))
            .ok_or(DiscoveryError::Expired)?;
        Ok(Some((
            snap.clone(),
            snap.verify(&self.state.authority.verifying_key()?)?,
            lease,
        )))
    }

    pub fn accept_registry(
        &mut self,
        snap: &SignedRegistry,
        now: Reading,
    ) -> Result<bool, DiscoveryError> {
        self.check_clock(now)?;
        let key = self.state.authority.verifying_key()?;
        let payload = snap.verify(&key)?;
        crate::registry::check_lifetime(payload.issued_at, payload.expires_at, now.wall.as_secs())?;
        self.check_stamp(payload.stamp)?;
        if let Some(previous) = &self.state.registry {
            let old = previous.verify(&key)?;
            if old.stamp == payload.stamp && previous.payload == snap.payload {
                return Ok(false);
            }
            payload.stamp.newer_than(&old.stamp)?;
        }
        let lease = Lease::new(payload.issued_at, payload.expires_at, now)?;
        let mut next = self.state.clone();
        next.registry = Some(snap.clone());
        next.wall_floor = now.wall.as_secs();
        next.registry_lease = Some(lease);
        self.commit(next)?;
        Ok(true)
    }

    pub fn name(&self, name: &str, now: Reading) -> Result<SignedNameBinding, DiscoveryError> {
        self.check_clock(now)?;
        let snap = self
            .state
            .registry
            .as_ref()
            .ok_or(DiscoveryError::NotFound)?;
        if !self.state.registry_lease.is_some_and(|l| l.valid_at(now)) {
            return Err(DiscoveryError::Expired);
        }
        snap.bindings
            .get(name)
            .cloned()
            .ok_or(DiscoveryError::NotFound)
    }

    pub fn accept_name(
        &mut self,
        snap: &SignedNameBinding,
        name: &str,
        now: Reading,
    ) -> Result<NameBindingPayload, DiscoveryError> {
        self.check_clock(now)?;
        let key = self.state.authority.verifying_key()?;
        let payload = snap.verify(&key, name, now.wall.as_secs())?;
        self.check_stamp(payload.stamp)?;
        let mut next = self.state.clone();
        let hash = *blake3::hash(&snap.payload).as_bytes();
        if let Some(names) = &mut next.names {
            let old = names.anchor.verify_committed(&key)?;
            if old.stamp != payload.stamp {
                payload.stamp.newer_than(&old.stamp)?;
                names.hashes.clear();
                next.names_lease = Some(Lease::new(payload.issued_at, payload.expires_at, now)?);
            } else {
                if !next.names_lease.is_some_and(|l| l.valid_at(now)) {
                    return Err(DiscoveryError::Expired);
                }
                if old.registry_digest != payload.registry_digest
                    || old.issued_at != payload.issued_at
                    || old.expires_at != payload.expires_at
                {
                    return Err(DiscoveryError::Stale);
                }
                if let Some(previous) = names.hashes.get(name) {
                    if *previous != hash {
                        return Err(DiscoveryError::Stale);
                    }
                    return Ok(payload);
                }
            }
            if names.hashes.len() >= MAX_NAMES {
                return Err(invalid("name freshness cache full"));
            }
            names.anchor = snap.clone();
            names.hashes.insert(name.into(), hash);
        } else {
            next.names_lease = Some(Lease::new(payload.issued_at, payload.expires_at, now)?);
            next.names = Some(Names {
                anchor: snap.clone(),
                hashes: BTreeMap::from([(name.into(), hash)]),
            });
        }
        next.wall_floor = now.wall.as_secs();
        self.commit(next)?;
        Ok(payload)
    }

    pub(crate) fn check_name_lease(&self, now: Reading) -> Result<(), DiscoveryError> {
        self.check_clock(now)?;
        if self
            .state
            .names_lease
            .is_some_and(|lease| lease.valid_at(now))
        {
            Ok(())
        } else {
            Err(DiscoveryError::Expired)
        }
    }
}

fn digest(state: &State) -> Result<[u8; 32], DiscoveryError> {
    Ok(*blake3::hash(&postcard::to_stdvec(state).map_err(|e| invalid(&e.to_string()))?).as_bytes())
}
fn encode(state: &State) -> Result<Vec<u8>, DiscoveryError> {
    serde_json::to_vec(&Envelope {
        state: state.clone(),
        digest: digest(state)?,
    })
    .map_err(|e| invalid(&e.to_string()))
}
