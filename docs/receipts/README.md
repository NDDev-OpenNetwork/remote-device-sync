# Checkpoint receipt histories

`rds-receipts.jsonl` is the active append-only hash chain.

`w8-journal-admission-119d122.jsonl` preserves the independent journal branch's
original chain before integration with the initial-path-observation branch.
Both branches began at the same eight-entry prefix. Their later entries are
valid separate histories, not consecutive entries of one chain. Do not rewrite
their hashes or concatenate the suffixes.

The archived journal benchmark/generated report was committed at `fb2cf2a`,
and its review at `e033e33`. Those commits remain ancestors after the merge.
Fresh combined-source checkpoints append to the active chain. Validate each
history independently with `rds-bench validate-receipts --log <path>`.
