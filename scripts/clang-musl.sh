#!/bin/sh
# clang-musl wrapper (issue #10 / REQ-DEPLOY-1 p3.5) — route clang at the musl
# target while keeping clang semantics that the uWS C sources require
# (GCC rejects their `always_inline` on static functions; see
# /tmp/uws_musl.log and the warning in src/uws_sys/build.rs).
#
# The sysroot comes from the musl.cc GCC toolchain installed at
# ~/musl-cross/x86_64-linux-musl-cross (p1, commit 0a4a11f7): its
# x86_64-linux-musl/ subtree carries the musl libc headers and static
# libs, and the GCC installation provides the C++ headers.
#
# Producer caveat (p3-fix, commit fcfec13b): clang records the same
# .comment producer for glibc and musl targets, so `.comment` cannot
# distinguish them. The target-detection TU (scripts/check-musl-tu.c +
# scripts/clang-musl.sh self-test below) is the authoritative criterion:
# musl does not define __GLIBC__.
exec /usr/bin/clang --target=x86_64-linux-musl \
  --sysroot="$HOME/musl-cross/x86_64-linux-musl-cross/x86_64-linux-musl" \
  "$@"
