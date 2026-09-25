# Sync protocol integrity receipt — 2026-09-25

Scope: W1.9, based on `e3930e9`; synthetic local Linux tests. No deployment,
owned-backend promotion or wave-close claim. The implementation contract is in
[sync-protocol.md](../sync-protocol.md).

## Baseline evidence

Four newly added real-transport tests failed before the engine change:

- A pull accepted an Offer for a different safe relative path.
- The pull-serving role accepted Done for a different manifest root.
- Need silently accepted missing/surplus words and ignored padding bits.
- An empty ManifestPart kept the receiver waiting instead of rejecting it.

A separate `rds-core` regression failed because an otherwise valid postcard value
with trailing payload bytes was accepted inside its declared frame. Private logs
retain both failing runs. Valid framing and message tags were not changed.

An additional corrupted-body/silent-peer case failed against the intermediate
working tree after the first protocol fixes: the blocking writer had rejected the
body, but collection waited for another network frame. The writer now signals
every exit through an owned one-shot channel. Collection wakes and retrieves the
storage or task error without waiting for the peer. The same test then passed,
and the full check matrix was repeated after this final behavior change.

## Change and coverage

The receiver binds a pull response before journal creation, registers its uni-stream
route before advertising Need and permits only requested unique indices. Header
index/length/hash must agree with the manifest. Both senders verify the completion
root. Need and batch sizes are exact and finite; empty batches/streams fail promptly.
The writer drains before complete verified journal state can be assembled. Identical
reuse now reports zero actual transferred bytes as well as zero fetched chunks.
All shared postcard frame readers reject trailing content.
The public Rust `bits_to_indices` helper now returns `Result`; library consumers
must propagate malformed-bitmap errors instead of receiving a truncated list.

Ten protocol integration tests pass locally. Path and Done-root cases run on both
iroh and owned noq; additional owned-transport cases cover malformed bitmaps,
oversized/empty manifest and chunk batches, repeated and unrequested indices,
incorrect header hashes, a corrupt body followed by silence, retained prior destination contents and eventual journal
lock release after failure. Positive coverage checks normalized requested names,
exact content and zero-byte reuse. Invalid local basenames and a zero session
budget fail before transfer work.

Absolute deadline tests exercise every role. Silent push/pull peers time out, and
a serving receiver consuming continuous valid manifest progress still reaches its
original deadline. Production defaults are one hour per session and five minutes
per stalled I/O operation; explicit library entry points accept another total
budget. These tests use short budgets to establish behavior, not performance targets.

## Final validation

Passed after the worker-exit correction: formatting; default, X11 and all-feature
workspace Clippy with warnings denied; **267 workspace tests across 50 targets**;
and **59 all-feature tests across 18 targets** for `rds-net`, `rds-agent` and
`rds-relay`. Existing normal/resumed/impaired transfers and concurrent desktop/sync
tests remained green. The workspace's one ignored test is the separate 4096-row
migration qualification, already executed for `e3930e9`; it was not repeated for
the subsequent sync-only changes. `cargo-deny` is not installed locally.

[Machine-readable final results](rds-sync-protocol-20260925-data.json) bind command
durations, test counts, toolchain and source fingerprints. The initial passing
matrix and the later body-corruption failure are retained separately in private
evidence; only the repeated matrix after the worker notification is the final
result. No wave checkpoint or performance improvement is asserted.

## Remaining scope

Per-transfer IDs and protocol/version/limit negotiation remain W2.2. The current
Sync route supports one live receive per connection; this change does not introduce
concurrent transfers routed by ID. Existing blocking disk work can outlive async
cancellation and may finish a complete replacement without a Done response.
Publication/cancellation barriers and complete resource accounting remain W2.5/W8.
Native macOS, physical storage failures and deployed mixed-version rollout still
require their own qualification. No new dependency or external runtime helper
was introduced.
