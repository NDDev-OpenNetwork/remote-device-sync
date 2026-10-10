# Workspace state churn

Source: `6008b424464f225e1bb2f6cd6e5cbb0fa72f0362`; tree: `cabb24acd8490efff733b10eafe4b4ba481785ff`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 13.250 | 13.875 | 14.083 |
| 4 | 100 | 1024 | 23.167 | 23.292 | 23.334 |
| 8 | 100 | 1024 | 34.166 | 34.417 | 34.500 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
