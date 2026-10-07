# Current implementation continuation — 2026-10-07

This is the current execution order for stabilizing the implemented system.
It supplements [remediation-plan.md](remediation-plan.md); W0–W10 identifiers
retain their original acceptance requirements. Source, tests and live artifact
observations decide status. A historical receipt, successful build or sampled
RTT does not establish installed interactive stability.

## Audit boundary

The ten-day session census was collected from the installed Codex, Claude Code,
Devin, Cursor and Grok stores. Recent project work was found in Codex and Devin;
the other three stores supplied no recent local project sessions. This does not
establish absence of cloud, deleted or other-device sessions. Raw transcripts,
host identities, paths and runtime receipts remain in private device/estate
storage. The old Codex thread is still indexed, but its original JSONL is absent;
its SQLite projection contains 1,072 user/assistant messages through the last
available projected message. The index's newer activity time is not a transcript.

The audit compares merged source, open branches/PRs, installed executable
digests, endpoint settings, policy, running processes and retained diagnostics.
Public source began at `8002c11`, including the qualified persistent-relay
increment from PR #116. The estate checkout and installed viewer can consume
older revisions independently; neither is silently assumed to equal main.

## Findings verified in current code

| ID | Finding | Implementation boundary | Required next evidence |
|---|---|---|---|
| S1 | A sibling ACK can predate the outstanding-work stall that triggers path retirement. General ACK freshness is insufficient evidence of progress during that failure. | `rds-net/src/ack_progress.rs`, both Iroh latency and owned Noq policies | Deterministic pre-stall/post-stall proof tests, both real blackhole/recovery suites, installed comparison |
| S2 | Persistent relay registration is implemented and qualified, but registration, peer address publication, validated paths and installed use are distinct states. | `rds-net/tests/relay_registrations.rs`, endpoint settings, private rollout | Confirm complete addresses at both endpoints, >60 s idle readiness, ordered selected-route failures without application replay |
| S3 | Pending publication work includes public/private branches and an older native-viewer PR; the private runtime PR conflicts with current estate main. | Git branches, PRs, immutable estate gitlink | Preserve unpublished commits; reconcile against actual merged code; signed atomic commits and reviewed PR merges |
| S4 | Current docs still contain obsolete present-tense claims about TCP-only SSH, missing transfer, core runtime dependencies, grants and relay failover. | Architecture, capability matrix, platform docs, progress ledger | Replace current claims from code; retain dated evidence and unsupported capability labels |
| S5 | Input ACK, payload receipt, decoded-frame submission and physical pixels prove different things. | Native diagnostics, visual probe, bench reports | Exact-build typing/Backspace count and causal submission evidence; loss and unanswered probes included |
| S6 | Optional platform/media modules remain real stubs. Directory sync, native serving on macOS/Wayland, audio and automatic GDS grant issuance are not implemented. | Backend probe files, audio crate, sync engine, GDS policy adapters | Explicit later implementation/qualification tasks; never mark these complete from X11 or loopback results |

## Wave A — interactive recovery correctness (W3.6, W6.8)

1. Preserve a bounded incident interval and join metadata by session/sequence.
   Report local queueing, server injection, reply writing, framing, transport
   progress and reconnects separately. Never derive one-way latency by subtracting
   unsynchronized host clocks. Treat logging gaps as unavailable evidence.
2. Reproduce S1 before changing recovery: a standby's otherwise fresh ACK from
   before the failure must not authorize retiring reliable STREAM work.
3. Put the corrected proof predicate in the shared ACK observation boundary and
   use it in both policies. Require a still-fresh, nonstalled sibling confirmation
   after the failed path's outstanding-work interval began. Keep engine loss
   detection, congestion control, authorization and last-path protection.
4. Verify actual established-stream recovery, standby restoration, ACK routing,
   probe-only debt, idle resumption, runtime clocks and high-RTT paths. Include
   both blocked links as a negative case; retain every failed candidate.
5. Run fmt, relevant strict all-target Clippy/default/Noq tests on macOS and
   Linux, Linux X11 checks, supply-chain checks and CI. Record a scoped bench
   report; do not invent an unregistered checkpoint or close W3/W6 from it.
6. Publish signed commits through a PR. Bind the installed candidate to its
   exact tree/features/artifact digest; update only the scoped RDS components
   when their current work permits it. Observe real ordinary typing, deletion,
   foreground/background transitions and reconnect counts afterward.

Exit: the invalid retirement decision is prevented in both backends, positive
recovery still passes, and current installed evidence states the remaining tails
honestly. This is not an unconditional physical-network availability promise.

## Wave B — finish the already requested relay rollout (W3.3, W10.4/W10.5)

1. Reobserve the actual reserve services; an unfinished session's report may
   predate a service installation. Verify dedicated user/cgroup, bounded
   resources, authenticated admission, exact binary and external reachability.
2. Use the existing private estate's ordered relay origins as the only host
   authority. Keep one controller/directory authority and the faster usable
   direct path. Reserve relays forward opaque traffic; no database replication.
3. Complete the bounded remaining reserve installation with its reviewed
   artifact/host plan. Never overwrite an unexpected existing service.
