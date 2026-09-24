# Cancel queued sync stores — 2026-09-25

Scope: W2.5 cooperative chunk-writer cancellation. The relay-control validation
run exposed a G6 repeated-kill recovery failure: a final immediate retry found
the receiver's journal lock still held. Exclusive refusal was correct; the
existing test assumed a 30 ms sleep guaranteed cleanup, and a canceled chunk
sink continued draining all queued disk writes.

Two deterministic before-fix tests occupied the runtime's sole blocking worker,
queued a real verified chunk, then dropped the sink or canceled finish before
the writer could start. Both showed the canceled queued chunk was still stored.
A cancellation guard now aborts unstarted blocking work and stops a running
writer between stores. It stays owned across finish's await and publishes its
flag before job-channel closure. Normal finish still drains and verifies data.

This does not interrupt an executing syscall, revoke a completed chunk commit,
join arbitrary blocking work from Drop, or release the receive lock prematurely.
Open/scan and assembly cancellation barriers remain separate work. The G6 test
now bounds its final recovery phase to ten seconds and checks byte-identical
convergence across transient refusals, rather than equating a fixed sleep with
remote cleanup. Persistent errors still fail that phase; other normal-transfer
and malformed-input tests keep their existing single-attempt assertions.

Formatting, sync Clippy, three focused unit tests and 14 sync e2e tests passed.
[Commands and source hashes](rds-sync-cancel-20260925-data.json) record the checks.
These checks ran in the working snapshot containing the relay-control correction;
the subsequent relay-control receipt records the final full-workspace matrix.
The initial failed full run and both deterministic failures are retained in
private evidence. No dependency, wire format or runtime helper program changed.
No remediation wave, physical power-loss, native macOS or deployed acceptance
is closed by this increment.
