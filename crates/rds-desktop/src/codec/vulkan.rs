//! Vulkan Video codec path — planned.
//!
//! Vendor-agnostic GPU encode/decode via `ash` `vk::Video*` types:
//! H.264 first, HEVC and AV1 behind capability probes. Video-queue
//! loaders are resolved with `vkGetDeviceProcAddr` (ash 0.38 predates
//! the generated loaders). Buffers arrive as imported dmabuf /
//! `wgpu::Texture` so the pipeline stays GPU-resident end to end.

/// Whether a Vulkan device with the required video queues exists.
/// Always `false` until implemented.
pub fn available() -> bool {
    false
}
