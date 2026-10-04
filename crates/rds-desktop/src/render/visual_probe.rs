//! Opt-in causal input-to-submit measurement against a controlled visual marker.
use crate::{DesktopError, RawFrame};
use std::{collections::VecDeque, time::Instant};

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
}
impl VisualProbeSpec {
    pub fn validate(&self) -> Result<(), DesktopError> {
        let extent = |start: u32, size: u32| {
            size > 0 && start.checked_add(size).is_some_and(|end| end <= 16384)
        };
        if !(4..=32).contains(&self.cell_width)
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
    pub unanchored_clicks: u64,
    pub canceled: u64,
    pub evicted: u64,
    pub frames_without_marker: u64,
    pub input_to_submit_min_ms: Option<f64>,
    pub input_to_submit_p50_ms: Option<f64>,
    pub input_to_submit_p95_ms: Option<f64>,
    pub last_counter: Option<u16>,
}

pub(super) struct VisualProbe {
    spec: VisualProbeSpec,
    report: VisualProbeReport,
    pending: VecDeque<(u16, u64, Instant)>,
    next: Option<u16>,
    latencies: Vec<f64>,
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
        })
    }
    pub(super) fn click(&mut self, point: (f64, f64), seq: u64, now: Instant) {
        if !self.spec.inside(point) {
            return;
        }
        let Some(next) = self.next.and_then(|v| v.checked_add(1)) else {
            self.report.unanchored_clicks += 1;
            return;
        };
        self.next = Some(next);
        if self.pending.len() == 128 {
            self.pending.pop_front();
            self.report.evicted += 1;
        }
        self.pending.push_back((next, seq, now));
    }
    /// Called only after this exact raw frame was presented by the native GPU path.
    /// No network ACK or merely decoded frame can complete the measurement.
    pub(super) fn presented(&mut self, frame: &RawFrame, now: Instant) {
        let Some(counter) = self.spec.counter(frame) else {
            self.report.frames_without_marker += 1;
            return;
        };
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
            .is_some_and(|(wanted, _, _)| *wanted <= counter)
        {
            if let Some((_, seq, started)) = self.pending.pop_front() {
                let delay = now.saturating_duration_since(started).as_secs_f64() * 1000.;
                if self.latencies.len() == 1024 {
                    self.latencies.remove(0);
                }
                self.latencies.push(delay);
                self.report.samples += 1;
                tracing::info!(target:"rds_desktop::visual_probe", input_seq=seq, marker_counter=counter, input_to_submit_ms=delay, "controlled visual response submitted");
            }
        }
    }
    pub(super) fn reset(&mut self) {
        self.report.canceled += self.pending.len() as u64;
        self.pending.clear();
        self.next = None;
        self.report.last_counter = None;
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
        probe.click((3., 27.), 3, start + std::time::Duration::from_millis(10));
        probe.presented(
            &marker(spec.prefix ^ 1, 2),
            start + std::time::Duration::from_millis(20),
        );
        assert_eq!(probe.report().samples, 0);
        probe.presented(
            &marker(spec.prefix, 0),
            start + std::time::Duration::from_millis(25),
        );
        assert_eq!(probe.report().pending, 2);
        // A newer cumulative response proves both clicks even if an
        // intermediate frame was replaced before native presentation.
        probe.presented(
            &marker(spec.prefix, 2),
            start + std::time::Duration::from_millis(42),
        );
        let report = probe.report();
        assert_eq!(report.samples, 2);
        assert_eq!(report.pending, 0);
        assert_eq!(report.input_to_submit_min_ms, Some(32.));
        probe.presented(
            &marker(spec.prefix, 2),
            start + std::time::Duration::from_millis(100),
        );
        assert_eq!(probe.report().samples, 2);
        probe.click((3., 27.), 4, start);
        probe.reset();
        assert_eq!(probe.report().canceled, 1);
        probe.presented(&marker(spec.prefix, 3), start);
        assert_eq!(probe.report().samples, 2);
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
