# Workspace state churn

Source: `94260b1d41b994861eaed5b5564728b84010cd29`; tree: `796eed95715cb0018373c5525e19950d0266d9f2`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 11.084 | 11.334 | 11.417 |
| 4 | 100 | 1024 | 19.417 | 19.542 | 19.584 |
| 8 | 100 | 1024 | 29.167 | 82.458 | 82.917 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
