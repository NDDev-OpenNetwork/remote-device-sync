# Destination-bound receive journals — 2026-10-10

Status: implemented; the registered checkpoint passed at clean `7cb2096`.
Final cross-platform/native qualification and broader stability remain open.
Milestone: W1.10/W8.2, preserving W4 signed path-scope boundaries.

## Source finding

`Journal::open_cancellable` selects state using only `manifest.root`, then
rewrites advisory metadata with the requested destination before scanning parts.
A synthetic baseline probe retained a complete unassembled file for destination
A, opened destination B with the same manifest and no payload, and assembled B
from A's cached part. The destination check in `engine::serve_inner` covers B;
it does not authorize reuse of A. Knowledge of content hashes is insufficient
authority to transfer cached content across destinations.

## Implemented change

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

## Verification

The baseline probe against `618685e` reused one cached part for the other
destination, requested no payload and assembled that destination; its isolation
assertion failed. At `29bcfd6`, all 121 sync test executions passed, including
five new journal cases and the path-scoped real-stream case on both Iroh and
Noq. The cases cover independent equal-content destinations, verified legacy
resume, missing/torn/mismatched/linked legacy metadata, new-layout torn metadata,
and collection that preserves other destinations. Existing alias refusal,
returned-error and process-exit recovery checks remain effective.

After integrating PR147's reviewed evidence without changing these production
sources, clean `7cb209677f2f9f4d759314c76b19f19882af8943` passed
`scripts/checkpoint.sh w8-journal-scope`: formatting, strict workspace Clippy,
workspace tests and the separate sync suite. There were 1,062 passing test
executions, zero failures and two existing ignored checks. The receipt chain
extends the combined admission history; no prior receipt hashes were rewritten.

The [release benchmark](bench-w8-journal-scope.md) and
[source/digest-bound JSON](bench-w8-journal-scope.json) record 100 samples per
case on macOS arm64: verified 4 MiB resume p95 28.965 ms, equal-content admission
for another destination p95 20.905 ms, pre-cancellation p95 6 µs, and admission
beside 8,192 retained foreign names p95 31.109 ms. Every second-destination
sample requested all chunks while preserving the original destination's reuse.
These are local warmed-cache measurements, not a before/after speed claim or
physical-durability evidence. PR149's CodeQL analysis completed with zero open
alerts; scanner coverage and policy are unchanged.

The separate main-branch control RTT failure (445 ms p95 versus 400 ms across
601 probes) remains part of the [stability plan](../stability-plan-20261009.md).
This successful storage checkpoint does not resolve that transport/service tail.

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
