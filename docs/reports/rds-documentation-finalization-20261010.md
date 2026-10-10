# Current documentation reconciliation — 2026-10-10

Source reviewed and checkpointed: `51bf87057c6286496797529c24477905d630c8b6`.
Only documentation and generated checkpoint evidence change. Production code,
Cargo manifests/lockfile, vendored engines, build scripts, wire and runtime
policy are identical to the prior published source. Artifact installation and
consumer-pin provenance belong to the private deployment, independently of this report.

The source audit corrected stale AutoNoVsync presentation, default independent
windows, duplicated plan authorities, completed audio/journal/manager work
still described as pending, future platform probe orders presented as support,
and relay examples/blanket failover/TLS metadata claims. The capability matrix
now records the actual eight-tab workspace, profiles and collapsible panels.
Historical plans and research retain their dated requirements/evidence, with
current navigation and explicit history labels; original failures are preserved.

The current renderer chooses supported Mailbox, otherwise Fifo; see
[wgpu 30 presentation modes](https://docs.rs/wgpu/30.0.1/wgpu/enum.PresentMode.html).
The workspace defaults, profile ownership and eight-tab limit were checked in
`rds-cli::viewer_main` and `rds-desktop::render`; audio packet/jitter and journal
cancellation/collection/destination claims were checked in their owning crates.
The local manager uses IPC v5. The IETF multipath document remains an approved
[draft in the publication process](https://datatracker.ietf.org/doc/draft-ietf-quic-multipath/),
not a basis for claiming completed RDS topology/transport acceptance.

Validation: active local file links and Markdown anchors passed; CLI/bench
capability-matrix tests passed. The registered W6 checkpoint passed 1,143 test
executions with zero failures and two explicit qualification ignores, strict
workspace/viewer feature Clippy, launch and managed multiple-desktop contracts.
The fresh release model benchmark used 100 samples of 1/4/8 tabs and 1,024 transitions
per sample, p95 32.375/20.125/68.208 µs. These are local model timings with no
hardware-independent speed limit, native latency or improvement claim.

The first checkpoint completed its test stages but the release benchmark build
failed: CMake noticed a compiler change in a reused Opus build tree, dropped the
caller's install configuration and attempted the default system prefix. The
system install was refused. The original log/cache was retained and that scoped
build tree was recreated with the current compiler and all required flags;
the complete unchanged gate then passed. No global privileges, directory
permissions, package pins, service lifecycle or test thresholds changed.
[CMake's build-tree contract](https://cmake.org/cmake/help/latest/manual/cmake.1.html)
and [install-prefix contract](https://cmake.org/cmake/help/latest/variable/CMAKE_INSTALL_PREFIX.html)
were checked for this correction.

Physical multi-PC/display/clipboard/WAN/suspend/soak, native macOS/Wayland
serving, hardware codecs, audio device I/O/A–V, sync quotas/conflicts/recursive
apply and automatic issuer/account/seat integration retain their acceptance
boundaries. Original toolbar specks were not reproduced, and the Noq impaired
control-reader incident is not closed by this passing checkpoint.

Ignored checks are explicit: `openssh_interop_key_agent_exec_and_os_pty` requires
installed OpenSSH tools and an unlocked isolated test account;
`full_capacity_migration_preserves_every_floor` is the separate 4,096-identity
signed migration capacity qualification. They are not both native-UI checks.
The later reviewed receipt corrects that classification without rewriting the
earlier hash-chain entry or converting either skip into a pass.
