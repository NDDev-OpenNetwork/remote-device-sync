# Persistent identity transactions — 2026-09-25

Scope: W2.4 stable identity and W3.2 owned runtime prerequisites, base `7616a3b`.
Linux temporary fixtures only. No deployed key or running service was changed.

Three real public-API regressions failed before correction: the loader followed
final symlinks, accepted mode 0644 keys and accepted additional hard links.
Existing raw-format reload and malformed-length refusal already passed.
Source inspection also identified direct writes into the final filename, no
file/parent synchronization and no concurrent-creator recovery.

The shared backend-independent identity store keeps the 32-byte format, uses a
stable advisory lock on the parent directory inode and publishes a complete
synchronized seed
with no-replace rename. Bounded reads, file/owner/mode checks and inode alias
checks protect the boundary. Only its exact reserved pending file is recovered;
unknown state and unsafe existing keys fail without replacement. CLI/agent call
it on a blocking worker. See the full [contract](../identity-storage.md), including
compatibility changes and existing-parent requirements.

The final focused transaction run passed **17 tests**. Binary configuration
coverage includes eight concurrent real CLI creators. Fault coverage includes
returned errors and actual child process exit at seven initialization, write and
publication boundaries, independent creators, legacy-writer
races, parent-path replacement, bounded contention and recovery through another
key name. Final coverage includes restrictive-umask exits before permissions
are established,
explicit unlock with a cloned directory descriptor, oversized-pending refusal
and the common socket/directory fixture. Linux's
FIFO creation call is excluded from macOS at compile time; native macOS remains
unexecuted here.

A real CLI regression also reproduced success creating an unreadable mode `000`
seed under umask `0777`, followed by reload failure. New marker/staging descriptors
now receive mode `0600`. The parent inode is locked before metadata exists, and
empty private interrupted creations are recovered using no-follow metadata.
Real CLI fresh-marker/existing-marker tests confirm `0600` and the same identity
on reload. No seed or public identity is exported into the receipt.

The first CLI run found the older test scratch directory inherited group-write
permission. The fixture now explicitly creates 0700 directories; no application
check was weakened. The first workspace concurrency run reached the documented
two-second Busy bound. Final concurrency tests accept only successful identity
or Busy without an identity, then retry Busy after contention and require the
same committed identity. This does not promise every concurrent caller succeeds
within two seconds on a busy filesystem; the original failure is retained in
private verification logs.

A subsequent all-feature build stopped at compiler ENOSPC. After removing
old inactive test executables from this workspace, the full checks were rerun.
This is a build-environment failure, not storage fault-injection evidence.

Final formatting and default/X11/all-feature workspace Clippy passed.
Workspace: **390 tests across 68 targets**, 1 existing ignored test.
All-feature net/relay/agent/CLI: **194 tests across 39 targets**.
Isolated default-feature-free identity/network: **36 tests across 2 targets**.
Exact commands, durations and source hashes are in the
[machine-readable receipt](rds-identity-storage-20260925-data.json).
The only dependency adjustment enables rustix's existing process feature.
No new package, unsafe block or external runtime helper was introduced.
`cargo-deny` remains unavailable locally.

This qualifies the described local file transaction and retry contract. It is
not extended-ACL, native macOS, physical-power, real-disk-exhaustion or network
filesystem evidence. Existing ancestor directories are assumed durably
provisioned; kernel/filesystem I/O has no forced timeout. Local IPC ownership,
enrollment/rotation, directory shutdown and owned server runtime integration
remain open. No remediation wave is closed.
