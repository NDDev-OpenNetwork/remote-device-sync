# Workspace state churn

Source: `ab341dffbf5c4ea2b72af66a36c94430c8064a19`; tree: `4d27e8ca07a7a09589af18b2ceef98f070d7c3c6`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Tabs | Samples | Transitions/sample | p50 µs | p95 µs | p99 µs |
|---|---|---|---|---|---|
| 1 | 100 | 1024 | 32.708 | 34.917 | 35.458 |
| 4 | 100 | 1024 | 57.084 | 57.417 | 59.083 |
| 8 | 100 | 1024 | 68.000 | 82.959 | 83.250 |

Every sample checks capacity, exact revisions, non-reused identities and empty final ownership.

Synthetic release-build tab-model operations, 100 samples per case; no GPU, input-to-pixel, network, native memory/CPU or installed acceptance claim. No hardware-independent speed threshold.
