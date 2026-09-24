# RDS remediation execution

Started: 2026-09-24. Plan: [remediation-plan.md](remediation-plan.md).
Audit baseline: `87e7aeabaad861b0d3c63f6641c91539be19a3e0`.
Audit/plan snapshot: `6772d4a`. Historical findings stay in the dated audit;
this file records implementation progress rather than rewriting that evidence.

## Current state

All waves remain open. W1.1–W1.4 and W1.6–W1.8 passed the local Linux check
matrix; other tasks remain planned unless listed below. No deployment or owned-backend
promotion has occurred. Native macOS checks still require their platform lane.

| Task | State | Evidence / remaining scope |
|---|---|---|
| W0.1 | Partial | R01/R10 are agent regressions; R02 is now covered by transactional record/delete regressions; R03/R04 are journal regressions, with failures observed before fixing. R05 is covered by planted-link and directory-substitution tests. R06 failed before the name proof fix; R07 is covered by server expiry checks. R08/R09 and desktop body cancellation still need their owning fixes/tests. |
| W1.1 | Implemented; Linux checks passed | Denylist replacement retains its value without observers; atomic modification preserves concurrent revocations. Subscribe-before-check and initial watchdog snapshot check remove missed-update windows. Durable feed freshness remains W1.4. |
| W1.2 | Implemented; Linux checks passed | One authorization state owns admission, replay reservation and watchdog. ACK failure/cancellation closes the connection and releases the grant. Service admission checks live validity/revocation. Connection future teardown runs RAII cleanup. |
| W1.3 | Implemented; Linux checks passed | Client trust anchor, per-name domain-separated signatures, exact name/record binding, current validity and volatile anti-rollback. Native directory HTTPS/DNS added; durable revision linkage stays W1.4 and native macOS verification remains open. |
| W1.4 | Implemented; Linux checks passed | Shared durable policy acceptance, positive epochs/revisions, domain-separated signatures, bounded revocation leases, restart/boot rules, dual-signed rotation, atomic feed ownership and live closure. Name trust persists across CLI processes. Native macOS/power-loss qualification and external GDS rollback anchoring remain open. |
| W1.5 | Partial | Transactional bounded disk store, tombstones, generation anchor, publisher revisions, exact retry, durable announce, leased expiry, retained floors, bounded collection, configured enrollment and fair write admission are implemented. Strict HTTP framing, explicit migration and file-capacity renewal qualification remain open; see validation below. |
| W1.6 | Implemented; Linux checks passed | Reused bytes are verified and stored before `have`; edits, insertions, deletions, repeated chunks and destination removal/restart are tested. |
| W1.7 | Implemented; Linux checks passed | Exclusive random staging names and RAII cleanup preserve ordinary/link siblings and colliding names; failed assembly retains the old file. |
| W1.8 | Implemented; Linux checks passed | Directory-relative no-follow journal/destination I/O and a held source file replace path-check-then-open. Link planting and substitutions after open are tested. Native macOS verification remains pending. |
| W1.10 | Partial; Linux checks passed | Persistent per-root receive lock covers processes and filesystem name aliases; parts and assembly use sync/rename/parent-sync ordering. Abrupt-process-exit regression added. Power-loss, every commit boundary, orphan collection and native macOS durability still need qualification. |

## Authorization change

`rds-agent::authz` owns Pending → Authorizing → Granted → Closed. Services
remain pending while an authorization response is being written. The watcher
is installed before that I/O; a 15-second response deadline bounds it.
Duplicate Authz cannot replace a pending or granted lease. Failed verification,
response write, interrupted commit and canceled admission close the connection.
Closing the connection cannot be undone by a late admission task.

The watchdog checks the current denylist immediately, then observes updates
and wall-clock expiry. Each service also checks policy directly so correctness
does not depend only on watchdog scheduling. Replay reservations and watchdogs
are released through owned drop guards, including cancellation of the
connection service future.

Regression evidence before implementation:

- `revocations_survive_without_watchers`: failed; initial revoked ID was lost.
- `concurrent_revocations_are_not_lost`: failed; fewer than 16 concurrent IDs survived.
- `failed_authz_reply_closes_connection_and_releases_grant`: failed; stopped
  reply retained the connection beyond the test's bounded observation window.

Additional coverage exercises simultaneous admission ownership, a snapshot
already present before watchdog startup, admission cancellation, immediate
service-scope revocation/expiry and reservation cleanup.

