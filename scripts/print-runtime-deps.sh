#!/usr/bin/env bash
#
# scripts/print-runtime-deps.sh — deployment-side runtime dependency report.
#
# Prints every DT_NEEDED entry of an ELF binary (or all ELF binaries in a
# directory) and checks each against the target machine: resolved from
# RPATH/RUNPATH/ldconfig cache/system default paths, or MISSING.
#
# Purpose (2026-10-21 media-stack packaging ruling): the default bao build is
# pure-Rust media (zero gstreamer DT_NEEDED — deployable on a pristine Linux);
# a build with `--features media-gstreamer` hard-links the gstreamer system
# libraries (gstreamer-rs system-deps has no dlopen mode), so the packager must
# either ship the .so files alongside (scripts/package/desktop-linux.sh, with
# an $ORIGIN rpath) or declare the system dependencies. Run this script on the
# TARGET machine against the packaged binary to get the missing-library list
# before shipping.
#
# Usage:
#   scripts/print-runtime-deps.sh <binary-or-dir> [more paths...]
#   (no args) — defaults to the repo's own bao binary if built
#
# Exit codes: 0 = all resolved, 1 = at least one missing library, 2 = usage error.

set -euo pipefail

usage() {
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

# gstreamer 1.18+ runtime package names (Debian/Ubuntu apt form) — printed in
# the hint when a gstreamer library is reported missing. Kept in sync with
# doctor.rs `system_lib_check` rows (scripts/native-deps-inventory.sh gate).
gst_apt_hint() {
    cat <<'EOF'
  hint (apt): sudo apt-get install -y \
      libglib2.0-0 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
      libgstreamer-plugins-bad1.0-0 libgstreamer-gl1.0-0 \
      gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad
  (or repackage without the media-gstreamer feature for a pure-Rust build)
EOF
}

resolve_lib() {
    # $1 = soname, $2 = binary dir (for $ORIGIN-style relative rpaths we
    # approximate with the binary's own directory). Prints the resolved path
    # or nothing.
    local soname="$1" bindir="$2" rpaths

    # 1) RUNPATH/RPATH of the binary itself (ldd already applies them, but we
    #    also want the explicit path for the report).
    rpaths=$("$READELF" -d "$BIN" 2>/dev/null \
        | sed -n 's/.*\(RUNPATH\|RPATH\).*\[\(.*\)\].*/\2/p' | tr ':;' '  ')

    local dir
    for dir in $rpaths; do
        # Expand $ORIGIN to the binary's directory.
        dir=${dir//'$ORIGIN'/"$bindir"}
        if [ -e "$dir/$soname" ]; then
            printf '%s\n' "$dir/$soname"
            return 0
        fi
    done

    # 2) ldconfig cache.
    local hit
    hit=$(ldconfig -p 2>/dev/null | awk -v s="$soname" '$NF == s {print $NF; exit}')
    if [ -n "$hit" ]; then
        # ldconfig prints "soname (libc6,x86-64) => /path"; re-extract path.
        hit=$(ldconfig -p 2>/dev/null | grep -F "$soname" | head -1 | sed 's/.*=> //')
        if [ -n "$hit" ] && [ -e "$hit" ]; then
            printf '%s\n' "$hit"
            return 0
        fi
    fi

    # 3) system default dirs.
    for dir in /lib/x86_64-linux-gnu /usr/lib/x86_64-linux-gnu /lib64 /usr/lib64 /lib /usr/lib; do
        if [ -e "$dir/$soname" ]; then
            printf '%s\n' "$dir/$soname"
            return 0
        fi
    done
    return 1
}

check_binary() {
    local bin="$1"
    [ -f "$bin" ] || { echo "SKIP (not a file): $bin"; return 0; }
    "$READELF" -h "$bin" >/dev/null 2>&1 || { echo "SKIP (not ELF): $bin"; return 0; }

    local bindir
    bindir=$(cd "$(dirname "$bin")" && pwd)

    echo "=== $bin ==="
    local needed
    needed=$("$READELF" -d "$bin" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p' | sort -u)
    if [ -z "$needed" ]; then
        echo "  (no NEEDED entries)"
        return 0
    fi

    local missing=0 total=0 soname
    while IFS= read -r soname; do
        [ -n "$soname" ] || continue
        total=$((total + 1))
        local where
        if where=$(BIN="$bin" resolve_lib "$soname" "$bindir"); then
            printf '  OK      %-42s %s\n' "$soname" "$where"
        else
            printf '  MISSING %-42s\n' "$soname"
            missing=$((missing + 1))
        fi
    done <<<"$needed"

    echo "  summary: $total NEEDED, $missing missing"
    if [ "$missing" -gt 0 ]; then
        case "$needed" in
            *gstreamer*|*glib*) gst_apt_hint ;;
        esac
        return 1
    fi
    return 0
}

# ── main ────────────────────────────────────────────────────────────────────
command -v readelf >/dev/null 2>&1 || { echo "readelf not found (apt: binutils)"; exit 2; }
READELF=readelf

[ $# -ge 1 ] || usage

rc=0
for target in "$@"; do
    if [ -d "$target" ]; then
        while IFS= read -r -d '' f; do
            check_binary "$f" || rc=1
        done < <(find "$target" -maxdepth 2 -type f -print0 2>/dev/null)
    else
        check_binary "$target" || rc=1
    fi
done
exit "$rc"
