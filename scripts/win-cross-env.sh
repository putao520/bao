#!/usr/bin/env bash
# Bao Windows cross-compilation environment — Linux host → x86_64-pc-windows-msvc.
#
# Usage (MUST be sourced, not executed):
#   source scripts/win-cross-env.sh
#   cargo check -p bun_boringssl_sys --target x86_64-pc-windows-msvc
#
# What it exports (idempotent; safe to re-source):
#   PATH                              — prepends the llvm shim dir (lld-link /
#                                       llvm-rc / llvm-lib under their bare
#                                       names; moz `check_prog` only accepts
#                                       the bare `lld-link`, and the Windows
#                                       resource step wants `llvm-rc`)
#   WINSYSROOT                        — the assembled moz-style winsysroot;
#       consumed by SpiderMonkey's build/moz.configure/windows-toolchain
#       (bypasses the winreg registry probe entirely — no patch needed)
#   CC_/CXX_/AR_/LD_x86_64_pc_windows_msvc — deterministic cross toolchain:
#       clang-cl / clang-cl / llvm-lib / lld-link (cc-rs honors the
#       <var>_<triple> forms before probing PATH)
#   INCLUDE, LIB                      — MSVC-style semicolon lists (VC headers
#       + ATL + SDK shared/um/ucrt/winrt/cppwinrt; libs for the L3 link face)
#   CFLAGS_/CXXFLAGS_x86_64_pc_windows_msvc — the same include set in
#       `-imsvc <dir>` flag form (space-free paths via the vc14/sdk10 aliases)
#
# RUSTFLAGS: intentionally NOT exported. `cargo check` never links, so nothing
# is needed for the L1/L2 faces. The L3 full-link face (bao.exe via lld-link)
# passes its own `-C linker=`/`-C linker-flavor=lld-link` at the wave that owns
# it; do not grow a global RUSTFLAGS here.
#
# CPPFLAGS: intentionally NOT exported. The mozjs face gets its encoding_rs /
# glue / libz-rs include paths from build.rs DEP_* variables automatically;
# a global CPPFLAGS would leak into unrelated C builds.
#
# Sysroot layout ($BAO_WIN_CROSS_ROOT/wsroot, default /opt/bao-win-cross):
#   wsroot/VC/Tools/MSVC/14.44.35207/{include,lib/x64,atlmfc,bin/Hostx64/x64}
#   wsroot/Windows Kits/10/{Include,Lib}/10.0.26100/{shared,um,ucrt,winrt,cppwinrt}
#   wsroot/{vc14 -> VC/Tools/MSVC/14.44.35207, sdk10 -> "Windows Kits/10"}
#     (space-free aliases so CFLAGS_<triple> -imsvc needs no quoting games)
#   VC bin carries shims: cl.exe -> clang-cl (vc_path discovery only),
#   ml64.exe -> llvm-ml64 (required even under --disable-jit)
#
# Rebuild recipe (if the sysroot volume is lost — ~650 MB, needs network):
#   cargo install xwin --locked
#   xwin --accept-license --arch x86_64 --include-atl splat   # → .xwin-cache/splat
#   S=.xwin-cache/splat; W=<root>/wsroot; VC=$W/VC/Tools/MSVC/14.44.35207
#   mkdir -p $VC/bin/Hostx64/x64 $VC/lib/x64 $VC/atlmfc/lib \
#            "$W/Windows Kits/10/Include/10.0.26100" "$W/Windows Kits/10/Lib/10.0.26100"
#   cp -a $S/crt/include $VC/include
#   cp -a $S/crt/lib/x86_64/. $VC/lib/x64/
#   ln -sfn ../include $VC/atlmfc/include; ln -sfn ../../lib/x64 $VC/atlmfc/lib/x64
#   ln -sf "$(command -v clang-cl)" $VC/bin/Hostx64/x64/cl.exe
#   ln -sf "$(command -v llvm-ml64-23 || command -v llvm-ml)" $VC/bin/Hostx64/x64/ml64.exe
#   for d in shared um ucrt winrt cppwinrt; do
#       cp -a $S/sdk/include/$d "$W/Windows Kits/10/Include/10.0.26100/"; done
#   for d in um ucrt; do
#       cp -a $S/sdk/lib/$d "$W/Windows Kits/10/Lib/10.0.26100/"
#       mv "$W/Windows Kits/10/Lib/10.0.26100/$d/x86_64" \
#          "$W/Windows Kits/10/Lib/10.0.26100/$d/x64"; done
#   ln -sfn "VC/Tools/MSVC/14.44.35207" $W/vc14
#   ln -sfn "Windows Kits/10" $W/sdk10
#
# Probe dossier: win-cross 2026-09-21 wave (SM153 configure + C++ compile +
# js_static.lib and libuv uv.lib all verified green under this environment).
# Platform tracking: #18 / issues #33 #34 #35 / docs/platform-support.md.

# Sourced, not executed.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    echo "error: scripts/win-cross-env.sh must be sourced: source scripts/win-cross-env.sh" >&2
    exit 1
fi

