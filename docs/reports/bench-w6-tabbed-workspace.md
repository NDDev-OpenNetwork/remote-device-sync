# Workspace state churn

Source: `0b359db9feb495a31e2aec1433d3256767408a8e`; tree: `da2bb48cbaea3f4a2c84660a4ba77d10e059d562`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 48.292 | 66.250 | 66.542 |
| 4 | 100 | 1024 | 81.250 | 82.042 | 85.292 |
| 8 | 100 | 1024 | 75.167 | 91.625 | 102.833 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
