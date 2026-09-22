# Checkpoint C5 — 20260922-100118

Verdict: **pending review**

## Automated checks
- fmt/clippy/test: PASS (default + transport-noq lanes)
- session_v2 e2e (iroh + noq): PASS
  - header roundtrip + input metadata: rds-core unit tests
  - keyframe request roundtrip: PASS
  - bounded queue + newest-wins (240 fps pressure): PASS
  - input acks + heartbeat RTT: PASS
  - impairment 5% loss + 30ms jitter via ImpairingSocket (underneath
    QUIC, migration-immune): PASS — counters prove real drops
  - G5 latency: clean-link p95 ≤150ms asserted in soak lane;
    impaired lane asserts queue_p95 ≤100ms (protocol queues bounded),
    lat_p50 ≤500ms (median at path speed), tail ≤2s/3s (retransmit
    physics, not queueing) — split via capture/send/deliver ts
- soak 60fps: 20s smoke PASS; full 30min via RDS_SOAK_SECS=1800
  (run before merge or noted honestly)
- FrameHeader decoder + stream demux fuzz (proptest): PASS

## Manual checklist (fill before merge)
- [x] All "not run" items above explained — the 30min soak is the only
  conditional row and is noted honestly (20s smoke ran in-gate)
- [x] Reports committed: this file; C5 evidence is the session_v2 e2e
  suite (bench artifacts belong to the C1/C7 waves)
- [x] docs/ updated: architecture.md stream/media protocol sections
- [x] security/unsafe review done: review found a parked-`recv`
  lost-wakeup in `mailbox` (fixed: `Sender::drop` notifies; regression
  test `parked_recv_wakes_when_last_sender_drops`), an unbounded X11
  scroll loop (clamped at 32 clicks/event), and per-event display_id
  now checked against the session display — all fixed in this wave
