//! Static publisher enrollment and mutation budgets. No network-origin identity
//! hint spends a budget: callers verify and compare revisions first.
use crate::{DiscoveryError, EndpointKey, records::MAX_IDENTITIES, service::Limits};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Configured publishers, independent of self-signed reachability and names.
/// Default is empty (deny). Production never infers membership from a PUT.
#[derive(Clone, Debug, Default)]
pub struct Enrollment {
    keys: HashSet<EndpointKey>,
    unrestricted: bool,
}
impl Enrollment {
    pub fn new(keys: impl IntoIterator<Item = EndpointKey>) -> Result<Self, DiscoveryError> {
        let mut enrolled = Self::default();
        for (index, key) in keys.into_iter().enumerate() {
            if index >= MAX_IDENTITIES {
                return Err(DiscoveryError::Configuration(
                    "enrollment exceeds 4096 identities".into(),
                ));
            }
            let key_bytes = ed25519_dalek::VerifyingKey::from_bytes(&key.0)
                .map_err(|_| DiscoveryError::Configuration("invalid enrollment key".into()))?;
            if key_bytes.is_weak() || !enrolled.keys.insert(key) {
                return Err(DiscoveryError::Configuration(
                    "weak or duplicate enrollment key".into(),
                ));
            }
        }
        Ok(enrolled)
    }
    /// Explicit opt-in for isolated synthetic fixtures. Not exposed by the
    /// production CLI; a self-signed record is not GDS enrollment evidence.
    pub fn unrestricted_for_tests() -> Self {
        Self {
            unrestricted: true,
            ..Self::default()
        }
    }
    pub(crate) fn allows(&self, key: &EndpointKey) -> bool {
        self.unrestricted || self.keys.contains(key)
    }
}

