//! Opt-in causal input-to-submit measurement against a controlled visual marker.
use crate::{DesktopError, RawFrame};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

// Counter markers cannot identify which press caused a late increment once a
// target has ignored an earlier press. End this measurement attempt instead of
// matching an unrelated later click to the oldest queued input.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualProbeSpec {
    pub marker_x: u32,
    pub marker_y: u32,
    pub cell_width: u32,
    pub marker_height: u32,
    /// Remote source-pixel click target: x, y, width, height.
    pub click_rect: [u32; 4],
    /// High 48 bits of the controlled 64-cell marker; low 16 bits are its counter.
    pub prefix: u64,
    /// Explicit physical keys whose unmodified KeyDown increments the same marker.
    /// Empty preserves click-only measurement. Modifiers/toggles are forbidden.
    #[serde(default)]
    pub keyboard_codes: Vec<u32>,
}
impl VisualProbeSpec {
    pub fn validate(&self) -> Result<(), DesktopError> {
        let extent = |start: u32, size: u32| {
            size > 0 && start.checked_add(size).is_some_and(|end| end <= 16384)
        };
        let keyboard_valid = self.keyboard_codes.len() <= 16
            && self.keyboard_codes.iter().enumerate().all(|(index, code)| {
                (1..=255).contains(code)
                    && !matches!(
                        code,
                        29 | 42 | 54 | 56 | 58 | 69 | 70 | 97 | 100 | 125 | 126
                    )
                    && !self.keyboard_codes[..index].contains(code)
            });
        if !keyboard_valid
            || !(4..=32).contains(&self.cell_width)
            || !(4..=64).contains(&self.marker_height)
            || self.prefix == 0
            || self.prefix >= (1 << 48)
            || !extent(self.marker_x, self.cell_width * 64)
            || !extent(self.marker_y, self.marker_height)
            || !extent(self.click_rect[0], self.click_rect[2])
            || !extent(self.click_rect[1], self.click_rect[3])
        {
            return Err(DesktopError::Input(
                "invalid visual probe descriptor".into(),
            ));
        }
        Ok(())
    }
    fn inside(&self, point: (f64, f64)) -> bool {
        let [x, y, width, height] = self.click_rect;
        point.0 >= f64::from(x)
            && point.0 < f64::from(x + width)
            && point.1 >= f64::from(y)
            && point.1 < f64::from(y + height)
    }
    fn counter(&self, frame: &RawFrame) -> Option<u16> {
        if self.marker_x + self.cell_width * 64 > frame.width
            || self.marker_y + self.marker_height > frame.height
            || frame.stride < frame.width.checked_mul(4)?
        {
            return None;
        }
        let mut value = 0u64;
        for bit in 0..64 {
            let x = self.marker_x + bit * self.cell_width + self.cell_width / 2;
            let y = self.marker_y + self.marker_height / 2;
            let mut brightness = 0u32;
            for row in y - 1..=y + 1 {
                for column in x - 1..=x + 1 {
                    let offset = usize::try_from(row)
                        .ok()?
                        .checked_mul(usize::try_from(frame.stride).ok()?)?
                        .checked_add(usize::try_from(column).ok()?.checked_mul(4)?)?;
                    let pixel = frame.data.get(offset..offset.checked_add(3)?)?;
                    brightness += pixel.iter().map(|v| u32::from(*v)).sum::<u32>();
                }
            }
            let brightness = brightness / 27;
            let white = match brightness {
                0..=80 => false,
                170..=255 => true,
                _ => return None,
            };
            value = (value << 1) | u64::from(white);
        }
        (value >> 16 == self.prefix).then_some(value as u16)
    }
}

