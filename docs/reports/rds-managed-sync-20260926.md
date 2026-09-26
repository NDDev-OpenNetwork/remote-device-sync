# Managed single-file transfers — 2026-09-26

Scope: W2.4 manager APIs and the file-transfer part of W2.2 route isolation.
**All remediation waves remain open.** This receipt is local Linux validation,
not installed-device, native macOS or real-network release acceptance.

## Implemented contract

- `rds send/recv` and `rds session send/recv [--session <id>]` reuse the running
  agent's endpoint, authenticated connection and pinned device handle. The CLI
  does not load/create another identity or bind another endpoint.
- The manager depends on the existing lower-layer `rds-sync` engine. No new
  third-party package, external transfer program or shell helper was added.
- Local IPC v4 explicitly delegates caller-resolved absolute UTF-8 filesystem
  paths across the existing same-UID boundary. Local path text is bounded at
  8192 bytes. Remote confinement, directional grants, journal ownership,
  content verification and atomic destination replacement are preserved.
- A fresh 128-bit ID is carried in appended `SyncTransfer` control/uni tags.
  Legacy tag numbers and explicit-direct behavior remain compatible; old
  agents reject the new greeting before file I/O, without automatic downgrade.
- Eight active outgoing transfers per manager and one per session refuse excess
  work immediately. The uni router bounds live routes at 32 and removes dropped
  registrations instead of accumulating past transfer IDs.
- IPC cancellation resets this transfer's control streams, drops its chunk
  workers/inbox and leaves the session and unrelated TCP streams usable. Late
  tags cannot be routed to a different transfer ID. Successful completion uses
  the verified Done exchange plus control FIN completion.
- Safe `sync_send` / `sync_recv` operation duration and outcome records are
  accepted by the Rust JSON and Vector allowlists. Paths, IDs, file contents,
  credentials and upstream error text are not exported in these records.

## Validation

Local Linux x86_64, Rust 1.98.1; two build jobs, one Cargo operation at a time.
Synthetic loopback peers and disposable state only. Existing installed agents
were not replaced or restarted.

- `cargo fmt --check` — passed.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
- Clippy with `rds-desktop/x11` — passed.
- Clippy with agent/CLI desktop and agent `transport-noq` — passed.
- `cargo test --workspace` — passed, including legacy sync protocol/commit,
  manager lifecycle, native SSH and real CLI process tests.
- `cargo test -p rds-agent --features transport-noq --test local_sync` — three
  scenarios on both iroh and noq: pinned transfers/device selection and exact
  file bytes; cancellation/busy refusal with retained TCP and same-connection
  recovery; read-only/write-only/missing-sync grant enforcement.
- `cargo test -p rds-net --features transport-noq --test uni_lifecycle` — six
  tests passed, including delayed old tags after inbox replacement, live-route
  saturation, historical-route retirement and connection/router cleanup.
- The core wire fixture fixes existing tag numbers and verifies that legacy
  greeting/tag decoders refuse the extension. Existing IPC tests cover version
  mismatch without state mutation.
- Pinned Vector 0.58.0 container: all eleven configuration tests passed, including
  both new operation names with private-path fields stripped. This run used an
  isolated network and synthetic credential; it did not activate a deployment.
- `cargo deny` was unavailable locally. No third-party dependency changed;
  supply-chain CI remains an independent check.

The full regression run caught a legacy completion race: a normal peer close
could win the new cancellation watcher after sending Done. The watcher and FIN
handshake are now specific to the new tagged extension, preserving legacy
completion behavior; the original regression and full workspace tests passed.

## Supporting transport measurement

The existing `rds-bench` transfer scenario ran on clean source
`278d6d4bf456f449b5b1aa12be6064d1da824147` with both backends, debug profile,
8 MiB over a synthetic direct loopback path:

| Backend | Harness throughput | Evidence |
|---|---:|---|
| iroh | 8.42 MiB/s | [JSON](bench-managed-sync-20260926-iroh.json), [report](bench-managed-sync-20260926-iroh.md) |
| noq | 13.37 MiB/s | [JSON](bench-managed-sync-20260926-noq.json), [report](bench-managed-sync-20260926-noq.md) |

Reproduce with `cargo run -p rds-bench --features transport-noq -- run
--scenario transfer --iterations 3 --transfer-mib 8 --backend <iroh|noq>`.
This scenario transfers one bulk TCP stream; it does not use `iterations` to
repeat the throughput sample. These are single local samples, not managed-file
commit throughput, latency percentiles, a backend ranking or WAN acceptance.
The managed file/session correctness evidence is the integration suite above.

## Remaining boundaries

Upgrade local CLI and agent together for IPC v4; managed transfers require a
remote agent with `SyncTransfer` support. The published 0.1.0 preview is unchanged.
Files remain single-file transfers, not a directory reconciliation engine.
Blocking filesystem calls already running can finish after cancellation, and a
started commit or lost success reply is uncertain. There is no automatic replay
or rollback; transient remote busy refusal can persist during cleanup. Active
transfer limits do not establish a global bound on already-running disk syscalls.

Viewer manager APIs, interactive rendering/input focus, automatic GDS issuance
and renewal, update/rollback, native platform qualification, WAN/NAT/relay and
mixed-workload measurements remain in the full remediation plan. No checkpoint
script is invoked because this increment does not close a registered wave.