Local validation on 2026-09-24: `cargo fmt --check`; workspace clippy with
default features, `rds-desktop/x11`, and all features (all with `-D warnings`);
`cargo test --workspace`; `cargo test -p rds-agent --all-features` (5 unit and
15 integration tests). All passed. Native macOS execution is still pending.
`cargo-deny` is not installed locally. No wave checkpoint is claimed: these
changes do not close W1 or replace historical benchmark evidence.

The signed revocation feed still needs durable anti-rollback/freshness/offline
policy (W1.4); this patch does not close all of audit A02. Audience binding and
renewable leases remain W2.3. Long-lived service task ownership remains W2.5.

## Sync correctness and filesystem change

Baseline `cargo test -p rds-sync --test journal` failed four regressions:
partial reuse was lost on reopening, repeated reused chunks could not assemble,
and both the ordinary `.rds-part` sibling and its link variants were deleted.
The new journal materializes reused content, including size-changing deltas,
then assembles only verified parts. `fetched` counts network chunks, not reuse.

`confined::Directory` holds directory handles and uses safe `rustix` bindings
for relative no-follow opens, rename and unlink. The private state directory
has mode 0700, including journals created by the earlier implementation.
Staging files use exclusive creation and random 128-bit names; collision
handling never removes someone else's entry. All state reads are length-bounded
and refuse special files, symlinks and multiply linked part files. Cleanup
does not recursively walk arbitrary journal entries.
The journal namespace is reserved regardless of ASCII case; a second check
compares opened directory identities to refuse filesystem case/Unicode aliases
of `.rds-sync` before descending into a peer-requested path.

The configured root's ancestors and the local OS identity are trusted. A held
directory is an inode capability, including after rename; this does not promise
an absolute pathname boundary against a privileged/local actor relocating that
inode. A remote peer cannot select arbitrary roots or enter `.rds-sync`.
Journal and destination directory handles stay pinned throughout I/O; pull
chunk reads use the same opened file as the manifest, with positioned reads
shared across senders.

Receives within one root are serialized by one persistent lock inode. This
intentionally also prevents alias races on case/Unicode-insensitive filesystems
and avoids an unbounded collection of per-filename locks. Separate roots remain
independent. More granular parallel receives require a proven alias model.
Existing parts remain readable after upgrading, but old and new receive
processes must not share a root concurrently: old binaries do not take the
new lock. New staging files have mode 0600; file metadata preservation is W8.5.
The receiver watches control-stream cancellation while collecting chunks;
sender task ownership uses `JoinSet` so cancellation aborts its async children.
Blocking disk jobs can finish their current operation and drain their bounded
queue; fully interruptible scans and absolute budgets remain W1.9/W2.5.

File and parent sync occur before reporting commit. A parent-sync error after
rename reports uncertainty and keeps recovery state; the caller receives no
false `Done`. The process-crash test targets committed parts and lock recovery;
it does not simulate loss of OS caches or certify macOS power-loss durability.
Unknown orphan staging entries are preserved for future owned-state GC (W8).

Dependency rationale: `rustix` supplies thin safe OS calls; `rand` supplies
staging entropy. Both versions were already in `Cargo.lock`. No helper process,
native code block or new dependency version is introduced by the sync runtime.

## Measurement fixture correction

An intermediate workspace run failed the existing 20-second desktop soak:
frame age p95 1319 ms, p99 1657 ms, 835 received frames. Running the **same
compiled test alone**, without changing its thresholds, passed at p95 32 ms,
p99 63 ms and 1110 frames. This indicates sensitivity to the execution context;
it does not establish a unique root cause or certify production latency.

Inspection also found that the supposed clean in-process desktop lane used
default endpoints with public discovery enabled. That fixture now explicitly
binds loopback and disables discovery, so its direct-path claim is enforced.
Impairment remains on the separately configured controlled socket lane. The
150/250 ms p95/p99 budgets are unchanged. The final check matrix passed after this fixture correction and the journal
namespace-alias check: format; workspace clippy in default, X11 and all-feature
configurations; and `cargo test --workspace` (135 tests in 36 targets). The
sync crate contributes 5 unit, 13 journal and 14 end-to-end tests. Native macOS
and power-loss qualification remain open; no entire wave is closed.

## Name identity verification

Regression `unsigned_directory_name_cannot_select_an_identity` failed before
implementation: the client accepted the endpoint key in an unsigned JSON
answer. The directory now serves a `SignedNameBinding`, created by the registry
issuer for each name. The client requires an independently configured verifying
key, checks the signature's domain/version/name/lifetime, and the resolver then
binds the endpoint record to the verified key. No whole inventory or issuer
secret is returned to a lookup client.

