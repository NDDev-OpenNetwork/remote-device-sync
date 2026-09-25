# Directory record storage

Status: W1.5 transactions, versioned publication and expiry retention implemented,
not a completed directory acceptance gate. Configured enrollment and write-budget
fairness and strict HTTP framing are implemented. A bounded 4096-identity Linux
capacity/churn/reopen run passed. Explicit format-2 offline conversion is implemented.
Legacy cutover, longer load/physical-failure tests
and native macOS qualification remain in [the remediation plan](remediation-plan.md).

## Directory HTTP profile

The owned codec implements a restricted private API: one exchange per connection,
then close. It is not a general HTTP implementation. Both client and server
parse the same bounded header grammar. Header names are ASCII tokens, values
are ASCII printable bytes or space/tab; whitespace before a colon, folded fields,
bare CR/LF, other control bytes and non-ASCII header values are refused. Bodies
can contain arbitrary bytes, including UTF-8 JSON. Limits are 8192 bytes for the
entire head including its final CRLF pair, and 256 KiB for the encoded body.
Writers validate fields and limits before emitting any bytes.

Requests accept exactly HTTP/1.0 or HTTP/1.1, an ASCII token method of at most
16 bytes and an origin path of at most 512 bytes without query, fragment,
backslash or whitespace. HTTP/1.1 requires one valid Host authority; HTTP/1.0
without Host remains available for local diagnostic probes. A missing request
Content-Length means zero bytes. A present length must be a single decimal
value; duplicate fields are refused even when equal, as are comma lists.
Transfer-Encoding, non-identity Content-Encoding and Expect are unsupported
in both directions. No chunked or close-delimited response bodies are accepted.

Responses require a three-digit status in 200–599 and explicit Content-Length,
except bodyless 204/304. A 204 forbids Content-Length; a 304 may describe the
representation length but never reads a body. Interim responses are refused.
The directory does not implement HEAD and answers 405 without a body; overload
and worker errors after a parsed HEAD also omit the body. Readers buffer at
most 1 KiB ahead while parsing the head, preserve prefetched body bytes, and
may discard trailing bytes when dropped. Callers must close the connection:
there is no reuse, pipelining, second response or upgrade in this API.

The strict ambiguity rejection follows the framing risks described in
[RFC 9112 §§5–6](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3),
with a narrower supported profile; full RFC HTTP interoperability is not claimed.
DNS/TCP/TLS/HTTP share the client deadline; server header and body reads share
the connection deadline. A proxy or tunnel must preserve this profile. Particular
edge products and production latency have not been qualified. See the
[HTTP regression receipt](reports/rds-http-20260924.md).

## Signed mutation contract

An endpoint identity owns one positive `u64` revision sequence shared by record
updates and deletion. Revisions are independent of wall-clock timestamps.
Both payloads contain version 1, endpoint key, revision and issuance/expiry.
Record payloads also contain direct addresses, relay locators and services.
Ed25519 signatures cover `rds/endpoint-record/v1\0` or
`rds/endpoint-delete/v1\0` plus the exact postcard payload. The JSON envelope
has an untrusted key hint; verify the bounded signature before decoding fields,
then require agreement with the signed key. Unknown versions, trailing payload
bytes, mismatched identity and invalid fields are refused.

A higher revision replaces the previous mutation. An exact signed retry at the
same revision succeeds without rewriting disk or extending validity. Different
content at that revision and all lower revisions are stale, including a record
after a newer delete. A legitimate later publication can supersede a delete.
Tombstones prevent replay; GDS unenrollment is a separate authorization policy.

Limits are 8192 payload bytes, 32 direct addresses, 8 relay locators of at most
512 bytes each, and the six distinct service kinds. Sequence decoders reject
oversized length hints before reserving their capacity. Direct addresses require
a nonzero port and a specified, nonmulticast IP. HTTP(S) relay origins require
a host and no credentials, query, fragment or non-root path. Owned transport
locators use `rds-relay://<32-byte-hex-public-key>@<IP>:<port>`: the public key
occupies URI user-info, passwords are forbidden, and the socket must be an IP
literal with a nonzero port. IPv6 literals use brackets. `OwnedRelayRoute` is
shared by record validation and the noq adapter; no independent URL parser can
silently discard the identity or mishandle IPv6 brackets. Repeated fields are
refused. A record lives at most 3600 seconds; a delete at most 300 seconds.
Acceptance requires `issued_at <= now < expires_at`, including retries. Stores
refuse expired records on reads; clients verify freshness independently.
Server expiry also obeys the retained lease and clock rules below.

## Expiry, clock rollback and collection

