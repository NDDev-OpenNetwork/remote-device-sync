# Stability execution plan — 2026-10-09

Original audit baseline: `93be1f014329e2ea7d573c209c514a67d4b22fb8`.
Status reconciled against the implementation on 2026-10-10. This is the current
execution order for the unfinished requirements in [the remediation
plan](remediation-plan.md), replacing the order in [the continuation
plan](continuation-plan.md) and [October 8 completion plan](completion-plan-20261008.md).
It does not replace their acceptance criteria or the [capability
matrix](capability-matrix.md). A merged change, a successful test, an installed
artifact and an accepted product requirement are different facts.

The session-review window is September 29–October 9. Session transcripts,
device observations, deployment receipts and authority material stay in the
private estate/device state. Raw transcripts are never committed here. Missing
rollout files require checking surviving read-only history projections; session
titles and cwd indexes alone are insufficient. Ancestor-workspace sessions need
content review, and unrelated projects are outside the audit.

## Source-checked findings and remaining boundaries

| ID | Current code/evidence | Remaining boundary |
|---|---|---|
| A1 | `rds-audio::AudioPacket::validate` uses the existing libopus parser to check structure, one-frame duration and sample count before buffering/stateful decode; maximum frame and packet framing are distinct. [Audio repair](reports/rds-audio-stability-20261009.md). | Native device clocks/I/O, service admission and A/V acceptance remain W9; do not repeat the completed packet-validation repair. |
| A2 | Bounded jitter retains the newest set, rejects pre-playout packets, removes pre-start stale entries, accounts each concealed interval and refuses sequence exhaustion. Adversarial reorder/loss/overflow tests cover the library contract. | Device playout, real clock drift, loss/late interaction and synchronized video remain separate. |
| A3 | Journal preparation streams names, observes cancellation between entries/chunks and limits opportunistic collection to 4096 entries; state is destination-bound and verified legacy resume stays with that destination. [Preparation](reports/rds-journal-admission-20261010.md) and [binding](reports/rds-journal-destination-scope-20261010.md). | Persisted quota admission, fair background reclamation, local-writer overwrite/conflict policy and physical crash/disk-full acceptance remain open. |
| A4 | DesktopV5 text clipboard, RandR catalog, concurrent tagged sessions, eight-tab native workspace, per-display profiles, saved preferences and collapsible bars are implemented. [Workspace](reports/rds-tabbed-workspace-20261010.md) and [chrome](reports/rds-workspace-chrome-20261010.md) have scoped native evidence. | Physical multi-PC/display transitions and clipboard handoff, suspend/WAN/soak, rich formats and Linux viewer clipboard publication remain separate. Original toolbar specks were not reproduced. |
| A5 | Grant v3, directional desktop/sync scopes, per-session tags, local IPC v5 and bounded service/session lifetimes exist. | Automatic issuer/enrollment/renewal, account/seat isolation, negotiated global QoS and mixed-load qualification remain unfinished. |
| A6 | ScreenCaptureKit/image-copy/KMS/PipeWire probes return unavailable; macOS/Wayland input and hardware codec modules remain stubs; agent refuses audio. | Native serving/device audio require implementation and platform acceptance; no promotion from library tests. |
| A7 | ACK progress, initial validation deadlines, persistent relay registration, standby restoration and the observer-registration race fix exist in both adapters/vendored engines. | Physical topology parity, interface recreation/suspend/rebind, owned UDP-blocked carrier/federation and retired-path accounting remain open. The retained Noq control-reader incident is not closed by a passing later cohort. |
| A8 | Release docs distinguish the published 0.1.0 preview from current main; source, consumed gitlink, qualified artifact, running image and policy remain independent facts. | Current contract navigation must stay aligned with code; deployment receipts belong to the private estate and require live re-observation. |
| A9 | The original baseline left 30 vendored Noq alerts; [exact source review](reports/rds-noq-codeql-review-20261009.md) records individual dispositions without removing scanner coverage. | New alerts need their own review; historical alert counts are not a current provider observation. |
| A10 | The sync disk-pool fixture now serializes exhaustive probes and observes admission instead of sleeping. [Fixture correction](reports/rds-sync-pool-fixture-20261009.md) and subsequent registered checkpoints passed. | Preserve the original failure and existing production limits; this is completed fixture work, not a pending retry. |

