#!/usr/bin/env bash
#
# scripts/nightly-bump-gate.sh — W23-④ nightly toolchain bump gate (#18-④).
#
# Semantics (rust-toolchain.toml contract): the channel is pinned
# deliberately — the gate NEVER writes the pin. It produces the bump dossier:
#   --probe    (default, load-safe) registry-face evidence: current pin, the
#              latest available nightly, component availability, and the
#              candidate diff line. No toolchain download.
#   --verify DATE  full ladder against nightly-DATE (heavier): toolchain
#              install into the shared rustup home → `cargo +DATE check`
#              focused families → verdict dossier. Run on a quiet machine —
#              it compiles real crates.
#
# Fail-loud: any ladder step red = exit 1 with the first offending crate/
# diagnostic (upstream tightening attribution hint). The bump commit itself
# is always HUMAN (deliberate per rust-toolchain.toml) — this script only
# prepares the dossier.
#
# W23-④ stop-clause note (shared-machine load): --verify is opt-in; --probe
# alone satisfies the "registry-face evidence" fallback.

set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

PIN=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml | head -1)
PIN_DATE=${PIN#nightly-}

mode="${1:---probe}"

case "$mode" in
    --probe)
        echo "[nightly-gate] current pin: $PIN"
        echo "[nightly-gate] components: $(sed -n 's/^components = //p' rust-toolchain.toml | head -1)"
        echo "[nightly-gate] rustc on pin: $(rustc --version 2>/dev/null || echo 'not installed on this host')"
        echo "[nightly-gate] cargo on pin: $(cargo --version 2>/dev/null || echo 'not installed on this host')"
        # Registry face: the newest nightly rustup can see (no download).
        LATEST=$(rustup toolchain list 2>/dev/null | grep -oE 'nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}' | sort -u | tail -1)
        echo "[nightly-gate] locally known toolchains (rustup):"
        rustup toolchain list 2>/dev/null | sed 's/^/    /' || echo "    (rustup unavailable)"
        if [ -n "$LATEST" ]; then
            echo "[nightly-gate] latest locally-visible nightly: $LATEST"
            if [ "$LATEST" = "$PIN" ]; then
                echo "[nightly-gate] probe verdict: pin is current — no bump candidate"
            else
                echo "[nightly-gate] probe verdict: bump candidate exists: $PIN → $LATEST"
                echo "[nightly-gate] next step (human): run --verify ${LATEST#nightly-} on a quiet machine,"
                echo "[nightly-gate] then hand-edit rust-toolchain.toml channel line (the gate never writes the pin)."
            fi
        else
            echo "[nightly-gate] probe verdict: no nightly visible locally (offline host?) — registry face inconclusive, noted"
        fi
        # Nightly release manifest (remote, cheap): registry availability of the channel tip.
        if command -v curl >/dev/null 2>&1; then
            TIP=$(curl -sS --max-time 20 https://static.rust-lang.org/dist/channel-rust-nightly.toml 2>/dev/null | grep -m1 '^date = ' | sed 's/date = "//; s/"//')
            if [ -n "$TIP" ]; then
                echo "[nightly-gate] upstream nightly tip (static.rust-lang.org): nightly-$TIP"
                echo "[nightly-gate] upstream-vs-pin: pin is $(( ($(date -d "$TIP" +%s 2>/dev/null || echo 0) - $(date -d "$PIN_DATE" +%s 2>/dev/null || echo 0)) / 86400 )) day(s) behind the tip"
            else
                echo "[nightly-gate] upstream tip fetch failed (offline/timeout) — noted, not red"
            fi
        fi
        exit 0
        ;;
    --verify)
        DATE="${2:-}"
        if [ -z "$DATE" ]; then echo "usage: $0 --verify YYYY-MM-DD"; exit 2; fi
        CH="nightly-$DATE"
        echo "[nightly-gate] verify ladder against $CH (heavy — quiet machine expected)"
        rustup toolchain install "$CH" --profile minimal -c rustfmt,clippy || { echo "[nightly-gate] RED: toolchain install failed"; exit 1; }
        # Ladder 1: workspace check on the candidate (independent target dir to
        # avoid polluting the pinned chain's cache).
        TGT=$(mktemp -d)/tgt
        export CARGO_TARGET_DIR="$TGT"
        echo "[nightly-gate] ladder 1/2: cargo +$CH check --workspace --jobs 4"
        if ! cargo +"$CH" check --workspace --jobs 4 2>&1 | tail -20; then
            echo "[nightly-gate] RED: candidate $CH fails workspace check (upstream tightening?) — bump rejected, dossier above"
            exit 1
        fi
        echo "[nightly-gate] ladder 2/2: focused test families (stealth/runtime/cdp-server)"
        for p in bao_stealth bun_runtime cdp-server; do
            echo "[nightly-gate]   → $p"
            cargo +"$CH" nt -p "$p" 2>&1 | tail -3 || { echo "[nightly-gate] RED: $p tests red on $CH"; exit 1; }
        done
        echo "[nightly-gate] VERDICT: $CH verified green on the ladder — bump dossier ready:"
        echo "[nightly-gate]   rust-toolchain.toml: channel = \"$PIN\" → \"$CH\""
        echo "[nightly-gate]   (human step: edit the pin + commit with this dossier)"
        exit 0
        ;;
    --help|*)
        sed -n '2,20p' "$0"
        echo "usage: $0 [--probe | --verify YYYY-MM-DD]"
        exit 0
        ;;
esac
