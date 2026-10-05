# Desktop paste/control backpressure — 2026-10-05

This W6.4/W6.7 and O4 increment bounds control replies and improves native input
observation. It does not close installed network, application-response or
sustained stability acceptance.

## Implemented boundary

The serving clipboard-ready reply previously awaited an unbounded framed write.
Input ACKs and heartbeat replies inherited the thirty-second media budget. All
three now use a separate two-second write budget. Failure exits the owning
desktop control loop; `SessionSend` resets the stream, so a partial record cannot
be resumed or presented as a clean FIN. Media, authorization, connection and
unrelated service-stream policies remain unchanged. There is no input or paste
replay and no wire-format change.

Clipboard publication start/completion and ready-reply completion include only
transfer ID, byte count and stage duration. The viewer correlates exact ID/size
with local gesture-to-ready timing, at most eight outstanding entries and 1024
completed samples. Wrong, duplicate and retired replies cannot create successful
samples. Evictions, cancellations and unmatched replies remain counted. Pending
and completed delays over 250 ms trigger bounded before/after incident windows.
Keyboard and button ACK series are independent of motion and each bounded to
1024 samples; the existing aggregate series remains available.

## Regression scope

- A real two-byte-capacity Tokio pipe holds a partially written clipboard reply:
  it returns `TimedOut` at two seconds, rather than awaiting indefinitely.
- A timely pipe preserves clipboard-ready, input-ACK and heartbeat records in
  order with exact values.
- Nearly 2000 fast motion acknowledgements can push a slow keyboard/button sample
  out of the aggregate series; their independent series still report 900/700 ms.
- Clipboard tracking refuses wrong IDs/sizes and duplicates, bounds outstanding
  work, counts evictions and rejects replies after epoch cancellation.
- A completed slow paste entirely between diagnostic ticks still records an
  incident, as does a pending paste; metadata contains no clipboard payload.

These synthetic regressions establish the local boundary. They do not establish
that an unbounded write caused a particular live complaint, that a target GUI
consumed the paste, or that submitted GPU frames reached physical scanout.

## Validation status

The desktop viewer library's 100 unit tests passed after the implementation.
An initial test build referenced an unavailable test-only serializer; it was
corrected to compare decoded control variants without adding a dependency.
An initial CLI invocation used a nonexistent feature name; subsequent checks
must use the actual `rds-cli/desktop` feature. Formatting, strict workspace/native
clippy, full workspace tests, Linux X11 and installed qualification remain to be
recorded before this increment is considered complete.