Acceptance persists the signed operation with its original boot identifier,
wall-clock acceptance time and suspend-inclusive continuous-clock deadline.
The existing OS clock adapter is shared with policy leases. A process restart
in the same OS boot retains that deadline. An OS reboot invalidates previously
accepted leases; a newer signed revision is required, even when wall time still
falls inside the original validity interval. Exact retries never rearm a lease.

Every storage operation samples time inside its owner lock. Backward wall time
below the last runtime observation or the last committed clock floor refuses
access until time catches up; this refusal alone does not poison storage.
Each mutation/retirement transaction raises the durable wall floor. Reads do
not write that floor while the record remains valid: the original continuous
deadline still bounds its lifetime across a same-boot process restart. An
unobserved clock excursion cannot be reconstructed; there is no promise to
detect rollback of the whole state directory without the external GDS anchor.

When a read observes an invalid lease, it commits retirement before returning
expiry. Deletes continue to read as not found. Retirement keeps the endpoint's
revision, a BLAKE3 digest of the exact signed mutation (including its kind), and
whether it was a deletion; it removes the addresses, services and signature.
The floor is trusted, integrity-protected local database state, **not** a
transferable publisher-signed proof. Never import a bare floor from a peer or
use it as independent enrollment evidence. Conflicting equal revisions and
lower revisions remain stale; an exact retired retry returns expiry. A valid
higher revision can replace the floor. Identity history has no TTL: only a
future authenticated retirement/migration procedure may release its slot.

`RecordStore::collect_expired` inspects at most 64 identities per call, rotating
over the catalog. `Directory` schedules a pass every second, with one owned
blocking maintenance job separate from request-worker capacity. It skips
missed ticks, never overlaps passes and stops scheduling on drop. A started
blocking transaction may finish after drop. Storage mutex/transaction ordering
prevents collection from deleting a concurrent successor. With 4096 identities,
a sweep takes up to 65 scheduled passes; slow storage can delay it further.
Reads always enforce expiry independently. No unknown filesystem orphan is
removed. Database pages are reusable; physical file shrinking is not promised.
Metrics count retired entries and collection failures; `len()` counts retained
record content awaiting collection, not currently reachable peers.

## File-capacity harness

`rds-bench directory-capacity --state-dir <new-directory> --rounds 2
--json <report.json> --md <report.md>` fills the file store with 4096 synthetic
publishers. It uses the public storage bounds `MAX_RECORD_IDENTITIES` and
`MAX_RECORD_DATABASE_BYTES`, not copied benchmark-only limits. Production values
remain unchanged. The command refuses an existing state directory, never cleans
up history, and leaves its synthetic database for inspection on success or error.

Each even-numbered round count (2–64) exercises alternating small/large records,
then deletes and reactivates half the identities. Large fixtures fill all address,
raw relay-locator and service-count bounds; their actual signed payload size is
reported. Every round reopens and compares the complete catalog against expected
signed bytes. New admission at full identity capacity must fail without charging
a callback or poisoning later renewals, including after deletion and a collection
pass. This does not collect or modify production state.

Reports include per-operation latency, sampled logical/allocated database bytes
and reopen latency. Signing and post-call file statistics are excluded from timed
mutations; signature verification and both durability commits are included.
Reopen includes whole-catalog validation. A failed invariant stops the command
without manufacturing a successful report. These are storage measurements, not
connection RTT, physical power-loss tests, filesystem-exhaustion injection or a
production service-level guarantee. The [capacity receipt](reports/rds-capacity-20260924.md)
records the run configuration and remaining qualification.

## Publisher ownership

`RecordIssuer` owns the signing key in memory and a protected state directory
containing `publisher.json` and `publisher.lock`. It commits the next revision,
signed pending mutation, checksum and issuance clock floor before returning
bytes for publication. No secret key is written into that state. A restart
reuses the exact pending bytes. Missing initialized state, corruption, changed
identity, backward time below the persisted issuance floor or a failed commit
never resets the counter. An uncertain commit closes the issuer until reopen.
The observed wall clock also cannot regress within a running issuer.

`rds-agent --record-state` defaults to the key path with its extension replaced
by `publisher-state`. Open it before binding the endpoint; there is no volatile
production fallback. Embedding tests and the synthetic benchmark explicitly
choose `RecordIssuer::memory`. Restoring an entire old publisher directory needs
an external GDS anchor; a server revision conflict is fatal, not a signal to
guess a larger counter or trust unsigned server state.

