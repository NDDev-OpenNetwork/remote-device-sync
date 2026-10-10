# Workspace state churn

Source: `3fe2a49eabeb0a2801f088b0b901574f5d7a8e8e`; tree: `b596965e2b3b8e338e0f157f6a2882774aa666dc`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 48.667 | 66.833 | 67.667 |
| 4 | 100 | 1024 | 81.375 | 84.250 | 91.833 |
| 8 | 100 | 1024 | 75.167 | 95.333 | 226.000 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