The directory verifies all per-name proofs agree with the uploaded registry,
refuses snapshots without them, and stops serving names at expiry. The old
wire response is intentionally unsupported; the issuer must re-sign snapshots
and clients must provision `--registry-key`. Migration is documented in
`deployment.md`. Maximum binding lifetime is 24 hours with no future timestamp
allowance; bounds are 256 names per snapshot, 256 KiB serialized snapshot,
256-byte binding payload and 1024 remembered names per client. Cache overflow
refuses new names instead of discarding freshness evidence.

The timestamp cache is shared across client clones and rejects older or
conflicting same-issued-at responses. It is not durable and cannot discover a
withheld newer revocation or name removal. Revision/epoch persistence,
authority rotation and outage policy are W1.4.
Pinned ticket/key paths remain independent of name resolution.

Local validation: format, default/X11/all-feature workspace clippy and
`cargo test --workspace` passed (141 tests in 38 targets). Six new name-trust
tests exercise unsigned answers, wrong names/issuers/records, signature-domain
confusion, malformed/oversized proofs, redirects, future/expired/excessive
lifetimes, rollback/equivocation and server expiry. Native macOS remains open.

## Directory HTTPS and DNS

`Client::from_endpoint` accepts a validated HTTP(S) origin or legacy plaintext
socket address. TLS checks public WebPKI roots and hostname/IP SAN; explicit
private CA bundles replace the roots. URL credentials, paths, queries,
fragments, malformed origins and CA configuration on plaintext are refused.
Requests send the correct Host authority. There is no TLS-verification bypass,
HTTP downgrade or redirect following. Signed identity checks still run over TLS.

One deadline covers DNS, bounded staggered TCP attempts (16 maximum), handshake
and response. Dropping that future aborts its pending dial tasks; native OS
resolver work can complete later without extending the caller's deadline.
The TLS-only server listener reserves capacity before spawning and bounds the
entire handshake/request. Its owned JoinSet bounds active/completed handles and
aborts async request children on service drop. Blocking stores remain W1.5;
this is not a claim of complete W2.5
task ownership throughout the agent.

Six TLS tests passed locally: DNS lookup and signed record/name roundtrip;
missing/wrong name authority despite valid TLS; untrusted CA, wrong hostname
and expired certificate rejection; strict origin/PEM configuration and Host
header; stalled handshake timeout without plaintext retry; capacity recovery
and pending request teardown. The full Linux matrix passed: format;
default/X11/all-feature workspace clippy with warnings denied; and
`cargo test --workspace` (147 tests in 39 targets). Native macOS, edge proxy
compatibility and certificate renewal automation remain open.
All three binaries (`rds`, `rds-agent`, `rds-server`) also built and executed
`--help` successfully with their new directory TLS/CA options present.

Dependency rationale: rustls/tokio-rustls provide standard TLS, rustls-pki-types
parses PEM, webpki-roots supplies trust data, and url validates/normalizes DNS/IP
origins. rcgen generates synthetic test certificates only. All versions already
existed in Cargo.lock. No helper binary is added. The selected existing ring
crypto provider contains native code/assembly; this is not a pure-Rust crypto
implementation claim. TLS and protocol policy stay in RDS Rust code.

## Next sequence

While validating W1.4, the workspace run exposed a measurement race in
`bounded_queue_newest_wins`: draining a live queue returned 65 headers over
time even though the instantaneous capacity is 64. Receiver depth now has the
same atomic diagnostic access as sender depth, and the test checks occupancy
before and during a bounded batch. Frame order, freshness and the 64-entry
limit remain asserted; no capacity or latency budget was relaxed. The focused
regression passed after this W0.1 fixture correction. Full matrix results are
recorded with the policy change that triggered the run.

Continue W1.5 directory transactions and W1.9 wire
binding/accounting. Each change retains
its own failing-before/passing-after evidence. No general CI or long soak is
made a blanket barrier to independent development; actual invariant failures
are repaired and platform evidence is recorded separately.

## Durable policy acceptance and managed revocation

W1.4 replaces second-resolution timestamp ordering with explicit authority
epochs and revisions. Name proofs bind to the signed registry digest; the CLI
persists one global revision floor across names/processes. Policy snapshots and
lease metadata commit together through protected, exclusively owned staging
before publication. Failed writes poison the store and close managed access;
corruption, missing initialized state or unknown bootstrap keys never select an
empty fallback. A lower-revision bootstrap cannot replace the directory's
committed state. Unchanged cache hits do not rewrite/sync the marker file.

