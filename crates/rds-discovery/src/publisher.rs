//! Single-owner durable revision allocation and exact signed retry bytes.
use crate::{
    DeletePayload, DeleteRequest, DiscoveryError, EndpointKey, EndpointRecord, Payload, Service,
    authority::invalid,
    persist::AtomicFile,
    record_wire::{DELETE_TTL, MAX_RECORD_TTL, RECORD_VERSION},
};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::Path, time::Duration};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordDraft {
    pub addrs: Vec<SocketAddr>,
    pub relay_urls: Vec<String>,
    pub services: Vec<Service>,
    pub ttl: Duration,
}

#[derive(Clone, Serialize, Deserialize)]
enum Mutation {
    Record(EndpointRecord),
    Delete(DeleteRequest),
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    format: u16,
    key: EndpointKey,
    revision: u64,
    wall_floor: u64,
    pending: Option<Mutation>,
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    state: State,
    digest: [u8; 32],
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

/// Own one publisher per endpoint identity. Every new mutation and its revision
/// commit before it is returned for network publication. A restart can retry
/// the exact signed bytes; a lost reply does not require guessing server state.
pub struct RecordIssuer {
    key: SigningKey,
    state: State,
    disk: Option<AtomicFile>,
    failed: bool,
    observed_wall: u64,
}
impl RecordIssuer {
    /// Explicitly volatile issuer for isolated tests/embedded ephemeral devices.
    pub fn memory(key: SigningKey) -> Self {
        Self {
            state: State {
                format: 1,
                key: EndpointKey(key.verifying_key().to_bytes()),
                revision: 0,
                wall_floor: 0,
                pending: None,
            },
            key,
            disk: None,
            failed: false,
            observed_wall: 0,
        }
    }
    pub fn open(path: &Path, key: SigningKey, now: u64) -> Result<Self, DiscoveryError> {
        let disk = AtomicFile::open_named(path, "publisher.json", "publisher.lock")?;
        let mut issuer = Self::memory(key);
        match disk.read()? {
            Some(bytes) => {
                let envelope: Envelope =
                    serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))?;
                if digest(&envelope.state)? != envelope.digest {
                    return Err(invalid("publisher state checksum mismatch"));
                }
                issuer.state = envelope.state;
                issuer.validate()?;
            }
            None if disk.initialized()? => {
                return Err(invalid("initialized publisher state missing"));
            }
            None => disk.write(&encode(&issuer.state)?)?,
        }
        issuer.check_clock(now)?;
        if !disk.initialized()? {
            disk.seal()?;
        }
        issuer.disk = Some(disk);
        Ok(issuer)
    }
    pub fn key(&self) -> EndpointKey {
        self.state.key
    }

    fn validate(&self) -> Result<(), DiscoveryError> {
        if self.state.format != 1 || self.state.key.0 != self.key.verifying_key().to_bytes() {
            return Err(invalid("publisher format or identity mismatch"));
        }
        let expected = match &self.state.pending {
            Some(Mutation::Record(record)) => {
                let p = record.verify()?;
                Some((p.key, p.revision, p.issued_at))
            }
            Some(Mutation::Delete(tomb)) => {
                let p = tomb.verify()?;
                Some((p.key, p.revision, p.issued_at))
            }
            None => None,
        };
        if match expected {
            Some((key, revision, issued)) => {
                key != self.state.key
                    || revision != self.state.revision
                    || self.state.wall_floor != issued
            }
            None => self.state.revision != 0 || self.state.wall_floor != 0,
        } {
            return Err(invalid("publisher revision history inconsistent"));
        }
        Ok(())
    }
    fn check_clock(&mut self, now: u64) -> Result<(), DiscoveryError> {
        if self.failed {
            return Err(DiscoveryError::Store(
                "publisher commit uncertain; reopen required".into(),
            ));
        }
        if now < self.state.wall_floor || now < self.observed_wall {
            return Err(invalid("publisher wall clock moved backwards"));
        }
        self.observed_wall = now;
        Ok(())
    }
    fn next_revision(&self) -> Result<u64, DiscoveryError> {
        self.state
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("publisher revision exhausted"))
    }
    fn commit(&mut self, revision: u64, now: u64, pending: Mutation) -> Result<(), DiscoveryError> {
        let next = State {
            revision,
            wall_floor: now,
            pending: Some(pending),
            ..self.state.clone()
        };
        let bytes = encode(&next)?;
        self.failed = true;
        if let Some(disk) = &self.disk {
            let stored = disk
                .read()?
                .ok_or_else(|| invalid("publisher state disappeared"))?;
            let envelope: Envelope =
                serde_json::from_slice(&stored).map_err(|e| invalid(&e.to_string()))?;
            if digest(&envelope.state)? != envelope.digest
                || envelope.digest != digest(&self.state)?
            {
                return Err(invalid("publisher state changed during ownership"));
            }
            disk.write(&bytes)?;
        }
        self.state = next;
        self.failed = false;
        Ok(())
    }
    /// Reuse an unchanged record until one third of its signed lifetime has
    /// elapsed. Retries do not refresh expiry or allocate a revision each poll.
    pub fn record(
        &mut self,
        draft: RecordDraft,
        now: u64,
    ) -> Result<EndpointRecord, DiscoveryError> {
        self.check_clock(now)?;
        let ttl = draft.ttl.as_secs();
        if ttl == 0 || ttl > MAX_RECORD_TTL || draft.ttl.subsec_nanos() != 0 {
            return Err(invalid("record TTL must be 1..=3600 whole seconds"));
        }
        if let Some(Mutation::Record(record)) = &self.state.pending {
            let payload = record.verify()?;
            if payload.addrs == draft.addrs
                && payload.relay_urls == draft.relay_urls
                && payload.services == draft.services
                && payload.expires_at - payload.issued_at == ttl
                && now.saturating_sub(payload.issued_at) < (ttl / 3).max(1)
            {
                return Ok(record.clone());
            }
        }
        let revision = self.next_revision()?;
        let record = EndpointRecord::sign(
            &Payload {
                version: RECORD_VERSION,
                key: self.state.key,
                revision,
                addrs: draft.addrs,
                relay_urls: draft.relay_urls,
                services: draft.services,
                issued_at: now,
                expires_at: now
                    .checked_add(ttl)
                    .ok_or_else(|| invalid("TTL overflow"))?,
            },
            &self.key,
        )?;
        self.commit(revision, now, Mutation::Record(record.clone()))?;
        Ok(record)
    }
    /// Deletions use the same revision sequence as publications. A fresh pending
    /// deletion is retried exactly; a later publication advances past it.
    pub fn delete(&mut self, now: u64) -> Result<DeleteRequest, DiscoveryError> {
        self.check_clock(now)?;
        if let Some(Mutation::Delete(tomb)) = &self.state.pending
            && tomb.verify_fresh_at(now).is_ok()
        {
            return Ok(tomb.clone());
        }
        let revision = self.next_revision()?;
        let tomb = DeleteRequest::sign(
            &DeletePayload {
                version: RECORD_VERSION,
                key: self.state.key,
                revision,
                issued_at: now,
                expires_at: now
                    .checked_add(DELETE_TTL)
                    .ok_or_else(|| invalid("TTL overflow"))?,
            },
            &self.key,
        )?;
        self.commit(revision, now, Mutation::Delete(tomb.clone()))?;
        Ok(tomb)
    }
}
