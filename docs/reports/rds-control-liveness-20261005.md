# Independent native control liveness — 2026-10-05

This W6.7/W2.6 increment closes a native recovery gap: decoded video previously
rearmed the only progress watchdog even when control had no current heartbeat
confirmation. A successful write only supplies local writer evidence.

Implemented:16exact outstanding heartbeat tuples, checked per-session sequence,
monotonic send-time confirmation,8-second ordinary progress threshold and the
existing30-second absolute clipboard allowance. Late/unmatched/duplicate/retired
responses do not fabricate current control progress. Both native entry paths
use the same monitor. Managed teardown remains desktop-scoped; unrelated peer
streams and server processes remain outside it. No replay or wire change.

27 CLI desktop library tests pass. A real bounded Tokio pipe regression keeps
video progress fresh and successful control writes flowing without echoes:
control expires at8seconds, one input arrives once, and cancellation drops the
held media leg. A60-second matched-echo pipe retains the existing session.
Tests cover tuple matching, stale/out-of-order response, bounded retirement,
sequence exhaustion and exact clipboard completion/absolute grace. An initial
new pipe test raced EOF against the completed task; it now explicitly verifies
EOF then awaits the actual control failure. Strict expanded workspace clippy
passes. Formatting, ordinary/full platform CI and installed measurements must
be recorded independently before acceptance. No wave-close gate is claimed.
