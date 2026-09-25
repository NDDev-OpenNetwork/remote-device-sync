# Local device connections and session ownership

Status: opt-in manager implemented, **W2.4 partial**. This does not close W2,
W5 or W6. See the [Linux receipt](reports/rds-local-sessions-20260925.md).

## Ownership and use

`rds-agent --control-dir <private-absolute-directory>` hosts outgoing sessions
beside inbound service. Both use clones of the **same bound endpoint**, identity,
relay registration and metrics registry. The manager never reads a key or binds
another UDP endpoint. The agent supervises and closes both services.

`rds-client` owns outgoing protocol operations and the local manager/client.
It depends on core/net/discovery/observe. Agent and CLI depend on it; the former
`rds-cli` library exports remain compatibility re-exports. No new third-party
dependency or external runtime program was added. Existing Tokio, rustix,
postcard, BLAKE3 and random-number libraries supply the underlying mechanisms.

Use a dedicated absolute control directory under private user state. Its parent
must exist; the leaf is created with mode 0700 when absent. Components must be
real directories owned by this user or root, without other-user write access.
A root-owned sticky ancestor such as `/tmp` is allowed. On macOS use canonical
paths, such as `/private/tmp` instead of the `/tmp` symlink.

```sh
# Merge the control flag into the local agent's existing configuration.
rds-agent --control-dir /absolute/private/rds-control <agent-options>

rds session --control-dir /absolute/private/rds-control connect <peer-ticket>
rds session --control-dir /absolute/private/rds-control connect <second-ticket>
rds session --control-dir /absolute/private/rds-control list
rds session --control-dir /absolute/private/rds-control use <session-id>
rds session --control-dir /absolute/private/rds-control ping
rds session --control-dir /absolute/private/rds-control info
rds session --control-dir /absolute/private/rds-control ssh -L 127.0.0.1:2222
rds session --control-dir /absolute/private/rds-control disconnect <session-id>
```

`list --json` returns instance ID, generation, endpoint, selected handle and
entries. Handles print as 32 hexadecimal characters. `ping`, `info`, `ssh` and
`forward` accept `--session <id>`. `connect --grant-file <path>` uses a grant
issued to **the local agent identity**; file and resulting frame must each fit
64 KiB. Session commands dispatch before identity/config loading and reject
direct endpoint/directory/grant options rather than silently ignoring them.

Tickets and pinned keys use the existing resolver. Agent `--directory` enables
signed-record lookup. Names additionally require `--registry-key`; optional
`--registry-epoch`, `--registry-state` and repeated `--registry-rotation` provide
durable trust/rotation. The trust-store default is beside the endpoint key.
Registry trust is distinct from incoming revocation authority. It loads on a
blocking worker before endpoint bind. The manager inherits the verified
resolver; unsigned client-supplied name-to-key mappings are not accepted.

SSH currently means an RDS-managed tunnel to the target sshd, used by an SSH
client at the printed loopback address. **An embedded terminal/PTY is not part
of this increment.** Listeners reject non-loopback addresses. Other local users
can reach a local TCP port: account authentication and host-key verification
remain SSH responsibilities. Unix control-socket UID checks do not authenticate
clients of the forwarded TCP port.

## Consistency and lifetime

- At most 32 sessions, including pending dials. Random 128-bit handles are
  process-lifetime values. Restart loses ephemeral sessions/selection, changes
  the manager instance ID and never restores a remote command.
- Snapshot generation changes on transitions. Short state transactions never
  hold a lock across resolution, dialing or service I/O.
- One session per peer. Repeated connect with the same credential fingerprint
  reuses a connected session without another dial/Authz. A pending duplicate
  returns `Busy`; different credentials require explicit disconnect. The table
  stores a digest, not the grant payload. Requests/grants/upstream errors are
  not logged or used as metric labels.
- Remote Authz validates managed admission; plain allowlist mode uses one Ping
  reply to establish readiness. TCP-only grants need no Ping permission. Existing
  remote scope, expiry and revocation enforcement continues to apply.
- The first successful connection is selected when selection is empty. `use`
  changes defaults for new operations; existing streams keep their peer. A
  forward pins a handle before listening, including for later accepted sockets.
- Disconnect closes that peer and its streams or cancels its pending dial. Peer
  loss clears a selected handle; no other device is silently substituted. The
  forward checks its pinned session once per second and exits after its loss or
  manager loss. A stalled control request can extend detection to its deadline.
