//! Frame presentation — planned.
//!
//! `wgpu` surface with our own YUV→RGB conversion; policy is
//! newest-frame-only (decode and present the freshest frame, drop the
//! rest). Hardware decode lands as `wgpu::Texture` from the Vulkan
//! Video path; the software path uploads BGRA via `queue.write_texture`.
//!
//! Optional console client: DRM atomic direct present on Linux.

/// Always `false` until implemented.
pub fn available() -> bool {
    false
}
