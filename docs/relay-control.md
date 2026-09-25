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
frame cannot be followed by another writer's bytes. Notice tasks belong to the
broadcast future. Notices are best effort within the existing two-second drain
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
interruption budgets, TCP/443 fallback, global attachment limits or RSS/FD bounds.
Client receive-queue and peer-map limits are defined below. Accept-task ownership
and fully joined server shutdown remain W2.5 work. The notice group limit is per broadcast, not a
global limit across every concurrent detach. The two registration deadlines are
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
datagrams are not replayed by the application. Full server task ownership and
global budgets remain open. These loopback tests do not establish physical network or service
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
overhead, server tasks or global process RSS. Warm relay replacement and the
full service qualification matrix remain open.
