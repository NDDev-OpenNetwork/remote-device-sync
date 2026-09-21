//! Portal/libei input injection — planned.
//!
//! `ashpd` RemoteDesktop → `ConnectToEIS` hands us the EIS socket; `reis`
//! (pure-Rust libei/libeis) speaks the wire protocol. The portal drops
//! out of the data path after setup — events go straight to the
//! compositor with per-session scoping.

/// Always `false` until implemented.
pub fn available() -> bool {
    false
}
