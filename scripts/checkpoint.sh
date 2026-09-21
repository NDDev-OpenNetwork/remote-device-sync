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
c1)
    note "gate c1 — owned noq transport parity"
    green_bars

    note "clippy + tests with transport-noq"
    cargo clippy --workspace --all-targets \
        --features rds-net/transport-noq,rds-agent/transport-noq,rds-cli/transport-noq,rds-bench/transport-noq \
        -- -D warnings || fail "clippy noq"
    cargo test -p rds-net -p rds-agent \
        --features rds-net/transport-noq,rds-agent/transport-noq \
        || fail "noq tests"

    note "turmoil deterministic simulation"
    cargo test -p rds-net --features transport-noq --test turmoil_sim \
        || fail "turmoil sim"

    note "noq bench suite"
    cargo run -q -p rds-bench -- run --scenario all --backend noq \
        --json "$REPORTS/bench-${TS}-noq.json" --md "$REPORTS/bench-${TS}-noq.md" \
        || fail "noq bench suite"

    write_checkpoint "c1" "pending review" \
        "- fmt/clippy/test: PASS
- clippy/tests with transport-noq: PASS
- turmoil partition/repair sim: PASS
- noq suite: bench-${TS}-noq.{json,md}
- impaired-path migration: see checkpoint-c1.md measurement table"
    ;;
c2)
    note "gate c2 — owned relay transport"
    green_bars

    note "clippy + tests with transport-noq and owned-relay"
    cargo clippy --workspace --all-targets \
        --features rds-net/transport-noq,rds-agent/transport-noq,rds-cli/transport-noq,rds-bench/transport-noq,rds-relay/owned-relay \
        -- -D warnings || fail "clippy relay"
    cargo test -p rds-net -p rds-agent -p rds-relay \
        --features rds-net/transport-noq,rds-agent/transport-noq,rds-relay/owned-relay \
        || fail "relay tests"

    note "relay wire decoder fuzz (proptest)"
    cargo test -p rds-core relay::tests::decode_never_panics_and_roundtrips \
        || fail "decoder fuzz"

    write_checkpoint "c2" "pending review" \
        "- fmt/clippy/test: PASS
- clippy/tests with transport-noq + owned-relay: PASS
- relay proto unit tests (rds-core): PASS
- owned relay e2e (attach→handshake→datagrams→streams, replacement, drain): PASS
- rate-limiter unit tests: PASS
- decoder fuzz (proptest): PASS
- relay-kill → second-relay migration: NOT RUN (single-relay config; needs multi-relay attach)"
    ;;
c3)
    note "gate c3 — discovery service + publish/resolve"
    green_bars

    note "directory suite (roundtrip, staleness, rate-limit, delete, registry, hostile, outage)"
    cargo test -p rds-discovery || fail "rds-discovery tests"

    note "announce + resolve e2e (publish, republish, resolve→connect, fallbacks)"
    cargo test -p rds-net --test announce_e2e || fail "announce e2e"

    note "agent e2e (relay + authz regression)"
    cargo test -p rds-agent --test e2e || fail "agent e2e"

    note "G3 cold resolve→connect→first-byte"
    cargo run -q -p rds-bench -- run --scenario resolve-connect \
        --json "$REPORTS/bench-${TS}-resolve.json" --md "$REPORTS/bench-${TS}-resolve.md" \
        || fail "resolve-connect bench"
    python3 - "$REPORTS/bench-${TS}-resolve.json" <<'PY' || fail "G3 budget exceeded"
import json, sys
r = json.load(open(sys.argv[1]))["reports"][0]
ok, total = r["attempts"]
p50 = r["rtt"]["p50_ns"] / 1e6
p95 = r["rtt"]["p95_ns"] / 1e6
print(f"resolve-connect: {ok}/{total} ok, p50={p50:.1f}ms p95={p95:.1f}ms")
sys.exit(0 if (ok == total and p50 <= 300.0) else 1)
PY

    write_checkpoint "c3" "pending review" \
        "- fmt/clippy/test: PASS
- directory suite (roundtrip, expiry, staleness/replay, forgery, rate limit, signed delete, registry authz, hostile input, outage): PASS
- announce e2e (publish, TTL refresh, addr-change republish): PASS
- resolve e2e (ticket/bare-key/name fallbacks, resolve→connect): PASS
- record + http parser fuzz (proptest): PASS
- G3 cold resolve→connect→first-byte ≤300ms: see bench-${TS}-resolve.{json,md}"
    ;;
*)
    cat <<EOF
unknown or unregistered gate: '${GATE}'
registered gates: c0 c1 c2 c3
a checkpoint with no registered checks is refused by design.
EOF
    exit 2
    ;;
esac
