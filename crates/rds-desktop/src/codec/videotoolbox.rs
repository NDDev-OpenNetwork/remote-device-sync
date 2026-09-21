//! VideoToolbox codec path — planned (macOS).
//!
//! ScreenCaptureKit IOSurfaces encode through VideoToolbox
//! (`video-toolbox` crate); decode returns CVPixelBuffers for direct
//! presentation.

/// Always `false` until implemented.
pub fn available() -> bool {
    false
}
