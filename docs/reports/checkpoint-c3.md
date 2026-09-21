# Checkpoint C3 — 2026-09-21

Verdict: **PASS** — the discovery service is correct, hostile-input
safe, and the cold `rds ssh <name>` path beats the 300 ms budget by
~4× on LAN (p50 80.0 ms, p95 139.0 ms, 100/100).

## Automated checks

- `cargo fmt --check`: PASS
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `cargo clippy --workspace --all-targets --features rds-desktop/x11 -- -D warnings` (Linux): PASS
- `cargo test --workspace`: PASS
- `cargo test -p rds-discovery`: PASS — 9 unit + 11 e2e
  - publish→fetch roundtrip, forged signature → 401, stale replay →
    409, expired PUT → 410, per-key PUT pacing → 429, signed delete +
    garbage delete body → 400, registry name lookup, registry PUT
    forged → 401 / no-key → 401 / stale replay → 409, hostile input
    (garbage request line, oversized head, huge content-length,
    truncated body, chunked) → 4xx never hang, blackhole/refused
    directory → bounded `Unreachable`, TTL refresh keeps record live
- `cargo test -p rds-net --test announce_e2e`: PASS — 5 e2e
  - publish on start, addr-change republish (offline → relay attached),
    resolve fallbacks (ticket/bare-key/name/dead directory), aged-out
    record refused at resolve, resolve→connect by bare key over a real
    iroh relay
- `cargo test -p rds-agent --test e2e`: PASS — relay + allowlist
  regression intact
- record + http parser proptests: PASS — arbitrary bytes never panic;
  signed records roundtrip and single-byte corruption breaks verify
- G3 `resolve-connect` bench: **p50 80.0 ms, p95 139.0 ms, 100/100**
  on iroh (`bench-20260921-231105-resolve.{json,md}`); noq backend:
  p50 64.2 ms, 10/10 — cold name→key→record→connect→first-byte with a
  fresh client endpoint per iteration.

## What the wave built

- `rds-discovery`: `Payload.issued_at`; `check_freshness` rejects
  same-age-or-older records (replay protection); `DiscoveryError::Stale`;
  signed `DeleteRequest`/`DeletePayload` tombstones with replay
  rejection; `RecordStore::{remove,len}`; `MemoryStore`/`FileStore`
  enforce freshness on put.
- `rds_discovery::registry`: `SignedRegistry`/`RegistryPayload` —
  estate-signed name→key snapshot, `verify_fresh` checks signature +
  expiry + strictly-newer replacement.
- `rds_discovery::http`: owned minimal HTTP/1.1 codec — MAX_HEAD 8 KiB,
  MAX_BODY 256 KiB, `Content-Length` only, one request per connection.
- `rds_discovery::service`: directory over real TCP — PUT/GET/DELETE
  records, name lookup, registry PUT, health, prometheus metrics;
  per-key PUT pacing + global per-minute window; per-conn timeout.
- `rds_discovery::client`: one connection per request under a single
  3 s overall timeout — unreachable/blackholed directory surfaces as
  `Unreachable`, never a hang.
- `rds_net::announce`: publishes on start, refreshes at `ttl/3`, polls
  `endpoint.addr()` at `ttl/6` (250 ms–5 s clamp) and republishes on
  change — never dies silently, failures log + retry next tick.
- `rds_net::resolve_target`: ticket → bare key → device name; bare key
  + directory fetches and `verify_fresh`-checks the stored record and
  requires non-empty advertised addrs; names go through the signed
  registry. No `--server` → names fail with a clear flag hint; a dead
  directory is a bounded error, tickets unaffected.
- `rds` CLI `--server`, `rds-agent --directory/--record-ttl`,
  `rds-server` relay + directory + `--registry-key/--registry`
  composition.
- `rds-bench --scenario resolve-connect`: the G3 measurement harness
  (directory + signed registry + announcing agent + fresh client
  endpoint per iteration).

## Bugs found and fixed by the gate

- Test-side: the new resolve→connect e2e timed out while the known-good
  relay e2e passed. Root cause was in the test, not the transport:
  `Endpoint::accept()` returns `Option<Incoming>` and **awaiting the
  `Incoming` drives the server-side handshake** — the test polled
  `accept()` but never awaited the returned future, so the server sent
  no handshake flight and the client retried Initials for ~36 s.
  Fixed by awaiting the `Incoming` concurrently with the client
  connect (same failure class as the noq buffered-attempt bug from
  WS1). Diagnostics removed; the test now uses the resolved record's
  addr, not the ticket's.
- `put_min_interval` default of 2 s rejected legitimate republishes
  (change-triggered announce writes). Signature already scopes each
  writer to its own slot, so per-key pacing defaults to 0; the global
  per-minute window remains the abuse bound.
- Tombstone ordering `>=` refused same-second deletes; `>` is correct —
  the owner controls both records and tombstones.
- Truncated-body hostile test hung until the client half-closed; the
  test now shuts down its write side, matching a real peer's FIN.

## Deferred

- Multi-relay failover: still deferred from C2 — `EndpointConfig`
  takes a single relay attachment; multi-relay selection lands with
  discovery-driven relay sets.
- Relay-in-turmoil simulation: still needs the `RelaySocket` simulation
  socket seam noted in C2.
