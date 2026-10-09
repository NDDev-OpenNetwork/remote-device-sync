//! Bounded audio packet, codec and playout primitives for RDS.
//!
//! The crate deliberately owns no device callback, network stream or async
//! task. Platform capture/playback adapters and the agent service consume
//! these primitives later. Keeping packetization and loss behavior here gives
//! every platform the same limits and makes malformed audio fail closed before
//! it reaches a device sink.

use std::collections::BTreeMap;

use bytes::Bytes;
use thiserror::Error;

/// Maximum encoded size of one Opus frame in the current one-frame packet
/// contract. RFC 6716 permits multi-frame packets whose total size can be
/// larger; this crate intentionally does not expose those yet.
pub const MAX_OPUS_FRAME_BYTES: usize = 1275;
/// Maximum packet size in the RDS one-frame profile: a maximum-size frame
/// plus its TOC byte (RFC 6716 section 3.2.2). Additional Opus padding is
/// accepted only within this same packet bound.
pub const MAX_OPUS_PACKET_BYTES: usize = MAX_OPUS_FRAME_BYTES + 1;
/// The default interactive frame duration.
pub const DEFAULT_FRAME_DURATION_MS: u32 = 20;
/// The standards-defined Opus frame durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDuration {
    Ms2_5,
    Ms5,
    Ms10,
    Ms20,
    Ms40,
    Ms60,
}

impl FrameDuration {
    pub const DEFAULT: Self = Self::Ms20;

    pub const fn duration_micros(self) -> u32 {
        match self {
            Self::Ms2_5 => 2_500,
            Self::Ms5 => 5_000,
            Self::Ms10 => 10_000,
            Self::Ms20 => 20_000,
            Self::Ms40 => 40_000,
            Self::Ms60 => 60_000,
        }
    }

    pub const fn samples(self, sample_rate: u32) -> usize {
        ((sample_rate as u64 * self.duration_micros() as u64) / 1_000_000) as usize
    }
}
/// Maximum number of packets retained by one jitter buffer.
pub const MAX_JITTER_PACKETS: usize = 64;

/// PCM format negotiated between source and codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// Samples per second. Opus supports 8, 12, 16, 24 and 48 kHz.
    pub sample_rate: u32,
    /// Channel count (mono or stereo).
    pub channels: u16,
}

impl AudioFormat {
    pub const MONO_48_KHZ: Self = Self {
        sample_rate: 48_000,
        channels: 1,
    };

    pub fn validate(self) -> Result<(), AudioError> {
        if !matches!(self.sample_rate, 8_000 | 12_000 | 16_000 | 24_000 | 48_000) {
            return Err(AudioError::InvalidFormat("unsupported sample rate"));
        }
        if !matches!(self.channels, 1 | 2) {
            return Err(AudioError::InvalidFormat("audio must be mono or stereo"));
        }
        Ok(())
    }

    /// Return samples for a whole-millisecond duration kept for source
    /// compatibility with the initial bounded API. Use [`Self::frame_samples_for`]
    /// when the standards-defined 2.5 ms duration is needed.
    pub fn frame_samples(self, duration_ms: u32) -> Result<usize, AudioError> {
        let duration = match duration_ms {
            5 => FrameDuration::Ms5,
            10 => FrameDuration::Ms10,
            20 => FrameDuration::Ms20,
            40 => FrameDuration::Ms40,
            60 => FrameDuration::Ms60,
            _ => {
                return Err(AudioError::InvalidFrame(
                    "frame duration must be 2.5, 5, 10, 20, 40 or 60 ms",
                ));
            }
        };
        self.frame_samples_for(duration)
    }

    pub fn frame_samples_for(self, duration: FrameDuration) -> Result<usize, AudioError> {
        self.validate()?;
        Ok(duration.samples(self.sample_rate))
    }
}

/// One encoded audio packet ready for a future wire stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    /// Monotonic packet sequence within one audio session.
    pub seq: u64,
    /// Capture timestamp in microseconds on the source's monotonic clock.
    pub timestamp_us: u64,
    /// Samples per channel represented by this packet.
    pub samples: u32,
    /// Opus payload without an Ogg container.
    pub data: Bytes,
}

