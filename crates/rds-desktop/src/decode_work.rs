//! Decode scheduling diagnostics; cancellation aborts queued work while a
//! running native call retains its existing global permit.
use crate::DesktopError;
use rds_core::FrameHeader;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::{sync::Semaphore, task::AbortHandle};

#[cfg(all(feature = "x11", any(target_os = "linux", target_os = "macos")))]
#[path = "decode_cpu.rs"]
mod cpu;
#[cfg(not(all(feature = "x11", any(target_os = "linux", target_os = "macos"))))]
#[path = "decode_cpu_unavailable.rs"]
mod cpu;

const BUDGET: u8 = 0;
const QUEUED: u8 = 1;
const NATIVE: u8 = 2;
const DONE: u8 = 3;
const SLOW: Duration = Duration::from_millis(250);

pub(crate) struct Probe {
    started: Instant,
    phase: AtomicU8,
    admitted_ms: AtomicU64,
    native_ms: AtomicU64,
    finished_ms: AtomicU64,
    native_cpu_us: AtomicU64,
    seq: u64,
    bytes: usize,
    width: u32,
    height: u32,
    keyframe: bool,
}
impl Probe {
    pub(crate) fn new(header: &FrameHeader, bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            phase: AtomicU8::new(BUDGET),
            admitted_ms: AtomicU64::new(0),
            native_ms: AtomicU64::new(0),
            finished_ms: AtomicU64::new(0),
            native_cpu_us: AtomicU64::new(u64::MAX),
            seq: header.seq,
            bytes,
            width: header.width,
            height: header.height,
            keyframe: header.keyframe,
        })
    }
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
    fn mark(&self, phase: u8) {
        let elapsed = self.elapsed_ms();
        match phase {
            QUEUED => self.admitted_ms.store(elapsed, Ordering::Relaxed),
            NATIVE => self.native_ms.store(elapsed, Ordering::Relaxed),
            _ => {}
        }
        self.phase.store(phase, Ordering::Release);
    }
    fn report(&self, reason: &'static str) {
        if self.started.elapsed() < SLOW {
            return;
        }
        let phase = self.phase.load(Ordering::Acquire);
        let admitted = self.admitted_ms.load(Ordering::Relaxed);
        let native = self.native_ms.load(Ordering::Relaxed);
        let elapsed = self.elapsed_ms();
        let work_ms = (phase >= NATIVE).then(|| {
            let end = if phase == DONE {
                self.finished_ms.load(Ordering::Relaxed)
            } else {
                elapsed
            };
            end.saturating_sub(native)
        });
        let cpu_us = self.native_cpu_us.load(Ordering::Relaxed);
        let cpu_ms = (cpu_us != u64::MAX).then_some(cpu_us / 1000);
        tracing::warn!(
            reason,
            frame_seq = self.seq,
            keyframe = self.keyframe,
            payload_bytes = self.bytes,
            width = self.width,
            height = self.height,
            decode_phase = match phase {
                BUDGET => "budget",
                QUEUED => "queued",
                NATIVE => "native",
                _ => "completed",
            },
            total_ms = elapsed,
            budget_wait_ms = if phase == BUDGET { elapsed } else { admitted },
            queue_wait_ms = (phase >= QUEUED).then(|| if phase == QUEUED {
                elapsed.saturating_sub(admitted)
            } else {
                native.saturating_sub(admitted)
            }),
            native_work_ms = work_ms,
            native_cpu_ms = cpu_ms,
            native_non_cpu_ms = work_ms
                .zip(cpu_ms)
                .map(|(wall, cpu)| wall.saturating_sub(cpu)),
            "desktop decode scheduling health"
        );
    }
}
struct Caller {
    probe: Arc<Probe>,
    abort: Option<AbortHandle>,
    reason: &'static str,
}
impl Drop for Caller {
    fn drop(&mut self) {
        if self.probe.phase.load(Ordering::Acquire) != DONE {
            self.probe.report(self.reason);
            if let Some(abort) = &self.abort {
                abort.abort();
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    BudgetClosed,
    Timeout,
    Worker(tokio::task::JoinError),
}
impl From<Error> for DesktopError {
    fn from(error: Error) -> Self {
        Self::Decode(match error {
            Error::BudgetClosed => "decoder budget closed".into(),
            Error::Timeout => "decoder timed out".into(),
            Error::Worker(error) => error.to_string(),
        })
    }
}

pub(crate) async fn run<T: Send + 'static>(
    slots: &'static Semaphore,
    timeout: Duration,
    probe: Arc<Probe>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Error> {
    let mut caller = Caller {
        probe: probe.clone(),
        abort: None,
        reason: "caller_stopped_waiting",
    };
    let slot = slots.acquire().await.map_err(|_| {
        caller.reason = "budget_closed";
        Error::BudgetClosed
    })?;
    probe.mark(QUEUED);
    let task_probe = probe.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let cpu_started = cpu::now();
        task_probe.mark(NATIVE);
        let result = work();
        let cpu_finished = cpu::now();
        task_probe
            .finished_ms
            .store(task_probe.elapsed_ms(), Ordering::Relaxed);
        if let Some(micros) = cpu_finished
            .zip(cpu_started)
            .and_then(|(end, start)| end.checked_sub(start))
            .and_then(|duration| u64::try_from(duration.as_micros()).ok())
        {
            task_probe.native_cpu_us.store(micros, Ordering::Relaxed);
        }
        task_probe.mark(DONE);
        task_probe.report("completed_slowly");
        result
    });
    caller.abort = Some(task.abort_handle());
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => {
            caller.reason = "worker_failed";
            Err(Error::Worker(error))
        }
        Err(_) => {
            caller.reason = "work_timeout";
            Err(Error::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    fn header() -> FrameHeader {
        FrameHeader {
            seq: 1,
            capture_ts_ms: 0,
            encode_done_ts_ms: 0,
            send_ts_ms: 0,
            keyframe: true,
            codec: rds_core::Codec::H264,
            width: 64,
            height: 64,
        }
    }
    #[cfg(all(feature = "x11", any(target_os = "linux", target_os = "macos")))]
    #[tokio::test]
    async fn thread_cpu_observation_distinguishes_work_from_native_waiting() {
        static SLOTS: Semaphore = Semaphore::const_new(1);
        let probe = Probe::new(&header(), 12);
        run(&SLOTS, Duration::from_secs(3), probe.clone(), || {
            let start = cpu::now().expect("supported platform thread clock");
            let limit = Instant::now() + Duration::from_secs(2);
            while cpu::now().unwrap().saturating_sub(start) < Duration::from_millis(20) {
                assert!(
                    Instant::now() < limit,
                    "CPU fixture was indefinitely starved"
                );
                std::hint::spin_loop();
            }
            std::thread::sleep(Duration::from_millis(80));
        })
        .await
        .unwrap();
        let cpu_us = probe.native_cpu_us.load(Ordering::Acquire);
        let wall_ms =
            probe.finished_ms.load(Ordering::Acquire) - probe.native_ms.load(Ordering::Acquire);
        assert!(cpu_us >= 20_000, "actual native CPU work must be observed");
        assert!(
            wall_ms >= 100,
            "native waiting remains part of elapsed time"
        );
        assert!(
            cpu_us < wall_ms * 500,
            "sleep cannot be reported as CPU work"
        );
    }
    #[test]
    fn cancellation_before_native_start_aborts_queued_decode() {
        static SLOTS: Semaphore = Semaphore::const_new(1);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release, held) = std::sync::mpsc::channel();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                let _ = held.recv_timeout(Duration::from_secs(3));
            });
            ready.await.unwrap();
            let probe = Probe::new(&header(), 12);
            let ran = Arc::new(AtomicBool::new(false));
            let work_ran = ran.clone();
            let result = tokio::time::timeout(
                Duration::from_millis(50),
                run(&SLOTS, Duration::from_secs(1), probe.clone(), move || {
                    work_ran.store(true, Ordering::Release)
                }),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(probe.phase.load(Ordering::Acquire), QUEUED);
            release.send(()).unwrap();
            blocker.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while SLOTS.available_permits() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                !ran.load(Ordering::Acquire),
                "abandoned queued native work must not run"
            );
        });
    }
    #[tokio::test]
    async fn running_decode_keeps_its_permit_after_caller_cancellation() {
        static SLOTS: Semaphore = Semaphore::const_new(1);
        let (release, held) = std::sync::mpsc::channel();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let probe = Probe::new(&header(), 12);
        let worker_probe = probe.clone();
        let task = tokio::spawn(async move {
            run(&SLOTS, Duration::from_secs(1), worker_probe, move || {
                entered.send(()).unwrap();
                let _ = held.recv_timeout(Duration::from_secs(3));
            })
            .await
        });
        ready.await.unwrap();
        assert_eq!(probe.phase.load(Ordering::Acquire), NATIVE);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(SLOTS.available_permits(), 0);
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while SLOTS.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(probe.phase.load(Ordering::Acquire), DONE);
    }
}
