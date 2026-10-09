# Noq 1.3.0 established-path snapshot

The unmodified crates.io package was imported in commit `80ef738`. Archive
SHA-256: `b78be567e796cfa74bb9bdc4117790af2d505eb887019cf8803244353eb09d89`.
Upstream revision: `c1f411562e6078852749b8bcf1190523096a107f`, subdirectory
`noq`. Original Apache-2.0/MIT licenses, source and package manifest are retained.

The only adapter code addition is `Connection::established_paths`, a read-only
projection of the protocol engine's established path IDs under the existing
connection mutex. Iroh subscribes to path events before reading this snapshot.
An event subscription alone cannot recover paths established before registration.
The manifest binds the sibling `noq-proto` source so standalone adapter builds
use the same additive API as the enclosing workspace.

The protocol engine excludes paths still pending initial establishment,
abandoned paths retained for accounting, and closed connections. It enumerates
actual IDs rather than assuming that IDs stay below the concurrent-path limit.
Ordinary subsequent address migration/validation remains owned by Noq; this
snapshot does not grant a path permission to carry packets or change selection,
cryptography, wire format, congestion control or retransmission rules.

Vendoring is needed because the published adapter exposes an event stream and
individual-ID lookups but no current established-path snapshot. Do not edit the
Cargo registry cache, infer IDs from counters or duplicate protocol state in an
observer. Upstream convergence remains a separate tracked task.
