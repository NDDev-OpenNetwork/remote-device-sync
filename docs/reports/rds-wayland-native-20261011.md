# Native Wayland functional qualification — 2026-10-11

Scope: the production portal adapter at signed source `da0d7d8`, on Linux
x86_64, Ubuntu 26.04, GNOME 50, portal backend 50.0 and PipeWire 1.6.2.
Host identity, addresses, endpoint keys and permission tokens remain private.

After explicit local selection, the combined portal returned two monitor
streams. Each produced three real software H.264 frames which decoded to the
declared output dimensions. Source extents were 1920×1080 and 3840×2160.
The prepared EI worker also resolved both monitor regions and a resumed
keyboard. The source closed its native workers and portal owner normally.

A fresh process restored the saved permission without a new selection dialog.
Both extents and numeric display IDs were retained; three decoded frames per
monitor passed again. The private state remained a single-link mode-0600 file.
This verifies restoration within that permission history, not global physical
monitor identity or hotplug behavior.

An additional opt-in native example checked both controlling leases' EI sync
ACKs. It submits a key-up to a fresh source which has never held that key. The
hold aggregator emits no keyboard event; only the compositor sync round trip
is sent. The real EI socket fixture now verifies that unowned releases leave
its complete key-event log unchanged. Both native ACKs and another three decoded
frames per monitor passed. No foreign application receives typing, clicks,
scrolling or pointer movement from these checks.

During attended preparation, an earlier request remained pending with an empty
screen-cast session. GNOME's dialog can permit interaction without selected
monitor tiles; its empty-stream ready path explains a possible hang. This is a
source-backed inference, not proof of every prior selection. Explicit local
SIGTERM cancellation joined the owned request/session/connection cleanup. A new
request with both tiles selected returned normally. Desktop and unrelated input
service identities were retained throughout. The initially timed-out requests
and failed CI validation/Noq fixture remain in the boundary report.

The native example and all-target portal Clippy passed. Seventeen deterministic
portal cases cover frame/geometry/state, real EI sockets, directed early D-Bus
responses, denial, request identity, cancellation order and native worker exit.
The registered checkpoint and its synthetic handshake measurement remain
separate evidence. Native frames and EI sync ACKs do not measure end-to-end
latency, physical key-to-pixel behavior or a long GPU soak.

Compositor-initiated permission revocation, physical pointer/key behavior,
hotplug identity acceptance and installed multi-device qualification retain
their separate gates. Portal clipboard, relative pointer input, hardware codecs
and unattended login-screen access are not implemented by this increment.
