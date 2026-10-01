//! Native application activation is explicit for CLI and packaged launches.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]
use winit::event_loop::EventLoop;

#[cfg(target_os = "macos")]
pub(super) fn paste_text() -> Result<Option<String>, crate::DesktopError> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    let main = objc2_foundation::MainThreadMarker::new().ok_or_else(|| {
        crate::DesktopError::Input("clipboard access requires the main thread".into())
    })?;
    let _main = main;
    // SAFETY: called for an explicit paste gesture on AppKit's verified main
    // thread. Retained Cocoa objects live through the copy; nothing is logged.
    let text = unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }
        .map(|s| s.to_string());
    if text
        .as_ref()
        .is_some_and(|s| s.len() > crate::clipboard::MAX_TEXT_BYTES)
    {
        return Err(crate::DesktopError::Input(
            "clipboard text exceeds 1 MiB".into(),
        ));
    }
    Ok(text)
}
#[cfg(not(target_os = "macos"))]
pub(super) fn paste_text() -> Result<Option<String>, crate::DesktopError> {
    Ok(None)
}

#[cfg(target_os = "macos")]
pub fn choose_resolution() -> Option<u32> {
    use objc2_app_kit::{NSAlert, NSApplication};
    use objc2_foundation::{MainThreadMarker, NSString};
    let main = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(main);
    // SAFETY: all AppKit operations run on its verified main thread, with
    // retained strings/buttons for the modal lifetime; no user data is parsed.
    let response = unsafe {
        let alert = NSAlert::new(main);
        app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Regular);
        alert.setMessageText(&NSString::from_str("RDS — Video quality"));
        alert.setInformativeText(&NSString::from_str("Full HD is the default. Choose 720p for a slower connection, or Original for the remote screen's native resolution."));
        for title in ["Full HD · 1080p", "HD · 720p", "Original", "Cancel"] {
            alert.addButtonWithTitle(&NSString::from_str(title));
        }
        activate_application();
        alert.runModal()
    };
    match response {
        1000 => Some(1080),
        1001 => Some(720),
        1002 => Some(0),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn choose_resolution() -> Option<u32> {
    Some(1080)
}

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

#[cfg(target_os = "macos")]
pub(super) fn window_attributes() -> winit::window::WindowAttributes {
    use winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
    winit::window::Window::default_attributes().with_option_as_alt(OptionAsAlt::Both)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn window_attributes() -> winit::window::WindowAttributes {
    winit::window::Window::default_attributes()
}

#[cfg(target_os = "macos")]
pub(super) fn activate_application() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;
    if let Some(main) = MainThreadMarker::new() {
        // SAFETY: explicit viewer launch on the verified AppKit main thread.
        // Request application activation after its window exists; window focus
        // alone does not activate a background application on current macOS.
        let app = NSApplication::sharedApplication(main);
        // The modern request exists on macOS 14+. Preserve the advertised
        // older system floor without sending an unsupported Objective-C selector.
        unsafe {
            let modern: bool = objc2::msg_send![&*app, respondsToSelector: objc2::sel!(activate)];
            if modern {
                app.activate();
            } else {
                // Required compatibility path only when the modern selector is absent.
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn activate_application() {}
