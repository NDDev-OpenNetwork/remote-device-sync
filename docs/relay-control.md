# Owned relay control framing and drain

`rds-net::relay_control` supplies one codec for both ends of `rds-relay/0`.
Every control message, including Drain and PeerGone, is a big-endian u32 length
followed by exactly one postcard value. Bodies must be 1–4096 bytes. Oversized
lengths are rejected before allocation/read; the decoder rejects trailing bytes
rather than accepting an otherwise valid value followed by garbage. It never
clamps an advertised length. Fragmented and coalesced frames preserve boundaries.

The server's registration budget is 15 seconds for accepting the control stream,
reading Register and writing Registered. The socket client's separate 15-second
budget covers dialing, opening the stream, writing Register and reading the
reply. These do not replace complete role/startup timeout policy.

## Notices and grace

Drain refuses new registration and broadcasts framed notices. A second check
under the attachment-table lock prevents registration already in flight from
inserting a slot after the drain snapshot. Up to 16 notice writers run at once;
each has a one-second budget covering the writer lock and complete frame write.
Error, timeout or cancellation closes that recipient connection so a partial
frame cannot be followed by another writer's bytes. Notice futures are polled
inline by their owner, with no spawned writer tasks. Notices are best effort within the existing two-second drain
budget; this is not a delivery acknowledgement protocol.

A socket records the actual Drain notice but keeps sending and receiving through
the existing tunnel during grace. Receipt no longer immediately blackholes a
still-usable link. The server closes after grace. Automatic warm replacement,
new candidate publication and active-session migration remain W3.3 work: a
relay-only session still cannot survive removal of its sole path.

When an endpoint detaches, only the current slot owner can remove presence and
recent-flow history or send PeerGone. A stale connection replaced by the same
identity cannot invalidate the successor's history. Flow recording and detachment
use the same attachment/history lock order to avoid resurrecting a departed
history entry. Recipients can decode PeerGone and the next control frame without
losing alignment. PeerGone is currently an observed hint, not full migration.

## Ownership

Server control reading and datagram forwarding share one connection future;
completion/error of either ends the other. Malformed or lost control closes the
tunnel. Pongs use the same framed, bounded writer as other notices.

Dropping a RelaySocket closes its outer connection and aborts both client pumps.
Explicit `RelaySocket::close(&mut self)` aborts and joins the pumps and closes the
helper endpoint; repeated close is safe. Drop itself cannot synchronously join.
A retained diagnostic RelayHandle does not retain the socket's I/O tasks.

## Qualification and remaining work

Real owned-QUIC tests reproduce the missing Drain notice and dropped-socket leak
before the fix. Other tests cover grace-period datagrams through actual relay
sockets, replacement and actual PeerGone, following-frame alignment, oversized
registration replies, writer-lock deadline and early cancellation. Codec tests
cover all control variants, fragmentation, coalescing, truncation and bad lengths.

This does not establish a warm secondary relay, physical failover, SSH/video/sync
interruption budgets, TCP/443 fallback or process RSS/FD bounds.
Client and server application limits are defined below. The notice group limit
is per broadcast, not a single shared limit across every concurrent detach.
The two registration deadlines are
not a separate long-stall campaign. Native macOS and deployed service acceptance
remain open. See the [receipt](reports/rds-relay-control-20260925.md).

## Relay-link failure boundary

Relay sends treat a closed tunnel, unavailable datagram support, an oversized
packet or an unknown synthetic destination as loss on that relay route. They
consume/drop that packet without returning a logical-socket I/O error to the
QUIC connection driver. Other direct paths on the connection remain usable;
QUIC retains responsibility for loss detection and expiry of the failed path.
Non-relay destinations passed directly to RelaySender remain programming errors.
Generic UDP child I/O errors have not been reclassified by this change.

`RelayHandle::is_available()` reports an open authenticated local tunnel with
negotiated datagram support. It uses only a weak connection handle and holds no
strong I/O reference across awaits. Drain grace can be both draining and available;
actual tunnel closure or socket destruction makes it unavailable. This is local
link state, not proof that a remote peer is attached or reachable.

