# Journal preparation benchmark

Source: `04ce32dfeb4d30048551002be3a47ded45c511cc`; tree: `5206fe94b1ed285e4e7f8fdd0065b9b801182d49`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Case | Samples | p50 µs | p95 µs | p99 µs | Max µs |
|---|---:|---:|---:|---:|---:|
| verified-resume-4MiB | 100 | 25037 | 27918 | 31788 | 33221 |
| equal-content-other-destination | 100 | 17043 | 20409 | 22129 | 25257 |
| pre-canceled-admission | 100 | 3 | 4 | 4 | 4 |
| admit-beside-8192-foreign-entries | 100 | 22259 | 27324 | 31041 | 31544 |

Fixture: 4194304 bytes, 16 chunks; 8192 foreign entries retained.

Synthetic local filesystem, warmed cache and debug/release profile determined by invocation. No before/after speed claim, process kill, real disk full, native power loss, network, quota or fair-GC acceptance. Every reported sample passed its content/admission checks; failure aborts report generation.
