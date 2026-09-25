# Agent-owned local sessions — Linux implementation receipt

Date: 2026-09-25. Baseline: `461c4ebdd70c3057fe99b008df32668ccedbf4c5`.
Scope: W2.4 partial, with W2.5/W2.6 lifetime and W10 aggregate observations.
No wave closure, deployment, default-backend promotion or latency claim.
Contract, usage and remaining sequence: [local sessions](../local-sessions.md).

## Delivered behavior

- The opt-in local manager uses the agent's already-bound endpoint for multiple
  outgoing peers. CLI session commands load no key and bind no UDP endpoint.
- Shared request/forwarding operations move to `rds-client`, below both agent
  and CLI; old library exports remain compatible. No new third-party library,
  dependency version, external runtime program or remote ALPN was introduced.
- Versioned, length-bounded Unix IPC provides connect/list/select/disconnect,
  Ping/Info and transparent framed TCP bodies. Both endpoints check effective UID; private
  directory/socket checks, exclusive directory ownership and inode-aware stale
  recovery protect the local namespace.
- Pending and connected state is bounded, with per-peer credential-aware reuse,
  cancellation, random handles, instance/generation snapshots and no implicit
  command replay. Selected-device changes leave existing streams/listeners
  pinned to their original peer.
- Incoming and outgoing connections share the endpoint without transferring
  endpoint-close responsibility to the manager. Normal close joins children;
  drop cancels owned children; grant scopes and revocation remain enforced.
- Agent registry trust options feed the existing verified directory resolver.
  Aggregate manager metrics join the authenticated admin endpoint, with no
  peer/session labels. Existing Vector filtering accepts these agent metrics.

## Functional and regression evidence

`rds-agent/tests/local_manager.rs` exercises real IPC, QUIC and TCP, not a mock
manager. The multiple-device and pending-dial scenarios run with iroh and owned
noq when enabled. Coverage includes connection reuse without a new dial,
selection and explicit-session routing, listener pinning even for sockets
accepted after switching, peer closure, restart handle fencing, cancellation,
credential conflict, repeatable shutdown, and a retained shared endpoint still
usable after manager shutdown.

Additional cases verify regular-file/symlink/loose-directory refusal, concurrent
owners, preserving a live unrelated listener and replacement inode, dead-socket
recovery, wrong protocol version, oversized frames and unsafe socket mode.
A TCP-only signed grant connects without Ping permission; Ping remains denied
and revocation closes the held TCP stream. Real agent subprocess coverage checks
control preflight before key creation, identity agreement, outgoing service and
SIGTERM cleanup. The CLI subprocess reads the running manager's identity and
creates no default key/config directory; inappropriate direct options fail.

A new failure regression filled all 64 TCP slots, dropped every local client,
and retained silent peer TCP sockets. Before correction it failed with
`abandoned IPC bodies retained all 64 stream slots`. A read EOF only half-closes
`copy_bidirectional`; its other direction could remain parked indefinitely.
A readiness-only correction was rejected after the reverse-half-close regression
lost a late upload. The final local protocol carries explicit bounded Data and
Finish frames. Byte-direction FIN never closes IPC; caller disappearance can
therefore cancel a silent body without guessing from OS readiness. The owned
AsyncRead/AsyncWrite adapter waits for upload FIN publication on shutdown.
Normal completion preserves queued QUIC bytes; cancellation resets streams.

The regression proves delayed data survives half-close in both directions,
multiple chunks cross bounded queues intact, malformed body chunks are rejected,
capacity exhaustion leaves control responsive, and caller drop frees stream slots.
All eight manager integration tests passed with the explicit-FIN protocol.

Core property tests exercise arbitrary local request/response bytes and handle
strings. TCP destinations deserialize through the existing validated canonical
parser. State tests cover pending-session capacity, release, stop and nonblocking
weak metric observation under contention.

## Final verification

Final results and source/log hashes are recorded in the accompanying
[machine-readable receipt](rds-local-sessions-20260925-data.json).

All six checks passed with unchanged Rust/Cargo input hashes, using two Cargo
build jobs and an isolated existing build cache.

| Check | Result |
|---|---|
| `cargo fmt --check` | passed |
| Workspace all-target Clippy, default | passed with warnings denied |
| Workspace all-target Clippy, X11 | passed with warnings denied |
| Workspace all-target Clippy, all features | passed with warnings denied |
| `cargo test --locked --workspace` | 484 passed, 2 ignored; 84 result targets |
| Agent/CLI/client/core tests with all features | 96 passed, 0 ignored; 20 result targets |

Expanded checks overlap the workspace run; their counts are not additive.
The receipt records exact commands, source SHA-256 values and private-log hashes.
No test log, device identity or local credential is published.

## Qualification limits

- Native macOS tests were not run here. The host has no usable Apple SDK;
  earlier cross-checks failed in `ring` before RDS. macOS credential, directory
  locking, stream lifetime and signal behavior still need the native CI/device
  lane. Linux results do not substitute for it.
- Permission/credential code was exercised with real same-UID sockets; an actual
  second-UID operating-system scenario remains unqualified. This is a per-user
  daemon, not an account-isolation or privileged OS broker.
- Tests cover multiple direct loopback peers, not WAN/NAT/relay-registration
  continuity, physical network loss, sleep/wake, long-run resource curves or
  changed latency. No new rds-bench thresholds or performance evidence.
- Existing verified name-resolution suites remain in the workspace. The new
  agent trust configuration still needs private GDS enrollment/name/grant flow
  qualification. Session lists are transient connections, not estate inventory.
- No Vector/OpenObserve pipeline re-run, deployment or delivered production
  alert was performed for this increment. `cargo-deny` is unavailable here.
- Direct compatibility commands still own separate endpoints; migration and
  exclusive cross-process ownership of a key remain open. Manager IPC does not
  yet serve desktop/media/sync. SSH is still a tunnel to sshd, without an embedded
  terminal. Rendering/native backends and remaining W0–W10 work are unchanged.

This receipt establishes the implemented opt-in increment only. The next
sequence remains ownership migration, native SSH/PTY, real viewer/input switching,
GDS lifecycle and device/network/operational acceptance.
