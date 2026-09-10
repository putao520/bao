#!/usr/bin/env bash
# bench/run.sh — reproducible baseline orchestrator (issue #19 Phase A).
#
# Runs each bench as its own process (deadline isolation from the test fleet —
# bench/METHODOLOGY.md §6), binding every result to commit / host / date via
# BAO_BENCH_* env capture. Results land in bench/results/<date>-<commit>/.
#
# Usage:
#   bench/run.sh                      # all 5 seed benches
#   bench/run.sh fetch-small-payload  # one bench (repeatable unit)
#   bench/run.sh soak                 # long soak (NOT in the default set —
#                                      # minutes-to-hours scale; first bounded
#                                      # round 60min, 72h via SOAK_DURATION_MINS)
#
# Environment:
#   CARGO_TARGET_DIR      defaults to the shared build cache (see CLAUDE.md)
#   RUNS                  per-bench process reruns (default 3; page-churn/soak use 1)
#   SOAK_DURATION_MINS    soak duration (default 60); per-cycle series lands
#                         next to the result as <bench>.run-<k>.segments.jsonl

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/var/cargo-builds/3c/6184ceb77072ba}"
PROFILE="${BAO_BENCH_PROFILE:-test-ci}"

GIT_COMMIT="$(git rev-parse HEAD)"
GIT_SHORT="$(git rev-parse --short HEAD)"
GIT_DIRTY="$(git status --porcelain -- bench/ src/ Cargo.toml Cargo.lock | wc -l | awk '{print ($1>0)?1:0}')"
RUSTC_V="$(rustc -V 2>/dev/null || echo unknown)"

export BAO_BENCH_GIT_COMMIT="$GIT_COMMIT"
export BAO_BENCH_GIT_DIRTY="$GIT_DIRTY"
export BAO_BENCH_RUSTC="$RUSTC_V"
export BAO_BENCH_PROFILE="$PROFILE"

RESULTS_DIR="bench/results/$(date -u +%Y-%m-%d)-${GIT_SHORT}"
mkdir -p "$RESULTS_DIR"

echo "[bench] commit=$GIT_SHORT dirty=$GIT_DIRTY profile=$PROFILE rustc=$RUSTC_V"
echo "[bench] results → $RESULTS_DIR"

# bench name → runs (process-level reruns; browser churn is seconds-per-cycle,
# R=1 recorded with reason per METHODOLOGY §3; soak is minutes-scale, R=1)
declare -A BENCH_RUNS=(
  [runtime-create-drop]="${RUNS:-3}"
  [realm-create-drop]="${RUNS:-3}"
  [fetch-small-payload]="${RUNS:-3}"
  [rss-sample]="${RUNS:-3}"
  [page-churn]="${RUNS_PAGE:-1}"
  [soak]="${RUNS_SOAK:-1}"
)

ORDERED_BENCHES=(
  runtime-create-drop
  realm-create-drop
  fetch-small-payload
  rss-sample
  page-churn
)

# Build once (link-only on a warm cache) before any measurement.
echo "[bench] building bench-harness ($PROFILE)..."
cargo build -p bench-harness --profile "$PROFILE" --jobs 4
BIN="$CARGO_TARGET_DIR/$PROFILE/bench-harness"
if [ ! -x "$BIN" ]; then
  echo "[bench] FATAL: harness binary not at $BIN" >&2
  exit 1
fi

run_one() {
  local bench="$1" run_idx="$2"
  local out="$RESULTS_DIR/$bench.run-$run_idx.json"
  echo "[bench] $bench run-$run_idx → $out"
  # --out writes the JSON document to a dedicated file: servo/engine log noise
  # (libEGL, wiring warnings) lands on stdout and must stay out of the result.
  local log="$RESULTS_DIR/$bench.run-$run_idx.log"
  case "$bench" in
    page-churn)
      # Browser stack needs a DISPLAY — xvfb provides an isolated virtual one.
      xvfb-run -a "$BIN" "$bench" --out "$out" >"$log" 2>&1
      ;;
    soak)
      # Browser stack (xvfb) + minutes-scale duration; the per-cycle series
      # streams to $out-derived .segments.jsonl inside the harness.
      xvfb-run -a "$BIN" "$bench" --duration-mins "${SOAK_DURATION_MINS:-60}" --out "$out" >"$log" 2>&1
      ;;
    *)
      "$BIN" "$bench" --out "$out" >"$log" 2>&1
      ;;
  esac
}

SELECTED=("${ORDERED_BENCHES[@]}")
if [ "$#" -gt 0 ]; then
  SELECTED=("$@")
fi

FAILED=0
for bench in "${SELECTED[@]}"; do
  if [ -z "${BENCH_RUNS[$bench]:-}" ]; then
    echo "[bench] FATAL: unknown bench '$bench'" >&2
    exit 2
  fi
  runs="${BENCH_RUNS[$bench]}"
  for ((k = 1; k <= runs; k++)); do
    if ! run_one "$bench" "$k"; then
      echo "[bench] FAILED: $bench run-$k (see $RESULTS_DIR/$bench.run-$k.json error field)" >&2
      FAILED=1
    fi
  done
done

# Summary table (failures included — honest reporting).
echo
echo "[bench] summary:"
for f in "$RESULTS_DIR"/*.run-*.json; do
  name="$(basename "$f" .json)"
  if command -v jq >/dev/null 2>&1; then
    err="$(jq -r '.error // empty' "$f")"
    if [ -n "$err" ]; then
      echo "  $name: FAILED — $err"
    else
      echo "  $name:"
      jq -r '.metrics[] | "    \(.name) \(.unit): p50=\(.p50) p95=\(.p95) n=\(.n)\(if .unstable then " [unstable cv=\(.cv)]" else "" end)"' "$f"
    fi
  else
    echo "  $name: (jq not installed — inspect $f directly)"
  fi
done

exit "$FAILED"
