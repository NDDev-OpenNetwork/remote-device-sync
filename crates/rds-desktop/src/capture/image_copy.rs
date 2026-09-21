//! Wayland `ext-image-copy-capture-v1` backend — planned.
//!
//! Direct compositor protocol for privileged clients (no portal consent
//! dialog): per-frame damage, dmabuf buffer negotiation, cursor capture
//! sessions. Bindings: `wayland-client` + `wayland-protocols::ext::
//! image_copy_capture`. This is the preferred capture path where the
//! agent runs with compositor-granted permissions.

use crate::{Capturer, DesktopError};

/// Returns `Ok(None)` until the backend lands.
pub fn probe(_screen: u32) -> Result<Option<Box<dyn Capturer>>, DesktopError> {
    Ok(None)
}
