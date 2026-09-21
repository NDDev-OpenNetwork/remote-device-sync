//! Encoder/decoder backends, selected by platform probe + negotiation.
//!
//! Priority per platform (docs/platforms.md): Vulkan Video (`vulkan`,
//! all GPU vendors) → VA-API (`vaapi`, Linux) → VideoToolbox
//! (`videotoolbox`, macOS) → V4L2 mem2mem → software `openh264` floor.
//! All encoders emit Annex-B and honor `Encoder::request_idr` before
//! the next frame.

#[cfg(feature = "x11")]
pub mod openh264;

#[cfg(target_os = "linux")]
pub mod vaapi;
#[cfg(target_os = "macos")]
pub mod videotoolbox;
pub mod vulkan;
