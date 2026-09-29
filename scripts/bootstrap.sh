#!/usr/bin/env bash
# scripts/bootstrap.sh — one-shot environment setup for Bao.
#
# Gets a fresh clone to `target/release/bao --version` without the user having
# to understand the monorepo, the mozjs-from-source build, or the nightly
# toolchain requirement. Safe to re-run.
#
# Usage:
#   ./scripts/bootstrap.sh          # debug build
#   ./scripts/bootstrap.sh release  # optimized build
#
# For a richer diagnostic (with fix hints), run `bao doctor` after building.
set -euo pipefail

BUILD_MODE="${1:-debug}"

# ─── color helpers (only if stdout is a tty) ────────────────────────────────
if [ -t 1 ]; then
    GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RED=$'\033[31m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
else
    GREEN=""; YELLOW=""; RED=""; BOLD=""; RESET=""
fi
say()  { printf "%s==>%s %s\n" "$GREEN" "$RESET" "$*"; }
warn() { printf "%sWARN:%s %s\n" "$YELLOW" "$RESET" "$*"; }
err()  { printf "%sERR :%s %s\n" "$RED" "$RESET" "$*" >&2; }

# ─── 1. Rust toolchain (nightly required) ───────────────────────────────────
if ! command -v cargo >/dev/null 2>&1; then
    err "Rust is not installed."
    say "Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
    # shellcheck disable=SC1091
    source "${HOME}/.cargo/env"
fi

if ! rustc --version 2>/dev/null | grep -q nightly; then
    warn "Bao requires Rust nightly. Current: $(rustc --version 2>/dev/null || echo none)"
    say "Setting nightly via rust-toolchain.toml (handled by cargo)."
    # rust-toolchain.toml in the repo pins the channel; cargo will fetch it on build.
fi

# ─── 2. C/C++ compiler (mozjs compiles SpiderMonkey from source) ────────────
if ! command -v clang >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1 && ! command -v cc >/dev/null 2>&1; then
    err "No C/C++ compiler found. mozjs requires one."
    if command -v apt-get >/dev/null 2>&1; then
        say "Detected apt. Installing build-essential..."
        sudo apt-get update && sudo apt-get install -y build-essential pkg-config
    else
        warn "Install a C/C++ compiler (clang or gcc) and pkg-config manually, then re-run."
        exit 1
    fi
fi

command -v pkg-config >/dev/null 2>&1 || warn "pkg-config not found — some vendored crates may fail to locate system libs."

# ─── 2.5 System libraries & tools the vendored chains hard-require ─────────
# W37 (#18-E reproducibility closure): the clean-OS gap chain surfaced by
# scripts/bootstrap-repro.sh (10 layers, 2026-09-29). gcc cannot substitute
# for clang here — the vendored chains (boringssl_sys/uws_sys/lsquic_sys)
# hardcode clang/clang++ (doctor.rs precedent); uws_sys's C face includes
# <mimalloc.h>/<libdeflate.h>; the servo media stack resolves
# gstreamer-play/webrtc via pkg-config (the play/webrtc .pc files ship in the
# gstreamer1.0-plugins-bad RUNTIME package on Ubuntu 24.04 — measured, the
# -dev package alone does not carry them); and bao-servo-script-bindings'
# build.rs drives its codegen through `uv`.
SUDO=""
if [ "$(id -u)" != "0" ] && command -v sudo >/dev/null 2>&1; then SUDO="sudo"; fi
if command -v apt-get >/dev/null 2>&1; then
    say "Installing native build dependencies (clang, mimalloc, libdeflate, gstreamer faces)..."
    export DEBIAN_FRONTEND=noninteractive
    $SUDO apt-get update -qq
    $SUDO apt-get install -y -qq \
        clang pkg-config \
        libmimalloc-dev libdeflate-dev \
        libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
        gstreamer1.0-plugins-bad libgstreamer-plugins-bad1.0-dev \
        libasound2-dev libudev-dev libssl-dev \
        libarchive-dev libc-ares-dev zlib1g-dev || \
        warn "some native packages failed to install — the build may fail; see the messages above."
    unset DEBIAN_FRONTEND
fi

# ─── 2.6 uv (bao-servo-script-bindings codegen driver) ─────────────────────
if ! command -v uv >/dev/null 2>&1; then
    say "Installing uv (required by bao-servo-script-bindings codegen)..."
    curl -LsSf https://astral.sh/uv/install.sh | sh
    export PATH="$HOME/.local/bin:$PATH"
fi
command -v uv >/dev/null 2>&1 || warn "uv still unavailable — bao-servo-script-bindings codegen may fail."

# ─── 3. Build ────────────────────────────────────────────────────────────────
cd "$(dirname "$0")/.."

# Generate the codegen stubs bun_core/build.rs requires (Bun upstream uses
# `bun bd`; Bao uses deterministic SM-migration stubs). Idempotent.
say "Ensuring codegen stubs (build/debug/codegen)..."
./scripts/ensure-codegen.sh

say "Building bao_bin ($BUILD_MODE). First build compiles SpiderMonkey — this is slow."
# W37: the repo's .cargo/config.toml redirects [build] target-dir to a
# host-specific absolute path (single-compile-universe shape). A clean machine
# must be SELF-CONTAINED — bootstrap pins the target dir to the clone so the
# verification below finds the binary where it looks.
export CARGO_TARGET_DIR="$PWD/target"
if [ "$BUILD_MODE" = "release" ]; then
    # --jobs 1 is NOT required for build (only for `cargo test` per the EBUSY patch).
    cargo build --release -p bao_bin
    BIN="target/release/bao"
else
    cargo build -p bao_bin
    BIN="target/debug/bao"
fi

# ─── 4. Verify ───────────────────────────────────────────────────────────────
if [ -x "$BIN" ]; then
    say "Build OK. Running $BIN doctor:"
    echo
    "$BIN" doctor
    echo
    say "${BOLD}Bao is ready.${RESET} Try:"
    printf "    %s run -e 'console.log(\"hello from bao\")'\n" "$BIN"
    printf "    %s browser --url https://example.com\n" "$BIN"
else
    err "Build finished but $BIN was not found."
    exit 1
fi
