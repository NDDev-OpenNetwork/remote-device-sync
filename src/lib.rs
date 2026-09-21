//! Remote device access for the GDS estate.
//!
//! `remote-device-sync` will provide SSH-based connectivity and remote desktop
//! sessions between devices, brokered through the GDS server, so any enrolled
//! device can reach another. The transport and session contracts are still
//! being designed; this crate currently carries only the skeleton.

/// Transport channel under evaluation for device-to-device sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// SSH tunnel carrying the session.
    Ssh,
    /// Remote desktop protocol on top of the SSH channel.
    Rdp,
}

/// Returns the transports the initial design is evaluating.
pub fn transports() -> &'static [Transport] {
    &[Transport::Ssh, Transport::Rdp]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_is_always_available() {
        assert!(transports().contains(&Transport::Ssh));
    }
}
