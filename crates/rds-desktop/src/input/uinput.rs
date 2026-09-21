//! `/dev/uinput` virtual input device — planned.
//!
//! Creates a virtual keyboard/pointer via `evdev`/`input-linux` ioctls.
//! Privileged path for unattended and headless sessions; requires the
//! agent to run with uinput access. Not for attended sessions where the
//! compositor permission model applies.

/// Always `false` until implemented.
pub fn available() -> bool {
    false
}
