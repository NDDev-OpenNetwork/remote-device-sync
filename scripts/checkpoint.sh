#!/usr/bin/env bash
# checkpoint.sh — run a wave's gate checks and emit the report skeleton.
#
# Usage: scripts/checkpoint.sh <gate>
#
# A gate id is registered exactly once below; an unknown id exits 2 —
# a checkpoint with no registered checks is refused by design
# (docs/implementation-plan.md §Checkpoint protocol).
#
# Reports land in docs/reports/: bench-<ts>.{json,md} plus
# checkpoint-<gate>.md, a checklist to fill in honestly before merge.

set -euo pipefail
cd "$(dirname "$0")/.."

GATE="${1:-}"
TS="$(date -u +%Y%m%d-%H%M%S)"
REPORTS="docs/reports"
mkdir -p "$REPORTS"

note() { printf '\033[1m== %s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAIL\033[0m %s\n' "$*" >&2; exit 1; }

green_bars() {
    note "fmt"
    cargo fmt --check || fail "cargo fmt --check"
    note "clippy"
    cargo clippy --workspace --all-targets -- -D warnings || fail "clippy default"
    if [ "$(uname -s)" = "Linux" ]; then
        cargo clippy --workspace --all-targets --features rds-desktop/x11 -- -D warnings \
            || fail "clippy x11"
    fi
    note "test"
    cargo test --workspace || fail "cargo test"
}

write_checkpoint() {
    local gate="$1" verdict="$2" extra="$3"
    cat > "$REPORTS/checkpoint-${gate}.md" <<EOF
# Checkpoint ${gate^^} — ${TS}

Verdict: **${verdict}**

## Automated checks
${extra}

## Manual checklist (fill before merge)
- [ ] All "not run" items above explained
- [ ] Reports committed: bench-*.json, bench-*.md, this file
- [ ] docs/ updated for anything this wave changed
- [ ] security/unsafe review done for new code paths
EOF
    note "wrote $REPORTS/checkpoint-${gate}.md"
}

case "$GATE" in
c0)
    note "gate c0 — measurement harness baseline"
    green_bars

    note "bench suite run 1"
    cargo run -q -p rds-bench -- run --scenario all \
        --json "$REPORTS/bench-${TS}-a.json" --md "$REPORTS/bench-${TS}-a.md" \
        || fail "bench run A"

    note "bench suite run 2 (reproducibility)"
    cargo run -q -p rds-bench -- run --scenario all \
        --json "$REPORTS/bench-${TS}-b.json" --md "$REPORTS/bench-${TS}-b.md" \
        || fail "bench run B"

    note "reproducibility compare (p95 ±15%)"
    cargo run -q -p rds-bench -- compare \
        "$REPORTS/bench-${TS}-a.json" "$REPORTS/bench-${TS}-b.json" --tol 0.15 \
        || fail "suite not reproducible"

    cp "$REPORTS/bench-${TS}-a.md" "$REPORTS/baseline-iroh.md"
    write_checkpoint "c0" "pending review" \
        "- fmt/clippy/test: PASS
- suite A: bench-${TS}-a.{json,md}
- suite B: bench-${TS}-b.{json,md}
- reproducibility p95 ±15%: PASS
- baseline: baseline-iroh.md"
    ;;
*)
    cat <<EOF
unknown or unregistered gate: '${GATE}'
registered gates: c0
a checkpoint with no registered checks is refused by design.
EOF
    exit 2
    ;;
esac
