# Stability execution plan — 2026-10-09

Baseline: `93be1f014329e2ea7d573c209c514a67d4b22fb8`. This is the current
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

## Findings checked against the baseline

| ID | Actual code/evidence | Remaining boundary |
|---|---|---|
| A1 | `rds-audio/src/lib.rs`: packet admission checks byte length and the caller's sample count, without parsing the Opus frame table or confirming its duration. Decoder checks duration only after a stateful decode. | Validate packet structure and duration before buffering or touching decoder state; distinguish the 1275-byte frame limit from packet framing. |
| A2 | `JitterBuffer::push/pop/start_at`: full buffers discard a retained packet before considering an older incoming packet; leading queued packets survive `start_at`; one `Gap` skips an arbitrary sequence range. Sequence/counters wrap. | Specify monotonic playout, exact loss duration, retention policy, startup and sequence exhaustion; test adversarial reordering and all loss boundaries before audio I/O. |
| A3 | `rds-sync/src/journal.rs` already has `collect_superseded` and verified metadata/inode checks. `Directory::children` materializes an entire directory. | Preserve safe same-destination cleanup. General quotas, bounded collection, interrupted collection and newer-destination conflict policy remain open; do not describe all GC as absent. |
| A4 | `DesktopV5`, `clipboard::{Assembly,ReverseSender}`, native clipboard worker and `render::clipboard` implement bidirectional text. `viewer_main::window_plan`, `x11_displays` and `multiple_desktops` implement independent windows and monitor routes. | Exact installed-build acceptance, focus/copy handoff, multiple independent native windows and physical monitor transitions still need evidence. Retain V2–V4 compatibility and view-only refusal. |
| A5 | `rds-core` has grant v3, per-session desktop/sync tags and local wire v5; `rds-agent` limits/authz and `rds-client::local` own bounded service/session lifetimes. | Automatic issuer enrollment/renewal, account/seat isolation, negotiated global resource/QoS limits and mixed-load qualification are unfinished. Do not redo already implemented IDs and directional grants. |
| A6 | `capture/{sck,image_copy,kms,pipewire}.rs` return `Ok(None)`; native macOS/Wayland input and hardware codec modules return `false`. Agent explicitly refuses audio. | Native serving and device audio are implementation work, not a documentation or feature-flag change. No capability promotion from library tests. |
| A7 | ACK progress, independent initial validation, persistent relay registration and standby restoration exist in both backend adapters/vendored engines. | Physical topology parity, interface recreation, suspend/rebind, owned UDP-blocked carrier, federation and complete retired-path accounting remain open. |
| A8 | Release packaging has digest/provenance checks; current release documentation also describes an older published preview. Current docs and dated progress sections sometimes mix those epochs. | Separate current implementation from historical release content; bind source, consumed gitlink, artifact, running image and effective policy independently. |

## Wave 1 — repair the audio foundation (W9.1, W0.1)

1. Add failing regressions for malformed frame tables, forged sample counts,
   valid multi-frame packets outside the selected contract, maximum frame plus
   header, and invalid input leaving decoder state unchanged. Use the existing
   libopus parser; do not introduce another Opus implementation.
2. Define the existing fixed 20 ms, one-frame packet profile precisely. Apply
   the same validation in wire conversion, jitter admission and decode. Keep
   codec-supported channel conversion distinct from the negotiated PCM channel
   count. Validate negotiated sample counts, not arbitrary caller integers.
3. Define bounded jitter behavior before adding device clocks: retain the
   newest bounded set on overflow, reject packets before the playout floor,
   discard pre-start stale entries, represent every concealed interval, and
   avoid sequence wrap reopening old audio. Make large discontinuities explicit
   instead of silently compressing elapsed audio into one PLC call.
4. Add deterministic reorder/loss/duplicate/overflow/exhaustion tests and real
   libopus encode/decode tests on all supported rates/channels. Preserve the
   original failing cases in the report. No audio service advertisement.
5. Run workspace checks plus a registered audio-foundation checkpoint and a
   reproducible codec/buffer benchmark through `rds-bench`. Commit fixes in
   separate packet-validation and jitter changes, then their evidence/docs.

Exit: an exact, bounded library contract; device audio and A/V synchronization
remain W9.1. The source is not installed merely to claim a new audio service.

## Wave 2 — finish the existing desktop delivery (W6, W9.2, W4.5/W10.4)

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

Exit: reviewed source, consumed source and installed/running artifacts have
independent verified provenance. This closes the bounded feature rollout, not
the full desktop or transport milestone.

## Wave 3 — bounded durable sync (W1.9/W1.10, W8.1–W8.6)

1. Bound journal enumeration and cleanup work/memory under existing receive
   locks. Preserve unknown entries, unsafe aliases and unrelated destinations;
   cancellation must release work at a documented boundary. Test large stale
   catalogs, malformed metadata, symlinks/hardlinks and interrupted cleanup.
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
