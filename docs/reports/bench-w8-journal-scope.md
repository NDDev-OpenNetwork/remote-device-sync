# Journal preparation benchmark

Source: `7cb209677f2f9f4d759314c76b19f19882af8943`; tree: `c0d98b530df19e0204fa1971e001d354d9e7f7bb`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Case | Samples | p50 µs | p95 µs | p99 µs | Max µs |
|---|---:|---:|---:|---:|---:|
| verified-resume-4MiB | 100 | 25308 | 28965 | 30180 | 30652 |
| equal-content-other-destination | 100 | 18853 | 20905 | 22091 | 22964 |
| pre-canceled-admission | 100 | 5 | 6 | 7 | 8 |
| admit-beside-8192-foreign-entries | 100 | 25926 | 31109 | 32039 | 33083 |

Fixture: 4194304 bytes, 16 chunks; 8192 foreign entries retained.

Synthetic local filesystem, warmed cache and debug/release profile determined by invocation. No before/after speed claim, process kill, real disk full, native power loss, network, quota or fair-GC acceptance. Every reported sample passed its content/admission checks; failure aborts report generation.