The managed agent observes revoked IDs and freshness in one value. Startup can
use a verified same-boot cache only until its original absolute lease ends;
replies and restarts cannot rearm that lease. OS reboot needs a newer revision.
Admission, admission commit, new service requests and the live watchdog all
check policy. A feed has one owner; shutdown/fatal persistence error prevents
late publication, and obsolete callbacks cannot overwrite a successor.
Authority rotation requires signatures by both the current and next key and
advances the epoch by exactly one.

Five real-QUIC managed-feed cases passed, covering fresh/missing policy,
revocation, outage, cached restart with older network replies, feed drop and
failed disk commit. Nine policy-state cases cover ordering, boot/clock changes,
rotation, linked/corrupt/missing state and bounded wire inputs. Fault and abrupt
child-exit tests cover five persistence boundaries. Directory/client integration
tests cover actual restart and cross-process-style cache reuse. The worker
budget test confirms a timed-out HTTP request cannot free a still-running disk
job's permit. Its initial test assertion was corrected to use the client's
typed `RateLimited` error for HTTP 429; no production limit changed.

Linux validation: `cargo fmt --check`; default, X11 and all-feature workspace
Clippy with warnings denied; `cargo test --workspace` (**170 tests, 43 targets**).
The owned transport/relay feature suite also passed (**52 tests, 17 targets**).
The five managed-feed cases were additionally rerun with their endpoints
explicitly selecting noq, and all passed. `rds`, `rds-agent` and `rds-server`
built and executed `--help` with the new state/epoch/rotation flags.
The first workspace failure and the separate desktop fixture correction are
recorded above. `cargo-deny` is not installed. Native macOS, physical suspend,
power loss and real estate deployment were not run. No wave is closed and no
performance qualification is claimed. See the [policy receipt](reports/rds-policy-20260924.md)
and [migration/operational contract](policy-state.md).

## Record transaction foundation

W1.5 now keeps delete tombstones in both stores, serializes mutations and stores
record/tombstone/catalog updates in an embedded Rust transaction. The file
store's separately synced generation anchor refuses database-only rollback of
acknowledged history. Corrupt/missing initialized files and legacy directories
are refused; unexpected I/O closes the instance until recovery. Eight new
integration tests and six unit tests cover replay, concurrency, confinement,
bounded storage, partial writes, sync faults and process exits. No runtime
subprocess or database service is added. Migration is deliberately not inferred
from old files; deployment remains pending the versioned publisher work.

Formatting and all three Clippy lanes passed. The default workspace run failed
the existing parallel desktop soak latency gate (p95 176 ms/p99 1447 ms); a
single isolated diagnostic rerun passed at 10/16 ms. The complete sequential
test run passed **184 tests, 44 targets**. This does not erase the default-run
failure or close W0/W6 performance evidence. No thresholds changed. See the
[record transaction receipt](reports/rds-records-20260924.md) for exact scope.
That storage patch left signed revisions and durable publisher allocation for
the following change, then expiry/quotas, enrollment fairness and HTTP framing.

## Versioned endpoint publication

The next W1.5 change replaces timestamp ordering with one positive revision
sequence for record updates and deletion. New signature domains bind version,
identity, revision and bounded validity. Verification precedes variable-field
decoding; exact signed retries are idempotent, while equal-revision conflicting
content and older mutations are refused. Both stores enforce current lifetime
on writes and reads; resolver checks remain independent of the server.

`RecordIssuer` persists the counter and pending signed bytes before publication.
The CLI owns durable state, while fixtures explicitly select volatile issuers.
Same-second address changes receive different revisions. Retries preserve signed
bytes until actual renewal/change; restart resumes the pending operation. Fatal
history/disk failures and permanent protocol refusals reach the agent supervisor
instead of silently stopping the announce loop. Existing connections close on
that fatal CLI path. A lost successful HTTP reply is exercised through a real
directory and proxy, with an identical second request accepted successfully.

Ten revision/issuer cases and three announce lifecycle cases cover retry,
conflicts, delete ordering, expiry, wire bounds, restart, missing history and
permanent rejection. Publisher fault tests cover five write/rename/sync phases,
including abrupt child exits. Old fixtures that advanced `issued_at` into the
future were updated to use revisions. The expiry test now checks both a 410
from the real store and independent client rejection of an expired signed
record returned by a hostile HTTP 200 response.

