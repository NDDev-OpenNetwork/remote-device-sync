//! Bounded metadata flight recorder for slow input and stalled video.
use rds_desktop::render::ViewerSnapshot;
use std::collections::VecDeque;

const HISTORY: usize = 30; // Two-second snapshots: one minute before a fault.
const AFTER_MS: u64 = 10_000;
const COOLDOWN_MS: u64 = 30_000;

struct Incident {
    triggered_ms: u64,
    reasons: Vec<&'static str>,
    snapshots: Vec<serde_json::Value>,
}

#[derive(Default)]
pub(super) struct Recorder {
    history: VecDeque<serde_json::Value>,
    pending: Option<Incident>,
    previous_reconnects: Option<u64>,
    previous_slow_acks: Option<u64>,
    cooldown_until_ms: u64,
}

impl Recorder {
    pub(super) fn observe(&mut self, snapshot: &ViewerSnapshot) -> Option<Vec<u8>> {
        let value = serde_json::to_value(snapshot).ok()?;
        self.history.push_back(value.clone());
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let reconnected = self
            .previous_reconnects
            .is_some_and(|previous| snapshot.report.reconnects > previous);
        self.previous_reconnects = Some(snapshot.report.reconnects);
        let slow_ack = self
            .previous_slow_acks
            .is_some_and(|previous| snapshot.report.slow_input_acks > previous);
        self.previous_slow_acks = Some(snapshot.report.slow_input_acks);
        if let Some(incident) = &mut self.pending {
            // Keep memory bounded even if the diagnostic timer runs rapidly.
            if incident.snapshots.len() < HISTORY + 6 {
                incident.snapshots.push(value);
            }
            if snapshot.elapsed_ms.saturating_sub(incident.triggered_ms) >= AFTER_MS {
                self.cooldown_until_ms = snapshot.elapsed_ms.saturating_add(COOLDOWN_MS);
                return self.finish(false);
            }
            return None;
        }
        if snapshot.elapsed_ms < self.cooldown_until_ms {
            return None;
        }
        let mut reasons = Vec::new();
        if slow_ack {
            // Completed stalls can fall entirely between periodic snapshots.
            reasons.push("input_ack_delayed");
        }
        if snapshot
            .report
            .oldest_input_ack_age_ms
            .is_some_and(|ms| ms >= 250)
        {
            reasons.push("input_ack_pending");
        }
        if snapshot.decoded_frame_age_ms.is_some_and(|ms| ms >= 3000) {
            reasons.push("decoded_video_stalled");
        }
        // Occlusion is a normal OS policy, not a frozen visible screen.
        if !snapshot.occluded
            && snapshot.render_stage != "surface occluded"
            && snapshot.status == "Connected"
            && snapshot
                .unpresented_frame_age_ms
                .is_some_and(|ms| ms >= 1000)
        {
            reasons.push("visible_submission_stalled");
        }
        if reconnected {
            reasons.push("desktop_reconnected");
        }
        if !reasons.is_empty() {
            tracing::warn!(
                ?reasons,
                elapsed_ms = snapshot.elapsed_ms,
                "desktop incident recording started"
            );
            self.pending = Some(Incident {
                triggered_ms: snapshot.elapsed_ms,
                reasons,
                snapshots: self.history.iter().cloned().collect(),
            });
        }
        None
    }

