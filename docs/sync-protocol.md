# Single-file sync protocol contract

Status: W1.9 request binding, wire bounds, unique chunk accounting and session
budgets are implemented. Transfer IDs/version negotiation, physical cancellation
and native-platform qualification remain open in W1.9/W2/W8. This document does
not claim a complete directory synchronization product.

## Binding and completion

A pull request's normalized relative path must equal the normalized Offer path
before reading a manifest or opening receive state. A peer cannot substitute a
different valid path inside the configured root. Existing confinement rules still
apply independently: no traversal, absolute path, journal namespace or symlink
escape. Local push basenames must be UTF-8 and must not normalize into another
path. The source inode remains pinned through manifest and positioned chunk reads.

Both push and pull senders require `Done.root` to equal the offered manifest root.
The receiver sends Done only after draining successful chunk stores, checking
complete journal state, assembling and verifying the root, and completing the
existing durable replacement sequence. Transfer completion is scoped to the
current control stream and manifest. There is no new wire version or transfer-ID
field in this change; explicit cross-session IDs and negotiation remain W2.2.

## Reader and progress bounds

Every shared `rds-core` frame remains at most 64 KiB. Its postcard payload must
decode exactly; trailing bytes after an otherwise valid value are refused.
Valid message layouts and tags are unchanged.

| Input | Enforced receiver condition |
|---|---|
| Manifest | At most 262,144 chunks, each nonzero and at most `MAX_CHUNK`; contiguous coverage of the declared size |
| ManifestPart | 1–512 entries, no more than the remaining announced chunk count |
| Need | Exactly `ceil(chunk_count / 64)` words; unused high bits are zero |
| Chunk streams | At most four consumed streams; an empty stream is refused |
| ChunkSet | 1–4096 indices; every index was requested and occurs at most once per transfer |
| ChunkHdr | Index, length and hash agree with the declared set and manifest before body allocation |
| Chunk body | Exact declared length; content hash verified by the journal before durable acceptance |

The receiver claims its Sync uni-stream inbox before sending Need, so an immediate
response cannot arrive ahead of registration. A second live receive cannot claim
the same connection route. Requested indices form a bounded set; queued writes
cannot make duplicate indices count toward completion. The journal writer drains
before the final completeness check. A one-shot worker-exit signal wakes collection
on verification/storage failure or panic, including when the peer sends no further
frames. Error propagation does not depend on reaching the next queue send.
Send/receive byte counts describe the unique
payload actually transferred; identical reuse reports zero wire chunks and bytes.

## Time and ownership

The ordinary serve, send and receive entry points have a **one-hour absolute
session budget**, starting when that operation is invoked. Local scan/journal work,
control I/O and chunk transfer all consume the same budget. Receiving more valid
messages never extends it. Library callers can select a different nonzero budget
with `serve_with_timeout`, `send_file_with_timeout` or `recv_file_with_timeout`.
CLI/agent configuration still uses the default; unified configurable policy and
negotiation remain W2.1/W2.2.
The budget uses Tokio's monotonic clock and belongs to one process session.
Resume starts a new budget while retaining verified journal history.

Individual frame reads/writes, chunk body reads/writes, waiting for a chunk stream
and opening an outgoing chunk stream retain a five-minute stall bound, additionally
limited by the absolute session budget. Empty batches and streams fail immediately.
The default permits disk-bound work on slow devices; it is not a measured latency
target. Zero or overflowing absolute budgets are rejected before transfer work.

Cancellation drops the async operation, its owned sender task set, and receive
queue producer. Filesystem syscalls already executing on the blocking pool cannot
be canceled. Bounded queued work may drain and a started assembly may still finish
its complete atomic replacement after timeout. No Done is emitted by that canceled
operation. Treat completion as uncertain and reconcile destination content before
claiming rollback or retrying conflicting work; the journal's root lock remains
owned until disk work releases it. Stronger cancellation/publication barriers and
per-session resource accounting remain W2.5/W8, not an implied guarantee here.

## Validation

[The 2026-09-25 receipt](reports/rds-sync-protocol-20260925.md) records the failing
baseline cases, real-transport regressions, compatibility checks and remaining
qualification. Existing transfer, resume, corruption, impaired-link and combined
desktop/sync fixtures remain part of the workspace check matrix.
