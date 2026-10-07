# Native diagnostic evidence preservation — 2026-10-05

This O4/W10.1 and W6.7 increment repairs observed diagnostic failure boundaries.
It changes no remote wire, input replay, bitrate, path default or server lifecycle.
Private installed transport and host observations remain in the estate.

## Corrected boundaries

- A window previously retained only its initial reasons. Later control silence,
  repair and reconnection now merge into its bounded reason set; distinct recovery
  or evidence-loss transitions survive the ordinary age-symptom cooldown.
- A non-owning output-counter observer makes dropped/oversized records and sink
  write failures visible in live snapshots and incident history. Missing counters
  remain null. The observer retains no writer, queue or worker.
- Storage health reports completed success/error, backlog/eviction, oversize and
  last-success clocks. Four at-most256KiB windows survive failed writes and retry
  once per diagnostic tick. Retry capacity and deliberate loss remain explicit.
- A failed partial snapshot/incident write leaves no completed JSON name. Private
  temporary publication cleans its own inode; incident publication is no-replace
  and preserves existing entries. No power-loss/lossless guarantee is claimed.

## Regression and validation

22 CLI library tests and30 observation library tests pass, including synthetic
ENOSPC retention/recovery, retry-capacity overflow, partial JSON cleanup,
existing-file preservation, later control/recovery reason merging and non-owning
sink-failure observation. Strict expanded native workspace clippy passes.
An initial new test borrowed a subscriber where an owned subscriber was required;
it was corrected to use a retained Dispatch. Full workspace/CI and installed
native qualification remain separate evidence to record before completion.
