# Idle desktop recovery and input-position safety — 2026-10-03

Scope: W6.3/W6.6/W6.7 native-viewer reliability follow-up. This is a regression
receipt, not closure of WAN, quality, suspend or physical input-to-pixel gates.

## Changes and acceptance boundary

- Managed and direct native sessions share a decoded-progress watchdog. It sends
  a recovery-image request after three and eight seconds without a decoded
  frame, on the current control channel, before the existing fifteen-second
  reconnect. Only fresh decoded progress rearms the attempts. Control writes
  retain their two-second deadline and cancellation owns both session legs.
- The macOS event-loop activity prevents automatic idle system sleep and requests
  latency-critical timer/I/O precision. Its display-sleep bit remains absent;
  lock, explicit sleep and lid policy remain OS behavior. The token ends through
  its existing RAII owner. No persistent OS settings or remote process lifetime
  are changed by this increment.
- The 1024-entry input queue no longer removes motion from before an accepted
  click/key/scroll. Only adjacent same-display absolute motion can collapse,
  including when the queue is full. Overflow preserves the accepted prefix
  until termination and fails closed rather than redirecting a click or losing
  a release silently.
- Local input-ACK correlation now retains 1024 pending sequences, matching the
  input queue's burst capacity. Dispatch, matched/unmatched ACKs and evicted
  tracking records are separate observations. Metadata-only trace records add
  the local event-to-ACK duration; no keys, coordinates or payloads are logged.
  Live snapshots also count watchdog repair requests.

## Regression evidence

On macOS arm64:

```sh
cargo test --locked -p rds-cli --features desktop --lib desktop::control::tests
cargo test --locked -p rds-desktop --features x11,viewer --lib render::
cargo clippy --locked --workspace --all-targets --features rds-agent/desktop,rds-cli/desktop -- -D warnings
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --check
cargo deny check
```

All commands above passed. The control suite has six tests and the render suite
has eleven. Render tests require `viewer`; the `x11` feature alone does not
compile/run these native queue regressions.

The framed control fixture covers sixty virtual seconds with input and
heartbeats, a repair at three seconds and decoded progress on the original
channel. It forbids repeated repair after recovery and confirms the original
session future remains alive. Separate fixtures cover the fifteen-second
unrecovered bound, timer rearming, a stalled write and dropping both legs on
cancel. These are real framed duplex controls with simulated decoder progress,
not an installed network measurement.

Queue fixtures preserve 800 button edges in order, reject full-queue motion/key/
release additions without evicting earlier click positions, allow same-display
tail motion replacement, and forbid cross-display collapse. ACK fixtures retain
all 800 delayed replies, accept out-of-order observation and exclude duplicates
from latency samples. The platform test verifies the idle-system bit is set and
the idle-display bit is absent.

The first broader workspace run exhausted the shell's 256-descriptor soft
limit in the local-manager integration fixtures; a sibling fixture also reported
a remote refusal under that pressure. Repeating all eleven local-manager tests
with `ulimit -n 4096` passed. This adjusts only the test shell's resource limit.
The full desktop-feature workspace rerun with that same descriptor limit passed:
827 tests, no failures, two intentionally ignored native X11 display tests. The
default workspace run also passed with 795 tests, no failures and two ignored
X11 tests. Installed activity/idle qualification remains separate and is not
inferred from these software checks.

## Remaining qualification

The former allowing-idle-sleep activity and a short successful idle observation
did not establish uninterrupted operation. Longer private runtime logs include
video stalls and unsuccessful connection recovery. The scoped activity and
soft video repair address distinct failure modes; they do not prove those
longer outages share one root cause.

Installed `pmset` assertion, idle/active transitions with native visibility,
first-click-to-visible-frame delay after a long pause, overnight soak and physical
sleep/network-change recovery remain separate acceptance evidence. A responsive
heartbeat, a submitted GPU frame and an input ACK each describe different stages;
none alone proves physical display response or permanent WAN availability.
