#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════
# coverage-cli-bin-mode.sh — C9 bin-mode line-coverage measurement for the
# CLI handler faces (REQ-CLI-001 [test:C9-CLI-FACES], groundedIn
# PRD-REQ-SCRIPT-RUN/PRD-REQ-TOOLCHAIN).
#
# WHY: `src/bao_cli/tests/cli_faces.rs` exercises the run_* handler arms and
# the doctor check functions by spawning the REAL `bao` binary. llvm-cov's
# nextest mode only builds test targets, so the spawned binary is not
# instrumented and `bao_cli/src/{cli,doctor}.rs` report 0% forever — a
# measurement artifact, not an untested face (see
# .plans/coverage-governance-20261017.md). This pipeline builds an
# instrumented `bao` binary in a PRIVATE CARGO_TARGET_DIR, points
# BAO_TEST_BAO_BIN (the test-side binary ladder's first-priority entry) at
# it, and harvests the profraw files those spawns emit, producing an lcov
# that makes cli.rs / doctor.rs measurable.
#
# PIPELINE:
#   0. lsquic native libs   cargo build -p bun_lsquic_sys (plain flags) to
#                           materialize its out dir — see LSQUIC LINK NOTE
#   1. build instrumented   RUSTFLAGS="-C instrument-coverage
#                           --cfg=coverage --cfg=coverage_nightly
#                           -L native=<lsquic-out> -l static=lsquic
#                           -l static=lshpack"
#                           cargo build -p bao_bin --profile test-ci
#                           (private CARGO_TARGET_DIR — never the shared
#                           workspace target: instrumented RUSTFLAGS would
#                           churn every concurrent agent's debug profile,
#                           and feature-unification differs between the bin
#                           tree and llvm-cov's test tree so their caches
#                           cannot be shared anyway)
#   2. verify               readelf must see __llvm_prf_cnts in the bin
#                           (fail loudly if RUSTFLAGS did not take)
#   3. run the CLI faces    cargo nextest run -p bao_cli with
#                           BAO_TEST_BAO_BIN=<bin> and
#                           LLVM_PROFILE_FILE=<profraw-dir>/...
#   4. merge + export       llvm-profdata merge -sparse
#                           llvm-cov export --format=lcov --object <bin>
#   5. HARD GATE            cli.rs and doctor.rs must each show LH > 0 in
#                           the lcov; zero covered lines on either file
#                           exits 1 (that is a broken pipeline, not a
#                           legitimate 0%).
#
# IDEMPOTENT: the private target dir is kept between runs (step 1 is an
# incremental no-op after the first cold build; the first build compiles the
# full engine stack instrumented — expect O(hours) at -j4, minutes after).
# The profraw dir is wiped every run so stale profiles from a previous bin
# build can never merge against the current one.
#
# ENV KNOBS (all optional):
#   BAO_CLI_COV_TARGET_DIR   private build dir      (default /var/cargo-builds/bao-cli-cov-target;
#                                                   deliberately NOT /tmp — this machine sweeps
#                                                   /tmp of large trees mid-build and a 47-minute
#                                                   cold build was shredded to 593M that way)
#   BAO_CLI_COV_OUT_DIR      lcov/profdata output   (default <target>/coverage-out)
#   BAO_CLI_COV_JOBS         cargo -j               (default 4 — Rust CI discipline)
#   BAO_CLI_COV_SKIP_BUILD=1 reuse an existing instrumented bin as-is
#
# LSQUIC LINK NOTE: on a cold private target dir the build-script link
# directives for liblsquic.a/liblshpack.a have been observed missing from the
# final link line (the shared workspace target links fine; a fresh dir loses
# `cargo:rustc-link-lib=static=lsquic` between the build-script output and
# the bao_bin link line — root cause not yet pinned, recorded here so the
# workaround stays honest). The script therefore pre-builds `bun_lsquic_sys`
# to materialize its out dir and appends `-L native=<out> -l static=lsquic
# -l static=lshpack` to RUSTFLAGS explicitly. Same flags for every unit, so
# fingerprints stay consistent across the two stages.
# ═══════════════════════════════════════════════════════════════════════
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

JOBS="${BAO_CLI_COV_JOBS:-4}"
TARGET_DIR="${BAO_CLI_COV_TARGET_DIR:-/var/cargo-builds/bao-cli-cov-target}"
PROFILE="test-ci"
OUT_DIR="${BAO_CLI_COV_OUT_DIR:-$TARGET_DIR/coverage-out}"
PROFRAW_DIR="$OUT_DIR/profraw"
LCOV="$OUT_DIR/cli-bin-mode.lcov"

