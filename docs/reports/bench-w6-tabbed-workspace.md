# Workspace state churn

Source: `a9515128f0fdfdba0b045b17d9a77bf9ff51db86`; tree: `7ecf810270bab6a9effb8e0f0825f69481df65e5`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 31.166 | 31.667 | 33.083 |
| 4 | 100 | 1024 | 52.542 | 54.375 | 100.625 |
| 8 | 100 | 1024 | 75.500 | 77.958 | 100.542 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
