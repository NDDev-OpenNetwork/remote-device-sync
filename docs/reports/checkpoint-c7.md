# Checkpoint C7 — 20260922-105321

Verdict: **pending review**

## Automated checks
- fmt/clippy/test: PASS (default + transport-noq lanes)
- metrics known-traffic (3 tests): PASS
  - direct echo → via=direct counters only; bytes_sent/recv cover payload
  - relay-only ticket + path pinning → via=relay counters only
  - prometheus render emits every name the bench reports cite
- bench report G7: metrics block embedded from endpoint registries
  (client_/agent_ prefixed snapshot at run time): PASS
- directory /v1/metrics: per-endpoint PUT counters under anonymized
  blake3-16 labels; raw key + peer addr absent from body: PASS
- exposure: /v1/metrics is loopback-only at the router (non-loopback
  peers get 404) — remote scrape via SSH/local exporter, documented in
  architecture.md
- session spans: rds.conn{peer, session_id} ⊃ rds.stream{service}
  on every agent connection; sync engine logs session events
- QNT attempt/success counters driven by the noq policy driver;
  iroh reports paths_seen{via=direct} as the equivalent signal

## Manual checklist (fill before merge)
- [x] All "not run" items above explained — every check ran
- [x] Reports committed: bench-20260922-c7-metrics.{json,md}, this file
- [x] docs/ updated: architecture.md observability section documents
  the registry, via-split, QNT scope and the loopback-only scrape
- [x] security/unsafe review done: no new unsafe; metrics labels are
  blake3-16 prefixes (no keys/addrs/content); /v1/metrics gated to
  loopback peers at the router
