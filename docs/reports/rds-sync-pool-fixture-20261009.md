# Shared disk-pool fixture correction — 2026-10-09

The first `w9-audio-foundation` checkpoint on macOS failed in the unrelated
`engine::tests::disk_jobs_share_one_bounded_pool` assertion `jobs never
overlapped`. All 46 other sync library tests passed. The earlier full workspace
run passed, so the new observation is retained as a real fixture failure.

The test used 15 ms sleeps to infer overlap, while another concurrent fixture
reserved 31 of the same 32 process-wide permits to test cancellation. The
remaining worker could run serially; elapsed sleep cannot establish admission
concurrency. Tokio's semaphore is fair and an `acquire_many` reservation can
hold later requests behind it. This is consistent with the
[semaphore contract](https://docs.rs/tokio/1.48.0/tokio/sync/struct.Semaphore.html).

The two fixtures that deliberately exhaust the shared pool now serialize their
ownership with a test-only async mutex. Admitted workers announce entry and wait
on a condition variable until the test observes a full pool. A drop guard opens
that gate on failure as well as success, so blocking tasks cannot strand runtime
shutdown. Every join result is checked. The assertion is stronger: peak active
work must equal the full 32-slot bound and return to zero.

The production semaphore, filesystem operations, cancellation ownership and
limits are unchanged. The corrected sync library suite passes all 47 tests and
strict all-target Clippy on macOS. The registered checkpoint is rerun after this
correction; the failed attempt is not counted as a pass. Cross-platform CI and
the final checkpoint receipt remain the source of broader evidence.
