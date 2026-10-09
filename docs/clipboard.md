# Desktop clipboard contract

The desktop control stream supports bounded UTF-8 text in both directions.
The original viewer-to-device paste remains `DesktopControl::ClipboardChunk`;
the reverse direction is an additive `DesktopV5` capability.

Reverse transfer uses an offer/request handshake:

1. The serving backend notices that another native application owns
   `CLIPBOARD` and sends `DesktopEvent::ClipboardOffer { id, format, bytes }`.
2. The viewer applies its local policy and requests that exact offer with
   `DesktopControl::ClipboardRequest { id, format }`.
3. The server sends `DesktopEvent::ClipboardChunk` records. The viewer replaces
   its native text clipboard only after all chunks have reassembled and passed
   UTF-8 validation.

Only `ClipboardFormat::TextUtf8` is implemented. Images, files, rich MIME data,
primary selections and arbitrary URI/file transfer are separate future
capabilities. A peer must not infer those formats from text bytes.

Both directions share the existing limits: 1 MiB total, 16 KiB viewer/server
chunks, ordered offsets, one active assembly per session, a five-second idle
deadline and a thirty-second total deadline. The server owns one bounded native
clipboard worker per desktop session. X11 uses the ICCCM selection conversion
protocol, including INCR for larger text; it does not invoke `xclip`, `xsel` or
another helper process. XFixes selection-owner notifications wake the worker;
older X servers use a bounded 4 Hz fallback poll. A failed owner, unsupported
format, stale offer or invalid UTF-8 ends only that transfer and returns a
typed error.

Clipboard payloads never enter `Debug`, tracing fields or diagnostics. Logs
contain transfer IDs and byte counts only. A remote paste published by the
session's own X11 selection owner is ignored by the external-owner watcher, so
the two directions do not echo each other indefinitely.

A focused viewer requests remote offers. An explicit Cmd+C/Cmd+X also permits
one offer arriving within five seconds after switching to a local app, with
the native clipboard change-count captured at the gesture. A new local copy
cancels that handoff; background windows cannot arm it. A request may complete
after focus changes under the same change-count fence. A newer explicit copy
drains an older transfer without publishing its stale snapshot. View-only
sessions can neither read nor publish clipboard content.

The native macOS viewer publishes a completed reverse transfer through
`NSPasteboard` on the AppKit main thread. Its local-to-remote Cmd+V handling is
still explicit. Cmd+C/Cmd+X translate to remote Ctrl+C/Ctrl+X and retain Shift
for terminal Ctrl+Shift+C. AppKit's isolated named-pasteboard probe qualifies
native publication; ordinary installed cross-device use remains a separate
observation. Linux Wayland portal clipboard remains a platform gate.

`DesktopV5` is additive and has no silent downgrade. A caller that needs the
legacy peer contract sets `SessionOpts::reverse_clipboard` to `false`; the
older V2/V3/V4 greetings then retain their existing wire layouts.