const WINDOW: Duration = Duration::from_secs(60);
struct Window {
    start: Instant,
    used: u32,
}
impl Window {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            used: 0,
        }
    }
    fn reset(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.start) >= WINDOW {
            *self = Self::new(now);
            true
        } else {
            false
        }
    }
    fn take(&mut self, now: Instant, limit: u32) -> bool {
        self.reset(now);
        if self.used >= limit {
            return false;
        }
        self.used += 1;
        true
    }
}
struct Writer {
    window: Window,
    reserved: bool,
    last: Option<Instant>,
}
struct State {
    writers: HashMap<EndpointKey, Writer>,
    admissions: Window,
    burst: Window,
    registry: Window,
    revocations: Window,
}
pub(crate) struct Limiter(Mutex<State>);
impl Default for Limiter {
    fn default() -> Self {
        let now = Instant::now();
        Self(Mutex::new(State {
            writers: HashMap::new(),
            admissions: Window::new(now),
            burst: Window::new(now),
            registry: Window::new(now),
            revocations: Window::new(now),
        }))
    }
}
impl Limiter {
    pub(crate) fn record(
        &self,
        key: EndpointKey,
        known: bool,
        limits: &Limits,
    ) -> Result<(), DiscoveryError> {
        self.record_at(key, known, limits, None)
    }
    fn record_at(
        &self,
        key: EndpointKey,
        known: bool,
        limits: &Limits,
        clock: Option<Instant>,
    ) -> Result<(), DiscoveryError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| DiscoveryError::Store("admission state poisoned".into()))?;
        let now = clock.unwrap_or_else(Instant::now);
        if !state.writers.contains_key(&key) && state.writers.len() >= MAX_IDENTITIES {
            state
                .writers
                .retain(|_, writer| now.saturating_duration_since(writer.window.start) < WINDOW);
            if state.writers.len() >= MAX_IDENTITIES {
                return Err(DiscoveryError::RateLimited);
            }
        }
        let State {
            writers,
            admissions,
            burst,
            ..
        } = &mut *state;
        let writer = writers.entry(key).or_insert_with(|| Writer {
            window: Window::new(now),
            reserved: false,
            last: None,
        });
        if writer.window.reset(now) {
            writer.reserved = false;
        }
        if writer
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < limits.put_min_interval)
            || writer.window.used >= limits.writer_per_minute
        {
            return Err(DiscoveryError::RateLimited);
        }
        // A known identity has one protected mutation per minute, independent
        // of other writers and new enrollment traffic. This covers TTL >= 180s
        // with TTL/3 renewals. Shorter TTLs also need the shared burst budget.
        let reserved = known && !writer.reserved;
        let allowed = if reserved {
            true
        } else if known {
            burst.take(now, limits.put_per_minute)
        } else {
            admissions.take(now, limits.admissions_per_minute)
        };
        if allowed {
            // Refused attempts never debit the identity either. Replaying a
            // signed but not-yet-committed operation during shared saturation
            // cannot burn its owner's later chance to publish.
            writer.window.used += 1;
            writer.last = Some(now);
            writer.reserved |= reserved;
            Ok(())
        } else {
            Err(DiscoveryError::RateLimited)
        }
    }
    pub(crate) fn policy(&self, revocations: bool, limits: &Limits) -> Result<(), DiscoveryError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| DiscoveryError::Store("admission state poisoned".into()))?;
        let window = if revocations {
            &mut state.revocations
        } else {
            &mut state.registry
        };
        if window.take(Instant::now(), limits.policy_per_minute) {
            Ok(())
        } else {
            Err(DiscoveryError::RateLimited)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(index: usize) -> EndpointKey {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&(index as u64).to_le_bytes());
        EndpointKey(key)
    }
    #[test]
    fn every_configured_slot_has_renewal_capacity_at_shared_saturation() {
        let limiter = Limiter::default();
        let limits = Limits {
            put_per_minute: 0,
            admissions_per_minute: 0,
            ..Default::default()
        };
        let now = Instant::now();
        for index in 0..MAX_IDENTITIES {
            // Internal accounting identifiers; the service verifies actual
            // Ed25519 signatures before this API is reachable.
            limiter
                .record_at(identity(index), true, &limits, Some(now))
                .unwrap();
        }
        assert!(
            limiter
                .record_at(identity(MAX_IDENTITIES), true, &limits, Some(now))
                .is_err()
        );
        assert!(
            limiter
                .record_at(identity(0), true, &limits, Some(now))
                .is_err()
        );
        for index in 0..MAX_IDENTITIES {
            limiter
                .record_at(identity(index), true, &limits, Some(now + WINDOW))
                .unwrap();
        }
        assert_eq!(limiter.0.lock().unwrap().writers.len(), MAX_IDENTITIES);
    }
    #[test]
    fn refused_identity_never_charges_shared_capacity_and_windows_reset() {
        let limiter = Limiter::default();
        let limits = Limits {
            writer_per_minute: 1,
            put_per_minute: 1,
            admissions_per_minute: 1,
            ..Default::default()
        };
        let now = Instant::now();
        limiter
            .record_at(identity(1), true, &limits, Some(now))
            .unwrap();
        for _ in 0..10 {
            assert!(
                limiter
                    .record_at(identity(1), true, &limits, Some(now))
                    .is_err()
            );
        }
        assert_eq!(limiter.0.lock().unwrap().burst.used, 0);
        limiter
            .record_at(identity(2), false, &limits, Some(now))
            .unwrap();
        assert!(
            limiter
                .record_at(identity(3), false, &limits, Some(now))
                .is_err()
        );
        limiter
            .record_at(identity(1), true, &limits, Some(now + WINDOW))
            .unwrap();
        limiter
            .record_at(identity(3), false, &limits, Some(now + WINDOW))
            .unwrap();
    }
    #[test]
    fn enrollment_refuses_weak_duplicate_and_excessive_configurations() {
        let key = EndpointKey(
            ed25519_dalek::SigningKey::from_bytes(&[131; 32])
                .verifying_key()
                .to_bytes(),
        );
        assert!(!Enrollment::default().allows(&key));
        assert!(Enrollment::new([key]).unwrap().allows(&key));
        assert!(Enrollment::new([key, key]).is_err());
        assert!(Enrollment::new([EndpointKey([0; 32])]).is_err());
        let keys = (0..=MAX_IDENTITIES).map(|index| {
            EndpointKey(
                ed25519_dalek::SigningKey::from_bytes(&identity(index).0)
                    .verifying_key()
                    .to_bytes(),
            )
        });
        let error = Enrollment::new(keys).unwrap_err();
        assert!(error.to_string().contains("exceeds 4096"), "{error}");
    }

    #[test]
    fn replay_during_admission_saturation_does_not_debit_the_waiting_identity() {
        let limiter = Limiter::default();
        let limits = Limits {
            writer_per_minute: 1,
            admissions_per_minute: 0,
            ..Default::default()
        };
        let now = Instant::now();
        for _ in 0..5 {
            assert!(
                limiter
                    .record_at(identity(1), false, &limits, Some(now))
                    .is_err()
            );
        }
        let limits = Limits {
            admissions_per_minute: 1,
            ..limits
        };
        limiter
            .record_at(identity(1), false, &limits, Some(now))
            .unwrap();
    }
}
