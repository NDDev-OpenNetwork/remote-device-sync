# Workspace state churn

Source: `f686ac215ad872a711360ad925ade96273ffb300`; tree: `e63fd3e7ad87884b5817bc8b1153fd03b4fbe0a0`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 48.084 | 48.833 | 58.458 |
| 4 | 100 | 1024 | 63.709 | 96.292 | 151.417 |
| 8 | 100 | 1024 | 82.417 | 106.834 | 132.084 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
