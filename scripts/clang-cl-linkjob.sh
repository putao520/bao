#!/usr/bin/env bash
# clang-cl shim: translate DLL link jobs into direct lld-link invocations.
#
# Why: mozangle `build_dlls` builds libEGL.dll/libGLESv2.dll by invoking the
# compiler once with many `.obj` inputs plus `/LD /Fe<out> /Fo<objdir>\ /link
# /DEF:<def>`. Two clang-cl driver mismatches against real `cl.exe` semantics:
#   1. cl.exe ignores `/Fo` on a link job; clang-cl errors:
#      "cannot specify '/Fo<...>' when compiling multiple source files"
#   2. with the .obj inputs, clang-cl routes the link to the host default
#      linker (/usr/bin/ld) instead of an MSVC-mode linker.
# (both observed 2026-09-27, mozangle 0.6.0 build.rs:155 assertion on the
# Linux->x86_64-pc-windows-msvc cross)
#
# Contract: byte-exact passthrough for every job without `/LD` (all cc-crate
# compile jobs untouched); `/LD` jobs are re-issued as `lld-link` with the
# DLL face (/DLL /MACHINE:x64 /OUT: /DEF: + obj/lib inputs). lld-link resolves
# defaultlibs from the directives embedded in the objects against LIB, which
# scripts/win-cross-env.sh exports. Installed by scripts/win-cross-env.sh as
# shim-bin/clang-cl (shadowing the system binary within this env), which also
# satisfies mozbuild's derived `<dirname>/clang` frontend probe. Twin:
# scripts/clang-musl.sh (same wrapper pattern, musl target).
# Resolve the REAL clang-cl: the first PATH entry that is not this script.
# This shim is installed as shim-bin/clang-cl (shadowing the system binary),
# so a plain `exec clang-cl` would recurse into itself.
_self="$(readlink -f "$0")"
_real_clang_cl=""
IFS=: read -ra _path_dirs <<< "$PATH"
for _d in "${_path_dirs[@]}"; do
    [ -x "$_d/clang-cl" ] || continue
    _cand="$(readlink -f "$_d/clang-cl")"
    if [ "$_cand" != "$_self" ]; then
        _real_clang_cl="$_d/clang-cl"
        break
    fi
done
unset _path_dirs _cand
if [ -z "$_real_clang_cl" ]; then
    echo "clang-cl-linkjob.sh: no real clang-cl found on PATH" >&2
    exit 127
fi

args=()
linkjob=0
for a in "$@"; do
    case "$a" in
    /LD | -LD) linkjob=1 ;;
    esac
done

if [ "$linkjob" != 1 ]; then
    exec "$_real_clang_cl" "$@"
fi

out=""
def=""
inputs=()
for a in "$@"; do
    case "$a" in
    /Fo* | -Fo*) ;;                      # objdir for intermediates — link job ignores
    /Fe*) out="${a#/Fe}" ;;
    -Fe*) out="${a#-Fe}" ;;
    /DEF:*) def="${a#/DEF:}" ;;
    -DEF:*) def="${a#-DEF:}" ;;
    /MP | -MP | /nologo | -nologo) ;;    # driver-only flags
    /LD | -LD | /link | -link) ;;        # absorbed into /DLL and the direct mode
    *) inputs+=("$a") ;;
    esac
done

cmd=(/nologo /DLL /MACHINE:x64)
[ -n "$out" ] && cmd+=("/OUT:$out")
[ -n "$def" ] && cmd+=("/DEF:$def")
exec lld-link "${cmd[@]}" "${inputs[@]}"
