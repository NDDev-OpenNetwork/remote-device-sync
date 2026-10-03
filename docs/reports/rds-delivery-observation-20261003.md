# Delivery observation follow-up — 2026-10-03

W6.6/W6.7 and observability follow-up, without a completed stability gate.
Installed qualification of the payload-aware controller still found a multi-
second media pause with fresh control echoes. Receiver metadata localized one
independent picture to a slow body read, and serving metadata showed the
keyframe receipt barrier holding further capture. The precise transport cause
was not established; no endpoint identities, private runtime data or payloads
are part of this public receipt.

Ordinary health and reduction records now include the selected path id,
instantaneous congestion-window bytes and cumulative UDP sent/received bytes.
A path change invalidates cross-path counter subtraction. UDP counters cover
the entire connection's selected path, including other services; they are not
an isolated media-goodput or available-capacity estimate.

Successful receipts that previously crossed their soft deadline now produce
an info completion record, pairing the initial warning's sequence with its
keyframe flag, payload size and enqueue/receipt/total duration. Existing timely
completion trace records, independent five-second production health and receiver
slow-body logs remain. This allows diagnostics to distinguish a frozen producer,
waiting keyframe, slow enqueue and slow transport receipt without requiring
per-frame verbose tracing. No keys, input values, clipboard data or pixels are
logged, and no wire, deadline, recovery or scheduling behavior changes.

Verification on macOS arm64: fmt, strict workspace all-target desktop-feature
clippy and all 45 session tests passed. Native Linux/macOS CI and an installed
receipt remain separate. This increment adds diagnostic fields and does not
assert a network or click-to-visible latency improvement.