Record validation and the owned transport share a typed `rds-relay://` locator
parser, preserving relay identity and IPv4/IPv6 sockets. The real owned relay
test now goes through announce, directory storage and resolution before forcing
the handshake, datagrams and stream exchange onto the relay. This caught a
compatibility gap in an HTTP-only validation rule; the previous IPv6 bracket
parse failure is also corrected. No IPv6 network qualification is implied.

The first workspace run exposed a state-lock lifetime issue: a descriptor alias
can retain flock after its owner drops. A deterministic reproducer failed;
explicit guard release now passes across atomic state, database and sync root
locks. See the [separate lock receipt](reports/rds-locks-20260924.md), committed
as `8a18064`. This correction does not add sleep/retry to hide a failed invariant.
The final default `cargo test --workspace` run passed **204 tests across
46 targets**, including the parallel desktop smoke test and crash/reopen cases.
Formatting and default, X11 and all-feature workspace Clippy also passed with
warnings denied. This does not qualify sustained production latency or erase
the preceding failed-run evidence.

`cargo test -p rds-net -p rds-agent -p rds-relay --all-features` passed
**58 tests across 18 targets**, including the real owned-relay feature (not just
the noq endpoint feature). The agent binary built and executed `--help` with
`--record-state`. See the [publisher receipt](reports/rds-publisher-20260924.md).
Native macOS, physical failures, migration and real estate deployment remain
unqualified. This receipt precedes the expiry-retention change below.

## Record expiry and retention

W1.5 now persists each record/delete acceptance lease, using the same boot and
suspend-inclusive clock adapter as policy state. Exact retries cannot rearm it.
Expiry observed by a read commits retirement and a clock floor before returning;
expired addresses/signatures are reclaimed while revision/digest/kind history
remains. OS reboot requires a newer signed revision. Backward wall time refuses
operations until it catches up. Whole-state rollback still needs a GDS anchor.

Both stores share expiry/order decisions and retain at most 4096 identities;
memory deployments can choose a lower capacity. Collection keeps those identity
slots. A higher revision for an existing identity remains possible at identity
saturation. One owned maintenance worker visits at most 64 rows per pass,
separately from request-worker capacity, with retirement/failure metrics.
Concurrent renewal and collection serialize through the store transaction.

An HTTP 410 triggers a durable local successor; losing its reply retries the
same new bytes. HTTP 409 remains fatal. An already acknowledged publisher checks
the server again on its normal renewal, so immediate recovery after server reboot
is not claimed. Native platform, physical-failure and latency qualification
remain open. The database is format 3; deployment migration is still on hold.

The Linux matrix passed: formatting; default/X11/all-feature workspace Clippy
with warnings denied; **221 workspace tests across 47 targets**; and **59 tests
across 18 targets** for the network/agent/relay all-feature lane. Targeted checks
also passed: 38 discovery unit tests, two service-collection tests and four
real-HTTP publisher lifecycle tests. See the
[expiry receipt](reports/rds-expiry-20260924.md). No wave gate is closed.

This receipt precedes the admission change below.

## Publisher enrollment and write fairness

Default directory configuration now denies record access. Production explicitly
lists publishers with `--directory-allow`, independent of relay/agent permissions
and registry names. The static list is bounded to 4096 distinct nonweak keys;
removal hides records while preserving floors. GDS live reconciliation remains W4.

The before-fix regressions failed: unconfigured publication succeeded, and one
writer's refused requests spent the shared counter and blocked another device's
renewal. Both now pass. Stores verify and compare before invoking quota admission
under the same compare/commit owner. Exact retries, stale operations and bad
signatures do not spend the owner's quota; concurrent identical copies charge
once. Callback refusal does not change the record or poison storage.

Known identities have one protected mutation in each fixed 60-second window.
New admissions and extra writes have separate shared budgets, with per-identity
limits checked first. Each policy role has its own budget after signature and
revision validation. TTL/3 renewal with TTL >= 180 fits the reserved cadence;
faster renewals need burst capacity. The guarantee concerns write quotas, not
isolation from arbitrary network, CPU or disk saturation.

Validation and limits are recorded in the
[admission receipt](reports/rds-admission-20260924.md). The full Linux matrix
passed: formatting; default/X11/all-feature Clippy with warnings denied;
**234 workspace tests across 48 targets**; **59 all-feature network/agent/relay
tests across 18 targets**; and server build/`--help` with `--directory-allow`.
No wave completion is claimed. Next: strict HTTP framing, explicit
migration/file-capacity qualification, then W1.9 wire/accounting.