impl AudioPacket {
    pub fn from_wire(
        frame: rds_core::AudioFrame,
        format: AudioFormat,
        frame_samples: usize,
    ) -> Result<Self, AudioError> {
        let packet = Self {
            seq: frame.seq,
            timestamp_us: frame
                .capture_ts_ms
                .checked_mul(1000)
                .ok_or(AudioError::InvalidPacket("wire timestamp overflow"))?,
            samples: frame.samples,
            data: Bytes::from(frame.data),
        };
        packet.validate(format, frame_samples)?;
        Ok(packet)
    }

    /// Check framing and the negotiated duration without changing a decoder.
    /// PCM channels describe decoder output: Opus can legitimately encode a
    /// mono frame for a stereo output, so the TOC channel bit need not match.
    fn validate(&self, format: AudioFormat, frame_samples: usize) -> Result<(), AudioError> {
        format.validate()?;
        let valid_duration = [
            FrameDuration::Ms2_5,
            FrameDuration::Ms5,
            FrameDuration::Ms10,
            FrameDuration::Ms20,
            FrameDuration::Ms40,
            FrameDuration::Ms60,
        ]
        .into_iter()
        .any(|duration| duration.samples(format.sample_rate) == frame_samples);
        if !valid_duration || self.samples as usize != frame_samples {
            return Err(AudioError::InvalidFrame("packet sample count mismatch"));
        }
        if self.data.is_empty() || self.data.len() > MAX_OPUS_PACKET_BYTES {
            return Err(AudioError::InvalidPacket("packet size is outside bounds"));
        }
        let parsed = opus::packet::parse(&self.data)
            .map_err(|_| AudioError::InvalidPacket("malformed Opus packet"))?;
        if parsed.frames.len() != 1 {
            return Err(AudioError::InvalidPacket("expected one Opus frame"));
        }
        let samples = opus::packet::get_nb_samples(&self.data, format.sample_rate)
            .map_err(|_| AudioError::InvalidPacket("invalid Opus duration"))?;
        if samples != frame_samples {
            return Err(AudioError::InvalidFrame("Opus duration mismatch"));
        }
        Ok(())
    }

