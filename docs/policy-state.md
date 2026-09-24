# Durable policy state (W1.4)

This is the implemented policy acceptance contract. GDS issuer automation,
enrollment and recovery anchoring remain W4; native macOS qualification remains
open. This change does not close the W1 wave or establish product readiness.

## Signed ordering and limits

Registry snapshots, individual name proofs and revocation snapshots carry
`SnapshotStamp { authority: { key, epoch }, revision }`. Epochs and revisions
are positive integers. The issuer must allocate revisions durably, independently
for each policy stream; timestamps are not a revision allocator. Multiple updates
in the same second are valid. Each new revision is a complete replacement.

| Object | Signature prefix (followed by postcard payload) | Limits |
|---|---|---|
| Registry | `rds/registry/v2\0` | 256 names; 256 KiB HTTP JSON body; 24-hour validity |
| Name proof | `rds/name-binding/v2\0` | Version 2; exact requested name; 256-byte raw payload; 24-hour validity |
| Revocations | `rds/revocations/v1\0` | 1024 grant hashes; 40 KiB raw payload; 300-second validity |
| Authority rotation | `rds/authority-rotation/v1\0` | Both authorities sign; 1024-byte raw payload; 1-hour acceptance window |

Signatures are checked before payload deserialization. All validity checks
require `issued_at <= now < expires_at`; there is no allowance for future
snapshots. A lower revision or differing payload at the same revision is
refused. Identical retries are idempotent and never renew a lease. Names include
the BLAKE3 digest of the registry payload, its revision and identical validity
interval. The client remembers a global registry revision and up to 1024 name
proof hashes, refusing overflow rather than evicting anti-rollback history.
Receiving a proof for one name discloses no other inventory entries.

## Commit and restart

`PolicyStore` owns one protected directory and a lifetime-exclusive file lock.
Directory and staging permissions are 0700 and 0600. Descendant operations use
no-follow directory-relative handles; nonregular and multiply linked state or
lock files are refused. Configured ancestors and the local service identity are
trusted. The state file is bounded to 2 MiB and stores signed snapshots,
revision/authority history, lease metadata and a corruption checksum together.

Acceptance verifies and compares, writes an exclusively created random staging
file, syncs the file, renames it over the state, syncs the directory, then
publishes runtime policy. A failed commit poisons that store instance: it cannot
authorize, serve cached policy or advance memory. Reopen validates the complete
on-disk state. A lock-file initialization marker detects loss of the committed
state file; neither missing initialized state nor corruption causes an empty
policy fallback. A lower-revision startup registry cannot replace a committed
newer registry. Unknown staging orphans are never removed opportunistically.

The checksum detects accidental corruption, not modifications by a trusted OS
identity. Restoring or deleting the entire protected state, including its marker,
cannot be distinguished from first enrollment locally. Protection against that
requires an external GDS revision anchor. Do not delete policy directories as a
recovery procedure. Process-crash tests are not power-loss qualification;
hardware cache semantics and native macOS durability still need their lanes.

## Freshness and active access

A lease persists the boot identifier, accepted wall time, accepted continuous
time and absolute continuous deadline. It expires at the earlier of signed wall
expiry and that deadline. The OS continuous clock includes suspend. A wall or
continuous reading below its accepted value makes the lease unusable.
Replaying a snapshot or restarting the process does not recalculate
its deadline. A new OS boot requires a strictly newer authority revision;
an old cached snapshot cannot rearm access. The issuer must publish periodic
revisions even when membership is unchanged. See [platforms.md](platforms.md)
for the clock adapters and primary references.

Managed grant admission starts closed until a verified fresh snapshot exists.
The feed loads a valid same-boot cache before serving. Revoked IDs and freshness
are one watched value. Bad signatures, HTTP failures, missing snapshots or older
responses preserve that value only until its original expiry. Disk uncertainty
closes admission immediately and stops the feed; restart is required after the
storage issue is fixed. Stopping the feed invalidates its ownership atomically;
a delayed result cannot republish access or overwrite a replacement feed.

The grant state machine checks policy at admission, admission commit and each
new service request. Its live watchdog checks updates and polls clock validity
at most every second. On invalidation it closes the QUIC connection. Healthy
revocation propagation is poll interval (1–60 seconds, default 30) plus fetch,
validation and durable commit. Under network withholding, authorization lasts
no longer than the current snapshot's remaining lease (at most 300 seconds).
Live closure adds up to one watchdog tick and scheduler delay; this is not a
hard real-time deadline. Grant expiry can terminate earlier. Suspend carries no
traffic; validity is checked on resume. General agent task teardown remains W2.5.

The explicit embedding API `use_local_revocations()` selects local policy; it
has no managed freshness bound. The production CLI never selects it for grant
mode. Allowlist-only operation remains an explicit separate configuration.

## Configuration and migration

Upgrade issuer, directory, agent and name clients together. Re-sign snapshots
using `publish(key, epoch, revision, contents, ttl)` or stamped `sign` payloads.
Old unsigned/undomained snapshots and version-1 name proofs are rejected.
Provision verifying keys independently of directory replies; signing keys stay
with the authority. Tickets and pinned endpoint IDs retain their separate trust.

| Process | Configuration | Default durable directory |
|---|---|---|
| Directory | `--registry-key`, `--registry-epoch`, `--policy-state` | `--directory` with extension replaced by `policy` |
| Name CLI | `--registry-key`, `--registry-epoch`, `--registry-state` | Endpoint key path with extension replaced by `registry-state` |
| Managed agent | `--issuer`, `--directory`, `--revocations-key`, `--revocations-epoch`, `--revocations-state` | Endpoint key path with extension replaced by `revocations-state` |

Epoch defaults to 1. State directories belong to their respective service
identities; the directory, agent and CLI must not share one store. CLI processes
using the same authority share short exclusive transactions in their configured
directory. Use a separate directory for a different estate/trust root. Persistent
storage never falls back to memory. Library `PolicyStore::memory` and
`Client::with_registry_key` are explicitly ephemeral embedding/test options.
The feed API takes a `PolicyStore` and returns an owned `RevocationFeed` handle;
keep it alive for the full managed service lifetime.

All three binaries accept repeatable `--authority-rotation <receipt.json>` in
epoch order. The receipt must carry valid signatures from both the old key and
the new, different key, and advance epoch by exactly one. At most 16 transitions
are retained. Keep the original bootstrap configuration: the authenticated chain
advances the current authority. An unknown replacement root is not recovery.
An already committed identical receipt remains valid for restart after its
delivery window; an uncommitted expired receipt is refused. Rotation clears
old-authority cached policies, so access stays closed until new-authority
snapshots arrive. Replace/remove an old-key registry bootstrap file as part of
that rollout. Online rotation delivery and chain compaction remain W4.

Directory storage/verification runs in a blocking pool bounded to 16 jobs per
service by default (`Limits::max_workers`). A worker keeps its permit after an
HTTP timeout until its disk job ends; saturation returns 429. Connection lifetime
still bounds asynchronous requests. The CLI name acceptance phase has a separate
two-second deadline in addition to the default three-second network exchange;
an already running disk job may finish after timeout without returning a result
to the canceled caller. Endpoint-record transactions/quotas remain W1.5.
