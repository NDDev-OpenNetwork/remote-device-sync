# Journal preparation benchmark

Source: `119d122295280efc23417c9a656d296f4bb2f5da`; tree: `67c55b01a975e6733c0f3321cd594224d9e0b357`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Case | Samples | p50 µs | p95 µs | p99 µs | Max µs |
|---|---:|---:|---:|---:|---:|
| verified-resume-4MiB | 100 | 19899 | 24578 | 26167 | 27044 |
| pre-canceled-admission | 100 | 2 | 2 | 2 | 3 |
| admit-beside-8192-foreign-entries | 100 | 17951 | 20792 | 22211 | 27019 |

Fixture: 4194304 bytes, 16 chunks; 8192 foreign entries retained.

Synthetic local filesystem, warmed cache and debug/release profile determined by invocation. No before/after speed claim, process kill, real disk full, native power loss, network, quota or fair-GC acceptance. Every reported sample passed its content/admission checks; failure aborts report generation.
