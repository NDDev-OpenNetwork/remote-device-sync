//! Named deadline classes (W2.6) — one taxonomy every stage of a
//! session binds against, so "unbounded" is a bug the type system
//! names rather than a scattered constant someone forgot.
//!
//! Each [`TimeoutClass`] owns exactly one kind of stall. Enforcement
//! sites today:
//!
//! | Class | Enforced at |
//! |---|---|
//! | `dial` | `rds_client::connect_with_deadlines` (whole attempt,
//!   hole punching and relay fallback included) |
//! | `handshake` | `backends::noq::dial::race` per candidate;
//!   `rds_agent::TimeoutPolicy::{handshake, hello}` on the serving
//!   side for inbound handshakes and stream greetings |
//! | `authz` | `rds_agent::TimeoutPolicy::authz` — grant verification
//!   and refusal/`HelloAck` replies |
//! | `idle` | transport path liveness (`keep_alive_interval` 5s,
//!   `default_path_max_idle_timeout` 15s) and the stream-greeting
//!   bound (`uni` tag read) |
//! | `progress` | `rds_sync` transfer engine — `READ_STALL` per frame,
//!   `PHASE_STALL` on peer-local phases, `TRANSFER_TIMEOUT` absolute |
//! | `shutdown` | `rds_agent::TimeoutPolicy::shutdown` join budget;
//!   endpoint `wait_all_draining` bound |
//!
//! Retry semantics pair with [`RetryPolicy`]: bounded exponential
//! backoff with equal jitter, and [`retry_wait`] makes every retry
//! sleep cancellable so a shutdown never waits out a backoff it could
//! have left immediately.

use std::future::Future;
use std::time::Duration;

pub use crate::announce::RetryPolicy;

/// The largest accepted deadline; guards against effectively-unbounded
/// configuration (mirrors `rds_agent::MAX_TIMEOUT`).
pub const MAX_DEADLINE: Duration = Duration::from_secs(3600);

/// The stall kind a deadline bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutClass {
    /// Whole outbound connect attempt — hole punching, candidate races
    /// and relay fallback inside one envelope.
    Dial,
    /// Per-candidate QUIC handshake, inbound handshake, or per-stream
    /// greeting — connection establishment work that must not livelock.
    Handshake,
    /// Grant verification and authorization replies.
    Authz,
    /// No-activity liveness: transport path keepalive and streams that
    /// opened but never greeted.
    Idle,
    /// Transfer progress: a peer must move bytes or finish a phase
    /// inside the bound even though total work is unbounded by it.
    Progress,
    /// Shutdown joins and drain waits — a closing endpoint never waits
    /// on a peer's unbounded future.
    Shutdown,
}

/// One deadline per [`TimeoutClass`], with production defaults taken
/// from the constants the enforcement sites already use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlinePolicy {
    /// Whole dial envelope. Default 30s (`rds_client` connect bound).
    pub dial: Duration,
    /// Handshake/greeting bound. Default 15s.
    pub handshake: Duration,
    /// Authorization reply bound. Default 15s.
    pub authz: Duration,
    /// Idle-liveness bound. Default 15s (transport path idle).
    pub idle: Duration,
    /// Per-frame progress bound. Default 300s; phase and session
    /// budgets derive as multiples at the transfer engine.
    pub progress: Duration,
    /// Shutdown join bound. Default 5s.
    pub shutdown: Duration,
}

impl DeadlinePolicy {
    /// Production defaults, `const` so transport-layer enforcement
    /// sites can name a class bound at compile time.
    pub const DEFAULT: Self = Self {
        dial: Duration::from_secs(30),
        handshake: Duration::from_secs(15),
        authz: Duration::from_secs(15),
        idle: Duration::from_secs(15),
        progress: Duration::from_secs(300),
        shutdown: Duration::from_secs(5),
    };
}

impl Default for DeadlinePolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl DeadlinePolicy {
    /// The configured bound for one class.
    pub fn deadline(&self, class: TimeoutClass) -> Duration {
        match class {
            TimeoutClass::Dial => self.dial,
            TimeoutClass::Handshake => self.handshake,
            TimeoutClass::Authz => self.authz,
            TimeoutClass::Idle => self.idle,
            TimeoutClass::Progress => self.progress,
            TimeoutClass::Shutdown => self.shutdown,
        }
    }

