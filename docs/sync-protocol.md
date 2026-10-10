# Single-file sync protocol contract

Status: W1.9 request binding, wire bounds, unique chunk accounting and session
budgets are implemented. Managed transfers run the negotiated version-2 session
(`SyncTransferV2` route + `SyncMsg::Session` envelope); the version-1 routes
(`Sync`, `SyncTransfer`) remain for compatibility. Physical cancellation and
native-platform qualification remain open in W1.9/W2/W8. This document does
not claim a complete directory synchronization product.

## Session negotiation (v2)

A managed transfer opens a `StreamHello::SyncTransferV2 { id }` control stream.
A peer that cannot decode the variant refuses at the greeting — before any
filesystem operation — with no silent fallback, matching ALPN posture. After
`HelloAck::Ok`, every frame on the control stream and on the transfer's
`UniHello::SyncTransferV2 { id }` chunk streams is wrapped in
`SyncMsg::Session { transfer_id, msg }`; a frame carrying any other transfer ID
fails the transfer.

The first envelope is `SessionMsg::Hello { version, limits }` from the opener;
the responder answers `SessionMsg::HelloAck { version, limits }`. The version
is bound by the route tag (currently 2) — a mismatched `version` field refuses.
`SessionLimits` declares `max_chunk`, `max_chunks` and `fetch_streams`; the
session runs at the pairwise minimum, never wider than the tighter peer or this
implementation's compiled bounds. Zero-valued limits are unusable and refused.
Negotiation completes before any manifest read or filesystem mutation.
`SessionMsg::Hello`/`HelloAck` after the greeting, or a `Session` frame on a
v1 stream, is a protocol violation; on v1 a `Cancel` still maps to `Refuse`.

## Binding and completion

A pull request's normalized relative path must equal the normalized Offer path
before reading a manifest or opening receive state. A peer cannot substitute a
different valid path inside the configured root. Existing confinement rules still
apply independently: no traversal, absolute path, journal namespace or symlink
escape. The private `.rds-sync` namespace is reserved at every depth, including
case variants. Local push basenames must be UTF-8 and must not normalize into another
path. The source inode remains pinned through manifest and positioned chunk reads.

Both push and pull senders require `Done.root` to equal the offered manifest root.
The receiver sends Done only after draining successful chunk stores, checking
complete journal state, assembling and verifying the root, and completing the
[durable replacement and journal recovery sequence](sync-journal.md). On the v2
route every transfer is additionally scoped to its negotiated transfer ID:
frames for a different transfer fail, and a delayed chunk stream from a canceled
transfer can only reach a fresh route (IDs are minted per attempt and never
reused on a connection), never a replacement transfer. The legacy v1 routes
scope completion to the control stream and manifest only.

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
Ordinary terminal control retains and joins the chunk-store worker before
returning its receive error, with a bounded cleanup deadline and disposal of its
locked output. Waiter-drop and uninterruptible filesystem limits remain as
described below.
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

Expected phase-frame reads/writes, chunk body reads/writes, waiting for a chunk stream
and opening an outgoing chunk stream retain a five-minute stall bound, additionally
limited by the absolute session budget. Empty batches and streams fail immediately.
The default permits disk-bound work on slow devices; it is not a measured latency
target. Zero or overflowing absolute budgets are rejected before transfer work.

While chunk streams are active, both wire profiles supervise terminal control
traffic without an independent control-idle timeout: silence on that stream is
normal while data progresses. `Refuse`, v2 `Cancel`, control EOF and unexpected
frames end the data phase; data-I/O stalls and the absolute session deadline
remain enforced. The owned reader projects refusal/cancellation/EOF to blocking
work before a phase consumes its queued message. An early `Done` is retained by
the sender and validated after its owned chunk tasks have drained.

Cancellation drops the async operation, its owned sender task set, and receive
queue producer. Preparation and assembly own drop guards that project abandonment
into queued/running disk work; the writer discards stores that have not started.
Filesystem syscalls already executing on the blocking pool cannot be canceled.
Assembly checks cancellation before staging, between chunks and before publication.
If cancellation races an already-started rename/durability sequence, completion
remains uncertain: reconcile destination content before claiming rollback or
retrying conflicting work. No `Done` is emitted by the canceled operation. The
journal's root lock remains owned until disk work releases it. Quotas and broader
per-session resource policy remain W2.5/W8, not an implied guarantee here.

On the v2 route cancellation is additionally typed: `send_file_cancel` and
`recv_file_cancel` take a `CancellationToken` (the managed client binds the
session's entry token), and a triggered token writes `SessionMsg::Cancel` on
the control stream so the peer observes a deliberate abort rather than a bare
stream reset. The receiving side watches the control stream during collection,
so a peer `Cancel` stops chunk receive deterministically instead of waiting for
the absolute deadline. A v1 stream cannot express `Cancel`; sending one there
maps to `Refuse`, which the peer also observes during the data phase.

## Validation

[The 2026-09-25 receipt](reports/rds-sync-protocol-20260925.md) records the failing
baseline cases, real-transport regressions, compatibility checks and remaining
qualification. Existing transfer, resume, corruption, impaired-link and combined
desktop/sync fixtures remain part of the workspace check matrix.