    /// Convert to the current core wire record. The core wire timestamp is in
    /// whole milliseconds, so a sub-millisecond source remainder is truncated
    /// at this boundary; sequence and sample counts remain exact.
    pub fn to_wire(&self) -> rds_core::AudioFrame {
        rds_core::AudioFrame {
            seq: self.seq,
            capture_ts_ms: self.timestamp_us / 1000,
            samples: self.samples,
            data: self.data.to_vec(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AudioError {
    #[error("invalid audio format: {0}")]
    InvalidFormat(&'static str),
    #[error("invalid audio frame: {0}")]
    InvalidFrame(&'static str),
    #[error("invalid audio packet: {0}")]
    InvalidPacket(&'static str),
    #[error("Opus codec error: {0}")]
    Codec(String),
}

/// Official libopus encoder with fixed frame sizing and bounded output.
pub struct OpusEncoder {
    inner: opus::Encoder,
    format: AudioFormat,
    frame_samples: usize,
    next_seq: u64,
}

impl OpusEncoder {
    pub fn new(format: AudioFormat) -> Result<Self, AudioError> {
        let frame_samples = format.frame_samples_for(FrameDuration::DEFAULT)?;
        let channels = match format.channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => unreachable!("AudioFormat validates channel count"),
        };
        let inner = opus::Encoder::new(format.sample_rate, channels, opus::Application::Audio)
            .map_err(|error| AudioError::Codec(error.to_string()))?;
        Ok(Self {
            inner,
            format,
            frame_samples,
            next_seq: 0,
        })
    }

    pub fn format(&self) -> AudioFormat {
        self.format
    }

    pub fn frame_samples(&self) -> usize {
        self.frame_samples
    }

    pub fn encode(&mut self, pcm: &[f32], timestamp_us: u64) -> Result<AudioPacket, AudioError> {
        let expected = self.frame_samples * self.format.channels as usize;
        if pcm.len() != expected {
            return Err(AudioError::InvalidFrame("PCM frame has the wrong length"));
        }
        let mut data = vec![0u8; MAX_OPUS_PACKET_BYTES];
        let len = self
            .inner
            .encode_float(pcm, &mut data)
            .map_err(|error| AudioError::Codec(error.to_string()))?;
        data.truncate(len);
        let packet = AudioPacket {
            seq: self.next_seq,
            timestamp_us,
            samples: self.frame_samples as u32,
            data: Bytes::from(data),
        };
        packet.validate(self.format, self.frame_samples)?;
        self.next_seq = self.next_seq.wrapping_add(1);
        Ok(packet)
    }
}

/// Official libopus decoder. An empty payload is used only by decode_plc;
/// packet loss is represented by a jitter-buffer gap.
pub struct OpusDecoder {
    inner: opus::Decoder,
    format: AudioFormat,
    frame_samples: usize,
}

impl OpusDecoder {
    pub fn new(format: AudioFormat) -> Result<Self, AudioError> {
        let frame_samples = format.frame_samples_for(FrameDuration::DEFAULT)?;
        let channels = match format.channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => unreachable!("AudioFormat validates channel count"),
        };
        let inner = opus::Decoder::new(format.sample_rate, channels)
            .map_err(|error| AudioError::Codec(error.to_string()))?;
        Ok(Self {
            inner,
            format,
            frame_samples,
        })
    }

    pub fn decode(&mut self, packet: &AudioPacket) -> Result<Vec<f32>, AudioError> {
        packet.validate(self.format, self.frame_samples)?;
        let mut pcm = vec![0.0; self.frame_samples * self.format.channels as usize];
        let decoded = self
            .inner
            .decode_float(&packet.data, &mut pcm, false)
            .map_err(|error| AudioError::Codec(error.to_string()))?;
        if decoded != self.frame_samples {
            return Err(AudioError::Codec(
                "decoder returned an unexpected frame size".into(),
            ));
        }
        Ok(pcm)
    }

    /// Decode one lost packet using Opus packet-loss concealment.
    pub fn decode_plc(&mut self) -> Result<Vec<f32>, AudioError> {
        let mut pcm = vec![0.0; self.frame_samples * self.format.channels as usize];
        let decoded = self
            .inner
            .decode_float(&[], &mut pcm, false)
            .map_err(|error| AudioError::Codec(error.to_string()))?;
        if decoded != self.frame_samples {
            return Err(AudioError::Codec(
                "PLC returned an unexpected frame size".into(),
            ));
        }
        Ok(pcm)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Accepted,
    Duplicate,
    Late,
    DroppedOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopOutcome {
    Packet,
    Gap { sequence: u64 },
}

/// Sequence-aware bounded reorder buffer. It never waits on a timer and never
/// allocates beyond its configured packet count; the playout owner decides
/// when a Gap should become decoder PLC.
pub struct JitterBuffer {
    format: AudioFormat,
    frame_samples: usize,
    capacity: usize,
    target_delay: usize,
    next_sequence: Option<u64>,
    packets: BTreeMap<u64, AudioPacket>,
    pub accepted: u64,
    pub duplicates: u64,
    pub late: u64,
    pub overflow: u64,
    pub gaps: u64,
}

impl JitterBuffer {
    pub fn new(
        format: AudioFormat,
        capacity: usize,
        target_delay: usize,
    ) -> Result<Self, AudioError> {
        let frame_samples = format.frame_samples_for(FrameDuration::DEFAULT)?;
        if capacity == 0 || capacity > MAX_JITTER_PACKETS || target_delay >= capacity {
            return Err(AudioError::InvalidFrame("invalid jitter-buffer bounds"));
        }
        Ok(Self {
            format,
            frame_samples,
            capacity,
            target_delay,
            next_sequence: None,
            packets: BTreeMap::new(),
            accepted: 0,
            duplicates: 0,
            late: 0,
            overflow: 0,
            gaps: 0,
        })
    }

    pub fn push(&mut self, packet: AudioPacket) -> Result<PushOutcome, AudioError> {
        packet.validate(self.format, self.frame_samples)?;
        if self.next_sequence.is_some_and(|next| packet.seq < next) {
            self.late += 1;
            return Ok(PushOutcome::Late);
        }
        if self.packets.contains_key(&packet.seq) {
            self.duplicates += 1;
            return Ok(PushOutcome::Duplicate);
        }
        let mut outcome = PushOutcome::Accepted;
        if self.packets.len() == self.capacity {
            self.packets.pop_first();
            self.overflow += 1;
            outcome = PushOutcome::DroppedOverflow;
        }
        self.packets.insert(packet.seq, packet);
        self.accepted += 1;
        Ok(outcome)
    }

    /// Pop the next packet or report a sequence gap once the target delay is
    /// exceeded. The caller should invoke decoder PLC for a gap.
    pub fn pop(&mut self) -> Option<(PopOutcome, Option<AudioPacket>)> {
        let next = match self.next_sequence {
            Some(next) => next,
            None => {
                let next = *self.packets.first_key_value()?.0;
                self.next_sequence = Some(next);
                next
            }
        };
        if let Some(packet) = self.packets.remove(&next) {
            self.next_sequence = Some(next.wrapping_add(1));
            return Some((PopOutcome::Packet, Some(packet)));
        }
        if self.packets.len() > self.target_delay {
            let available = *self.packets.first_key_value()?.0;
            self.next_sequence = Some(available);
            self.gaps += available.wrapping_sub(next);
            return Some((PopOutcome::Gap { sequence: next }, None));
        }
        None
    }

    /// Set the first expected sequence before playout begins. This lets a
    /// receiver report leading loss instead of silently choosing the first
    /// packet that happened to arrive.
    pub fn start_at(&mut self, sequence: u64) -> Result<(), AudioError> {
        if self.next_sequence.is_some() {
            return Err(AudioError::InvalidFrame("jitter playout already started"));
        }
        self.next_sequence = Some(sequence);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn format(&self) -> AudioFormat {
        self.format
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seq: u64) -> AudioPacket {
        AudioPacket {
            seq,
            timestamp_us: seq * 20_000,
            samples: 960,
            data: Bytes::from_static(&[0xf8]),
        }
    }

    #[test]
    fn format_bounds_are_explicit() {
        assert!(AudioFormat::MONO_48_KHZ.validate().is_ok());
        assert_eq!(AudioFormat::MONO_48_KHZ.frame_samples(20).unwrap(), 960);
        assert_eq!(
            AudioFormat::MONO_48_KHZ
                .frame_samples_for(FrameDuration::Ms2_5)
                .unwrap(),
            120
        );
        assert_eq!(
            AudioFormat::MONO_48_KHZ
                .frame_samples_for(FrameDuration::Ms60)
                .unwrap(),
            2_880
        );
        assert!(
            AudioFormat {
                sample_rate: 44_100,
                channels: 2
            }
            .validate()
            .is_err()
        );
        assert!(AudioFormat::MONO_48_KHZ.frame_samples(2).is_err());
        assert!(AudioFormat::MONO_48_KHZ.frame_samples(15).is_err());
    }

    #[test]
    fn frame_duration_samples_cover_all_supported_rates() {
        for (sample_rate, expected) in [
            (8_000, [20, 40, 80, 160, 320, 480]),
            (12_000, [30, 60, 120, 240, 480, 720]),
            (16_000, [40, 80, 160, 320, 640, 960]),
            (24_000, [60, 120, 240, 480, 960, 1_440]),
            (48_000, [120, 240, 480, 960, 1_920, 2_880]),
        ] {
            let format = AudioFormat {
                sample_rate,
                channels: 2,
            };
            for (duration, samples) in [
                (FrameDuration::Ms2_5, expected[0]),
                (FrameDuration::Ms5, expected[1]),
                (FrameDuration::Ms10, expected[2]),
                (FrameDuration::Ms20, expected[3]),
                (FrameDuration::Ms40, expected[4]),
                (FrameDuration::Ms60, expected[5]),
            ] {
                assert_eq!(format.frame_samples_for(duration).unwrap(), samples);
            }
        }
    }

    #[test]
    fn opus_round_trip_is_bounded_and_exactly_sized() {
        let format = AudioFormat::MONO_48_KHZ;
        let mut encoder = OpusEncoder::new(format).unwrap();
        let mut decoder = OpusDecoder::new(format).unwrap();
        let pcm = vec![0.0; encoder.frame_samples()];
        let packet = encoder.encode(&pcm, 123_000).unwrap();
        assert_eq!(packet.seq, 0);
        assert!(packet.data.len() <= MAX_OPUS_PACKET_BYTES);
        let decoded = decoder.decode(&packet).unwrap();
        assert_eq!(decoded.len(), pcm.len());
        assert_eq!(decoder.decode_plc().unwrap().len(), pcm.len());
    }

    #[test]
    fn jitter_reorders_and_reports_a_gap() {
        let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 1).unwrap();
        assert_eq!(jitter.push(packet(2)).unwrap(), PushOutcome::Accepted);
        assert_eq!(jitter.push(packet(1)).unwrap(), PushOutcome::Accepted);
        assert_eq!(jitter.push(packet(1)).unwrap(), PushOutcome::Duplicate);
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Packet, Some(p))) if p.seq == 1
        ));
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Packet, Some(p))) if p.seq == 2
        ));
        assert!(jitter.pop().is_none());
        assert_eq!(jitter.gaps, 0);
    }

    #[test]
    fn jitter_reports_a_gap_after_delay() {
        let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 1).unwrap();
        assert_eq!(jitter.push(packet(0)).unwrap(), PushOutcome::Accepted);
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Packet, Some(p))) if p.seq == 0
        ));
        assert_eq!(jitter.push(packet(2)).unwrap(), PushOutcome::Accepted);
        assert_eq!(jitter.push(packet(3)).unwrap(), PushOutcome::Accepted);
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Gap { sequence: 1 }, None))
        ));
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Packet, Some(p))) if p.seq == 2
        ));
    }

    #[test]
    fn jitter_start_sequence_reports_leading_loss() {
        let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 4, 0).unwrap();
        jitter.start_at(5).unwrap();
        assert_eq!(jitter.push(packet(6)).unwrap(), PushOutcome::Accepted);
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Gap { sequence: 5 }, None))
        ));
        assert!(matches!(
            jitter.pop(),
            Some((PopOutcome::Packet, Some(p))) if p.seq == 6
        ));
    }

    #[test]
    fn jitter_drops_oldest_on_overflow_and_rejects_late_packets() {
        let mut jitter = JitterBuffer::new(AudioFormat::MONO_48_KHZ, 2, 0).unwrap();
        assert_eq!(jitter.push(packet(0)).unwrap(), PushOutcome::Accepted);
        assert_eq!(jitter.push(packet(1)).unwrap(), PushOutcome::Accepted);
        assert_eq!(
            jitter.push(packet(2)).unwrap(),
            PushOutcome::DroppedOverflow
        );
        assert_eq!(jitter.overflow, 1);
        assert!(jitter.pop().is_some());
        assert_eq!(jitter.push(packet(0)).unwrap(), PushOutcome::Late);
    }

    #[test]
    fn wire_conversion_documents_millisecond_timestamp_quantization() {
        let packet = AudioPacket {
            seq: 7,
            timestamp_us: 12_345,
            samples: 960,
            data: Bytes::from_static(&[1]),
        };
        assert_eq!(packet.to_wire().capture_ts_ms, 12);
    }

    #[test]
    fn wire_conversion_rejects_wrong_frame_shape() {
        let frame = rds_core::AudioFrame {
            seq: 1,
            capture_ts_ms: 4,
            samples: 1,
            data: vec![1],
        };
        assert!(AudioPacket::from_wire(frame, AudioFormat::MONO_48_KHZ, 960).is_err());
    }
}
