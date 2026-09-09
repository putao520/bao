#!/bin/sh
# C++ flavor of scripts/clang-musl.sh — see that file for rationale and the
# producer-vs-TU criterion note. The -I pins the GCC installation's target-side
# C++ headers (musl.cc layout: <toolchain>/x86_64-linux-musl/include/c++/<ver>/
# holds the actual libstdc++ headers — verified via find + musl-g++ TU probe;
# the top-level include/c++/ mirror does not carry them).
TOOLCHAIN="$HOME/musl-cross/x86_64-linux-musl-cross"
exec /usr/bin/clang++ --target=x86_64-linux-musl \
  --sysroot="$TOOLCHAIN/x86_64-linux-musl" \
  -I"$TOOLCHAIN/x86_64-linux-musl/include/c++/11.2.1" \
  -I"$TOOLCHAIN/x86_64-linux-musl/include/c++/11.2.1/x86_64-linux-musl" \
  "$@"
