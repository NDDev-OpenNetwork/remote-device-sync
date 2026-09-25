# Engineering conventions

Rules every change follows. CI enforces what it can; the rest is review.

## Dependency and layering rules

- Dependencies point strictly down the crate map in
  `docs/architecture.md`. Adding an upward or sideways dep is an
  architecture change: discuss it in the PR body.
- `rds-core` is a leaf: no io, no async runtime, no platform code.
- No new third-party dependency without need: prefer standards,
  thin FFI bindings to OS/driver APIs, and our own protocol code.
  Justify in the PR description; `deny.toml` gates licenses.
- Public API of a crate lives in `lib.rs` re-exports; platform modules
  may be `pub` but unstable surfaces are `#[doc(hidden)]` or doc-noted.

## Safety

- `unsafe` is confined to platform-backend modules (`rds-desktop`
  capture/codec/input, `rds-net` socket layer, and the read-only
  `rds-discovery/clock/macos.rs` boot-identifier adapter). Workspace lint flags it
  everywhere else; backends opt out per module with a `// SAFETY:` note
  per block.
- Never trust the wire: every decoder/parser bounds its inputs; every
  record verifies before use (`rds-discovery` does this on `put`).

## Errors and async

- Library crates return typed errors (`thiserror`); binaries may use
  `anyhow`. No `unwrap`/`expect` outside tests and
  impossible-invariant paths (commented).
- Tokio is the only runtime. No `std::thread` for core loops;
  `spawn_blocking` for sync FFI (capture backends, codecs).
  `rds-observe` isolates synchronous stderr in one bounded output adapter
  thread, outside service/transport loops; a stuck OS write must not hold
  Tokio runtime shutdown. See [observability](observability.md) for the
  bounded wait and explicit possible record loss.
- No unbounded queues on latency paths: bounded `mpsc`, drop-stale
  policy at the producer, never let backlog accumulate.

## Wire protocol

- `rds-core` owns every wire type; postcard + explicit length prefix;
  64 KiB max frame on control paths. Media streams carry raw codec
  bitstream with a fixed header — no serde in the hot path.
- Protocol versioning rides the ALPN (`rds/0`, `rds-relay/0`).

## Platform code

- One backend per file; `#[cfg(target_os)]` on the module declaration,
  not inside functions. Preference orders are fixed and documented in
  `docs/platforms.md`.
- A backend that probes unavailable returns `Ok(None)`; present-but-
  broken returns `Err` — and the demotion is logged.

## Testing lanes

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace` — green on Ubuntu and macOS in CI.
- Feature lanes: `--features rds-desktop/x11` on Linux;
  `--features rds-agent/desktop,rds-cli/desktop` on both OSes — the
  desktop feature must compile everywhere, not only where it runs.
- Protocol/interop tests live in `tests/` of the owning crate; e2e
  transport tests must not require a display.
- Benchmarks are acceptance gates for the transport and media work —
  numbers land in `docs/reports/` per roadmap.

## Commits and docs

- English only. Signed commits on `main` (ruleset enforced).
- Docs-as-code: architecture decisions in `docs/architecture.md`,
  research in `docs/research.md`, platform facts in
  `docs/platforms.md`. A design change that doesn't update its doc is
  incomplete.