## Completed Wave 1 — audio foundation repair (W9.1, W0.1)

The packet-validation and jitter repairs are implemented and qualified at the
registered library checkpoint; see [the source-bound report](reports/rds-audio-stability-20261009.md).
The existing fixed 20 ms/one-frame profile uses libopus validation consistently
in wire conversion, jitter admission and decode. Negative state-preservation,
reorder/loss/overflow/exhaustion tests and the reproducible codec benchmark
cover this increment. No audio service is advertised. Device audio and A/V
synchronization remain W9.1; these completed library tasks are not the next wave.

## Wave 2 — finish the existing desktop delivery (W6, W9.2, W4.5/W10.4)

The [feedback timer](reports/rds-feedback-windows-20261010.md) and
[late path observer](reports/rds-initial-path-observation-20261010.md) now have
failing-before regressions and qualified fixes. The standby failure was an
initial observer-registration race, not a reason to raise its timeout.
These source repairs are included in the qualified workspace increment.
Source-bound artifact/installation receipts remain private deployment evidence;
the physical acceptance rows below remain open.

The owner's multi-device/multi-display workflow is now implemented as a native
tabbed workspace, with per-display profiles and explicit session ownership.
[The workspace report](reports/rds-tabbed-workspace-20261010.md) records the
model/feature checks and real loopback manager/QUIC/H.264 native interaction.
The later [chrome follow-up](reports/rds-workspace-chrome-20261010.md) adds
panel collapse and fixes idle-frame restoration and redraw ownership.
Physical clipboard handoff and multi-PC/WAN acceptance remain separate rows;
source tests do not establish them.

The later main-branch CI run 37999738254 retained a separate control RTT p95
failure: 445 ms across 601 probes versus the unchanged 400 ms limit. The earlier
cohort enlargement did not repair runtime latency. The failing result is retained; the next paragraph records its harness
diagnosis, which is separate from installed runtime and physical latency.
The [codec-profile investigation](reports/rds-control-latency-tail-20261010.md)
subsequently reproduced unintended repair traffic when synthetic transport bytes
were decoded as H.264. The corrected header harness is feature-independent and
retains the 400 ms gate; it is not a claimed production transport latency repair.

1. Re-read current public/estate heads, open PRs and operation receipts. An
   expired plan or an earlier successful pin is not authorization to replay a
   stale transaction. Preserve other worktrees and unpublished commits.
2. Build or verify immutable Linux/macOS artifacts for the selected reviewed
   revision and features. Record lockfile, toolchain, tree, signatures and
   digests. Keep the established video profile and endpoint identity.
3. Use the estate's GDS plan/apply/verify path in an isolated estate checkout;
   freeze inputs through its terminal result. Commit the exact gitlink and
   private receipts, then refresh the controller without restarting it.
4. Before a scoped RDS installation, inspect actual protected process identity,
   cgroup ownership, API health and active jobs. Any lifecycle action on protected
   processes needs the owner's current explicit authorization. Recheck identity
   and health after the operation; a CLI exit code is insufficient.
5. Run native clipboard in both directions with empty, Unicode and large INCR
   text; immediate Copy/Cut focus handoff; local clipboard change during a
   transfer; two devices and two monitor routes; close one while exercising the
   others. Use an isolated test application/pasteboard and retain failures.
6. Observe ordinary input/Backspace, idle/background and reconnect counts on
   the installed build. Report ACK, complete payload, decoded submission and
   physical pixels separately. Missing physical evidence stays NOT RUN.

The source/checkpoint part of the bounded feature increment is complete.
Deployment completion is determined by the private estate's exact receipts.
Wave 2 physical/clipboard/mixed-load acceptance and the full desktop/transport
milestones remain open.

## Wave 3 — bounded durable sync (W1.9/W1.10, W8.1–W8.6)

