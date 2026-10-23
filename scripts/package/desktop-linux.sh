#!/usr/bin/env bash
#
# scripts/package/desktop-linux.sh — desktop Linux packaging example (skeleton).
#
# PRODUCT SHAPE (2026-10-21 media-stack packaging ruling): the media backend
# is a PACKAGING DECISION, uniform across the three platforms —
#
#   Windows : pure-Rust media always (no gstreamer cross-build path exists;
#             bao_browser has no media-gstreamer leg for cfg(windows)).
#   Linux   : EITHER pure Rust (default — deployable on a pristine system,
#             media capability honestly degraded: canPlayType("") /
#             isTypeSupported(false)) OR `--features media-gstreamer` for the
#             desktop product (this script's mode), which hard-links the
#             gstreamer 1.18+ system libraries and therefore MUST ship them.
#   macOS   : same two-way choice as Linux (feature is not target-gated).
#
# What this script does (mode = gstreamer, the desktop product shape):
#   1. cargo build --release --features media-gstreamer -p bao_bin
#   2. assemble dist/: bao binary + dist/lib/ holding every non-base system
#      .so the binary's DT_NEEDED resolves to (gstreamer/glib family and
#      their recursive deps), leaving libc/libm/libstdc++/libgcc/libz-style
#      base libs to the target system.
#   3. patchelf --set-rpath '$ORIGIN/../lib' on the binary (tool-missing →
#      loud error, not silent skip).
#
# Prerequisites (build machine): the gstreamer -dev packages (see
# `bao doctor`), plus patchelf and a recursive dependency walker
# (lddtree from pax-utils, or this script's BFS fallback).
#
# Usage:
#   scripts/package/desktop-linux.sh [--pure-rust] [--profile release] [--out DIR]
#
#   --pure-rust : package the DEFAULT build (no media-gstreamer). dist/lib/
#                 stays empty; no patchelf needed; runs on a pristine Linux.

set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
PROFILE="release"
MODE="gstreamer"
OUT=""
FEATURES="media-gstreamer"

while [ $# -gt 0 ]; do
    case "$1" in
        --pure-rust) MODE="pure-rust"; FEATURES="" ;;
        --profile) PROFILE="$2"; shift 2 ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# Base system libraries the target machine always provides (glibc runtime +
# compiler runtime + zlib). Everything else in DT_NEEDED gets bundled.
is_base_lib() {
    case "$1" in
        libc.so*|libm.so*|libdl.so*|libpthread.so*|librt.so*|ld-linux*|libresolv.so*|libnsl.so*) return 0 ;;
        libstdc++.so*|libgcc_s.so*|libz.so*|librtld*|libutil.so*) return 0 ;;
        *) return 1 ;;
    esac
}

# ── tool checks (fail-closed, no silent degrade) ────────────────────────────
need() { command -v "$1" >/dev/null 2>&1 || { echo "ERROR: '$1' not found (install it first: $2)"; exit 1; }; }
need readelf "apt: binutils"
if [ "$MODE" = "gstreamer" ]; then
    need patchelf "apt: patchelf  (rpath fixup is mandatory for the bundled-lib layout)"
fi

# ── 1. build ────────────────────────────────────────────────────────────────
cd "$REPO"
echo ">>> cargo build --profile $PROFILE -p bao_bin ${FEATURES:+--features $FEATURES}"
# shellcheck disable=SC2086
cargo build --profile "$PROFILE" -p bao_bin ${FEATURES:+--features $FEATURES}

BIN="target/$PROFILE/bao"
[ -f "$BIN" ] || { echo "ERROR: $BIN not produced"; exit 1; }

# ── 2. assemble dist ────────────────────────────────────────────────────────
OUT="${OUT:-dist/bao-linux-$MODE}"
rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/lib"
cp "$BIN" "$OUT/bin/bao"

if [ "$MODE" = "gstreamer" ]; then
    echo ">>> collecting bundled libraries into $OUT/lib"
    # BFS over DT_NEEDED: start from the binary, then follow each collected
    # .so's own NEEDED list (resolvable on THIS build machine, which has the
    # -dev runtime installed).
    declare -A seen=()
    queue=("$BIN")
    while [ ${#queue[@]} -gt 0 ]; do
        cur="${queue[0]}"; queue=("${queue[@]:1}")
        while IFS= read -r soname; do
            [ -n "$soname" ] || continue
            is_base_lib "$soname" && continue
            [ -n "${seen[$soname]:-}" ] && continue
            # resolve on the build machine via lddtree if present, else ldd.
            path=""
            if command -v lddtree >/dev/null 2>&1; then
                path=$(lddtree -l "$soname" "$cur" 2>/dev/null | head -1 || true)
            fi
            if [ -z "$path" ]; then
                path=$(ldd "$cur" 2>/dev/null | awk -v s="$soname" '$1 == s {print $3; exit}')
            fi
            if [ -z "$path" ] || [ ! -f "$path" ]; then
                echo "ERROR: cannot resolve $soname (needed by $(basename "$cur")) on this build machine" >&2
                exit 1
            fi
            seen[$soname]="$path"
            cp -L "$path" "$OUT/lib/"
            echo "  bundled: $soname <- $path"
            queue+=("$path")
        done < <(readelf -d "$cur" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p')
    done
    bundled=$(ls -1 "$OUT/lib" | wc -l)
    echo ">>> bundled $bundled libraries"

    # ── 3. rpath ────────────────────────────────────────────────────────────
    echo ">>> patchelf --set-rpath \$ORIGIN/../lib"
    patchelf --set-rpath '$ORIGIN/../lib' "$OUT/bin/bao"
else
    echo ">>> pure-rust mode: dist/lib left empty (target system needs no gstreamer)"
fi

# ── 4. self-check: the packaged binary must resolve everything ─────────────
echo ">>> self-check (print-runtime-deps.sh)"
"$REPO/scripts/print-runtime-deps.sh" "$OUT/bin/bao"

cat <<EOF

=== dist tree ready: $OUT ===
    $OUT/bin/bao        (rpath: \$ORIGIN/../lib in gstreamer mode)
    $OUT/lib/           ($(ls -1 "$OUT/lib" 2>/dev/null | wc -l) bundled .so)
Deploy: copy the whole tree; run on the target:
    $REPO/scripts/print-runtime-deps.sh $OUT/bin/bao
EOF
