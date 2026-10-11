# Workspace state churn

Source: `fcec25e7c2fc95237dc3b92a3dc681a8fe373d55`; tree: `73eb5c43d5dc31ce9ace23f7af76aea77f82e38b`; clean: `false`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 48.709 | 66.875 | 67.417 |
| 4 | 100 | 1024 | 80.875 | 81.708 | 87.084 |
| 8 | 100 | 1024 | 75.667 | 105.208 | 112.750 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
