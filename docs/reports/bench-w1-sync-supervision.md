# Journal preparation benchmark

Source: `89db73361a27244e9d97c11cc9cea52724cf8f92`; tree: `fdd2ccd95b1b4944e5749cb8a32454567681e2c6`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Case | Samples | p50 µs | p95 µs | p99 µs | Max µs |
|---|---:|---:|---:|---:|---:|
| verified-resume-4MiB | 100 | 24636 | 27104 | 30213 | 31683 |
| equal-content-other-destination | 100 | 16032 | 18075 | 19358 | 20118 |
| pre-canceled-admission | 100 | 5 | 8 | 23 | 26 |
| admit-beside-8192-foreign-entries | 100 | 21650 | 24985 | 26005 | 26015 |

Fixture: 4194304 bytes, 16 chunks; 8192 foreign entries retained.

Synthetic local filesystem, warmed cache and debug/release profile determined by invocation. No before/after speed claim, process kill, real disk full, native power loss, network, quota or fair-GC acceptance. Every reported sample passed its content/admission checks; failure aborts report generation.