The announce loop serializes disk jobs off the async runtime, observes address
changes at most one second apart, and renews unchanged data after one third of
its lifetime. Network retries reuse the current signed operation until a renewal
or actual data change. Dropping the owner prevents late publication from a
local disk job that finishes afterward. An already-sent request can still
commit remotely; revision ordering and exact retry handle that uncertainty.
Fatal issuer failures and permanent
HTTP 4xx protocol refusals reach `Announce::wait`; the CLI supervises that result
and closes its endpoint. Network failures, timeouts and rate limits retry.
An HTTP 410 allocates and commits one local successor on the next poll; its
subsequent network retries reuse the new exact bytes. No remote revision is
imported. This allows an active publisher to recover after server reboot on its
next publication (normally within TTL/3), without waiting for its pending bytes
to age out. A server restart does not yet push an immediate refresh request to
already acknowledged publishers.

## Transaction and recovery contract

`RecordStore` owns the interface. `MemoryStore` serializes mutations under one
write lock. `FileStore` uses redb 4.3, an embedded Rust library, with no database
daemon, command wrapper or SQL runtime. A transaction updates the signed record
or delete tombstone and catalog metadata together. Deletion preserves its
ordering floor; it does not remove the identity's history.

The file store holds one process-wide mutex through the database's immediate
durability commit and a separate atomic generation-anchor commit. Only after
both commits can readers observe the update or a successful mutation return.
Unexpected storage/validation errors poison that instance, requiring reopen;
semantic stale writes and capacity refusal do not poison it.

On open, validate every bounded row's signed content/lease or retained floor,
key, metadata counts, database identity and anchor. Missing initialized files,
corruption, rollback or unknown
files cause an error, never an empty-store fallback. A database exactly one
generation ahead of its anchor represents an interrupted, unacknowledged
mutation: validate it and finish its anchor. A database behind the anchor is
refused, including if the database engine repaired to an older committed root.
An unacknowledged operation may therefore be present after recovery; callers
must not assume that a lost reply means no mutation occurred.

The anchor detects accidental corruption and rollback of the database alone.
It cannot detect restoration of the entire trusted state directory, or an
administrator deliberately rewriting both files. External GDS anchoring remains
W4. Process exits and injected I/O faults are tested; physical power loss and
filesystem/device failure qualification are outstanding.

## Filesystem ownership and budgets

The configured ancestors and OS identity are trusted. The final directory is
opened without following a symlink and protected with mode 0700. Files are
created mode 0600; descriptor-relative opens reject symlinks and nonregular or
multiply linked files. Exclusive OS locks cover the directory owner and the
database descriptor for their lifetimes. Unsupported locking is an error.
redb receives that descriptor through an owned backend; it never opens a path.

Files are `records.redb`, `records.anchor` and `records.lock`. The lock's marker
distinguishes initialized state from a new directory. Atomic anchor staging
uses exclusively created `.policy-*.tmp` names shared with the policy-state
primitive; unknown orphans are left untouched. More than 129 directory entries
requires maintenance. Never remove locks, anchors or apparent leftovers while
a service is running.

Both stores permit at most 4096 remembered identities (including tombstones
and retired floors); `MemoryStore::with_capacity` may lower that limit.
Disk limits are 256 KiB per serialized cell, a 256 MiB database file and a
16 MiB database cache.
The backend checks growth and offset arithmetic before writing. These are
storage safety bounds, not total process-memory bounds. Identity-capacity
refusal does not block an existing identity's higher revision. Capacity
reservation at the database file limit is still unqualified. Expiry does not
free identity slots.

## Enrollment and write admission

`ServiceConfig` defaults to an empty publisher allowlist. Production
`rds-server --directory-allow <base32-device-key>` provisions it explicitly,
independently of relay membership, agent permissions and registry names.
At most 4096 distinct nonweak Ed25519 keys are accepted. Unknown keys cannot
publish, fetch or delete endpoint records (HTTP 403). Removing a key and
restarting hides its stored record while retaining replay history. It does not
close existing endpoint sessions; those use grant revocation. Static membership
is a provisioning boundary until the W4 GDS reconciler supplies live policy.
There is no production open-enrollment flag. Synthetic tests/benchmarks explicitly
choose `ServiceConfig::open_ephemeral`; raw stores remain storage primitives,
with the enrollment boundary in the directory service.

The envelope's key hint can cheaply reject unknown publishers but never
authorizes a write. Both stores verify signatures, key agreement, lifetime and
revision before invoking an admission callback under the same owner lock used
for compare/commit. The callback sees whether any identity history already
exists, including a deletion/expiry floor. Only a valid higher revision invokes
it. Exact signed retries, stale/conflicting revisions and forged operations do
not spend the owner's quota. Concurrent copies of a new operation charge once.
A refused callback leaves the mutation unapplied and does not poison storage.
Callbacks must be bounded and must not reenter the store. The ordinary `put` and
`remove` embedding APIs use an accepting callback; custom `RecordStore`
implementations must uphold the `put_admitted`/`remove_admitted` contract.

