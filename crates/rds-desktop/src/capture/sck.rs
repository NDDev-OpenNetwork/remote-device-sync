//! macOS ScreenCaptureKit backend — planned.
//!
//! `screencapturekit` safe bindings over Apple's framework: display
//! streams with IOSurface frames straight into VideoToolbox for encode.
//! Requires screen-recording TCC permission granted to the agent.

use crate::{Capturer, DesktopError};

/// Returns `Ok(None)` until the backend lands.
pub fn probe(_screen: u32) -> Result<Option<Box<dyn Capturer>>, DesktopError> {
    Ok(None)
}
