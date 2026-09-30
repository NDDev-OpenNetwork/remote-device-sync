//! Native window presentation. A single pending BGRA frame feeds a GPU surface;
//! window/input handling stays on the OS main thread and networking stays async.

#[cfg(feature = "viewer")]
mod gpu;
#[cfg(feature = "viewer")]
mod input;
#[cfg(feature = "viewer")]
mod platform;
#[cfg(feature = "viewer")]
mod viewer;
#[cfg(feature = "viewer")]
pub use viewer::{InputReceiver, Viewer, ViewerHandle, ViewerInput, ViewerReport};

/// The native viewer was compiled; actual adapter/window creation can still fail.
pub fn available() -> bool {
    cfg!(feature = "viewer")
}
