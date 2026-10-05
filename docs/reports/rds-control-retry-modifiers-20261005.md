# Interactive retry qualification and modifier transitions — 2026-10-05

W6.3/W6.7/W2.6 follow-up: a failed desktop attempt previously cleared its retry
streak whenever any decoded frame arrived. A control-only failure could thus
reopen repeatedly with only the first 500 ms delay. Both native entry paths now
require video plus a continuous 30-second window of exact heartbeat replies:
reply age at most 2 seconds and successive confirmed probe send times at most
3 seconds apart. Delayed replies or gaps restart qualification. A qualified
attempt may reset the streak when it eventually fails. Short unhealthy attempts
retain 500/1000/2000/4000/8000 ms bounded retry delays. This changes retry pressure,
not the 8-second control-stall budget or the remote protocol.

Physical key presses and modifier flags reconciliation now share idempotent held
transitions. An already held non-repeat press cannot repeat a clipboard side
effect or an input edge. Explicit native repeats retain their existing path.
Tests exercise Option/Alt + Shift with physical/flags notifications in either
order and balanced release. Modifier-only trace records contain sequence, the
modifier's physical code and press/release; letters and clipboard text remain
unlogged. The existing dispatch/ACK timing trace correlates these sequences.

Primary-source research: [winit 0.30.13 modifier handling](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/macos/view.rs)
reports physical modifier input and aggregate flags as separate events; flags
without physical input remain possible. [GNOME 46 keyboard manager](https://github.com/GNOME/gnome-shell/blob/46.0/js/misc/keyboardManager.js)
appends a locale layout to user layouts, and [GNOME input-source handling](https://github.com/GNOME/gnome-shell/blob/46.0/js/ui/status/keyboard.js)
contains its own modifier-switcher and keyboard hold/release path. A duplicate
native layout is therefore not proof of duplicate user configuration or the
cause of every switch delay. Desktop configuration and real input-source
qualification belong in private estate evidence. No dependency upgrade, server
restart, VPN change, input replay or wave-close claim is part of this increment.

Local CLI desktop library: 29 passed. Desktop viewer library: 101 passed.
Strict expanded workspace clippy passed before the final modifier-only trace;
exact final-source checks and installed qualification remain separate evidence.
