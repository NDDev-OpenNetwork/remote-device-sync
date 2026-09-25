# Owned relay control and grace — 2026-09-25

Scope: audit R08/T03, W3.3 framing/grace and local W2.5/W2.6 control ownership,
final base `5d07dcb`. Synthetic Linux loopback evidence; no deployment, backend
promotion or remediation wave closure.

## Defects and changes

Two actual-client regressions failed before the fix: the relay socket never
observed Drain, and dropping it left its outer attachment retained by pumps.
Server Drain/PeerGone wrote raw postcard bytes while readers expected a u32
length prefix. Client registration additionally clamped oversized frame lengths
and accepted a valid postcard prefix with trailing data.

Both ends now use one exact 1–4096-byte control codec. Registration has a whole
15-second budget on each side. Control notices have owned task groups with at
most 16 writers per broadcast and a one-second lock/write budget. A canceled or
failed partial write closes its connection instead of corrupting later framing.
Server control and forwarding share the same connection future.

Drain is observed before shutdown and existing socket traffic continues through
the two-second grace. New registration is checked again under the attachment
lock. Stale detachment cannot remove replacement history or emit false PeerGone;
flow history cannot be recreated after detach. Socket Drop closes the tunnel and
aborts pumps; explicit socket close joins them. See [contract](../relay-control.md).

## Validation

Real tests cover actual Drain receipt, socket-drop detachment, datagrams during
grace, same-key replacement followed by real PeerGone and a following Pong,
oversized registration refusal, writer-lock deadline and early cancellation.
Codec tests cover all variants, fragmentation, coalescing, invalid lengths,
truncation and trailing bytes. Focused network/relay validation passed 26 tests
across four targets. Existing owned relay handshake/stream tests remain intact.

The initial full matrix exposed an independent G6 repeated-kill journal-lock
refusal. Queued-store cancellation and bounded recovery-test convergence were
corrected in [the sync increment](rds-sync-cancel-20260925.md), commit `5d07dcb`.
The initial failed run remains in private evidence; the matrix below was rerun
against the corrected combined source. It does not hide or count that failed run.

Formatting and default/X11/all-feature Clippy passed with warnings denied.
The final workspace passed **329 tests across 59 targets**
(one previously qualified migration-capacity test ignored). All-feature
network/agent/CLI/relay passed **118 tests across 30 targets**;
feature-isolated codec validation passed **3 tests**.
[Machine-readable evidence](rds-relay-control-20260925-data.json) records
commands, durations, counts and source hashes. Full logs stay in private evidence.
Existing noq becomes a test-only direct relay dependency; no package/version,
wire tag, unsafe code or runtime helper was added. `cargo-deny` is unavailable.

## Still open

R08's missing-notice reproduction is corrected; T03/W3.3 remain partial for warm
replacement and measured SSH/video/sync migration. A sole relay path still ends
when its relay shuts down. Receive-queue and peer-map bounds, global attachment
admission, server accept ownership, joined server shutdown, physical failure and
native macOS qualification remain open. Notice limits are per broadcast, not a
global concurrency guarantee. No wave or production acceptance is closed.
