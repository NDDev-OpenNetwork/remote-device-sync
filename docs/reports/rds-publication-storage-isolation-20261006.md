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
Expanded native discovery65/65and net101/101regressions and strict workspace
desktop/Noq Clippy also pass. Linux/CI and installed qualification remain
pending. This increment is not deployed and does not identify the cause
of every network jitter event.
