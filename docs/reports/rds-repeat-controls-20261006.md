# XKB repeat control — 2026-10-06

This W6.3 input increment removes core keyboard-control map invalidation from
native holds. The X server source establishes a concrete unnecessary side effect;
Mutter46 rebuilds its keymap after that event. It is not proof that every prior
freeze, reconnect or long application response had this cause.

The XKB control update selects only PerKeyRepeat, preserves other controls and
array bits, skips no-op writes and retains shared hold/repeat/drop behavior.
All RDS controllers serialize those read-modify-write updates under their
existing hold mutex. Independent external settings writers remain outside
that ownership. No global autorepeat disable, keyboard remapping, input replay,
remote wire change or desktop restart is introduced.

The native Xvfb delayed-release regression additionally observes map events
through delayed KeyUp, explicit repeats, multiple owners, drop cleanup,
modifier edges and Backspace. The pre-existing timing and repeat assertions
remain. Local Linux native execution passed all9isolated Xvfb tests, including the
expanded repeat/map regression. XTEST's initial device adoption is warmed before
notification observation; the first unprepared fixture recorded its legitimate
NewKeyboardNotify and failed, so initialization was made explicit without
allowing any map notifications during the measured holds. Strict checks and
installed composited-desktop qualification remain pending.
This receipt does not close the native latency/stability acceptance gates.