Completed increments: journal preparation now observes peer/caller/async-drop
cancellation, streams catalog names and bounds opportunistic collection.
The [destination-binding repair](reports/rds-journal-destination-scope-20261010.md)
binds new state to normalized destinations and retains only verified
same-destination legacy resume. The [terminal-supervision follow-up](reports/rds-sync-terminal-supervision-20261010.md)
covers legacy/tagged control refusal/EOF, quiet control during active data
and dropped queued assembly at the registered `w1-sync-supervision` checkpoint.
These repairs retain their source/qualification evidence; quota admission,
fair background reclamation and destination overwrite policy are next.
They do not close native Wave 2 acceptance or recursive sync.

1. Preserve the existing bounded/cancellable preparation and destination
   ownership contracts while adding quotas and reclamation. Retain large-catalog,
   malformed-state, aliasing, interrupted-cleanup and control-supervision regressions.
2. Define quota admission and its persisted accounting using the existing
   journal owner. Reserve final-assembly space, retain resumability, distinguish
   disk-full from corruption, and avoid a second independent state database.
3. Define destination preconditions/overwrite policy for concurrent local edits.
   Recheck at publication; an advisory RDS lock does not exclude local writers.
   Keep no-follow directory handles and explicit uncertain-commit outcomes.
4. Exercise real sender/receiver process termination at 20 seeded boundaries on
   a 1 GiB file, disk-full, permission error, repeated chunks, aliasing and
   restart. Verify receiver digest and durable commit, including macOS.
5. Extend the existing `directory::{scanner,wire,reconcile}` foundation with
   a journal-backed one-way apply engine: operation IDs, persisted preconditions,
   delete policy and tombstones. Expose preview before destructive apply; add
   grant-scoped stream routing only after the storage engine is correct.
6. Add watch-plus-scan repair and filters, then two-way conflict preservation.
   Define modes/mtimes, symlink/hardlink/sparse semantics, case/Unicode aliases
   and unsupported attributes with Linux↔macOS fixtures. Never infer deletion
   from an incomplete scan or select a conflict winner implicitly.

Exit: `r8-transfer` and later `r8-directory-sync` need separately registered
checks; the existing `w8-directory-foundation` gate covers only scan/wire/preview.

## Wave 4 — policy, session and network completion (W2–W5)

| Order | Owning implementation | Required next change and verifier |
|---|---|---|
| 4.1 | `rds-core::grant`, `rds-discovery::{registry,policy,authority}`, private GDS issuer | Clean enroll→resolve→authorize→renew→revoke; rotation and rollback/offline tests; applied revision acknowledgements; no hand-edited grant workflow. |
| 4.2 | `rds-agent::{authz,limits}`, `rds-client::local`, `rds-net::{uni,deadline}` | Complete negotiated per-service/process budgets; bounded memory/FD/tasks; slow peer, cancellation, multi-user and SSH+desktop+sync fairness. Existing lifecycle guards remain owners. |
| 4.3 | `rds-net::backends`, `rds-relay`, `rds-bench` | Same topology definitions for both lanes: direct, relay, asymmetric loss, NAT/rebinding, interface/suspend, relay/directory outage. Add owned TLS carrier/federation only behind explicit policy and identity validation. |
| 4.4 | Existing ACK/path observers and sampler ownership | Complete accounting for short/retired paths; bounded restoration under thousands of session/path changes; no synthetic address advertisement or replay of application input. |
| 4.5 | `rds-ssh`, `rds-cli::ssh`, GDS account/host trust | Provision host/account trust; native macOS terminal restoration and mixed-load interop. Design scoped server/account broker and PTY reattachment explicitly; generic TCP failure never replays exec. |

Exit: no Noq/default promotion until actual parity passes. Register the proposed
`r2-session-core`, `r3-connectivity-parity`, `r4-enrollment-policy`, `r5-ssh`
gates as their automated checks become real. Unavailable topology is NOT RUN.

## Wave 5 — native platform/media work (W6–W7, W9)

