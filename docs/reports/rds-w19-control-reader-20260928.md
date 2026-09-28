# Shared control-reader hardening — 2026-09-28

Scope: W1.9 peer-ignored-cancel hardening. Linux, synthetic loopback
fixtures incl. a deliberately non-cooperative peer. No native macOS,
physical power-loss or real-network qualification is claimed.

## What existed

Typed `Cancel` and the chunk-boundary barriers (receipt:
`rds-w19-cancel-barriers-20260928.md`). The remaining gaps:

- `serve_inner` had **zero** cancel coverage: a peer `Cancel` during its
  manifest scan or `Need` wait was invisible — no reader was even
  draining the control stream outside whichever phase next called
  `wire.recv`, so an abort only surfaced when a phase happened to read.
- Blocking work saw only its own ad-hoc flags; `push_chunks` watched the
  raw control stream itself and dropped a peer `Cancel` without queueing
  it for the phase that needed its reason.
- Teardown depended on drop order: `send.reset(0)` fired even after a
  gracefully finished send, so a written `Refuse`/`Cancel` was retracted
  on the wire before the peer could read it — the typed abort contract
  silently degraded to a bare reset (this bug also existed on main for
  the caller-cancel path).

## Implementation

One owned reader per transfer, shared by every phase on both roles.

- `ControlFrames` — a spawned reader owns `recv` for the transfer's
  life. It decodes through `Wire::decode` (extracted so reads carry no
  stall bound; legitimate phase gaps outlive `READ_STALL`), queues
  frames in arrival order for `next(stall)`, and flips a shared `stop`
  flag the instant a `Cancel`, decode violation, flood or stream death
  is seen — including while a blocking filesystem phase runs and nothing
  polls the queue. Its epilogue always runs `recv.stop(0)`, the previous
  `Control::drop` contract, now unreachable-by-omission-proof.
- `Drop` cancels the reader via a `CancellationToken`; `close()` joins
  the task so the `recv.stop` epilogue completes deterministically.
- `serve`, `send_file_cancel`, `recv_file_cancel` destructure the
  streams, negotiate v2 first, then arm the reader — so a version/ID
  refusal never spawns a reader on the wrong wire profile.
- The shared `stop` flag reaches `manifest_from_file` on **both** roles
  (serve-side manifest scans now abort on peer cancel too) and merges
  with the caller-token flag in `receive`/`receive_chunks`, so a peer
  `Cancel` stops journal assembly at the next verified chunk and the
  destination is never published after the abort.
- The reader records the earliest terminal *cause* (peer cancel reason,
  undecodable frame, flood, stream end). When `STOP_SENDING` from our
  own teardown outruns the queued `Cancel`, the `stopped` select arm now
  reports the recorded cause — "peer canceled: caller canceled" — not
  the transport symptom.
- Reset-after-finish is eliminated everywhere: terminal frames
  (`Refuse`, typed `Cancel`) are sealed with `send.finish()`;
  `send.reset(0)` runs only when the send was never finished — the
  previous `Control::drop` reset semantics, made explicit. A canceled
  receive's typed `Cancel` now actually reaches the peer.
- `recv_done` validates the `Done` root against the manifest root and
  still surfaces an early `Done` captured while chunk tasks drained;
  phase waits report `Refuse`/`Cancel` with their reasons instead of
  "expected X".

## Verification

- `control_reader_flags_peer_cancel_before_consumption` — a real
  endpoint pair: `stop` flips on decoded `Cancel` before `next` runs,
  and the frame still arrives in order with its reason.
- `control_reader_preserves_order_and_reports_peer_fin` — ordered
  frames, `stop` clear, peer FIN ends the reader and fails `next`.
- `control_reader_close_stops_the_peer_send` — `close()` delivers
  `recv.stop` on the wire; the peer's send half observes it.
- `v2_peer_cancel_before_request_aborts_serve` / `..._during_pull_serve`
  — a typed `Cancel` as the first message and mid-scan both abort the
  serve promptly with a cancel-bearing outcome.
- `v2_recv_cancel_detaches_even_when_peer_ignores_it` — a deliberately
  stubborn peer that holds every stream open and never answers our
  `Cancel`: our receive still returns `canceled` promptly, never
  publishes the destination, and a second transfer on the same
  connection completes under its fresh route ID.
- `v2_token_cancel_writes_typed_cancel_and_server_observes_abort` —
  strengthened: the peer now reports the typed cancel reason, proving
  the sealed `Cancel` survives on the wire.
- `grant_scopes` e2e — Refuse delivery preserved under the new teardown.

Checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets --
-D warnings`, `cargo clippy --workspace --all-targets --features
rds-desktop/x11 -- -D warnings`, `cargo test --workspace` — all green
locally on Linux x86_64.

## Still open under W1.9

Native macOS qualification and physical power-loss qualification.
