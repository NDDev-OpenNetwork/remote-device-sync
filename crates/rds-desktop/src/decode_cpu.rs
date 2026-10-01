//! Read-only thread CPU clock; unsupported/failed observations stay absent.
use std::time::Duration;
pub(super) fn now() -> Option<Duration> {
    use rustix::time::{ClockId, DynamicClockId, clock_gettime_dynamic};
    let time = clock_gettime_dynamic(DynamicClockId::Known(ClockId::ThreadCPUTime)).ok()?;
    Some(Duration::new(
        u64::try_from(time.tv_sec).ok()?,
        u32::try_from(time.tv_nsec).ok()?,
    ))
}