4. Configure explicit bounded persistence and relay order, republish current
   peer addresses, and ensure the managed/native client resolves those addresses.
   A startup ticket with one relay cannot qualify multi-relay recovery.
5. Qualify >60 s idle registration and current selected paths. Test failures in
   an isolated fixture first; any live service fault must be scoped to an idle
   reserve, with current protected-process and active-session preflight.
6. Record the installed/source/policy/route facts in the private estate; update
   the immutable module gitlink through the existing GDS operation and verify
   its terminal outcome once. Public reports use synthetic topology only.

Exit: each declared reserve is reachable, independently registered and known to
both peers, with truthful measured recovery and no unrequested process restart.

## Wave C — state convergence and documentation (W0.5, W4.5, W10.8)

1. Reconcile the private runtime PR with estate main without dropping foreign
   changes or rewriting another active checkout. Preserve each operation receipt.
2. Review stale public PR #62 and issue #79 against merged implementations and
   regression tests; retire only work proved superseded. Review dependency PRs
   against vendored patch provenance, licenses, lockfile and feature matrix.
3. Update current architecture/platform/capability claims from implementations.
   Move obsolete design recommendations into explicitly dated historical context.
   Historical failed measurements remain retained and distinguishable.
4. Refresh generated private memories from their committed canonical sources
   with `gds memory verify`, apply the emitted candidate and validate. Do not edit
   Codex's live SQLite history or treat an old summary as current authority.
5. Confirm main OID, consumed gitlink, installed artifact digest, running process
   identity and effective endpoint/policy config independently on each device.
   A clean checkout alone is not complete synchronization.

Exit: published implementation, estate consumption, runtime receipts and current
documentation agree; any remaining mismatch has its owning task and verifier.

## Remaining product gates after current stabilization

| Work | Concrete remaining boundary | Acceptance |
|---|---|---|
| W0/W10 measurement | Cross-platform impairment/topology coverage, causal visible response, resource churn and secured diagnostic bundles | Reproducible receiver/pixel measurements, p50/p95/p99, failures and coverage; no reused historical acceptance |
| W2 lifecycle/QoS | Per-service fairness, complete resource/cancellation budgets, automatic grant renewal integration | Slow-peer and mixed SSH/desktop/sync tests; bounded tasks/RSS/FD; unrelated streams survive scoped cancellation |
| W3 owned transport | Physical interface/rebinding/suspend recovery, complete path-event/accounting coverage, UDP-blocked owned fallback and relay federation | Same meaningful topology matrix on both lanes; owned backend remains experimental until parity |
| W4 GDS policy | Enrollment/rotation, automatic issuance/renewal/revocation, revisioned applied-policy status | Clean enroll→resolve→authorize→renew→revoke with account/tenant/destination isolation |
| W5 terminal | Scoped native server/account broker, reconnectable PTY policy, host-key enrollment and native macOS/mixed-load evidence | No duplicate exec, explicit process survival, terminal restoration and verified host trust |
| W6 X11 desktop | Geometry/monitor changes, sustained quality/latency and mixed-load/current installed acceptance | Real capture/input/decode/presentation and count-correct typing/deletion; no unexplained tail hidden by a median |
| W7 native serving | ScreenCaptureKit/VideoToolbox/CGEvent, Wayland capture/EIS, permission and signed helper lifecycle | Real supported seats/devices, permission denial/regrant, lock/suspend/monitor transitions; stubs never advertise support |
| W8 sync | Large-file process crash matrix, source/conflict/overwrite policy, bounded journal GC; recursive/two-way engine afterward | Exact verified data and durable commit; safe conflicts/tombstones, Linux/macOS metadata and alias fixtures |
| W9 media | Audio/jitter/A-V clocks, file drop scopes, hardware codec probes; measured optional FEC/codec tiers | Permission-scoped real media, bounded buffers, device changes and proven improvement per hardware/network profile |
| W10 release | Native signing/notarization, actual immutable release provenance, drain/update compatibility and supported topology qualification | Clean install and current-artifact checks, safe scoped update, complete required acceptance rows |

These are unfinished product requirements, not defects to hide with placeholder
implementations. The existing remediation task tables supply their detailed
acceptance criteria. Each starts with code verification and primary-source
research, then a bounded implementation wave and its own evidence.

## Research discipline

The requested 2026-09-26 reference boundary applies to recommendations: use
published standards available by that date. Later source/build observations are
dated separately. [RFC 9000 §8.2](https://www.rfc-editor.org/rfc/rfc9000.html#section-8.2)
defines path validation; [§9.4](https://www.rfc-editor.org/rfc/rfc9000.html#section-9.4)
distinguishes path-specific loss/congestion state. [RFC 9002 §6.2](https://www.rfc-editor.org/rfc/rfc9002.html#section-6.2)
defines probe-based loss recovery. They do not prescribe the RDS retirement
threshold or prove current standby liveness from an older RTT. That predicate is
an RDS policy and needs negative as well as positive tests.

After each wave, reobserve code, open work, current runtime and relevant primary
sources. Amend this execution order when evidence changes; never declare a
whole milestone stable because one correction or loopback fixture passed.
