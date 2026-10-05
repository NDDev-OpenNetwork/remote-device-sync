# X11 input and native qualification

Scope: part of W6.4/W6.8. The Rust backend uses `x11rb` and the server's
standard XTEST/core protocol. No runtime helper process or dependency is added.

The wire carries Linux evdev codes. The backend reads the actual core
keyboard's XKB physical key names and resolves those names to native X keycodes.
This preserves remote layout/group interpretation on both evdev and legacy
XFree86 maps; an evdev `+8` guess could turn an Up arrow into Print Screen on
a legacy session. Zero, missing/ambiguous names, overflow and unsupported
physical keys are refused instead of injecting an unrelated key. The existing
x11rb dependency enables its XKB protocol feature; no new library or runtime
helper is introduced. Core keyboard-map notifications refresh the cache;
key-up and teardown release the exact native key recorded at key-down.
Buttons map explicitly: left/right/middle to X buttons 1/3/2 and evdev
side/extra/forward/back/task to 8–12. Server pointer remapping still applies;
unsupported native buttons produce an error rather than a successful ACK.

Native repeat is disabled only for an injected held key. The viewer supplies
intentional local OS repeats as repeated KeyDown events; non-modifier repeats
pulse one release/press while retaining the hold. Network-delayed KeyUp does
not generate server-side typematic letters. Shared per-key leases across
controllers/screens restore the original repeat bit after the final release.
Viewer and serving agent must be updated together. Native state restoration is
best effort on teardown; a killed process or failed X connection cannot guarantee
cleanup. Other local X clients and distinct agent processes do not participate
in this process-local ownership. Concurrent local-seat ownership remains open.

Absolute motion uses the selected root window and checked display coordinates.
Relative motion uses XTEST's relative flag, with checked signed 16-bit deltas;
it never interprets offsets as absolute coordinates. Absolute positions round
down to a pixel and relative deltas round to the nearest pixel. NaN/infinity,
out-of-display positions and unrepresentable values fail before injection.
Scroll uses fractional wheel steps, positive x left/positive y up, retaining
fractions and limiting each axis to 32 clicks per event. Every emitted request
is flushed and checked; isolated scroll input does not need a later event to
reach the server.

Input workers select the hello/event's X screen instead of DISPLAY's default
screen. Missing capture/input screens are refused rather than silently clamped.
An event must match the sink's bound screen. Key presses check keyboard focus's
root; relative/button/scroll input checks the pointer's current screen. X
screens are not individual RandR monitors. These checks are admission checks,
not an atomic security boundary against other local X clients changing focus.
XTEST shares the native seat; exclusive controller ownership, concurrent local
input, focus-change policy and multi-user consent remain separate work.

The session-owned sink records injected key/button holds. Closing it attempts
their release and completes a server roundtrip before closing its connection.
It does not release keys/buttons it has never pressed. This is best effort if
the X server or connection fails; a blocking native operation can finish after
session cancellation. An ACK means the backend accepted the request, not that
an application displayed its effect. IME, nonstandard physical names,
extended keys, dynamic monitor geometry and input-to-visible latency remain open.

## Required disposable fixture

Native capture/input tests are explicitly ignored during ordinary unit runs so
they never repaint or inject into an ambient developer desktop. Linux CI must
then run them serially on a dedicated two-screen Xvfb server. Missing DISPLAY,
failed backend initialization or failed injection is an error in that lane:

```sh
xvfb-run -a -s '-noreset -screen 0 1280x720x24 -screen 1 640x480x24' \
  cargo test --locked -p rds-desktop --features x11 -- --ignored --test-threads=1
```

Linux CI also runs the navigation case on a second disposable Xvfb instance
with `setxkbmap -keycodes xfree86 -layout us,ru`. It asserts that the native Up
key receives Alt, covering the legacy map where an evdev offset becomes Print
Screen. This fixture never changes an operator's desktop keymap.

The fixture checks real capture/DAMAGE, evdev keys/buttons, motion, lone scroll,
screen mismatch/nonexistent screens, invalid numeric input and release on drop.
`-noreset` keeps the disposable server alive between client lifetimes. Capture
also compares a known painted pixel; see the [qualification report](reports/rds-x11-native-20260926.md).
Xvfb is a test fixture only. Physical/composited X11, Wayland and macOS native
qualification and a graphical viewer still have their own gates.

Primary contracts: [XTEST](https://www.x.org/releases/X11R7.7/doc/xextproto/xtest.pdf),
[per-key repeat control](https://xorg.freedesktop.org/archive/current/doc/libX11/libX11/libX11.html#Manipulating_the_Keyboard_and_Pointer_Settings),
[Xorg evdev mapping](https://cgit.freedesktop.org/xorg/driver/xf86-input-evdev/tree/src/evdev.c),
[Linux event codes](https://docs.kernel.org/input/event-codes.html).

## Per-key repeat control and compositor notifications

Native holds suppress only the held key's autonomous repeat through XKB
PerKeyRepeat controls. Unchanged bits skip a write; changed bits preserve the
other repeat bits and controls. Core ChangeKeyboardControl is deliberately
avoided because it marks repeat explicit and emits MapNotify, potentially
invalidating a compositor's entire keymap on each press/release.
The existing process-wide hold ownership serializes RDS changes. External
keyboard-settings writers are not governed by this mutex. Intentional local
OS repeats still pulse a held key, and the last owner restores its original
repeat bit even during drop. The isolated Xvfb test checks both repeat safety
and absence of MapNotify/NewKeyboardNotify during ordinary input.
