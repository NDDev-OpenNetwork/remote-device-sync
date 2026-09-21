//! VA-API codec path — planned (Linux).
//!
//! libva surfaces from PipeWire/dmabuf feed H.264/HEVC/AV1 encode.
//! Fallback where Vulkan Video driver coverage gaps exist (older
//! Intel/AMD, hybrid graphics).

/// Whether a usable VAAPI device exists. Always `false` until
/// implemented.
pub fn available() -> bool {
    false
}
