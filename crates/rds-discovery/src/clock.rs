//! Signed-data leases use wall time and an OS monotonic clock including sleep.

use crate::{DiscoveryError, authority::invalid};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
#[path = "clock/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "clock/macos.rs"]
mod platform;

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub boot: [u8; 16],
    pub wall: Duration,
    pub continuous: Duration,
}

static BOOT: std::sync::OnceLock<Result<[u8; 16], String>> = std::sync::OnceLock::new();

impl Reading {
    pub fn now() -> Result<Self, DiscoveryError> {
        Self::sample(BOOT.get_or_init(platform::boot_id))
    }

    /// Observation-only clock: never initialize the boot cache or perform file
    /// I/O. Unknown until a product clock read has initialized the cache.
    pub fn cached_now() -> Option<Self> {
        Self::sample(BOOT.get()?).ok()
    }

    fn sample(boot: &Result<[u8; 16], String>) -> Result<Self, DiscoveryError> {
        // Take the continuous sample first so sampling cannot extend a lease.
        let t = rustix::time::clock_gettime(platform::CLOCK);
        let seconds = u64::try_from(t.tv_sec).map_err(|_| invalid("negative monotonic clock"))?;
        let nanos = u32::try_from(t.tv_nsec).map_err(|_| invalid("invalid monotonic clock"))?;
        if nanos >= 1_000_000_000 {
            return Err(invalid("invalid monotonic clock"));
        }
        let continuous = Duration::new(seconds, nanos);
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("clock before epoch"))?;
        let boot = boot.as_ref().map_err(|e| invalid(e))?;
        Ok(Self {
            boot: *boot,
            wall,
            continuous,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Lease {
    boot: [u8; 16],
    issued_at: u64,
    expires_at: u64,
    accepted_wall: Duration,
    accepted_continuous: Duration,
    deadline: Duration,
}

impl Lease {
    pub(crate) fn accepted_wall(&self) -> Duration {
        self.accepted_wall
    }
    pub fn new(issued_at: u64, expires_at: u64, now: Reading) -> Result<Self, DiscoveryError> {
        if issued_at > now.wall.as_secs() {
            return Err(invalid("signed data is from the future"));
        }
        let remaining = Duration::from_secs(expires_at)
            .checked_sub(now.wall)
            .filter(|d| !d.is_zero())
            .ok_or(DiscoveryError::Expired)?;
        let deadline = now
            .continuous
            .checked_add(remaining)
            .ok_or_else(|| invalid("signed-data deadline overflow"))?;
        Ok(Self {
            boot: now.boot,
            issued_at,
            expires_at,
            accepted_wall: now.wall,
            accepted_continuous: now.continuous,
            deadline,
        })
    }

    pub fn valid_at(&self, now: Reading) -> bool {
        now.boot == self.boot
            && now.wall >= self.accepted_wall
            && now.wall.as_secs() >= self.issued_at
            && now.wall.as_secs() < self.expires_at
            && now.continuous >= self.accepted_continuous
            && now.continuous < self.deadline
    }

    /// Remaining usable duration under both clocks; invalid leases return zero.
    /// This observes the existing deadline and never renews it.
    pub fn remaining_at(&self, now: Reading) -> Duration {
        if !self.valid_at(now) {
            return Duration::ZERO;
        }
        (Duration::from_secs(self.expires_at) - now.wall).min(self.deadline - now.continuous)
    }

    pub fn matches_interval(&self, issued: u64, expires: u64) -> bool {
        self.issued_at == issued
            && self.expires_at == expires
            && self.accepted_wall.as_secs() >= issued
            && self.accepted_wall.as_secs() < expires
            && self.deadline.checked_sub(self.accepted_continuous)
                == Duration::from_secs(expires).checked_sub(self.accepted_wall)
    }
}

fn parse_boot(bytes: &[u8]) -> Result<[u8; 16], String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| e.to_string())?
        .trim_matches(['\0', '\n', ' ']);
    if text.len() != 36 || [8, 13, 18, 23].iter().any(|i| text.as_bytes()[*i] != b'-') {
        return Err("invalid OS boot identifier".into());
    }
    let digits: String = text.chars().filter(|c| *c != '-').collect();
    if digits.len() != 32 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid OS boot identifier".into());
    }
    let mut id = [0; 16];
    for (index, slot) in id.iter_mut().enumerate() {
        *slot =
            u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_lease_uses_both_clocks_and_keeps_subsecond_freshness() {
        let start = Reading {
            boot: [3; 16],
            wall: Duration::from_millis(100_100),
            continuous: Duration::from_millis(10_000),
        };
        let lease = Lease::new(100, 101, start).unwrap();
        assert_eq!(lease.remaining_at(start), Duration::from_millis(900));
        assert!(lease.valid_at(start));
        let suspended = Reading {
            continuous: Duration::from_millis(10_800),
            ..start
        };
        assert_eq!(lease.remaining_at(suspended), Duration::from_millis(100));
        let forward = Reading {
            wall: Duration::from_millis(100_999),
            ..start
        };
        assert_eq!(lease.remaining_at(forward), Duration::from_millis(1));
        assert_eq!(
            lease.remaining_at(Reading {
                boot: [4; 16],
                ..start
            }),
            Duration::ZERO
        );
        assert_eq!(
            lease.remaining_at(Reading {
                continuous: Duration::from_millis(10_900),
                ..start
            }),
            Duration::ZERO
        );
    }
}
