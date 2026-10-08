# CLI configuration test stability — 2026-10-09

The audio-frame-duration pin was correctly refused by GDS before mutation
because its apply-time module re-verification found a required test failure in
`crates/rds-cli/tests/configuration.rs`. The failing commands were all using
the shared `tests/support/endpoint_cli.rs` helper, whose child-process exit
bound was five seconds. Under a cold, loaded macOS verification runner the
binary occasionally exceeded that bound before argument validation, producing
the misleading `invalid configuration did not fail before startup` panic.

The helper now keeps a bounded 30-second exit deadline and reports a neutral
command-timeout message. The product behavior and identity-safety assertions
are unchanged: invalid commands must still exit unsuccessfully and must never
create an endpoint identity. A direct rerun of the complete workspace shard
at public audio commit `0df2e50d` passed after the cold compile; this patch
makes that required verification deterministic under the documented runner
load instead of relying on a warm process start.
