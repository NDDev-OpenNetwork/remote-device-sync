# Impaired control-reader closure — 2026-10-10

Status: retained CI failure; cause not yet established. No budget or workload
change, and no acceptance inferred from later passing samples.

PR151 source `e6a9106`, macOS job `114164676476`, failed the default workspace
`session_v2` test because the control event reader ended during the 60-second
measurement. This is a session-liveness failure, not a p95 assertion. It followed
the bounded-queue case and ended approximately 45 seconds into the impaired
case. The job's filter omitted the relevant debug termination diagnostics.

Two predeclared standalone diagnostic cohorts on the same clean source passed
with 601 responses each and p95 346/362 ms. Both are retained; neither explains
or retires the CI closure. The earlier feature-dependent synthetic decoder
fixture has already been corrected and is not assumed to cause this new event.

Investigation order:

1. Retain terminal/reader/frame-worker diagnostics in the failing test output,
   including cohort progress and whether its server/encoded consumer ended.
2. Run the complete serialized session suite once with the same impairment,
   seed, deadlines and 400 ms bound to include the preceding-case lifecycle.
3. Use an observed terminal cause to construct a deterministic regression;
   repair the owning boundary and requalify. Do not change a timeout or retry
   until green, and do not label a diagnostic pass as a fix.