    /// Reject zero or effectively-unbounded deadlines on any class.
    /// Naming the offending class keeps operator errors actionable.
    pub fn validate(&self) -> Result<(), TimeoutClass> {
        for class in [
            TimeoutClass::Dial,
            TimeoutClass::Handshake,
            TimeoutClass::Authz,
            TimeoutClass::Idle,
            TimeoutClass::Progress,
            TimeoutClass::Shutdown,
        ] {
            let deadline = self.deadline(class);
            if deadline.is_zero() || deadline > MAX_DEADLINE {
                return Err(class);
            }
        }
        Ok(())
    }
}

/// What a bounded retry sleep decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryWait {
    /// The backoff elapsed; attempt the retry.
    Retry,
    /// Cancellation arrived first; abandon the loop instead of
    /// finishing out a backoff a shutdown already superseded.
    Cancelled,
}

/// Sleep for the policy's `failures`-th bounded backoff, or until
/// `cancel` completes — whichever happens first. Reconnect and
/// publish loops call this instead of bare `tokio::time::sleep` so a
/// graceful shutdown never parks inside a backoff.
pub async fn retry_wait<F>(policy: &RetryPolicy, failures: u32, cancel: F) -> RetryWait
where
    F: Future<Output = ()>,
{
    tokio::select! {
        _ = tokio::time::sleep(policy.delay(failures)) => RetryWait::Retry,
        _ = cancel => RetryWait::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn default_deadlines_cover_every_class_within_max() {
        let policy = DeadlinePolicy::default();
        assert_eq!(policy.validate(), Ok(()));
        for class in [
            TimeoutClass::Dial,
            TimeoutClass::Handshake,
            TimeoutClass::Authz,
            TimeoutClass::Idle,
            TimeoutClass::Progress,
            TimeoutClass::Shutdown,
        ] {
            let deadline = policy.deadline(class);
            assert!(deadline > Duration::ZERO);
            assert!(deadline <= MAX_DEADLINE);
        }
    }

    #[test]
    fn validate_rejects_zero_and_unbounded_per_class() {
        let policy = DeadlinePolicy {
            dial: Duration::ZERO,
            ..DeadlinePolicy::DEFAULT
        };
        assert_eq!(policy.validate(), Err(TimeoutClass::Dial));
        let policy = DeadlinePolicy {
            progress: MAX_DEADLINE + Duration::from_secs(1),
            ..DeadlinePolicy::DEFAULT
        };
        assert_eq!(policy.validate(), Err(TimeoutClass::Progress));
        let policy = DeadlinePolicy {
            shutdown: MAX_DEADLINE + Duration::from_secs(1),
            ..DeadlinePolicy::DEFAULT
        };
        assert_eq!(policy.validate(), Err(TimeoutClass::Shutdown));
    }

    #[tokio::test]
    async fn retry_wait_returns_retry_after_bounded_delay() {
        let policy = RetryPolicy {
            base: Duration::from_millis(4),
            cap: Duration::from_millis(8),
        };
        let wait = retry_wait(&policy, 1, std::future::pending()).await;
        assert_eq!(wait, RetryWait::Retry);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_wait_cancellation_wins_during_backoff() {
        let policy = RetryPolicy {
            base: Duration::from_secs(30),
            cap: Duration::from_secs(60),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let wait = retry_wait(&policy, 3, async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            flag.store(true, Ordering::Relaxed);
        })
        .await;
        assert_eq!(wait, RetryWait::Cancelled);
        assert!(cancelled.load(Ordering::Relaxed));
    }

    #[test]
    fn retry_delays_stay_bounded_so_reconnects_cannot_storm() {
        let policy = RetryPolicy {
            base: Duration::from_secs(1),
            cap: Duration::from_secs(8),
        };
        let mut peak = Duration::ZERO;
        for failures in 1..64 {
            let delay = policy.delay(failures);
            assert!(delay <= policy.cap);
            peak = peak.max(delay);
        }
        // With jitter the applied sleep can sit anywhere in
        // [delay/2, delay]; the ceiling must still honor cap.
        assert!(peak <= policy.cap);
        assert!(peak >= policy.base);
    }
}
