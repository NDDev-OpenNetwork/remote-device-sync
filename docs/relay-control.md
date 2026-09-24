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
The relay receive queue, peer-map bounds, accept-task ownership and fully joined
server shutdown remain W2.5 work. The notice group limit is per broadcast, not a
global limit across every concurrent detach. The two registration deadlines are
not a separate long-stall campaign. Native macOS and deployed service acceptance
remain open. See the [receipt](reports/rds-relay-control-20260925.md).
