//! Acknowledged control progress is independent of video and local writes.
use rds_core::DesktopControl;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

const CONTROL_STALL: Duration = Duration::from_secs(8);
const MAX_PROBES: usize = 16;
const CLIPBOARD_GRACE: Duration = Duration::from_secs(30);

struct Clipboard {
    id: u64,
    total: u32,
    deadline: Instant,
    last_sent: Instant,
    finished: bool,
}

struct State {
    next: u64,
    confirmed: Instant,
    pending: VecDeque<(u64, u64, Instant)>,
    clipboard: Option<Clipboard>,
}

#[derive(Clone)]
pub(super) struct ControlWatchdog(Arc<Mutex<State>>);

impl Default for ControlWatchdog {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(State {
            next: 0,
            confirmed: Instant::now(),
            pending: VecDeque::new(),
            clipboard: None,
        })))
    }
}

impl ControlWatchdog {
    pub(super) fn heartbeat(&self, ts_ms: u64) -> anyhow::Result<DesktopControl> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let seq = state.next;
        state.next = seq
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("desktop heartbeat sequence exhausted"))?;
        Ok(DesktopControl::Heartbeat { seq, ts_ms })
    }

    pub(super) fn sent(&self, message: &DesktopControl) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let DesktopControl::ClipboardChunk {
            id,
            offset,
            total,
            data,
        } = message
        {
            let end = u64::from(*offset).saturating_add(data.len() as u64);
            if *total > 1024 * 1024 || end > u64::from(*total) {
                return;
            }
            if *offset == 0 {
                let deadline = state
                    .clipboard
                    .as_ref()
                    .map_or_else(|| Instant::now() + CLIPBOARD_GRACE, |c| c.deadline);
                state.clipboard = Some(Clipboard {
                    id: *id,
                    total: *total,
                    deadline,
                    last_sent: Instant::now(),
                    finished: false,
                });
            }
            if let Some(c) = &mut state.clipboard
                && c.id == *id
                && c.total == *total
            {
                c.last_sent = Instant::now();
                c.finished = end == u64::from(*total);
            }
            return;
        }
        let DesktopControl::Heartbeat { seq, ts_ms } = *message else {
            return;
        };
        state.pending.retain(|(s, t, _)| (*s, *t) != (seq, ts_ms));
        if state.pending.len() == MAX_PROBES {
            state.pending.pop_front();
        }
        state.pending.push_back((seq, ts_ms, Instant::now()));
    }

    pub(super) fn clipboard_ready(&self, id: u64, bytes: u32) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = &state.clipboard
            && c.id == id
            && c.total == bytes
            && c.finished
        {
            state.confirmed = state.confirmed.max(c.last_sent);
            state.clipboard = None;
        }
    }

    /// Only an exact outstanding probe is evidence. Confirmation advances to
    /// send time, so a very late echo cannot make old control look current.
    pub(super) fn echoed(&self, seq: u64, ts_ms: u64) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let Some(index) = state
            .pending
            .iter()
            .position(|(s, t, _)| (*s, *t) == (seq, ts_ms))
        else {
            return false;
        };
        if let Some((_, _, sent)) = state.pending.remove(index) {
            state.confirmed = state.confirmed.max(sent);
            true
        } else {
            false
        }
    }

    pub(super) fn check(&self) -> anyhow::Result<()> {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let age = state.confirmed.elapsed();
        let publishing = state
            .clipboard
            .as_ref()
            .is_some_and(|c| Instant::now() < c.deadline);
        if age >= CONTROL_STALL && !publishing {
            tracing::warn!(
                last_confirmed_probe_age_ms = age.as_millis() as u64,
                pending_probes = state.pending.len(),
                "desktop control progress watchdog expired"
            );
            anyhow::bail!("remote control stopped making progress");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn late_wrong_duplicate_and_retired_echoes_cannot_mask_stalled_control() {
        let monitor = ControlWatchdog::default();
        let first = monitor.heartbeat(42).unwrap();
        monitor.sent(&first);
        assert!(!monitor.echoed(0, 43));
        tokio::time::advance(CONTROL_STALL).await;
        assert!(monitor.echoed(0, 42));
        assert!(
            monitor.check().is_err(),
            "late reply must retain its original send age"
        );
        assert!(!monitor.echoed(0, 42), "duplicate is not new progress");
        for _ in 0..MAX_PROBES + 1 {
            let message = monitor.heartbeat(42).unwrap();
            monitor.sent(&message);
        }
        assert_eq!(monitor.0.lock().unwrap().pending.len(), MAX_PROBES);
        assert!(
            !monitor.echoed(1, 42),
            "evicted probe is not current evidence"
        );
        assert!(monitor.echoed((MAX_PROBES + 1) as u64, 42));
        assert!(monitor.check().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn timely_confirmed_controls_rearm_without_rewinding_or_clock_collision() {
        let monitor = ControlWatchdog::default();
        let first = monitor.heartbeat(0).unwrap();
        monitor.sent(&first);
        tokio::time::advance(Duration::from_secs(1)).await;
        let second = monitor.heartbeat(0).unwrap();
        monitor.sent(&second);
        assert!(monitor.echoed(1, 0));
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(monitor.echoed(0, 0));
        assert!(
            monitor.check().is_ok(),
            "older matched echo must not rewind confirmation"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(monitor.check().is_err());
        monitor.0.lock().unwrap().next = u64::MAX;
        assert!(monitor.heartbeat(0).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn clipboard_keeps_existing_absolute_budget_and_exact_ready_confirmation() {
        let monitor = ControlWatchdog::default();
        monitor.sent(&DesktopControl::ClipboardChunk {
            id: 7,
            offset: 0,
            total: 4,
            data: vec![0; 2],
        });
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(monitor.check().is_ok());
        monitor.clipboard_ready(7, 4); // Incomplete transfer cannot confirm anything.
        monitor.sent(&DesktopControl::ClipboardChunk {
            id: 7,
            offset: 2,
            total: 4,
            data: vec![0; 2],
        });
        monitor.clipboard_ready(8, 4);
        monitor.clipboard_ready(7, 3);
        assert!(monitor.0.lock().unwrap().clipboard.is_some());
        monitor.clipboard_ready(7, 4);
        assert!(monitor.0.lock().unwrap().clipboard.is_none());
        assert!(monitor.check().is_ok());
        tokio::time::advance(Duration::from_secs(8)).await;
        assert!(monitor.check().is_err());
        let monitor = ControlWatchdog::default();
        monitor.sent(&DesktopControl::ClipboardChunk {
            id: 1,
            offset: 0,
            total: 4,
            data: vec![0; 2],
        });
        tokio::time::advance(Duration::from_secs(29)).await;
        monitor.sent(&DesktopControl::ClipboardChunk {
            id: 2,
            offset: 0,
            total: 4,
            data: vec![0; 2],
        });
        assert!(monitor.check().is_ok());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(
            monitor.check().is_err(),
            "new starts cannot extend the absolute grace indefinitely"
        );
    }
}
