//! Signed content may be reclaimed; the trusted local revision floor never is.
use super::*;
use crate::clock::{Lease, Reading};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Entry {
    Record(EndpointRecord),
    Deleted(DeleteRequest),
}
impl Entry {
    fn details(&self, key: &EndpointKey) -> Result<(u64, u64, u64), DiscoveryError> {
        let (actual, revision, issued, expires) = match self {
            Self::Record(record) => {
                let p = record.verify()?;
                (p.key, p.revision, p.issued_at, p.expires_at)
            }
            Self::Deleted(tomb) => {
                let p = tomb.verify()?;
                (p.key, p.revision, p.issued_at, p.expires_at)
            }
        };
        if actual != *key {
            return Err(DiscoveryError::BadSignature);
        }
        Ok((revision, issued, expires))
    }
    fn digest(&self) -> Result<[u8; 32], DiscoveryError> {
        Ok(*blake3::hash(&postcard::to_stdvec(self).map_err(error)?).as_bytes())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Stored {
    Active {
        entry: Entry,
        lease: Lease,
    },
    // This is integrity-protected database state, NOT a transferable signed
    // proof. Its digest binds the exact original operation, including its kind.
    Retired {
        revision: u64,
        digest: [u8; 32],
        deleted: bool,
    },
}
impl Stored {
    pub(super) fn verify(&self, key: &EndpointKey) -> Result<u64, DiscoveryError> {
        match self {
            Self::Active { entry, lease } => {
                let (revision, issued, expires) = entry.details(key)?;
                if !lease.matches_interval(issued, expires) {
                    return Err(error("stored record lease disagrees with signed interval"));
                }
                Ok(revision)
            }
            Self::Retired { revision, .. } => {
                ed25519_dalek::VerifyingKey::from_bytes(&key.0)
                    .map_err(|_| DiscoveryError::BadSignature)?;
                if *revision == 0 {
                    return Err(error("retired record has no revision floor"));
                }
                Ok(*revision)
            }
        }
    }
    pub(super) fn live(&self) -> bool {
        matches!(
            self,
            Self::Active {
                entry: Entry::Record(_),
                ..
            }
        )
    }
    pub(super) fn accepted_wall(&self) -> Duration {
        match self {
            Self::Active { lease, .. } => lease.accepted_wall(),
            Self::Retired { .. } => Duration::ZERO,
        }
    }
    fn digest(&self) -> Result<[u8; 32], DiscoveryError> {
        match self {
            Self::Active { entry, .. } => entry.digest(),
            Self::Retired { digest, .. } => Ok(*digest),
        }
    }
    pub(super) fn retire(
        &self,
        key: &EndpointKey,
        now: Reading,
    ) -> Result<Option<Self>, DiscoveryError> {
        if let Self::Active { entry, lease } = self
            && !lease.valid_at(now)
        {
            return Ok(Some(Self::Retired {
                revision: self.verify(key)?,
                digest: entry.digest()?,
                deleted: matches!(entry, Entry::Deleted(_)),
            }));
        }
        Ok(None)
    }
    fn read(&self) -> Result<Option<EndpointRecord>, DiscoveryError> {
        match self {
            Self::Active {
                entry: Entry::Record(record),
                ..
            } => Ok(Some(record.clone())),
            Self::Retired { deleted: false, .. } => Err(DiscoveryError::Expired),
            _ => Err(DiscoveryError::NotFound),
        }
    }
}

pub(super) struct Decision {
    pub replacement: Option<Stored>,
    pub result: Result<Option<EndpointRecord>, DiscoveryError>,
}

/// Retire before returning an observed expiry, even on a refused mutation.
/// A retry can acknowledge an existing lease but can never create a new one.
pub(super) fn decide(
    old: Option<&Stored>,
    incoming: Option<Entry>,
    key: &EndpointKey,
    now: Reading,
) -> Result<Decision, DiscoveryError> {
    let retired = old.map(|old| old.retire(key, now)).transpose()?.flatten();
    let current = retired.as_ref().or(old);
    let result = if let Some(entry) = incoming {
        let (revision, issued, expires) = entry.details(key)?;
        let lease = match Lease::new(issued, expires, now) {
            Ok(lease) => lease,
            Err(e) => {
                return Ok(Decision {
                    replacement: retired,
                    result: Err(e),
                });
            }
        };
        let previous = current.map(|old| old.verify(key)).transpose()?;
        if previous.is_none_or(|previous| revision > previous) {
            return Ok(Decision {
                replacement: Some(Stored::Active { entry, lease }),
                result: Ok(None),
            });
        }
        if previous == Some(revision)
            && current.map(Stored::digest).transpose()? == Some(entry.digest()?)
        {
            if matches!(current, Some(Stored::Retired { .. })) {
                Err(DiscoveryError::Expired)
            } else {
                Ok(None)
            }
        } else {
            Err(DiscoveryError::Stale)
        }
    } else {
        current
            .ok_or(DiscoveryError::NotFound)
            .and_then(Stored::read)
    };
    Ok(Decision {
        replacement: retired,
        result,
    })
}

pub(super) fn decode(bytes: &[u8], key: &EndpointKey) -> Result<Stored, DiscoveryError> {
    if bytes.len() > MAX_CELL {
        return Err(error("stored record exceeds limit"));
    }
    let entry: Stored = exact(bytes)?;
    entry.verify(key)?;
    Ok(entry)
}

pub(super) fn exact<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, DiscoveryError> {
    let (value, remaining) = postcard::take_from_bytes(bytes).map_err(error)?;
    if !remaining.is_empty() {
        return Err(error("trailing stored record bytes"));
    }
    Ok(value)
}

pub(super) fn observe(
    floor: Duration,
    observed: &mut Duration,
    now: Reading,
) -> Result<(), DiscoveryError> {
    if now.wall < floor || now.wall < *observed {
        return Err(error("record store wall clock moved backwards"));
    }
    *observed = now.wall;
    Ok(())
}
