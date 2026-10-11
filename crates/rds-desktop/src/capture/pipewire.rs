//! Ambient capture probing never opens a local consent dialog. Prepared
//! serving uses the optional Linux `WaylandDesktop::open` backend instead.
//! That owner selects monitors, holds the portal session and lends bounded
//! mapped BGRA/BGRx subscriptions to independently admitted serving sessions.

use crate::{Capturer, DesktopError};

/// A prepared permission owner is required; ambient probing is unavailable.
pub fn probe(_screen: u32) -> Result<Option<Box<dyn Capturer>>, DesktopError> {
    Ok(None)
}
