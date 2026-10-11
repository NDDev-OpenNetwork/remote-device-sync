//! Serving backend ownership stays in the agent, below transport/policy APIs.

use crate::{DesktopError, FrameProducer, InputSink};
use std::time::Duration;

/// One already-prepared desktop backend. Inventory must be an in-memory read;
/// native capture/encoder initialization runs on the serving capture worker.
/// Consent is local and precedes serving; remote requests cannot open a portal.
pub trait DesktopSource: Send + Sync + 'static {
    fn capabilities(&self) -> Result<rds_core::DesktopCaps, DesktopError>;
    fn producer(
        &self,
        display: u32,
        interval: Duration,
        height: Option<u32>,
        extent: Option<(u32, u32)>,
    ) -> Result<Box<dyn FrameProducer>, DesktopError>;
    /// Return a prepared in-memory lease; this method must not perform native I/O.
    fn input(&self, display: u32, extent: (u32, u32)) -> Result<Box<dyn InputSink>, DesktopError>;
    /// An absent native clipboard must never probe an unrelated backend.
    fn clipboard_supported(&self) -> bool {
        false
    }
    /// Create an in-memory clipboard lease; never opens consent or a different
    /// platform backend. Watching starts after this lease's initial snapshot.
    fn clipboard(
        &self,
        _display: u32,
        _watching: bool,
    ) -> Result<Box<dyn crate::clipboard::ClipboardBackend>, DesktopError> {
        Err(DesktopError::Input("clipboard backend unavailable".into()))
    }
}
