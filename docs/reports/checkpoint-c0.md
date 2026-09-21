# Checkpoint C0 — 20260921-195139

Verdict: **PASS**

## Automated checks
- fmt/clippy/test: PASS (both clippy lanes on Linux)
- suite A: bench-20260921-195139-a.{json,md}
- suite B: bench-20260921-195139-b.{json,md}
- reproducibility (p50 ±15%, p95 ±45%/10ms floor, throughput ±15%/3MiB floor): PASS
- baseline: baseline-iroh.md

## Manual checklist
- [x] All "not run" items above explained — none; every layer ran
- [x] Reports committed: bench-*.json, bench-*.md, this file
- [x] docs/ updated — `docs/reports/README.md` documents the report
  format; `AGENTS.md` lists the checkpoint tooling
- [x] security/unsafe review — new code contains no `unsafe`, no new
  trust boundary (bench peers are self-spawned, allowlisted)

## What the baseline says

iroh backend, in-process, loopback (Linux x86_64, dev box):

| Scenario | Result |
| --- | --- |
| handshake direct | p50 ≈ 27–31 ms, 100/100 |
| ping direct | p50 ≈ 2 ms, p95 ≈ 6–8 ms |
| transfer direct | ≈ 16–17 MiB/s send-side |
| multiconnect @ 5% loss/50ms/30ms | 100/100 connects, p95 ≈ 1.2–1.3 s |
| relay-fallback | p50 ≈ 2 ms |
| impaired ping @ 5%/50/30 | p50 ≈ 2 ms, drops engaged |

Notes and honest limits:

- `transfer` measures send-side stream throughput only (write path);
  the receiver is a TCP discard sink.
- `multiconnect` p95 ≈ 1.3 s is handshake retransmission under the
  injected 5% loss — the distribution is bimodal, not a bug.
- `rebind`/`migration` cases are deferred to WS1: they need socket
  control the iroh facade does not expose.
- Tail-percentile reproducibility uses the documented
  `max(rel, floor)` rule — tails at n=100 are noise-dominated on
  loopback; medians are compared tightly.