- Dropping an IPC connect caller cancels the operation and reservation. Failed
  reply writes roll back new sessions. If a reply was fully written but receipt
  is uncertain, `list` can show the completed connection and repeated connect
  reuses it. This is not exactly-once RPC or automatic remote-command replay.
- Shutdown seals admission, closes sessions and joins workers. Dropping the
  server aborts owned tasks. Manager shutdown leaves a retained shared endpoint
  usable. Transport closure is reconciled before state operations and on the
  sampling tick. Removed sessions are not a durable history.

Selection is currently CLI state. Desktop focus and pressed-input release
remain viewer work; this manager does not claim those semantics.

## Local access and budgets

`control.sock` has mode 0600 in the private directory. Both endpoints compare OS
peer credentials to effective UID using Tokio's Linux/macOS adapter. The
dedicated directory is exclusively locked for the server lifetime. Regular
files/symlinks are refused. A dead owned socket is recovered only under that
lock, after a refused connect probe and inode recheck. Live listeners are not
unlinked. Cleanup removes only the socket inode created by this owner. Same-UID
processes are within the trust boundary; this is not a multi-user privileged
broker.

| Resource | Bound |
|---|---|
| Sessions, including pending | 32 |
| IPC workers | 96; acceptance pauses at capacity |
| Long-lived TCP streams | 64; leaves control worker space |
| Request prelude / reply write | 5 seconds each |
| Resolve + dial + Authz + operation | 45 seconds total |
| Client exchange, including connect/response | 55 seconds |
| Framed postcard control message | 64 KiB; trailing payload rejected |
| Target text | 8192 bytes |
| CLI forwarding workers | positive 16-bit limit; default 64 |

Wire types live in `rds-core::local`, with explicit request/response versions.
Remote `rds/0` is unchanged. There are no unbounded task/command queues; the OS
backlog is separate. TCP bodies do not inherit the prelude deadline and use the
existing reset/stop cancellation guard. Operations never automatically retry a
remote side effect.

Local TCP bodies use explicit bounded `Data` / `Finish` frames. A direction FIN
never half-closes the Unix socket, so caller departure remains unambiguous while
a remote TCP service is silent. The Rust AsyncRead/AsyncWrite adapter owns one
pump and a 16 KiB buffer per direction. Drop aborts its pump and closes IPC;
explicit close joins it. Write shutdown waits until queued upload bytes and its
Finish frame have reached IPC. Both clean FINs release QUIC streams normally,
preserving pending data; cancellation/errors reset them. Empty/oversized body
chunks and data after upload FIN terminate that body. The remote TCP byte stream
and remote RDS protocol remain unchanged. No OS hangup heuristic is required.

Authenticated admin scrapes add aggregate
`rds_agent_local_manager_{snapshot_available,connected,connecting,capacity}`.
The weak observer uses `try_lock`, marks unavailable explicitly and retains no
network resources. Outgoing samplers feed existing shared network counters.
The Vector admin allowlist already accepts `rds_agent_`; no pipeline change or
production deployment is implied.

## Remaining sequence and exit checks

1. **W2.4 migration:** manager by default for CLI/viewer/sync, compatibility
   migration, and exclusive runtime ownership even for independently launched
   processes using the same key. Direct commands still bind independently:
   do not run them with the manager identity. Qualify native macOS credentials,
   actual distinct-user rejection, relay-registration reuse, crash/restart,
   FD/RSS budgets and manager service APIs for media/sync.
2. **W5 SSH:** maintained Rust SSH library behind an owned interface; host-key
   pinning/known-hosts, credentials/agent policy, raw-mode restoration, PTY
   resize, Ctrl-C, exit status and account/broker isolation. Verify two terminals
   while switching and failure without duplicate exec.
3. **W6 viewer:** newest-frame rendering and focus ownership; release pressed
   keys/buttons before changing devices. Reuse manager service APIs. Qualify
   native Linux backends and macOS capture/input permissions on real devices.
4. **GDS and operations:** enrollment/inventory, issuer/renewal/reconciliation,
   managed service leases, diagnostic bundles and private alert delivery.
5. **Release acceptance:** LAN/WAN, NAT, blocked UDP, relay loss, suspend/resume,
   revocation under load, simultaneous SSH/desktop/sync, long runs and measured
   setup/p99/input latency. Keep iroh default until owned parity is demonstrated.

Each step needs code, failure tests, platform evidence and an updated receipt.
All remediation waves remain open. This sequence preserves every other task in
the [full remediation plan](remediation-plan.md).
