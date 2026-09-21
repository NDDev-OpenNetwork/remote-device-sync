//! Audio pipeline for rds — scaffold.
//!
//! Remote-desktop audio rides its own stream, independent of video:
//! Opus-encoded frames with a small jitter buffer and an independent
//! clock. Platform capture/render:
//!
//! - Linux: PipeWire (`pipewire`/`libspa`) preferred, `cpal` fallback.
//! - macOS: `cpal` (CoreAudio).
//!
//! Encoding is `opus` (libopus bindings — the C library is the Opus
//! standard); resampling `rubato` when device rate differs.
//! Implementation lands with the audio milestone; the traits pin the
//! seam now.

use bytes::Bytes;

/// PCM format negotiated between source and codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// Samples per second (48000 for Opus).
    pub sample_rate: u32,
    /// Channel count (1 or 2).
    pub channels: u16,
}

/// One encoded audio packet ready for the wire.
#[derive(Debug)]
pub struct AudioPacket {
    /// Capture timestamp in microseconds, monotonic to the source.
    pub timestamp_us: u64,
    /// Opus payload.
    pub data: Bytes,
}

/// Source of system audio on the serving side.
pub trait AudioSource: Send + 'static {
    fn format(&self) -> AudioFormat;
    /// Next encoded packet; blocks as the pipeline dictates.
    fn next_packet(&mut self) -> Option<AudioPacket>;
}

/// Playback sink on the viewing side (jitter buffer lives inside).
pub trait AudioSink: Send + 'static {
    /// Queue a packet for playback; late packets may be dropped.
    fn play(&mut self, packet: AudioPacket);
}