mkdir -p "$TARGET_DIR" "$OUT_DIR"

log() { printf '[cli-bin-mode] %s\n' "$*"; }
die() { printf '[cli-bin-mode] ERROR: %s\n' "$*" >&2; exit 1; }

# ── toolchain probes ────────────────────────────────────────────────────
# llvm-tools lives inside the pinned nightly toolchain (rust-toolchain.toml);
# fall back to PATH for hosts that put llvm-profdata/llvm-cov there.
NIGHTLY_DIR="$(cd "$REPO_ROOT" && rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin"
if [ -x "$NIGHTLY_DIR/llvm-profdata" ] && [ -x "$NIGHTLY_DIR/llvm-cov" ]; then
    LLVM_BIN="$NIGHTLY_DIR"
elif command -v llvm-profdata >/dev/null && command -v llvm-cov >/dev/null; then
    LLVM_BIN="$(dirname "$(command -v llvm-profdata)")"
else
    die "llvm-tools not found — run: rustup component add llvm-tools"
fi
PROFDATA_BIN="$LLVM_BIN/llvm-profdata"
LLVMCOV="$LLVM_BIN/llvm-cov"
log "llvm tools: $LLVM_BIN"

command -v cargo-nextest >/dev/null 2>&1 || cargo nextest --version >/dev/null 2>&1 \
    || die "cargo-nextest not found — run: cargo install cargo-nextest"

# ── 0. lsquic native libs stage (see LSQUIC LINK NOTE above) ────────────
lsquic_out() {
    ls -td "$TARGET_DIR/$PROFILE"/build/bun_lsquic_sys-*/out 2>/dev/null | head -1
}
LSQ_OUT="$(lsquic_out)"
if [ -z "$LSQ_OUT" ] || [ ! -f "$LSQ_OUT/liblsquic.a" ]; then
    log "materializing lsquic native libs (stage A, plain flags)"
    env CARGO_TARGET_DIR="$TARGET_DIR" \
        cargo build -p bun_lsquic_sys --profile "$PROFILE" --jobs "$JOBS" \
        > "$OUT_DIR/lsquic-stage.log" 2>&1 \
        || die "bun_lsquic_sys stage build failed — see $OUT_DIR/lsquic-stage.log"
    LSQ_OUT="$(lsquic_out)"
fi
[ -n "$LSQ_OUT" ] && [ -f "$LSQ_OUT/liblsquic.a" ] \
    || die "liblsquic.a not found under $TARGET_DIR after stage build"

# Copy the archives to a STABLE path: cargo re-runs the lsquic build script
# on rebuilds and mints a fresh out-dir hash each time, so referencing the
# out dir directly would change -L/-l strings between runs and churn the
# build fingerprints. The stable path keeps RUSTFLAGS byte-identical
# run-over-run; content is refreshed from whichever out dir is newest.
LSQ_STABLE="$TARGET_DIR/lsquic-native"
mkdir -p "$LSQ_STABLE"
cp -f "$LSQ_OUT/liblsquic.a" "$LSQ_OUT/liblshpack.a" "$LSQ_STABLE/"
LSQ_OUT="$LSQ_STABLE"
log "lsquic out: $LSQ_OUT"

# ── 1. instrumented bin build ───────────────────────────────────────────
INSTRUMENT_RUSTFLAGS='-C instrument-coverage --cfg=coverage --cfg=coverage_nightly -L native='"$LSQ_OUT"' -l static=lsquic -l static=lshpack'
BIN="$TARGET_DIR/$PROFILE/bao"

if [ "${BAO_CLI_COV_SKIP_BUILD:-0}" = "1" ] && [ -x "$BIN" ]; then
    log "skip-build set, reusing existing bin: $BIN"
else
    log "building instrumented bao_bin (profile=$PROFILE, jobs=$JOBS, target=$TARGET_DIR)"
    log "  first run on a fresh dir is a FULL cold instrumented engine build (O(hours) at -j4)"
    env CARGO_TARGET_DIR="$TARGET_DIR" \
        RUSTFLAGS="$INSTRUMENT_RUSTFLAGS" \
        cargo build -p bao_bin --profile "$PROFILE" --jobs "$JOBS" \
        > "$OUT_DIR/build.log" 2>&1 \
        || die "instrumented build failed — see $OUT_DIR/build.log"
    log "build ok ($(tail -1 "$OUT_DIR/build.log" | cut -c1-100))"
fi
[ -x "$BIN" ] || die "bin missing after build: $BIN"

