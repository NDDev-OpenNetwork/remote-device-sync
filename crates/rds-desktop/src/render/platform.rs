//! Native application activation is explicit for CLI and packaged launches.
use winit::event_loop::EventLoop;

#[cfg(target_os = "macos")]
pub(super) fn event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    EventLoop::with_user_event()
        .with_activation_policy(ActivationPolicy::Regular)
        .build()
}

#[cfg(not(target_os = "macos"))]
pub(super) fn event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    EventLoop::with_user_event().build()
}
