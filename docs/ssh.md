# Native SSH client and standard PTY

Status: **W5.1/W5.3 partial**, with local Linux regression and OpenSSH
interoperability coverage. This is not a native SSH server, reconnectable PTY
broker, account-policy integration or release qualification.

## Decision and boundary

RDS owns connection selection, authorization, host trust, cancellation and the
operator experience. `rds-ssh` uses **russh 0.63.3** for the SSH protocol and key
formats; it accepts an async byte stream and owns no RDS endpoint or listener.
`rds-cli` supplies either a pinned manager TCP stream or an explicitly separate
direct connection. SSH does not need another relay or Cloudflare tunnel layer.

PTY allocation and resizing use RFC 4254 requests. The remote SSH server asks
its OS for a real PTY, running the authorized account's shell/command. The local
Unix adapter uses safe rustix termios/readiness APIs. Neither a custom terminal
emulator nor a custom SSH/cryptographic implementation is appropriate here.

The current remote adapter requires an already configured SSH server, normally
OpenSSH. It retains that server's host keys, account isolation and authentication
policy. This is an explicit interoperability dependency on the controlled device;
the Rust RDS client never launches `ssh`, `sshd`, `stty` or a shell helper.
A future Rust account/session broker must reuse SSH and OS PTY libraries and
prove privilege separation before replacing this boundary. W5.2 remains open.

