//! Ambient input probing cannot acquire local consent. The optional Linux
//! `WaylandDesktop` owner prepares RemoteDesktop/ConnectToEIS once and lends
//! scoped input leases. It never installs a global fallback input registry.

/// No ambient portal input exists without the explicit prepared owner.
pub fn available() -> bool {
    false
}
