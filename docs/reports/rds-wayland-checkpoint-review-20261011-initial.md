# Wayland checkpoint scope review — 2026-10-11

The final registered `w4-wayland-portal` run at `90b361c` passed 1,016 test
executions with zero failures and three explicit qualification ignores. It
retains the 100 shared loopback handshakes as a regression measurement; they do
not measure portal, physical input or visible-pixel latency. The original
generated [checkpoint](checkpoint-w4-wayland-portal.md) and benchmark bytes are
retained exactly as cited by their machine receipt. Its automatically generated
pending-native verdict is an honest boundary, not a failed automated run.

The [native functional receipt](rds-wayland-native-20261011.md) separately records
attended two-monitor capture/decode, permission restoration, explicit local
cancellation and non-mutating compositor EI sync ACKs. Seventeen portal cases
cover parser/geometry/state, real EI sockets, request identity, early directed
responses and cleanup ownership. Strict Linux portal/owned-transport and macOS
desktop/owned-transport Clippy passed. The published code candidate also passed
its Ubuntu/macOS full CI, portal job, supply-chain checks and both native package
builds. Earlier failed workflow parsing and priority-fixture compilation remain
in the boundary report.

Review: native objects stay on their bounded workers, without unsafe Send
wrappers; declarations and retained pixels have separate bounds. State uses
private ownership/modes, no-follow paths, an exclusive lock and atomic rotation.
Token-bearing replies are not formatted. Remote clients cannot start consent;
service policy precedes source capability probing. Source/pointer identities are
session-scoped, keyboard focus belongs to the compositor seat, and no unsupported
clipboard or Xwayland fallback expands access. Deadline/era and owned-hold checks
retain their documented limits for already-written bytes and started blocking
work.

Physical key/pointer behavior, compositor-initiated revocation, hotplug identity,
long GPU soak and installed multi-device UI acceptance remain distinct gates.
They are not promoted by library tests, EI ACKs or short frame samples. This
review accepts the experimental prepared backend and its functional evidence;
it does not close the broader A6/media/latency product milestones.
