# Durable publication during storage exhaustion — 2026-10-06

An announcer previously returned any issuer commit error to the agent supervisor,
which closed the endpoint and local manager. A full filesystem can therefore
interrupt authenticated live streams even though it only prevents publishing
the next signed address record.

ENOSPC and quota errors now retain their typed OS cause. The issuer remembers
one exact failed capacity attempt, retains its exclusive ownership, accepts only
the preceding owned state or that attempt, and synchronizes those verified
bytes before clearing uncertainty. Clock rollback, unrelated/corrupt state and
other I/O remain fatal. The announcer uses bounded jittered retries with pause
and recovery logs, without replacing the endpoint or replaying input. No signed
record is returned or published before durable revision allocation.

Five publisher tests pass, including all five real atomic-file checkpoints,
repeated capacity failure, competing-owner refusal, exact revision/retry after
reopening, and rejection of unrelated signed history/corruption. Strict Mac
all-target discovery/net/agent Clippy with desktop/owned transport passes.
Expanded native discovery64passed/1pre-existing ignored and net101/101regressions and strict workspace
desktop/Noq Clippy also pass. Linux/CI and installed qualification remain
pending. This increment is not deployed and does not identify the cause
of every network jitter event.

## Transfer completion regression exposed by CI

The Mac full test job at sourceb019d15 rejected the second immediate transfer in
`managed_transfers_pin_devices_and_reuse_the_agent_identity`, line117. A local
reproduction passed, so the failure is retained as intermittent evidence rather
than dismissed. The server previously queued FIN before dropping the outer
connection sync guard; the peer could observe completion and request its next
transfer while that guard still held admission. The engine now owns the guard,
joins its control reader, releases admission and then finishes the server stream.
Cancellation retains RAII cleanup. The existing real IPC/QUIC test now exercises
eight consecutive resumptions on both backends; greeting/transfer failures keep
local stage/ID/error diagnostics without replay or a wire change. The final Mac real IPC/QUIC local-sync suite passes3/3 (including cancellation,
directional grants and eight immediate resumptions per backend); sync engine
31/31 and strict workspace desktop/Noq Clippy also pass. Refreshed Linux/CI
and installed qualification are pending for this added fix.