# ── 2. instrumentation proof (anti-false-green gate) ────────────────────
if ! readelf -S "$BIN" 2>/dev/null | grep -q "__llvm_prf_cnts"; then
    die "bin has no __llvm_prf_cnts section — RUSTFLAGS did not instrument it; refusing to produce a 0% lcov masquerading as measurement"
fi
log "instrumentation verified (__llvm_prf_cnts present)"

# ── 3. run CLI face tests against the instrumented bin ─────────────────
# Fresh profraw dir every run: profiles are bound to the exact binary
# version; leftovers from an older build would poison the merge.
rm -rf "$PROFRAW_DIR"
mkdir -p "$PROFRAW_DIR"

log "running bao_cli tests (spawned bao writes profraw via LLVM_PROFILE_FILE)"
TEST_RC=0
env CARGO_TARGET_DIR="$TARGET_DIR" \
    RUSTFLAGS="$INSTRUMENT_RUSTFLAGS" \
    BAO_TEST_BAO_BIN="$BIN" \
    LLVM_PROFILE_FILE="$PROFRAW_DIR/bao-%p-%m.profraw" \
    cargo nextest run -p bao_cli --profile default \
    > "$OUT_DIR/nextest.log" 2>&1 || TEST_RC=$?

NEXT_SUMMARY="$(grep -E "Summary.*passed|tests? (passed|failed)" "$OUT_DIR/nextest.log" | tail -1 || true)"
log "nextest rc=$TEST_RC ${NEXT_SUMMARY:+($NEXT_SUMMARY)}"
if [ "$TEST_RC" -ne 0 ]; then
    log "test failures — see $OUT_DIR/nextest.log (continuing to lcov for diagnosis)"
fi

PROFRAW_COUNT="$(find "$PROFRAW_DIR" -name '*.profraw' | wc -l)"
[ "$PROFRAW_COUNT" -gt 0 ] || die "zero profraw files written — the spawns did not emit profiles (check LLVM_PROFILE_FILE propagation)"

# ── 4. merge + export ───────────────────────────────────────────────────
PROFDATA="$OUT_DIR/cli-bin-mode.profdata"
log "merging $PROFRAW_COUNT profraw files"
"$PROFDATA_BIN" merge -sparse "$PROFRAW_DIR"/*.profraw -o "$PROFDATA" \
    || die "llvm-profdata merge failed"

log "exporting lcov (this walks the full coverage map; can take minutes)"
"$LLVMCOV" export --format=lcov --object "$BIN" --instr-profile "$PROFDATA" \
    > "$LCOV" 2> "$OUT_DIR/export.log" \
    || die "llvm-cov export failed — see $OUT_DIR/export.log"
log "lcov written: $LCOV ($(wc -l < "$LCOV") lines)"

# ── 5. hard gate: cli.rs + doctor.rs must be measurably covered ─────────
# Pull LF/LH for a file record out of the lcov (SF paths are absolute).
lcov_stats() {
    # field-split by whitespace makes "LF:498" one token, so match the
    # "KEY:" prefix on the whole line and slice the value past the colon
    awk -v want="$1" '
        /^SF:/ { current = substr($0, 4) }
        /^LF:/ && current ~ want { lf = substr($0, 4) }
        /^LH:/ && current ~ want { lh = substr($0, 4) }
        /^end_of_record/ && current ~ want { print lh+0, lf+0; exit }
    ' "$LCOV"
}

GATE_FAIL=0
printf '\n[cli-bin-mode] bin-mode line coverage (spawn-instrumented binary)\n'
for f in "bao_cli/src/cli.rs" "bao_cli/src/doctor.rs"; do
    stats="$(lcov_stats "$f")"
    lh="$(cut -d' ' -f1 <<<"$stats")"
    lf="$(cut -d' ' -f2 <<<"$stats")"
    if [ -z "$stats" ] || [ "$lf" -eq 0 ]; then
        printf '  %-26s NOT FOUND IN LCOV (measurement path broken)\n' "$f"
        GATE_FAIL=1
        continue
    fi
    pct="$(awk "BEGIN{printf \"%.2f\", 100*$lh/$lf}")"
    printf '  %-26s LF=%-5s LH=%-5s line=%s%%\n' "$f" "$lf" "$lh" "$pct"
    if [ "$lh" -eq 0 ]; then
        printf '  %-26s GATE: zero covered lines — pipeline broken, exiting 1\n' "$f"
        GATE_FAIL=1
    fi
done

if [ "$GATE_FAIL" -ne 0 ]; then
    die "hard gate failed (see lines above)"
fi

if [ "$TEST_RC" -ne 0 ]; then
    die "coverage produced but tests failed ($TEST_RC) — see $OUT_DIR/nextest.log"
fi

log "DONE — $LCOV"
