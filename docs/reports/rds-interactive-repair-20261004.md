# Interactive repair and client-paced repeat — 2026-10-04

W6.6/W6.7 follow-up; native two-device latency/stability acceptance remains open.

An explicit RequestIdr previously set a producer flag while the producer could
be waiting for an obsolete independent picture or all three media permits.
The isolated transport regression kept input/heartbeat working but failed its
one-second replacement bound before the change. Session-local repair epochs
now cancel only that writer's obsolete media writes/receipt tasks. Raced encodes
and stale queued references are discarded; the next producer epoch forces IDR.

An owned key lease closes the admission gate until transport confirmation. An
old producer/receipt cannot install or release a newer epoch's gate. Dropping a
fresh key during cancellation requests another independent picture before its
successors. Requests coalesce while a recovery picture is in flight, preventing
repeated watchdog observations from repeatedly canceling its valid progress.
Epoch and gate flags update atomically. Existing deadlines, queue/reader budgets,
connection services and wire types remain unchanged.

Real-loopback cases cover Iroh/Noq partial independent writes and full admission,
an Iroh receipt wait through a pinned 80 ms/1 Mbps UDP proxy, native H.264
replacement decode/pixel checks, input ACKs and an unrelated bidirectional
stream. The reported request-to-observation bound includes its control fence;
it is not physical presentation or production WAN latency.

A second native Xvfb regression reproduced eight keypress events from one
physical-style KeyDown with its KeyUp delayed 160 ms. Server typematic generated
extra letters independently of the client's intent. The native viewer previously
ignored local OS repeat events. X11 injection now leases per-key native repeat
to off while retaining a real held key. Local repeats for forwarded, still-held
non-modifier keys pulse exactly one release/press. Repeated modifiers/locks do
not toggle, and repeat events do not repeat clipboard side effects.

Keyboard leases share counts across controllers and X screens. The last release
restores the original per-key native repeat setting. The native regression now
observes one keypress, three intentional client repeats, a hold surviving one
controller's release and complete key/repeat cleanup after the last owner drops.
It runs only on disposable Xvfb, never a developer's active desktop.

The native viewer and serving agent must be updated together for client-driven
repeat behavior. Hardware/composited input, sustained WAN loss/jitter, and native
causal click-to-visible timing require installed qualification. This report is
not a completed stability gate; final checks belong to the published candidate.

Local validation: formatting; default and desktop workspace Clippy with warnings
denied on macOS; the complete desktop suite with X11/viewer enabled; Linux X11
workspace Clippy; all eight native X11 input cases on disposable Xvfb. Default
workspace and published-candidate CI results are recorded by their actual run,
not inferred from these feature-specific checks. No real desktop input was
injected by the native regression fixture.

The first Linux CI run exposed an existing metrics-fixture race: server sampling
immediately after send.finish() could precede actual UDP delivery. The echo test
now uses a peer-observation fence before sampling, retaining the traffic
assertions without arbitrary sleep. This changes test synchronization only;
it does not change production transport accounting.
