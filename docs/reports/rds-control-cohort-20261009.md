# Complete impaired-control measurement — 2026-10-09

The final consumer verification at `06cebc3` retained a real failed observation:
the Noq impairment test measured control RTT p95 405 ms against its 400 ms limit.
The source change since the qualified functional build was documentation only.
A separate 15-second diagnostic reproduction also failed: 149 completed probes,
p50 141 ms, p95 420 ms and p99 522 ms. Video queue p95 was 1 ms, so the assertion's
old wording, “backlog leaked,” did not establish a cause.

The raw diagnostic contained eight samples above 400 ms, including adjacent
100 ms-spaced groups. Shared delay within a reliable stream is a plausible
explanation for this correlation, not a proved packet-level cause. QUIC's
[ordered stream delivery](https://www.rfc-editor.org/rfc/rfc9000.html#section-2.2)
explains why successive probes are not independent loss observations. The
[SRE measurement guidance](https://sre.google/sre-book/service-level-objectives/)
also distinguishes the measurement window and latency distribution from a
single scalar. Neither source prescribes an RDS threshold or sample count.

The fixture now keeps the same 5% per-direction loss, 50 ms base delay, 30 ms
jitter, seed, video load, 100 ms probe cadence and all latency limits, but
measures for 60 seconds and requires at least 500 control samples. Startup
samples stay included. Every issued sequence must receive exactly one response;
a bounded five-second drain retains delayed final replies while video continues.
Unanswered, duplicate, unsolicited and future-dated responses fail the test.
No application/transport implementation or deployed configuration changed.

## Retained results

| Run | Window | Completed probes | RTT p50 / p95 / p99 ms | Result |
|---|---:|---:|---|---|
| Consumer verification before change | 15 s | not emitted by old fixture | — / 405 / — | FAIL |
| Diagnostic before change | 15 s | 149 | 141 / 420 / 522 | FAIL |
| Complete cohort 1 | 60 s | 601 | 139 / 349 / 431 | PASS |
| Complete cohort 2 | 60 s | 601 | 138 / 333 / 386 | PASS |
| Complete cohort 3, full session suite | 60 s | 601 | 139 / 353 / 427 | PASS |

These macOS arm64 observations retain all three fixed post-change runs; they
are not retries of the old gate until green. The third run also passed all 13
session tests. Strict session-test Clippy and format checks pass. Cross-platform
CI and the next exact-source consumer verification remain independent evidence.

The control p95 limit remains **400 ms**. A longer measurement does not erase
the earlier failed windows or guarantee every 15-second interval. This is a
measurement-contract correction, not a transport latency improvement, native
input-to-pixel result, or promotion of the experimental Noq backend.