A real multipath regression first validates an explicitly opened relay path,
closes the relay and observes both tunnel handles become unavailable. It then
pings the failed path while exchanging 25 datagram requests and replies on the
existing direct connection, followed by bounded endpoint shutdown. Before the
fix, traffic and cleanup both exceeded their deadlines. Another regression
proves an unknown relay mapping does not break direct I/O and that adding the
mapping later allows the pending path to validate. The unknown-mapping branch
also failed with its original error-return behavior.

This does not implement warm relay replacement, relay-only session continuity,
generic socket failure isolation or all-path failure recovery. A QUIC connection
that has no working route still depends on transport timeout; dropped unreliable
datagrams are not replayed by the application. Global budgets across components
remain open. These loopback tests do not establish physical network or service
interruption budgets.

## Client peer ownership and receive bounds

Each attached tunnel has configurable positive limits: 1024 peer entries, 128
queued datagrams and 30 seconds of unpinned inactivity grace by default. The
registry never replaces a pinned identity with another peer. Registration or
authenticated outer-frame learning that conflicts with an existing live alias
returns/refuses `PeerRegistrationError::Collision`. A deterministic pair of
distinct public fixture keys reproduces a real alias collision; before this
correction, registering the second silently redirected the first route's raw
transport datagram. Inner QUIC identity pinning is unchanged; this was a routing
integrity failure, not demonstrated application plaintext exposure.

`RelayHandle::register_peer` now returns `Result<PeerLease, PeerRegistrationError>`.
Library callers must retain the lease while they need relay I/O. Automatic
outgoing admission reserves a lease before dialing, even for direct-only tickets,
so subsequently learned relay paths have an owner. Authenticated incoming
connections reserve at acceptance. The tracked weak policy task owns the lease
until closure, including when streams outlive the public Connection facade.
Multiple leases share one entry; dropping one does not release another's pin.
The metadata lease itself retains no connection, socket, task or payload queue.

Without an available lease, a valid direct candidate can still connect or be
accepted; the connection does not advertise its local synthetic route. A
relay-only outgoing dial fails before handshake with the registration error.
Incoming relay-only acceptance with a failed reservation closes the connection.
An existing owner is never evicted for a new connection. Noq can independently
probe learned QNT addresses before application selection; collision refusal is
not a new per-peer routing namespace. Such opaque probes still require the
correct authenticated peer and path validation before becoming eligible.

Last-lease drop starts inactivity grace for best-effort final transport packets.
Learning and lookup refresh that inactivity period. On new-peer pressure,
expired unpinned entries are removed and then the oldest unpinned entry may be
evicted; all-pinned capacity produces `Capacity`. Grace therefore is not a
guaranteed drain period. Expired idle storage is reclaimed lazily at lookup or
admission and never grows beyond the configured entry count. The synthetic
32-bit namespace still has collisions; incompatible simultaneous identities
require direct connectivity or a future namespace design.

Hash port zero is normalized to virtual port one because Noq rejects a zero
remote port. Previously nonzero mappings are unchanged. A deterministic public
key fixture that used to fail with InvalidRemoteAddress now establishes streams
in both directions using relay sockets with no direct transport child. This is
an internal address correction, not a key rotation or relay frame-format change.
Older dialers retain their zero-port behavior; mixed-version qualification for
this formerly failing identity class remains separate. Collision admission still
applies when normalization shares an alias with another identity.

The receive pump uses a bounded channel of shared byte slices. A full queue
drops the new datagram and counts the loss; it does not block control processing
or allocate an unbounded backlog. The pump yields after 64 frames. A receive
buffer smaller than the next packet drops the entire packet, never a truncated
prefix; at most eight queued packets are examined per poll before yielding.
QUIC is responsible for transport recovery; raw datagrams have no delivery
guarantee.

`RelayHandle::stats()` exposes queue capacity/occupancy, stored/pinned peer
counts and queue-full, rejected-peer and oversized-receive counters. Fields are
individual concurrent snapshots. The diagnostic handle retains bounded metadata
and weak I/O references; destroying the socket releases queued payloads even
while a handle survives. These limits do not bound the QUIC engine, allocator
overhead or global process RSS. Warm relay replacement and the
full service qualification matrix remain open.