#[derive(Clone, Default, Debug, serde::Serialize)]
pub struct VisualProbeReport {
    pub samples: u64,
    pub pending: usize,
    pub keyboard_samples: u64,
    pub keyboard_armed: bool,
    pub unanchored_keys: u64,
    pub modified_keys: u64,
    pub keyboard_input_to_submit_min_ms: Option<f64>,
    pub keyboard_input_to_submit_p50_ms: Option<f64>,
    pub keyboard_input_to_submit_p95_ms: Option<f64>,
    pub keyboard_input_to_submit_max_ms: Option<f64>,
    pub unanchored_clicks: u64,
    pub canceled: u64,
    pub timed_out: u64,
    pub modified_clicks: u64,
    pub correlation_lost: bool,
    pub evicted: u64,
    pub frames_without_marker: u64,
    pub unavailable_presentations: u64,
    pub input_to_submit_min_ms: Option<f64>,
    pub input_to_submit_p50_ms: Option<f64>,
    pub input_to_submit_p95_ms: Option<f64>,
    pub last_counter: Option<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeInput {
    Click,
    Key,
}

pub(super) struct VisualProbe {
    spec: VisualProbeSpec,
    report: VisualProbeReport,
    pending: VecDeque<(u16, u64, Instant, ProbeInput)>,
    next: Option<u16>,
    latencies: Vec<f64>,
    key_latencies: Vec<f64>,
}
impl VisualProbe {
    pub(super) fn new(spec: VisualProbeSpec) -> Result<Self, DesktopError> {
        spec.validate()?;
        Ok(Self {
            spec,
            report: VisualProbeReport::default(),
            pending: VecDeque::new(),
            next: None,
            latencies: Vec::new(),
            key_latencies: Vec::new(),
        })
    }
    pub(super) fn click(&mut self, point: (f64, f64), seq: u64, now: Instant) {
        if !self.spec.inside(point) {
            self.keyboard_focus_lost();
            return;
        }
        self.enqueue(seq, now, ProbeInput::Click);
    }
    pub(super) fn key(&mut self, code: u32, unmodified: bool, seq: u64, now: Instant) {
        if !self.spec.keyboard_codes.contains(&code) {
            return;
        }
        if !unmodified {
            self.report.modified_keys += 1;
            self.lose_correlation("modified controlled key");
            return;
        }
        self.expire(now);
        // Input remains ordered behind the focus click even when its response
        // has not been presented yet. Retain these provisional keys in the
        // same counter sequence; skipping them would match their responses to
        // later keys and report an impossibly short input-to-submit time.
        // They cannot complete until presentation also confirms the click.
        let focus_click_pending = self
            .pending
            .iter()
            .any(|(_, _, _, kind)| *kind == ProbeInput::Click);
        if !self.report.keyboard_armed && !focus_click_pending {
            self.report.unanchored_keys += 1;
            return;
        }
        self.enqueue(seq, now, ProbeInput::Key);
    }
    pub(super) fn keyboard_focus_lost(&mut self) {
        if !self.spec.keyboard_codes.is_empty() {
            self.reset();
        }
    }
    fn enqueue(&mut self, seq: u64, now: Instant, kind: ProbeInput) {
        self.expire(now);
        if self.report.correlation_lost {
            self.unanchored(kind);
            return;
        }
        let Some(next) = self.next.and_then(|v| v.checked_add(1)) else {
            self.unanchored(kind);
            return;
        };
        self.next = Some(next);
        if self.pending.len() == 128 {
            self.pending.pop_front();
            self.report.evicted += 1;
        }
        self.pending.push_back((next, seq, now, kind));
    }
    fn unanchored(&mut self, kind: ProbeInput) {
        match kind {
            ProbeInput::Click => self.report.unanchored_clicks += 1,
            ProbeInput::Key => self.report.unanchored_keys += 1,
        }
    }
    pub(super) fn modified_click(&mut self, point: (f64, f64)) {
        if self.spec.inside(point) {
            self.report.modified_clicks += 1;
            self.lose_correlation("modified target click");
        } else {
            self.keyboard_focus_lost();
        }
    }
    fn expire(&mut self, now: Instant) {
        let expired = self
            .pending
            .iter()
            .take_while(|(_, _, started, _)| {
                now.saturating_duration_since(*started) >= RESPONSE_TIMEOUT
            })
            .count();
        if expired > 0 {
            self.report.timed_out += expired as u64;
            self.lose_correlation("unanswered target click");
        }
    }
    fn lose_correlation(&mut self, reason: &'static str) {
        self.reset();
        self.report.correlation_lost = true;
        tracing::warn!(target:"rds_desktop::visual_probe", reason,
            "controlled visual measurement correlation lost; start a fresh attempt");
    }
    /// Called only after this exact raw frame was presented by the native GPU path.
    /// No network ACK or merely decoded frame can complete the measurement.
    #[cfg(test)]
    fn presented(&mut self, frame: &RawFrame, now: Instant) {
        self.presented_with_seq(frame, now, None);
    }
    pub(super) fn presented_with_seq(
        &mut self,
        frame: &RawFrame,
        now: Instant,
        frame_seq: Option<u64>,
    ) {
        self.expire(now);
        let Some(counter) = self.spec.counter(frame) else {
            self.report.frames_without_marker += 1;
            // A covered/moved target cannot prove later clicks correspond to
            // the earlier counter. Require a fresh anchor before measuring.
            self.reset();
            return;
        };
        if self.report.correlation_lost {
            self.report.last_counter = Some(counter);
            return;
        }
        if self.next.is_some_and(|expected| counter > expected) {
            self.lose_correlation("unrequested controlled response");
            return;
        }
        if self
            .report
            .last_counter
            .is_some_and(|previous| counter < previous)
        {
            self.reset();
        }
        self.report.last_counter = Some(counter);
        self.next = Some(self.next.unwrap_or(counter).max(counter));
        while self
            .pending
            .front()
            .is_some_and(|(wanted, _, _, _)| *wanted <= counter)
        {
            if let Some((_, seq, started, kind)) = self.pending.pop_front() {
                let delay = now.saturating_duration_since(started).as_secs_f64() * 1000.;
                if self.latencies.len() == 1024 {
                    self.latencies.remove(0);
                }
                self.latencies.push(delay);
                self.report.samples += 1;
                let input_class = match kind {
                    ProbeInput::Click => {
                        self.report.keyboard_armed = !self.spec.keyboard_codes.is_empty();
                        "click"
                    }
                    ProbeInput::Key => {
                        if self.key_latencies.len() == 1024 {
                            self.key_latencies.remove(0);
                        }
                        self.key_latencies.push(delay);
                        self.report.keyboard_samples += 1;
                        "keyboard"
                    }
                };
                tracing::info!(target:"rds_desktop::visual_probe", input_class, input_seq=seq, frame_seq, marker_counter=counter, input_to_submit_ms=delay, "controlled visual response submitted");
            }
        }
    }
    pub(super) fn reset(&mut self) {
        self.report.canceled += self.pending.len() as u64;
        self.pending.clear();
        self.next = None;
        self.report.last_counter = None;
        self.report.keyboard_armed = false;
    }
    pub(super) fn unavailable(&mut self) {
        self.report.unavailable_presentations += 1;
        self.reset();
    }
    pub(super) fn report(&self) -> VisualProbeReport {
        let mut report = self.report.clone();
        report.pending = self.pending.len();
        let mut sorted = self.latencies.clone();
        sorted.sort_by(f64::total_cmp);
        if !sorted.is_empty() {
            report.input_to_submit_min_ms = Some(sorted[0]);
            report.input_to_submit_p50_ms = Some(sorted[(sorted.len() - 1) * 50 / 100]);
            report.input_to_submit_p95_ms = Some(sorted[(sorted.len() - 1) * 95 / 100]);
        }
        let mut keys = self.key_latencies.clone();
        keys.sort_by(f64::total_cmp);
        if !keys.is_empty() {
            report.keyboard_input_to_submit_min_ms = Some(keys[0]);
            report.keyboard_input_to_submit_p50_ms = Some(keys[(keys.len() - 1) * 50 / 100]);
            report.keyboard_input_to_submit_p95_ms = Some(keys[(keys.len() - 1) * 95 / 100]);
            report.keyboard_input_to_submit_max_ms = keys.last().copied();
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> VisualProbeSpec {
        VisualProbeSpec {
            marker_x: 0,
            marker_y: 0,
            cell_width: 8,
            marker_height: 24,
            click_rect: [0, 24, 512, 8],
            prefix: 0x1234_5678_abcd,
            keyboard_codes: Vec::new(),
        }
    }
    fn marker(prefix: u64, counter: u16) -> RawFrame {
        let value = (prefix << 16) | u64::from(counter);
        let mut data = vec![100u8; 512 * 32 * 4];
        for bit in 0..64 {
            let shade = if value & (1 << (63 - bit)) != 0 {
                235
            } else {
                24
            };
            for y in 0..24 {
                for x in bit * 8..(bit + 1) * 8 {
                    let offset = (y * 512 + x) * 4;
                    data[offset..offset + 4].copy_from_slice(&[shade, shade, shade, 255]);
                }
            }
        }
        RawFrame {
            width: 512,
            height: 32,
            stride: 2048,
            data: data.into(),
        }
    }
    #[test]
    fn only_the_controlled_rendered_counter_completes_matching_clicks() {
        let spec = spec();
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.click((2., 26.), 0, start);
        assert_eq!(probe.report().unanchored_clicks, 1);
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((600., 26.), 1, start);
        assert_eq!(probe.report().pending, 0);
        probe.click((2., 26.), 2, start);
        probe.click((3., 27.), 3, start + Duration::from_millis(10));
        probe.presented(
            &marker(spec.prefix ^ 1, 2),
            start + Duration::from_millis(20),
        );
        assert_eq!(probe.report().samples, 0);
        assert_eq!(probe.report().canceled, 2);
        probe.presented(&marker(spec.prefix, 0), start + Duration::from_millis(25));
        probe.click((2., 26.), 2, start);
        probe.click((3., 27.), 3, start + Duration::from_millis(10));
        assert_eq!(probe.report().pending, 2);
        // A newer cumulative response proves both clicks even if an
        // intermediate frame was replaced before native presentation.
        probe.presented(&marker(spec.prefix, 2), start + Duration::from_millis(42));
        let report = probe.report();
        assert_eq!(report.samples, 2);
        assert_eq!(report.pending, 0);
        assert_eq!(report.input_to_submit_min_ms, Some(32.));
        probe.presented(&marker(spec.prefix, 2), start + Duration::from_millis(100));
        assert_eq!(probe.report().samples, 2);
        probe.click((3., 27.), 4, start);
        probe.reset();
        assert_eq!(probe.report().canceled, 3);
        probe.presented(&marker(spec.prefix, 3), start);
        assert_eq!(probe.report().samples, 2);
    }
    #[test]
    fn unavailable_surface_cancels_samples_and_requires_a_fresh_presented_anchor() {
        let spec = spec();
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((2., 26.), 1, start);
        probe.unavailable();
        probe.click((2., 26.), 2, start);
        let report = probe.report();
        assert_eq!(report.canceled, 1);
        assert_eq!(report.unanchored_clicks, 1);
        assert_eq!(report.pending, 0);
        probe.presented(&marker(spec.prefix, 2), start);
        assert_eq!(probe.report().samples, 0);
        probe.click((2., 26.), 3, start);
        probe.presented(&marker(spec.prefix, 3), start + Duration::from_millis(40));
        assert_eq!(probe.report().samples, 1);
        assert_eq!(probe.report().input_to_submit_min_ms, Some(40.));
    }
    #[test]
    fn an_unanswered_click_cannot_take_a_later_clicks_counter_response() {
        let spec = spec();
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((2., 26.), 1, start);
        // The target never responds to this press. A much later press increments
        // the same counter, so FIFO alone cannot prove which input caused it.
        let later = start + Duration::from_secs(188);
        probe.click((2., 26.), 2, later);
        probe.presented(&marker(spec.prefix, 1), later);
        assert_eq!(probe.report().samples, 0);
        assert_eq!(probe.report().pending, 0);
        assert_eq!(probe.report().timed_out, 1);
        assert!(probe.report().correlation_lost);
        // Continuing markers or a surface reset cannot silently re-enable an
        // attempt whose assumed one-press/one-increment contract failed.
        probe.reset();
        probe.presented(&marker(spec.prefix, 1), later);
        probe.click((2., 26.), 3, later);
        probe.presented(&marker(spec.prefix, 2), later);
        assert_eq!(probe.report().samples, 0);
    }
    #[test]
    fn timeout_boundary_and_modified_clicks_fail_closed() {
        let spec = spec();
        let start = Instant::now();
        for elapsed in [RESPONSE_TIMEOUT - Duration::from_nanos(1), RESPONSE_TIMEOUT] {
            let mut probe = VisualProbe::new(spec.clone()).unwrap();
            probe.presented(&marker(spec.prefix, 0), start);
            probe.click((2., 26.), 1, start);
            probe.presented(&marker(spec.prefix, 1), start + elapsed);
            assert_eq!(
                probe.report().samples,
                u64::from(elapsed < RESPONSE_TIMEOUT)
            );
            assert_eq!(probe.report().correlation_lost, elapsed >= RESPONSE_TIMEOUT);
        }
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.modified_click((600., 26.));
        assert!(!probe.report().correlation_lost);
        probe.click((2., 26.), 1, start);
        probe.modified_click((2., 26.));
        probe.presented(&marker(spec.prefix, 2), start);
        assert_eq!(probe.report().samples, 0);
        assert_eq!(probe.report().canceled, 1);
        assert_eq!(probe.report().modified_clicks, 1);
        assert!(probe.report().correlation_lost);
    }
    #[test]
    fn keyboard_requires_a_presented_focus_click_and_measures_keydown_only() {
        let mut spec = spec();
        spec.keyboard_codes = vec![30, 14];
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.key(30, true, 1, start);
        assert_eq!(probe.report().unanchored_keys, 1);
        assert_eq!(probe.report().pending, 0);
        probe.click((2., 26.), 2, start);
        assert!(!probe.report().keyboard_armed);
        probe.presented(&marker(spec.prefix, 1), start + Duration::from_millis(30));
        assert!(probe.report().keyboard_armed);
        probe.key(31, true, 3, start); // Not a controlled key.
        assert_eq!(probe.report().pending, 0);
        probe.key(30, true, 4, start + Duration::from_millis(40));
        probe.key(14, true, 5, start + Duration::from_millis(45));
        probe.presented(&marker(spec.prefix, 3), start + Duration::from_millis(90));
        let report = probe.report();
        assert_eq!(report.keyboard_samples, 2);
        assert_eq!(report.samples, 3);
        assert_eq!(report.keyboard_input_to_submit_min_ms, Some(45.));
        assert_eq!(report.keyboard_input_to_submit_max_ms, Some(50.));
        assert_eq!(report.pending, 0);
        probe.presented(&marker(spec.prefix, 3), start + Duration::from_millis(110));
        assert_eq!(probe.report().keyboard_samples, 2);
    }
    #[test]
    fn focus_loss_modifiers_and_unrequested_counters_cannot_fabricate_key_samples() {
        let mut spec = spec();
        spec.keyboard_codes = vec![30, 14];
        let start = Instant::now();
        for outside_click in [false, true] {
            let mut probe = VisualProbe::new(spec.clone()).unwrap();
            probe.presented(&marker(spec.prefix, 0), start);
            probe.click((2., 26.), 1, start);
            probe.presented(&marker(spec.prefix, 1), start);
            probe.key(30, true, 2, start);
            if outside_click {
                probe.click((600., 26.), 3, start);
            } else {
                probe.keyboard_focus_lost();
            }
            probe.presented(&marker(spec.prefix, 2), start);
            probe.key(14, true, 4, start);
            assert!(!probe.report().keyboard_armed);
            assert_eq!(probe.report().keyboard_samples, 0);
            assert_eq!(probe.report().canceled, 1);
            assert_eq!(probe.report().unanchored_keys, 1);
        }
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((2., 26.), 1, start);
        probe.presented(&marker(spec.prefix, 1), start);
        probe.key(30, false, 2, start);
        probe.presented(&marker(spec.prefix, 2), start);
        assert!(probe.report().correlation_lost);
        assert_eq!(probe.report().modified_keys, 1);
        assert_eq!(probe.report().keyboard_samples, 0);
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((2., 26.), 1, start);
        probe.presented(&marker(spec.prefix, 1), start);
        probe.key(30, true, 2, start);
        probe.presented(&marker(spec.prefix, 3), start); // Extra actor/counter increment.
        assert!(probe.report().correlation_lost);
        assert_eq!(probe.report().keyboard_samples, 0);
        assert_eq!(probe.report().canceled, 1);
    }
    #[test]
    fn typing_before_focus_click_presentation_keeps_the_actual_counter_sequence() {
        let mut spec = spec();
        spec.keyboard_codes = vec![30, 14];
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((2., 26.), 1, start);
        probe.key(30, true, 2, start + Duration::from_millis(10));
        probe.key(14, true, 3, start + Duration::from_millis(20));
        assert!(!probe.report().keyboard_armed);
        assert_eq!(probe.report().pending, 3);
        assert_eq!(probe.report().unanchored_keys, 0);
        // Presentation of only the click must not complete either later key.
        probe.presented(&marker(spec.prefix, 1), start + Duration::from_millis(150));
        assert_eq!(probe.report().keyboard_samples, 0);
        assert_eq!(probe.report().pending, 2);
        probe.presented(&marker(spec.prefix, 3), start + Duration::from_millis(180));
        let report = probe.report();
        assert_eq!(report.samples, 3);
        assert_eq!(report.keyboard_samples, 2);
        assert_eq!(report.keyboard_input_to_submit_min_ms, Some(160.));
        assert_eq!(report.keyboard_input_to_submit_max_ms, Some(170.));
        assert!(!report.correlation_lost);
    }
    #[test]
    fn collapsed_fast_typing_frame_and_focus_loss_preserve_measurement_boundaries() {
        let mut spec = spec();
        spec.keyboard_codes = vec![30, 14];
        let start = Instant::now();
        for lose_focus in [false, true] {
            let mut probe = VisualProbe::new(spec.clone()).unwrap();
            probe.presented(&marker(spec.prefix, 0), start);
            probe.click((2., 26.), 1, start);
            for seq in 2..=33 {
                probe.key(30, true, seq, start + Duration::from_millis(seq));
            }
            if lose_focus {
                probe.keyboard_focus_lost();
            }
            // The newest frame may contain the click and all 32 characters.
            probe.presented(&marker(spec.prefix, 33), start + Duration::from_millis(200));
            let report = probe.report();
            assert_eq!(report.pending, 0);
            assert_eq!(report.keyboard_samples, if lose_focus { 0 } else { 32 });
            assert_eq!(report.canceled, if lose_focus { 33 } else { 0 });
            if !lose_focus {
                assert_eq!(report.keyboard_input_to_submit_min_ms, Some(167.));
                assert_eq!(report.keyboard_input_to_submit_max_ms, Some(198.));
                assert!(report.keyboard_armed);
                assert!(!report.correlation_lost);
            }
        }
    }
    #[test]
    fn provisional_typing_does_not_survive_an_unanswered_focus_click() {
        let mut spec = spec();
        spec.keyboard_codes = vec![30];
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.click((2., 26.), 1, start);
        probe.key(30, true, 2, start + Duration::from_millis(10));
        probe.presented(&marker(spec.prefix, 2), start + RESPONSE_TIMEOUT);
        let report = probe.report();
        assert!(report.correlation_lost);
        assert!(!report.keyboard_armed);
        assert_eq!(report.samples, 0);
        assert_eq!(report.canceled, 2);
        assert_eq!(report.timed_out, 1);
    }
    #[test]
    fn keyboard_descriptor_bounds_preserve_click_only_defaults() {
        let spec = spec();
        let start = Instant::now();
        let mut probe = VisualProbe::new(spec.clone()).unwrap();
        probe.presented(&marker(spec.prefix, 0), start);
        probe.key(30, true, 1, start);
        assert_eq!(probe.report().unanchored_keys, 0);
        assert_eq!(probe.report().pending, 0);
        for codes in [
            vec![30, 30],
            vec![29],
            vec![58],
            vec![0],
            vec![256],
            (1..=17).collect(),
        ] {
            let mut invalid = spec.clone();
            invalid.keyboard_codes = codes;
            assert!(invalid.validate().is_err());
        }
        let mut valid = spec;
        valid.keyboard_codes = vec![14, 30];
        assert!(valid.validate().is_ok());
    }
    #[test]
    fn marker_bounds_and_ambiguous_pixels_fail_without_panicking() {
        let mut invalid = spec();
        invalid.marker_x = u32::MAX;
        assert!(invalid.validate().is_err());
        let spec = spec();
        let mut raw = marker(spec.prefix, 0);
        raw.data = bytes::Bytes::from_static(&[0; 16]);
        assert_eq!(spec.counter(&raw), None);
        raw = marker(spec.prefix, 0);
        raw.stride = 1;
        assert_eq!(spec.counter(&raw), None);
        raw = marker(spec.prefix, 0);
        raw.data = vec![120; raw.data.len()].into();
        assert_eq!(spec.counter(&raw), None);
    }
    #[cfg(feature = "x11")]
    #[test]
    fn controlled_marker_survives_actual_h264_encoding_and_decode() {
        use crate::{Decoder, Encoder};
        let spec = spec();
        let mut encoder = crate::H264Encoder::new(4_000_000, 60.).unwrap();
        let mut decoder = crate::H264Decoder::new().unwrap();
        for counter in [0, 1, 2, 15, 255] {
            let raw = marker(spec.prefix, counter);
            let encoded = encoder.encode(&raw).unwrap();
            let decoded = decoder
                .decode(&encoded)
                .unwrap()
                .expect("decoded marker frame");
            assert_eq!(spec.counter(&decoded), Some(counter));
        }
    }
}
