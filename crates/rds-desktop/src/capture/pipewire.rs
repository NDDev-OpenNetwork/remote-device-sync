//! XDG portal + PipeWire capture — planned.
//!
//! The consented Wayland path: `ashpd` ScreenCast/RemoteDesktop portal
//! (persist-mode restore tokens for non-interactive re-use) hands us a
//! PipeWire node; `pipewire`/`libspa` run the stream with dmabuf-first
//! buffer negotiation, damage rects and a cursor sub-stream.

use crate::{Capturer, DesktopError};

/// Returns `Ok(None)` until the backend lands.
pub fn probe(_screen: u32) -> Result<Option<Box<dyn Capturer>>, DesktopError> {
    Ok(None)
}
