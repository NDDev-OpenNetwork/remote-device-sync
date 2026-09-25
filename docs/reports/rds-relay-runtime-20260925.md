# Shared relay binary runtime — 2026-09-25

Scope: W3.2, with W2.1 configuration and W2.5 composition; base `a4a2ad2`.
Linux loopback fixtures, separate synthetic identities/state, no live changes.

Both `rds-relay` and `rds-server` previously exposed only iroh server startup.
They now share backend selection, validation, identity initialization, bind and
awaited shutdown. Iroh remains the default. Owned mode requires its build
feature, separate persistent key and peer allowlist; open admission requires an
explicit development flag. A positive connection cap includes incomplete
handshakes/registrations. Wrong-backend and unused TLS flags fail explicitly.

Relay PEM is parsed once with bounded reads before state creation. The host also
preflights authority/registry/rotation and directory TLS inputs before identity
or catalog initialization. Durable policy still validates against its current
rotated authority when opened. Valid keys survive later startup failure. The
host joins both service shutdowns; the owned relay's checked drain errors reach
process failure. See [configuration and compatibility](../relay-runtime.md).

**17 owned-feature and 13 default-feature focused real-binary tests passed.**
They exercise mutual inner authentication, bidirectional stream exchange and a
datagram on a single relay path, unknown-peer denial before capacity is filled,
cap recovery, development admission/drain, restart identity persistence, host
signed publication/resolution and catalog reuse. They also exercise preflight
failures without state side effects and iroh manual TLS startup/shutdown. The
TLS fixture checks listener startup, not a new TLS interoperability campaign.

Formatting (including shared include code) and normal/X11/all-feature workspace
Clippy passed. Workspace: **413 tests across 72 targets**,
1 existing ignored test. All-feature net/relay/agent/CLI/server:
**216 tests across 43 targets**. See
[commands, durations and source hashes](rds-relay-runtime-20260925-data.json).
The initial compile/lint refinement boxed large runtime enum fields; no focused
functional test failed. CI now includes the owned-server feature on both OSes;
local execution was Linux only. No new dependency version or helper daemon was
introduced. `cargo-deny` is unavailable locally.

Library endpoints are the fixture clients. This does not establish full real
agent/CLI SSH, desktop or sync acceptance, external reachability, native macOS,
UDP-blocked fallback, warm replacement, supervision or performance targets.
Per-source malformed-datagram accounting still needs correction. W3.2 and every
wave remain open pending the remaining acceptance work.