Accounting uses fixed 60-second monotonic windows, bounded to 4096 writer slots.
Defaults are 120 admitted mutations per identity, 600 new identity admissions,
and 600 shared extra writes per window. Check the identity's limit and optional
minimum spacing **before** charging shared capacity; a quota refusal debits
neither budget. Each remembered identity additionally gets one protected new
mutation per window, independent of new admission and shared extra traffic.
At 4096 identities these protected slots exceed the old single 600-write budget.
Counters reset with the directory process, not with HTTP connections or record
expiry. These are fixed windows, not strict sliding-window or byte-rate quotas.

TTL/3 renewal with TTL at least 180 seconds fits the protected cadence; the
default 300-second publisher uses it. Shorter TTLs and extra address changes
depend on burst capacity. PUT and DELETE share the identity limit. Deleted and
collected identities retain known-device classification. Successfully admitted
operations spend capacity before commit; an I/O failure does not refund that
attempt. An exact retry after a successful but lost reply does not debit again.

Registry and revocation updates each have a separate 60-new-revision budget.
Their admission callback runs after signature/freshness/order validation under
the policy lock and before persistence. Captured duplicates and invalid/stale
snapshots do not consume it. Endpoint traffic cannot spend policy-role capacity.

Connection counts, bounded bodies, absolute request deadlines and owned worker
permits bound pre-authentication resource use. The old unauthenticated global
write counter is removed: it let a small replay/refusal flood deny everyone
for a minute. This change protects write admission against one enrolled abusive
publisher; it is not isolation from arbitrary network floods, shared CPU,
filesystem saturation or OS failure. Production throughput and file-capacity
renewal reserve still require measurement. Membership removal alone does not
recycle stored identity slots; authenticated retirement remains W4/migration.

Metadata-only [observations](observability.md#durable-catalog-and-policy-observations)
are published by these owners after commit/reopen. Busy, failed and released
owners remain explicit; exporting telemetry takes no transaction locks and
cannot renew a lease or change stored ordering. No persistence format changes
are required for the observation adapter.

## Migration and operations

The previous per-key JSON directory and experimental format-1/format-2 databases
are not accepted or automatically imported. The leased database uses format 3
(`records-v3` and `metadata-v3`). Stored postcard values reject trailing bytes.
Unknown legacy files are preserved and startup refuses them.
`rds-server migrate-v2 --source <old> --destination <new-sibling>` explicitly
verifies and imports format-2 revisions as retired floors while preserving the
source bytes. The complete new directory is published only after validation and
a durable receipt. See [operation and remaining qualification](record-migration-plan.md)
before cutover. Timestamp format-1 and per-key legacy JSON still require a separate
authenticated procedure. Do not delete the legacy directory or start an empty
replacement to bypass refusal: that would discard replay history. Migration
availability is not deployment acceptance. Legacy signatures without a domain prefix are
rejected by the new wire format; consumers and publishers must upgrade together. Never invent
publisher revisions from wall time or copy one issuer state into multiple live
agents. Synthetic tests use new isolated
directories only.

Back up the entire directory while the service is stopped; a live copy can
mix database and anchor generations. Recovery requires the matching complete
state and independently verified GDS history when replay floors might regress.
Do not repair by deleting an anchor. Disk-full or sync failures close the store;
restore capacity, preserve evidence, then reopen and verify recovery.

## Dependency decision

redb supplies copy-on-write transactions and crash recovery in Rust, replacing
the old multi-file read/compare/write sequence. Implementing our own database
would add page integrity, reclamation and recovery obligations to RDS. The
adapter stays behind `RecordStore`; wire semantics, identity policy, limits and
the generation anchor remain ours. Version 4.3.0 is locked, MIT OR Apache-2.0,
MSRV 1.90; the workspace compiler is newer. Its only dependency added to this
workspace graph is redb itself (libc was already locked). No experimental
features are enabled. This is not a dependency-security audit.

Upstream references: [design](https://github.com/cberner/redb/blob/master/docs/design.md),
[custom storage backend](https://docs.rs/redb/4.3.0/redb/trait.StorageBackend.html).
Immediate durability and commit-error behavior were also checked in the locked
4.3.0 `transactions.rs` source. Native macOS execution remains outstanding.
