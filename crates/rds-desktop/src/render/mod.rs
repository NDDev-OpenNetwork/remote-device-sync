//! Native window presentation. A single pending BGRA frame feeds a GPU surface;
//! window/input handling stays on the OS main thread and networking stays async.

#[cfg(feature = "viewer")]
mod gpu;
#[cfg(feature = "viewer")]
mod input;
#[cfg(feature = "viewer")]
mod platform;
#[cfg(feature = "viewer")]
pub use platform::{NativeWindowState, choose_resolution};
#[cfg(feature = "viewer")]
mod viewer;
#[cfg(feature = "viewer")]
mod visual_probe;
#[cfg(feature = "viewer")]
pub use viewer::{InputReceiver, Viewer, ViewerHandle, ViewerInput, ViewerReport, ViewerSnapshot};
#[cfg(feature = "viewer")]
pub use visual_probe::{VisualProbeReport, VisualProbeSpec};

/// The native viewer was compiled; actual adapter/window creation can still fail.
pub fn available() -> bool {
    cfg!(feature = "viewer")
}
