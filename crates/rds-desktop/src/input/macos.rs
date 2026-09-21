//! macOS input injection — planned.
//!
//! `CGEvent` posting via `core-graphics`: keyboard, pointer and scroll
//! events into the console session. Requires accessibility/Input
//! Monitoring TCC permission for the agent.

/// Always `false` until implemented.
pub fn available() -> bool {
    false
}
