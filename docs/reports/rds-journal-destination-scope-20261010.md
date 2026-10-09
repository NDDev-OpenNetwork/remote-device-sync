# Destination-bound receive journals — 2026-10-10

Status: implemented; all 121 sync test executions passed at `29bcfd6`.
The registered full checkpoint and final cross-platform qualification are pending.
Milestone: W1.10/W8.2, preserving W4 signed path-scope boundaries.

## Source finding

`Journal::open_cancellable` selects state using only `manifest.root`, then
rewrites advisory metadata with the requested destination before scanning parts.
A synthetic baseline probe retained a complete unassembled file for destination
A, opened destination B with the same manifest and no payload, and assembled B
from A's cached part. The destination check in `engine::serve_inner` covers B;
it does not authorize reuse of A. Knowledge of content hashes is insufficient
authority to transfer cached content across destinations.

## Selected change

1. Bind new journal directory names to the normalized relative destination and
   content root. Keep chunk hashes, wire frames, receive locks and atomic part
   publication unchanged; no additional database or cryptographic protocol.
2. Resume an existing legacy content-root journal only when bounded, verified
   metadata matches the exact normalized destination, content root and size.
   Preserve legacy journals with missing, corrupt, mismatched or unreadable
   metadata. A fresh destination-bound journal remains usable beside them.
3. Preserve recovery from torn metadata in the new layout: its directory name
   already binds the destination, and every reused part must still verify.
4. Apply the same name/metadata attribution to superseded collection. Never
   remove another destination's journal. Cleanup removes the actual selected
   journal name, including the legacy compatibility case.
5. Add tests for equal-content independent destinations, safe legacy resume,
   legacy mismatch/torn metadata, new-layout torn metadata and cleanup. Keep
   alias refusal, fault injection, cancellation and work bounds effective.
6. Run all sync checks and workspace gates, a registered checkpoint and the
   existing release journal benchmark. Qualify Linux and macOS before rollout.

## Research and limits

The 2011 primary paper [Proofs of ownership in remote storage systems](https://research.ibm.com/publications/proofs-of-ownership-in-remote-storage-systems)
explains why possession of a compact content signature cannot authorize access
to another owner's deduplicated bytes. [OWASP authorization guidance](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html)
requires permissions on each protected resource. The selected design applies
those principles by retaining reuse within the authorized destination instead
of introducing a proof-of-ownership exchange. The paper predates the requested
September 26, 2026 research cutoff; the OWASP page was consulted October 10 and
is a living source, not a verified historical snapshot.

This does not establish tenant/account isolation beyond the existing path grant,
physical durability, space quotas or fair background collection. Same-OS-user
processes remain trusted. Downgrading to an older receiver reintroduces its old
journal-selection behavior; mixed-version local writers need separate handling.