    /// Preserve an in-progress fault window when the viewer shuts down.
    pub(super) fn finish(&mut self, interrupted: bool) -> Option<Vec<u8>> {
        let incident = self.pending.take()?;
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "triggered_elapsed_ms": incident.triggered_ms,
            "reasons": incident.reasons,
            "interrupted": interrupted,
            "snapshots": incident.snapshots,
        }))
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rds_desktop::render::ViewerReport;
    fn snapshot(ms: u64) -> ViewerSnapshot {
        ViewerSnapshot {
            pid: 42,
            elapsed_ms: ms,
            status: "Connected".into(),
            network_stage: "receiving".into(),
            render_stage: "presented".into(),
            ui_event_age_ms: 0,
            decoded_frame_age_ms: Some(10),
            encoded_frame_age_ms: Some(10),
            control_echo_age_ms: Some(10),
            submission_age_ms: Some(10),
            unpresented_frame_age_ms: None,
            pending_frame_bytes: 0,
            occluded: false,
            report: ViewerReport::default(),
        }
    }
    #[test]
    fn slow_input_preserves_bounded_before_and_after_history_and_shutdown_fault() {
        let mut recorder = Recorder::default();
        for ms in (0..80_000).step_by(2000) {
            assert!(recorder.observe(&snapshot(ms)).is_none());
        }
        let mut slow = snapshot(80_000);
        slow.report.oldest_input_ack_age_ms = Some(500);
        assert!(recorder.observe(&slow).is_none());
        let mut result = None;
        for ms in (82_000..=90_000).step_by(2000) {
            result = recorder.observe(&snapshot(ms));
        }
        let data: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let rows = data["snapshots"].as_array().unwrap();
        assert_eq!(rows.len(), 35);
        assert_eq!(rows.first().unwrap()["elapsed_ms"], 22_000);
        assert_eq!(rows.last().unwrap()["elapsed_ms"], 90_000);
        assert_eq!(data["reasons"][0], "input_ack_pending");
        slow.elapsed_ms = 92_000;
        assert!(recorder.observe(&slow).is_none());
        assert!(recorder.finish(true).is_none());
        slow.elapsed_ms = 122_000;
        recorder.observe(&slow);
        let data: serde_json::Value =
            serde_json::from_slice(&recorder.finish(true).unwrap()).unwrap();
        assert_eq!(data["interrupted"], true);
    }
    #[test]
    fn normal_occlusion_is_not_a_fault_but_video_loss_and_reconnect_are() {
        let mut recorder = Recorder::default();
        let mut hidden = snapshot(0);
        hidden.occluded = true;
        hidden.unpresented_frame_age_ms = Some(90_000);
        hidden.submission_age_ms = Some(90_000);
        recorder.observe(&hidden);
        assert!(recorder.finish(true).is_none());
        hidden.occluded = false; // Metal can report occlusion before the window event.
        hidden.render_stage = "surface occluded".into();
        recorder.observe(&hidden);
        assert!(recorder.finish(true).is_none());
        hidden.occluded = true;
        hidden.elapsed_ms = 2000;
        hidden.decoded_frame_age_ms = Some(4000);
        hidden.report.reconnects = 1;
        recorder.observe(&hidden);
        let data: serde_json::Value =
            serde_json::from_slice(&recorder.finish(true).unwrap()).unwrap();
        assert_eq!(
            data["reasons"],
            serde_json::json!(["decoded_video_stalled", "desktop_reconnected"])
        );
    }
    #[test]
    fn a_completed_slow_ack_between_snapshots_still_records_an_incident() {
        let mut recorder = Recorder::default();
        recorder.observe(&snapshot(0));
        let mut completed = snapshot(2000);
        completed.report.slow_input_acks = 2;
        assert_eq!(completed.report.pending_input_acks, 0);
        recorder.observe(&completed);
        let data: serde_json::Value =
            serde_json::from_slice(&recorder.finish(true).unwrap()).unwrap();
        assert_eq!(data["reasons"][0], "input_ack_delayed");
    }

    #[test]
    fn idle_and_a_fresh_update_do_not_report_a_visible_renderer_stall() {
        let mut recorder = Recorder::default();
        let mut idle = snapshot(90_000);
        idle.submission_age_ms = Some(90_000);
        idle.decoded_frame_age_ms = Some(1050); // Damage-driven idle refresh cadence.
        recorder.observe(&idle);
        assert!(recorder.finish(true).is_none());
        idle.elapsed_ms += 2000;
        idle.unpresented_frame_age_ms = Some(1);
        idle.pending_frame_bytes = 4096;
        recorder.observe(&idle);
        assert!(recorder.finish(true).is_none());
        idle.elapsed_ms += 2000;
        idle.decoded_frame_age_ms = Some(0); // Fresh replacements must not hide debt.
        idle.unpresented_frame_age_ms = Some(2001);
        recorder.observe(&idle);
        let data: serde_json::Value =
            serde_json::from_slice(&recorder.finish(true).unwrap()).unwrap();
        assert_eq!(
            data["reasons"],
            serde_json::json!(["visible_submission_stalled"])
        );
    }
}
