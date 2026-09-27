# Shared deadline classes + cancellable retry — 2026-09-27

Scope: W2.6 client-side remainder. Linux, synthetic loopback fixtures.
No backend promotion, deployment or remediation wave closure.

## What existed

The agent already owned the four serving-side classes (`TimeoutPolicy`:
handshake, hello, authz, shutdown — configurable `timeouts.*_secs`,
validated 1..=3600), and directory publish retries already used bounded
exponential backoff with equal jitter (`RetryPolicy`). The transfer engine
already carried progress bounds (`READ_STALL`, `PHASE_STALL`,
`TRANSFER_TIMEOUT`); transport path liveness carried the idle bounds
(5s keepalive, 15s path idle, 10s stream greeting).

What did not exist: the six classes as one named taxonomy, a shared
cancellable retry primitive, and a `dial` class an operator can choose —
the connect bound was a crate-private 30s constant.

## Implementation

`rds-net/src/deadline.rs` names the six W2.6 classes:

- `TimeoutClass` — Dial, Handshake, Authz, Idle, Progress, Shutdown.
- `DeadlinePolicy` — one `Duration` per class; `DEFAULT` is `const` so
  transport enforcement sites name a bound at compile time; defaults are
  the constants the sites already used (dial 30s, handshake 15s,
  authz 15s, idle 15s, progress 300s, shutdown 5s). `validate()` rejects
  a zero or >3600s deadline *per class*, returning the offending
  `TimeoutClass`.
- `retry_wait(policy, failures, cancel)` — the shared bounded retry
  sleep: `RetryPolicy::delay` backoff raced against a caller's
  cancellation future. Reconnect/publish loops use it so a shutdown
  never parks inside a backoff.
- `RetryPolicy::delay` is now public — it is the shared bounded-backoff
  primitive `retry_wait` builds on; announce keeps using it directly.

Wiring:

- `rds_client::connect` now delegates to `connect_with_deadlines`, which
  validates the policy up front and bounds the whole attempt —
  hole punching and relay fallback included — by `policy.dial`.
- `backends::noq::dial::race` names `DeadlinePolicy::DEFAULT.handshake`
  as the per-candidate bound.
- `rds_agent::TimeoutPolicy` documents that its four fields are the
  serving-side subset of the shared classes; `rds_sync::READ_STALL`
  documents itself as the `Progress` class site.

No wire type, protocol version, dependency or external runtime program
changed.

## Tests

- `default_deadlines_cover_every_class_within_max` — all six classes
  present, nonzero, ≤ `MAX_DEADLINE`.
- `validate_rejects_zero_and_unbounded_per_class` — per-class attribution
  (Dial/Progress/Shutdown shown).
- `retry_wait_returns_retry_after_bounded_delay`,
  `retry_wait_cancellation_wins_during_backoff` — bounded sleep vs.
  cancellation racing, cancellation wins.
- `retry_delays_stay_bounded_so_reconnects_cannot_storm` — 64 consecutive
  failures never exceed `cap`; no reconnect storm is possible.
- `dial_deadline_fails_fast_against_a_silent_peer` — a peer that accepts
  nothing fails the dial inside a 200ms bound (startup does not park on
  an unavailable path).
- `invalid_deadline_policy_fails_before_dial` — misconfiguration is a
  fast error before any network work.

## Still open in W2.6

Desktop/media-specific deadlines (desktop session stalls, audio
cadences) remain wave-scoped to W6/W9 where those engines live.
Startup recovery beyond the dial bound — retry orchestration across a
dead directory *and* relay at boot — is the W3 recovery-policy scope.
