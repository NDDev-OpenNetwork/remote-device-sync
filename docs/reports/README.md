# Reports

Evidence produced by the checkpoint protocol
(`../implementation-plan.md`). Nothing here is hand-measured — every
number comes out of `rds-bench` or a named tool run.

## Files

- `bench-<yyyymmdd>-<hhmmss>-{a,b}.json` — raw suite output;
  `rds-bench run --scenario all --json …`
- `bench-<…>.md` — rendered markdown of the same suite
- `baseline-<backend>.md` — the reference suite a backend must match
- `checkpoint-<id>.md` — gate report: automated results + the honesty
  checklist, committed with the wave's merge

## Reproduce

```sh
scripts/checkpoint.sh <gate>          # run a registered gate
cargo run -p rds-bench -- run --scenario all --json out.json --md out.md
cargo run -p rds-bench -- compare a.json b.json --tol 0.15
```

Impairment knobs on `run`: `--loss --delay-ms --jitter-ms --rate-mbps
--seed`. Same seed → same drop schedule; the proxy's counters are
reported per scenario so impairment engagement is provable.
