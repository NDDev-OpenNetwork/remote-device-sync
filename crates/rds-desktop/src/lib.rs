//! Desktop session pipeline for rds.
//!
//! Server side: `serve_desktop` captures a display, encodes frames and
//! streams each frame on its own uni-directional QUIC stream — stale frames
//! are reset rather than retransmitted, following the MoQ pattern.
//! Client side: `client` decodes the newest frames and sends input events
//! back on the control stream.

use bytes::Bytes;
use rds_core::{Codec, DesktopCaps, InputEvent};
use thiserror::Error;

pub mod client;
#[cfg(feature = "x11")]
mod codec;
mod session;
#[cfg(feature = "x11")]
pub mod x11;

#[cfg(feature = "x11")]
pub use codec::{H264Decoder, H264Encoder};
pub use session::serve_desktop;

/// One captured video frame, BGRA8 unless noted otherwise.
#[derive(Debug)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row of `data`.
    pub stride: u32,
    pub data: Bytes,
}

/// One encoded frame ready for the wire.
#[derive(Debug)]
pub struct EncodedFrame {
    pub codec: Codec,
    pub data: Bytes,
    pub keyframe: bool,
}

#[derive(Debug, Error)]
pub enum DesktopError {
    #[error("capture backend unavailable: {0}")]
    Capture(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("input injection failed: {0}")]
    Input(String),
    #[error("connection failed: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error("stream closed by peer")]
    Closed(#[from] iroh::endpoint::ClosedStream),
    #[error("stream write failed: {0}")]
    Write(#[from] iroh::endpoint::WriteError),
    #[error("stream read failed: {0}")]
    Read(#[from] iroh::endpoint::ReadError),
    #[error("stream read failed: {0}")]
    ReadExact(#[from] iroh::endpoint::ReadExactError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// A source of display frames.
pub trait Capturer: Send + 'static {
    /// Next frame in display order; blocks the calling thread as needed.
    fn capture(&mut self) -> Result<RawFrame, DesktopError>;
    fn displays(&self) -> Vec<rds_core::DisplayInfo>;
}

/// Frame encoder. Implementations must emit Annex-B streams and honor
/// `request_idr` before the next frame.
pub trait Encoder: Send + 'static {
    fn encode(&mut self, frame: &RawFrame) -> Result<EncodedFrame, DesktopError>;
    fn request_idr(&mut self);
    fn set_bitrate(&mut self, bps: u32);
}

/// Frame decoder for the viewing side.
pub trait Decoder: Send + 'static {
    /// Returns the decoded frame, or `None` when the codec needs more data.
    fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<RawFrame>, DesktopError>;
}

/// Injects remote input events into the serving session.
pub trait InputSink: Send + 'static {
    fn inject(&mut self, event: &InputEvent) -> Result<(), DesktopError>;
}

/// What this build can serve.
pub fn capabilities() -> Result<DesktopCaps, DesktopError> {
    #[cfg(feature = "x11")]
    {
        crate::x11::capabilities()
    }
    #[cfg(not(feature = "x11"))]
    Err(DesktopError::Capture("no capture backend built".into()))
}
