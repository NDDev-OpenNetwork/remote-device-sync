# Sync cancellation fixture: observe receiver admission

Scope: repair an independent W2.5/W8 fixture assumption exposed by the native
viewer PR's full Ubuntu CI; no runtime or wire change.

The original [Ubuntu failure](https://github.com/NDDev-OpenNetwork/remote-device-sync/actions/runs/36969086539/job/110719084734)
is retained in [issue79](https://github.com/NDDev-OpenNetwork/remote-device-sync/issues/79):
`kill_mid_transfer_resumes_identical` reopened a transfer after a fixed 150 ms
sleep and failed with `offer refused: cannot open transfer journal`.
Canceling the sender does not join an executing receiver filesystem operation;
the exclusive journal lock correctly remains held until that operation ends.

The fixture now joins the aborted sender, counts only hash-named part files,
requires actual part progress, and probes the receiver's journal admission under
one three-second bound. Only `SyncError::Io(WouldBlock)` is retried; all other
errors fail immediately. The admitted journal verifies retained chunks before
the single resumed network transfer. Byte identity and fewer-than-total fetched
chunks remain required. Accepted transfers are not retried by this helper.

A real-lock fixture retains a Journal past 150 ms and confirms typed refusal.
The bounded helper remains pending while the lock is held and admits after the
owner releases it. This reproduces the invalid timing assumption with an actual
filesystem lock, without pretending to cancel a running syscall.

The corrected macOS sync E2E lane passed 15 tests with zero failures or ignores;
strict all-target/all-feature sync lint and formatting passed. Linux and full
CI results accompany the repairing PR. These fixtures do not close physical
power-loss, native desktop latency/quality or sustained stability acceptance.

```sh
cargo test --locked -p rds-sync --test sync_e2e
cargo clippy --locked -p rds-sync --all-targets --all-features -- -D warnings
cargo fmt --check
```
