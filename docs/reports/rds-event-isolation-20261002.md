# Managed desktop event isolation

Scope: W2.4/W6.7 managed native latency and diagnostic remediation. Physical
input-to-pixel, loaded Full HD quality and sustained-network acceptance remain
open. This receipt does not close a milestone or predict WAN performance.

The remote control stream had its own priority, but the local bridge serialized
its events after video bodies on one Unix socket. The viewer then awaited native
decode before observing events from that combined queue. A healthy remote input
ACK or heartbeat could therefore wait on application media backpressure.

The new additive local v5 extension attaches an independent same-UID event
socket using one bounded, random, single-claim route. Video and upward controls
stay on the original socket. Agent control/event/video futures and native
control/event/decode futures are retained independently. Either socket departure
ends only that desktop; the shared peer remains usable. Source geometry,
encoded FIFO, one-message media queue, 32 MiB payload cap and global decoder
permits remain. Each socket uses the existing stream/worker admission; event
queues hold 128 entries, attachment has five seconds and event writes two seconds.
An unavailable/full event sink fails explicitly. There is no dropped-ACK success.

Existing commands/replies retain their wire tags; the original combined/headless
APIs stay explicit. An older manager refuses the extension; upgrade the local
agent and native viewer together. Remote ALPN, desktop framing and grants do not
change. A report flag identifies the separated managed mode. A Presented redraw
without a new CPU image now sets the stage to presented, avoiding a stale
acquiring-surface diagnostic despite an idle AppKit main thread.

## Regression evidence

- A real QUIC synthetic desktop feeds the agent bridge while a deterministic
  64-byte video pipe retains its frame body unread. Eight heartbeat echoes must
  cross the independent event socket before any payload is drained. Closing that
  subscriber must end the blocked video owner. The effective synthetic body is
  selected by the sender bitrate, rather than assumed to equal its size ceiling.
- Real Unix client reader queues retain three ordered encoded frames with the
  one-message FIFO full. An independent input ACK is observed without draining
  media. Desktop drop closes the owned event reader even if a control handle lives.
- Authenticated local manager requests and the actual new Client API run against
  both Iroh/Noq loopback peers. Input ACK/heartbeat arrive independently; dropping
  the desktop keeps the same peer usable for a subsequent Ping.
- Route capacity, single claim, owner-only cleanup, aborted subscription and
  unexpected event-socket input have direct lifecycle/negative assertions.
- Explicit serialization tests preserve existing command/reply discriminants.

Initial development checks retained a borrow error, a misspelled test event
field and an incorrect fixed synthetic-body-size assumption; those are not
passing evidence. Broader platform, lint, release and installed qualification
are recorded with the final candidate. Raw logs are retained privately. No
incomplete lane is claimed green.

The initial macOS five-crate all-feature check passed 309 tests, zero failures,
one explicit OpenSSH/account ignore in 42 groups. Its first broad lint attempt
stopped with OS disk exhaustion; the failure is retained. Only regenerable
incremental build cache was removed, and subsequent checks disable incremental
cache growth. Final verification includes the separated remote-refusal path
and its unchanged full TCP budget assertion. Platform/source finalization and
installed-device acceptance remain separate.

After the route-reservation review, final client tests passed 21/0/0. Claimed
routes remain reserved until their owner ends, so even an active-id collision
cannot replace a different registration. A deterministic one-byte event pipe
proves the two-second event-write deadline, owner cancellation and preservation
of a second desktop. The updated remote-refusal/full-TCP-budget test also passed
on both backend variants. Default and all-feature workspace lint passed; final
all-feature lint/release and Linux CI are being qualified against the candidate.
