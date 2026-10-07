# Owned sync test scratch — 2026-10-07

The v1 and negotiated-session integration fixtures created unique temporary
roots and returned only PathBuf. Those roots survived test completion and
accumulated synthetic transfer files across repeated local qualification.

A shared test-only Scratch owner now removes each exclusive root when its last
Arc owner drops. Serving tasks retain a clone while awaiting asynchronous engine
work; the root survives cancellation/worker ownership and is released when the
fixture runtime shuts down. Paths joined under the root remain ordinary paths;
production sync, journal durability, metadata and conflict semantics are unchanged.
No directory is selected for cleanup by matching a pre-existing name.

The lifetime regression checks retained worker ownership, last-owner removal and
symlink confinement: removing an owned fixture does not remove its outside alias
target. Both complete integration suites passed locally on macOS (24 tests total),
including cancellation, corrupt parts, torn journals, resume, repeated connection
kills, path substitution and impaired transfer. Strict Clippy and supported CI
remain required before merge. Historical scratch directories and failed incident
evidence are not silently deleted by this change.

The relevant [Arc ownership semantics](https://doc.rust-lang.org/1.98.1/std/sync/struct.Arc.html)
and [remove_dir_all symlink behavior](https://doc.rust-lang.org/1.98.1/std/fs/fn.remove_dir_all.html)
are established standard-library behavior. This uses no new dependency or
background cleanup daemon and makes no process-kill/power-loss cleanup guarantee.
