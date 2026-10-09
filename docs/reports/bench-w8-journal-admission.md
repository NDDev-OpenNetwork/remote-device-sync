# Journal preparation benchmark

Source: `aeb8045c8a68f0d3274fe8c9afca738b0e583550`; tree: `fc579c481e9ed904c2da773826302d5fb9cf916c`; clean: `true`; `rustc 1.98.1 (48a229cea 2026-09-01) (Homebrew)`; macos / aarch64.

| Case | Samples | p50 µs | p95 µs | p99 µs | Max µs |
|---|---:|---:|---:|---:|---:|
| verified-resume-4MiB | 100 | 23196 | 27820 | 30609 | 32273 |
| pre-canceled-admission | 100 | 2 | 2 | 2 | 3 |
| admit-beside-8192-foreign-entries | 100 | 22059 | 25011 | 27052 | 28018 |

Fixture: 4194304 bytes, 16 chunks; 8192 foreign entries retained.

Synthetic local filesystem, warmed cache and debug/release profile determined by invocation. No before/after speed claim, process kill, real disk full, native power loss, network, quota or fair-GC acceptance. Every reported sample passed its content/admission checks; failure aborts report generation.