## Server task ownership

`server::serve_with_limits` accepts positive `ServerLimits::max_connections`
(1–65535, default 256 via `serve`). A permit is reserved before spawning a
handshake task and remains held through registration, forwarding and detach
notification cleanup. Excess incoming attempts are refused without a queued
application worker. Handshakes have a 15-second budget; registration retains
its separate 15-second budget. This is library configuration for the owned
server, not a new CLI option or a change to production allowlist defaults.

The accept runner drives a JoinSet shared with the Relay owner, reaps completed
tasks and seals registration on shutdown. `close()` closes tunnels and the
endpoint, allows connection workers five seconds to finish, then aborts and
joins remaining workers. If the runner fails, the retained child set is still
joined by the close fallback. Concurrent and repeated callers share the stored
runner result. It is saved before fallback awaits, so canceling a waiter cannot
repoll a consumed JoinHandle or lose child handles. Normal runner cleanup
continues independently; another close caller can resume a canceled fallback.

Both `close()` and `drain()` now return `Result<(), ShutdownError>`. This is a
library API change: callers must inspect the result. Previously a runner failure
was only logged and appeared as successful unit completion. A failure is now
retained and returned after cleanup, including on repeated calls. Drop seals
registration, closes current tunnels and requests runner cleanup; it cannot
synchronously join and requires the executor to continue polling.

Drain grace and notices also belong to the runner. Canceling a `drain()` caller
does not cancel the requested drain, repeated calls wait for completion, and an
explicit close takes precedence. This fixes observed live tunnels after Relay
Drop and an observed server left draining indefinitely after caller cancellation.
The two-second grace still carries ordinary traffic; notices do not promise a
warm replacement path.

An attachment guard removes only its current slot and recent-flow references
when a session future is canceled. Old replacement owners cannot remove new
registrations, append flow history after removal or spend a successor's rate
bucket. Forwarding yields every 64 frames. Notices use at most 16 inline futures
per broadcast; active session broadcasts are bounded by the admission budget,
plus the runner's single drain broadcast. At most 64 recent sources are retained
per attached destination. These are application object/work bounds, not a
measurement of total process memory or lower-layer handshake allocation.

`Relay::lifecycle_stats()` reports configured/occupied connection slots, refused
attempts and flow-history occupancy. Fields are individual concurrent snapshots.
Real tests cover silent registration admission/refusal/reuse, cancellation after
actual forwarding history exists, concurrent close/drain and canceled callers.
A real attached-tunnel fixture also aborts the accept runner, cancels two close
waiters during fallback, then requires the same retained error, joined children,
released admission/history and zero live endpoint path drivers.
Full network impairment, long-stall, resource soak and native-platform/service
qualification remain separate work.

## Tunnel loss and path eligibility

Dropping a tunnel or ending either pump publishes unavailable state through a
shared watch channel. Connection policies observe that channel without owning
I/O and without accumulating one-shot subscribers on a long-lived tunnel.
Drain notices alone keep the tunnel available during grace.

Upon known local tunnel loss, a managed endpoint withdraws its synthetic QNT
advertisement, removes pending synthetic candidates and stops advertising or
dialing that relay. Known validated relay paths become ineligible regardless of
their old RTT; closable paths are abandoned and an eligible direct path becomes
Available. A failed last path cannot be closed by Noq's path API: it remains
Backup and outside policy selection while transport timeout/recovery applies.
Late Established events on failed relay paths receive the same treatment.
Unobserved engine paths, remote-only relay failures and arbitrary direct-link
failures still need broader reconciliation and recovery work. Injected raw
sockets without an attached RelayHandle do not provide this health signal.

The prior failure fixture intermittently retained a dead relay as Available
while its direct path was Backup: the first request reached the peer, but its
reply went over the failed relay. The managed regression now proves actual
STREAM transmission over the relay before failure, checks retirement/direct
selection within a bounded interval, then exchanges 25 direct datagram
roundtrips. It also checks removal of the failed relay from fresh addresses and
prompt refusal of a stale relay-only ticket. Transition-time datagrams remain
unreliable; this is not application replay or an interruption-free handover claim.
