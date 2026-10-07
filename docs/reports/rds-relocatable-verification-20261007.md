# Verification fixtures across isolated checkouts — 2026-10-07

The core layering test embedded `CARGO_MANIFEST_DIR` as an absolute runtime
filename. Reusing its compiled artifact after that checkout was removed failed
with `Cargo.toml readable: NotFound`; this did not establish an invalid core
dependency graph. The failing verification is retained separately.

Embed the tested manifest with `include_str!("../Cargo.toml")`. The dependency
assertions remain mandatory, and the compiler tracks the included input. The
existing test passes after the correction. Its actual compiled artifact also
passes when invoked from an empty directory with a nonexistent runtime
`CARGO_MANIFEST_DIR`, without reading a source checkout.

The explicit Linux observability infrastructure fixture needs live Compose
files instead. Resolve Cargo's runtime manifest directory there, rather than
a removed compilation path. This changes fixture location handling; it does
not claim a new Docker/pipeline qualification. Full platform CI remains the
broader verifier.

This is a verification-lifecycle correction with no transport or wire change.
The [standby measurements](bench-standby-20261007.md) remain scoped transport
evidence; they do not measure this fixture behavior or close a product gate.
[Rust's `include_str!` documentation](https://doc.rust-lang.org/std/macro.include_str.html)
defines compile-time file inclusion relative to the source file. Cargo's
[environment contract](https://doc.rust-lang.org/cargo/reference/environment-variables.html)
distinguishes values expanded during compilation from the running test's
environment. Both mechanisms predate the requested research boundary.
