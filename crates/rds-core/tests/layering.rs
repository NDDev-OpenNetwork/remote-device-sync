//! `docs/conventions.md` declares `rds-core` a leaf: "no io, no async
//! runtime, no platform code". Async frame I/O moved to
//! `rds-net::wire` under W2.7; this test keeps the manifest honest —
//! a runtime or transport dep edge here would silently re-break the
//! contract in every downstream service crate.

/// Dependency names that must never appear in rds-core's manifest.
/// Transports, async runtimes, and platform adapters all live
/// upstream of this crate.
const FORBIDDEN_DEPS: &[&str] = &[
    "tokio",
    "iroh",
    "iroh-base",
    "iroh-relay",
    "noq",
    "noq-proto",
    "russh",
    "rustls",
    "turmoil",
    "x11rb",
    "wayland-client",
];

#[test]
fn rds_core_manifest_has_no_runtime_or_backend_dependencies() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("Cargo.toml readable");

    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .expect("dependencies section")
        .split('[')
        .next()
        .expect("section body");

    for line in deps.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split('=').next().unwrap().trim();
        assert!(
            !FORBIDDEN_DEPS.contains(&name),
            "rds-core must stay a leaf: `{name}` is a runtime/backend dep"
        );
    }
}