1. Finish X11 geometry/focus/controller ownership and mixed-load native
   acceptance using `x11_displays`, input workers, frame delivery and renderer.
   Preserve existing monitor identity/refusal semantics on reconfiguration.
2. Implement macOS user-session capture, TCC denial/regrant and release-all input
   with stable signed helper identity. Add VideoToolbox with bounded callbacks
   and surface lifetime before claiming hardware quality/CPU improvement.
3. Implement portal ScreenCast/RemoteDesktop and PipeWire/EIS on declared
   Wayland compositor profiles; honor one-use restore tokens and revocation.
   Add privileged image-copy/KMS/uinput only as explicit permitted profiles.
4. After Wave 1, add injectable audio source/sink and scoped service negotiation,
   native capture/playback, bounded callback queues, real playout clock/drift,
   PLC deadlines and A/V synchronization. Microphone permission is separate.
5. Add file drop through the existing sync authorization, then profile optional
   GPU codecs/FEC/adaptive resolution with receiver quality and resource data.
   Windows, browser/RDP interop and broadcast retain their later roadmap scope.

Exit: real supported devices/compositors, permission and lifecycle tests,
current-artifact quality/latency/resource results. Stubs never satisfy a gate.

## Wave 6 — operational qualification and final reconciliation (W0/W10)

1. Complete secured diagnostic bundle/source metric coverage and lifecycle
   correlation; no clipboard, typed text, credentials, private paths or endpoint
   inventory in generic exports. Measure observer overhead and failure isolation.
2. Qualify actual release artifacts, native signing/notarization, clean install,
   atomic activation/rollback, protocol/state compatibility and service health.
   Keep published release documentation bound to its original source.
3. Run supported impairment/topology, mixed-load, churn and soak rows with exact
   source/features/toolchain, sample count, failures/skips and p50/p95/p99.
   Preserve failed evidence; do not weaken thresholds or retry to manufacture green.
4. Reconcile all current docs with code; mark dated research/progress as historical
   instead of deleting original failures. Refresh private generated memories
   from committed canonical sources and validate them. Replace stale navigation
   notes with current source links; never rewrite live session databases.
5. Reobserve local/origin OIDs, estate gitlink, controller snapshot, effective
   policy/configuration, installed digest and running image independently.
   Retain a named verifier for every remaining mismatch. No blanket stability
   claim while a required implementation or qualification row remains open.

## Research and after-wave procedure

Research retrieved on October 9 uses standards available by the requested
September 26, 2026 boundary. Current mutable platform pages are implementation
references, not proof of their historical contents.

- [RFC 6716 §§3.1–3.2](https://www.rfc-editor.org/rfc/rfc6716.html#section-3.1)
  defines Opus frame packing and duration; [libopus 1.5 API](https://opus-codec.org/docs/opus_api-1.5.pdf)
  supplies packet parsing and stateful decode/PLC contracts. The RDS one-frame
  profile and jitter retention policy are local decisions, not RFC mandates.
- [RFC 9000 §§2–4,8–9](https://www.rfc-editor.org/rfc/rfc9000.html)
  separates ordered streams, flow control and path validation. Prioritization
  alone cannot prove application fairness or live replacement progress.
- [Tokio blocking work](https://docs.rs/tokio/1.48.0/tokio/task/fn.spawn_blocking.html)
  requires explicit concurrency/cancellation ownership; aborting an async
  waiter does not terminate a running filesystem or codec call.
- [Directory synchronization](https://www.man7.org/linux/man-pages/man2/fsync.2.html)
  is separate from file-data synchronization. Crash claims need both barriers
  and tested recovery rather than rename alone.
- [XDG RemoteDesktop](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
  defines session/device permissions and EIS input. Platform support requires
  actual session integration, not only a trait or protocol enum.

After each bounded wave: inspect the final diff and relevant upstream contract,
run the exact checks for changed surfaces, retain the benchmark/checkpoint
receipt, make signed Conventional Commits, publish through a PR, and reobserve
public/estate/runtime state before choosing the next wave. Do not run concurrent
Cargo processes against the same target directory. Amend this plan when a new
reproducer changes the order; no new implementation starts from stale session
prose alone.
