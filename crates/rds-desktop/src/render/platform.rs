//! Native application activation is explicit for CLI and packaged launches.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
use winit::event_loop::EventLoop;

#[cfg(target_os = "macos")]
pub(super) fn event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    let event_loop = EventLoop::with_user_event()
        .with_activation_policy(ActivationPolicy::Regular)
        .build()?;
    // Winit validates the OS main thread before constructing this loop.
    if let Some(main) = MainThreadMarker::new() {
        let data = NSData::with_bytes(include_bytes!("../../assets/app-icon.png"));
        if let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) {
            // SAFETY: `main` proves AppKit's main-thread requirement. The
            // embedded PNG is owned by NSData and AppKit retains the NSImage.
            unsafe { NSApplication::sharedApplication(main).setApplicationIconImage(Some(&icon)) };
        }
    }
    Ok(event_loop)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    EventLoop::with_user_event().build()
}
