# Terminal sync store ownership — 2026-10-11

Scope: W1 store cancellation and the registered w1-sync-supervision gate.

[PR156 CI 38087649713](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/38087649713)
at `274a340` failed Ubuntu protocol_safety: scoped_offer_cannot_reuse_another_destinations_cached_payload reopened private/file after its refused shared transfer returned, but Journal::open reported WouldBlock. The failure and its source are retained; it was not the control 400ms gate. Earlier successful CI/cohorts do not erase it.

The chunk-store sink lived inside the future selected against terminal control. When Refuse won, dropping that future requested cancellation but detached its spawn_blocking join handle. A running filesystem worker cannot be aborted, and it could still own the root lock or return a locked Journal while the receive future had already reported termination. The lock itself correctly refused competing ownership.

The parent receive now retains that sink across control selection. Explicit terminal control/receive errors stop the bounded queue and join the exact writer under a five-second cleanup budget, dropping returned Journal output before exposing completed store cancellation. A canceled drain await retains its mutable join handle. If cleanup fails, the original transfer cause and incomplete-cleanup error both remain. Queue bounds, cancellation barriers, path-scoped state, byte/hash checks and transfer deadlines are unchanged.

A deterministic regression holds a real locked Journal inside an already-running worker. Cleanup remains pending and the root remains exclusively locked while the simulated syscall is held; releasing the worker permits one immediate reopen, without sleeps or retries used as a cleanup substitute. Existing queued-store/finish cancellation and normal drain tests remain. The full sync suite passed 127 executions, including the original real-transport scoped case on Iroh/Noq, malformed-input tests, resume/impairment, and journal/process regressions; strict sync Clippy passed. Registered wave qualification follows at the committed source.

This is a store-worker completion boundary. Async-waiter drop, abandoned assembly and a stuck filesystem syscall still cannot promise immediate physical cancellation or unconditional lock release. The existing cooperative barriers retain safe ownership until their actual work ends; no lock is forcibly deleted or reused.

Primary references: [Tokio spawn_blocking](https://docs.rs/tokio/1.48.0/tokio/task/fn.spawn_blocking.html) documents non-abortable started work; [JoinHandle](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html) documents detachment, cancel-safe mutable awaiting and destructor completion before a joined result. Current contracts: [sync journal](../sync-journal.md#receive-cancellation), [sync protocol](../sync-protocol.md).

Registered w1-sync-supervision completed at signed 04ce32d: fmt, strict
workspace Clippy, workspace and sync tests (1084 passed executions,
zero failures), plus 100 samples in each of four journal preparation cases.
[Benchmark](bench-w1-sync-supervision.md) and [checkpoint](checkpoint-w1-sync-supervision.md)
retain source/tree/toolchain and the hash-chained receipt. An initial local
qualification attempt failed at native Opus build because a reused CMake cache
changed compiler and discarded its Cargo-local install prefix. The failed output
was preserved and only that output tree was regenerated; no elevated install,
test skip or threshold change was used. PR156 Linux/macOS CI at 04ce32d also
passed. This increment is qualified at its test boundary, not physical power
loss or installed fleet acceptance.
