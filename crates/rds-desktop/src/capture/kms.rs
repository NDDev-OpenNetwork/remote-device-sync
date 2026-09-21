//! DRM/KMS scanout tap — planned.
//!
//! Reads the kernel scanout framebuffer directly (`drm` + `gbm` +
//! `drm-fourcc`, `FB_DAMAGE_CLIPS` for damage): works on Wayland, X11,
//! at the login screen and headless. Requires DRM master or
//! `CAP_SYS_ADMIN` — the privileged path for unattended access, not the
//! default for attended sessions. Multi-GPU aware.

use crate::{Capturer, DesktopError};

/// Returns `Ok(None)` until the backend lands.
pub fn probe(_screen: u32) -> Result<Option<Box<dyn Capturer>>, DesktopError> {
    Ok(None)
}
