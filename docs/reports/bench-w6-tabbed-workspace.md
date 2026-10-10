# Workspace state churn

Source: `5c3cf0cde2161d5e16fba9dac9719d33f3e5f210`; tree: `f8127f5bdf160e5bfa5841b6ab5d3bbcf3a6d370`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 32.625 | 33.334 | 34.416 |
| 4 | 100 | 1024 | 57.042 | 57.334 | 57.417 |
| 8 | 100 | 1024 | 67.834 | 82.834 | 84.166 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