BAO_WIN_CROSS_ROOT="${BAO_WIN_CROSS_ROOT:-/opt/bao-win-cross}"
WSROOT="$BAO_WIN_CROSS_ROOT/wsroot"

if [ ! -d "$WSROOT/vc14/include" ] || [ ! -d "$WSROOT/sdk10/Include" ]; then
    echo "error: winsysroot not found at $WSROOT (see the rebuild recipe in this file's header)" >&2
    return 1 2>/dev/null || exit 1
fi

# ── llvm shim dir ────────────────────────────────────────────────────────────
# Distro llvm ships version-suffixed binaries; the Windows build faces probe
# bare names (moz `check_prog("LINKER", ("lld-link",))`, llvm-rc for `rc`).
# Prefer an already-bare tool, else the newest versioned one present.
_shim="$BAO_WIN_CROSS_ROOT/shim-bin"
mkdir -p "$_shim"
for _tool in lld-link llvm-rc llvm-lib llvm-ar llvm-ml64; do
    if ! command -v "$_tool" >/dev/null 2>&1 && [ ! -e "$_shim/$_tool" ]; then
        for _v in "-23" "-20" "-19" "-18" "-17" ""; do
            if command -v "$_tool$_v" >/dev/null 2>&1; then
                ln -sf "$(command -v "$_tool$_v")" "$_shim/$_tool"
                break
            fi
        done
    fi
done
unset _tool _v _shim

# cl.exe / ml64.exe shims live inside the VC tree (moz's vc_path discovery and
# assembler probe search there, not on PATH).
_vcbin="$WSROOT/VC/Tools/MSVC/14.44.35207/bin/Hostx64/x64"
if [ ! -e "$_vcbin/cl.exe" ]; then
    ln -sf "$(command -v clang-cl)" "$_vcbin/cl.exe"
fi
if [ ! -e "$_vcbin/ml64.exe" ]; then
    for _v in llvm-ml64-23 llvm-ml64 llvm-ml-23 llvm-ml; do
        if command -v "$_v" >/dev/null 2>&1; then
            ln -sf "$(command -v "$_v")" "$_vcbin/ml64.exe"
            break
        fi
    done
fi
unset _vcbin _v

# ── include/lib faces (space-free via vc14/sdk10 aliases) ───────────────────
_vc="$WSROOT/vc14"
_sdk="$WSROOT/sdk10/Include/10.0.26100"
_sdklib="$WSROOT/sdk10/Lib/10.0.26100"

WIN_CROSS_INCLUDE_DIRS=(
    "$_vc/include"
    "$_vc/atlmfc/include"
    "$_sdk/shared"
    "$_sdk/um"
    "$_sdk/ucrt"
    "$_sdk/winrt"
    "$_sdk/cppwinrt"
)
WIN_CROSS_LIB_DIRS=(
    "$_vc/lib/x64"
    "$_vc/atlmfc/lib/x64"
    "$_sdklib/um/x64"
    "$_sdklib/ucrt/x64"
    # casefix: Linux lld-link opens import libs CASE-SENSITIVELY; the xwin
    # splat ships three casing variants per lib but not every spelling crates
    # request. This directory holds symlinks for the requested spellings that
    # the splat lacks (first entry: `Dbghelp.lib` — a crate link directive
    # spells it D-capital-only while the splat has dbghelp.lib/DbgHelp.Lib/
    # DBGHELP.lib). Extend here whenever the link reports a case-only miss.
    "$WSROOT/casefix"
)

INCLUDE=""
for _d in "${WIN_CROSS_INCLUDE_DIRS[@]}"; do
    INCLUDE="${INCLUDE:+$INCLUDE;}$_d"
done
LIB=""
for _d in "${WIN_CROSS_LIB_DIRS[@]}"; do
    LIB="${LIB:+$LIB;}$_d"
done

_win_cross_cflags=""
for _d in "${WIN_CROSS_INCLUDE_DIRS[@]}"; do
    _win_cross_cflags="$_win_cross_cflags -imsvc $_d"
done

export PATH="$BAO_WIN_CROSS_ROOT/shim-bin:$PATH"
export WINSYSROOT="$WSROOT"
export INCLUDE LIB
export CFLAGS_x86_64_pc_windows_msvc="$_win_cross_cflags -Wno-incompatible-pointer-types"
export CXXFLAGS_x86_64_pc_windows_msvc="$_win_cross_cflags -Wno-incompatible-pointer-types"
export CC_x86_64_pc_windows_msvc="$(command -v clang-cl)"
export CXX_x86_64_pc_windows_msvc="$(command -v clang-cl)"
export AR_x86_64_pc_windows_msvc="$BAO_WIN_CROSS_ROOT/shim-bin/llvm-lib"
export LD_x86_64_pc_windows_msvc="$BAO_WIN_CROSS_ROOT/shim-bin/lld-link"

unset _d _win_cross_cflags _vc _sdk _sdklib WSROOT

echo "win-cross env ready: root=$BAO_WIN_CROSS_ROOT target=x86_64-pc-windows-msvc"