The new Apache-2.0 russh dependency disables default features, enabling only
the existing `ring` cryptographic backend. Compression, RSA and DSA features
are off. `ring` includes native crypto/assembly, as it already does for RDS TLS;
this change does not claim a Rust-only transitive/native-object profile.
The exact version includes upstream PTY encoding, sensitive-debug redaction,
KEX/parser fixes and stalled-write timeout enforcement. Sources:
[upstream releases](https://github.com/Eugeny/russh/releases),
[0.63.3 timeout fix](https://github.com/Eugeny/russh/releases/tag/v0.63.3),
[SSH connection protocol](https://www.rfc-editor.org/rfc/rfc4254).

## Use and migration

Provision the remote server's **public** host key through an independently
trusted administration/enrollment channel. Do not learn it from an unverified
connection. Public and private files below are local operator files; none
belong in this public repository.

```sh
# Interactive shell; uses the running local agent's identity and connection.
rds ssh <device-or-ticket> --user <account> \
  --host-key /private/trust/device-host.pub --identity /private/keys/operator

# Explicit key in an existing SSH_AUTH_SOCK agent, including encrypted keys.
rds session ssh --user <account> --host-key /private/trust/device-host.pub \
  --agent-key /private/keys/operator.pub

# Exact command string, separate stdin/stdout/stderr, no PTY by default.
rds ssh <device-or-ticket> --user <account> \
  --host-key /private/trust/device-host.pub --identity /private/keys/operator \
  --exec 'printf "hello\n"; exit 23'

# Pin a connection even if the selected device changes concurrently.
rds session ssh --session <session-id> --user <account> \
  --host-key /private/trust/device-host.pub --identity /private/keys/operator

# The previous `rds ssh ... -L ...` behavior is now explicitly `forward`.
rds forward <device-or-ticket> -L 127.0.0.1:2222 --remote 127.0.0.1:22
```

`--remote` defaults to `127.0.0.1:22` on the controlled device and remains
constrained by its RDS TCP policy/grant. `--pty` requests a PTY for an exec;
`--no-pty` disables allocation, including for a shell. PTY mode requires terminal
stdin and stdout. `--exec` is one SSH command string interpreted by the remote
account's shell, not a reconstructed/implicitly quoted local argv.

`--direct --key-file <separate-rds-identity>` supports the same SSH options and
retains the existing exclusive endpoint-key ownership requirement. The SSH
authentication key is separate from the RDS endpoint identity.

This changes the meaning of `ssh` in both CLI command families: the old `-L`,
`--bind` and `--max-connections` options belong to `forward`. Local IPC is now
version 3 after the later [grant renewal increment](grant-leases.md); OpenTcp
continues to supply the SSH stream. The SSH engine itself introduces no remote
RDS/relay framing change.

## Trust and authorization

- The manager resolves/authenticates the RDS peer and pins a random session ID
  before OpenTcp. Selection changes never redirect an existing SSH stream.
  Manager restart cannot reuse the old handle.
- `--host-key` is an explicit per-invocation pin for that peer **and remote TCP
  target**. The SSH KEX signature and public key must verify before sending
  user authentication. Comments in public-key files are not key identity.
  There is no localhost alias, implicit known_hosts lookup, TOFU prompt or
  automatic replacement of a changed key. Rotation currently requires explicit
  out-of-band reprovisioning; GDS-signed host-key bindings remain work.
- Ed25519 and ECDSA keys are supported for server and user authentication.
  Key exchange offers ML-KEM768+X25519 and Curve25519, with strict-KEX extension
  negotiation. Ciphers are ChaCha20-Poly1305 and AES-256-GCM. Host certificates,
  RSA, passwords, keyboard-interactive/MFA and authentication certificates are
  unsupported in this increment; failure never weakens host verification.
- An explicit `--agent-key` selects exactly one local agent key. The client
  does not enumerate keys, fall back to other credentials, or forward the agent.
  All recognized server-initiated forwarding/session/X11 channels fail closed;
  unknown channel types are rejected by the library.
- Key files are bounded to 64 KiB, regular, opened nonblocking and without
  following a final symlink. Private keys require the current owner, one link
  and mode 0600/0400; public pins require current/root ownership and no group or
  other write permission. Raw file contents use zeroizing buffers. Parent path
  administration and other same-UID processes remain part of the local trust
  boundary, as for the manager.
- RDS currently grants access to the configured SSH TCP socket. SSH account
  authorization remains the remote server's responsibility. This is not yet a
  distinct GDS PTY/account/command capability.

## Terminal, flow control and lifetime

The client awaits positive PTY and shell/exec acknowledgements. It forwards
bytes without decoding UTF-8, samples RFC terminal modes before raw mode,
coalesces SIGWINCH sizes, sends stdin EOF, and drains both output streams until
channel close. Exit-status does not truncate later output. A command exiting
before consuming stdin retains its status/output. Missing status is a failure;
remote statuses 0–255 are preserved, larger values map to 255. Known exit signals
map to conventional 128+signal codes; unsupported names map to 255.

Typed Ctrl-C/Ctrl-Z travel as terminal input for the remote PTY to interpret.
Externally delivered INT/TERM/HUP/QUIT/TSTP cancel the local session and restore
termios and descriptor flags; local suspend/resume is not implemented. RAII
also restores on returned errors and unwinding. No process can promise local
restoration after SIGKILL, power loss or destruction of the terminal itself.

TTYs and pipes use cancellable nonblocking AsyncFd I/O, avoiding Tokio's
uncancellable blocking stdin reader. All original standard-descriptor flags
are saved before mutation because they may share an open file description.
Regular files and `/dev/null` use Tokio file I/O; filesystem stalls retain the
usual OS/blocking-I/O limitations.

Application chunks are 16 KiB; the advertised receive window is 256 KiB, maximum
packet size 32 KiB, and per-channel message queue eight entries. A bounded
512 KiB buffer per direction separates the library runner from the actual
transport. This lets two SSH peers flush concurrently without the deadlock
reproduced using tiny intermediate buffers. Tests echo more than a window over
a 4 KiB underlying stream. These settings are not a global RSS/FD bound or a
measured latency optimum; library-internal and hostile-peer budgets still need
broader qualification.

KEX+authentication (including agent signing) share a 30-second deadline;
channel/PTY/command setup has another 30-second budget. EOF/exit reporting starts
a bounded close wait. Keepalives every 15 seconds, a three-miss threshold and a
90-second inactivity limit distinguish idle responsive shells from dead peers.
Explicit shutdown waits at most two seconds per disconnect/join stage, then
closes and joins the owned transport bridge. Dropping setup or the running
session aborts that bridge even if the upstream protocol task has not returned.

SSH operations open individual TCP streams on a retained managed QUIC connection.
Canceling one does not disconnect the selected device or another SSH session.
No reconnect loop, command replay, process-survival guarantee, scrollback store
or reattachment is claimed. Existing authorized server facilities can keep a
process alive, but RDS does not provision or manage them here.

## Observability and remaining gates

Fixed `ssh_connect` and `ssh_session` operations export outcome and elapsed time
through the existing Rust/Vector allowlists, with no key, user, command, path or
terminal contents. **SSH with `RDS_LOG_FORMAT=json` requires `--log-file`**:
remote stderr is arbitrary program output and could impersonate a log envelope.
Only the separate telemetry file may enter the collector; never collect SSH
stdout/stderr as RDS telemetry. The client rejects JSON SSH without that separate
sink before connecting, and tests inject a syntactically valid forged event to
verify it stays exclusively in remote stderr.

```sh
RDS_LOG_FORMAT=json rds --log-file /private/logs/run-001.jsonl ssh <device> \
  --user <account> --host-key /private/trust/device-host.pub \
  --identity /private/keys/operator --exec 'printf "hello\n"'
```

The path must be absolute, in an existing 0700 directory owned by the current
user. Descriptor-relative traversal rejects symlinks/untrusted ancestors, with
the same root-owned sticky-ancestor exception as the manager. Creation is
exclusive with mode 0600: existing entries are never appended to or replaced.
Each process file is capped at 8 MiB; further writes count as telemetry output
errors without breaking SSH. The launcher/operator supplies a fresh filename
and retention policy; this is not a rotating audit store. The optional flag also
works for other CLI commands and for private text diagnostics.

Session success means a complete SSH result, including a
nonzero remote exit status; it does not mean the remote command succeeded.
Cancellation emits `cancelled`. When SSH owns terminal stdin/stdout without a
separate file, CLI console logging pauses behind an acknowledged barrier and
resumes after restoration.
The existing 1,024-record/4 MiB queue remains bounded; overflow is counted. This
keeps telemetry out of TUIs, at the cost of delayed/lost local CLI records during
long interactive sessions. File-backed telemetry remains live during a TUI;
agent metrics/telemetry continue independently.

Still required: GDS host-key provisioning/rotation and account scopes; native
macOS execution; real relay/NAT/failover and slow-output qualification; broader
upstream resource budgets, algorithms/certificates as policy requires; a scoped
Rust OS session broker only with account-isolation evidence; reconnectable PTY
authorization and bounded history; automatic issuer-driven renewal and mixed
SSH/video/sync lease/revocation soak. Explicit same-scope [grant renewal](grant-leases.md)
now preserves the underlying live TCP stream; it does not reattach a lost PTY.
The full W5 gate remains open.

The [dated implementation receipt](reports/rds-ssh-20260925.md) records the
final local checks, source/configuration hashes and qualification limits.
