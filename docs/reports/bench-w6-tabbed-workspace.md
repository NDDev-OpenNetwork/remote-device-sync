# Workspace state churn

Source: `51bf87057c6286496797529c24477905d630c8b6`; tree: `59988487e174a5daa27a7c38937885c8c68ad88e`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 11.458 | 32.375 | 32.583 |
| 4 | 100 | 1024 | 19.125 | 20.125 | 20.209 |
| 8 | 100 | 1024 | 28.459 | 68.208 | 68.584 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
