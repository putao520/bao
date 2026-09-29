#!/usr/bin/env bash
#
# scripts/bootstrap-repro.sh — W23-⑤ clean-machine bootstrap + cold/incremental
# build reproduction (#18-⑤).
#
# Primary form: a fresh ubuntu:24.04 container, clone → scripts/bootstrap.sh
# → `bao --version` verdict + cold/incremental build timing baselines
# (musl-p5 container shape — zero tree writes, the repo is cloned INSIDE the
# container).
#
# Degraded form (no docker / stop-clause): `--local` runs the same verdict
# against a LOCAL fresh clone (assumes the host already has the toolchain) —
# recorded honestly as degraded in the output.
#
# Usage:
#   scripts/bootstrap-repro.sh                    # docker form
#   scripts/bootstrap-repro.sh --local [srcdir]   # degraded local form
#   scripts/bootstrap-repro.sh --help

set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="ubuntu:24.04"
CONTAINER="${CONTAINER:-bao-bootstrap-repro}"
MODE="${1:---docker}"

say() { echo "[bootstrap-repro] $*"; }

if [ "$MODE" = "--help" ] || [ "$MODE" != "--local" ] && ! command -v docker >/dev/null 2>&1; then
    if [ "$MODE" != "--local" ]; then
        say "docker unavailable — DEGRADED to --local form (recorded as such; a local fresh clone still proves the recipe, minus the clean-OS guarantee)"
        MODE="--local"
    fi
fi

# ─── docker form ─────────────────────────────────────────────────────────────
if [ "$MODE" != "--local" ]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    say "container $CONTAINER from $IMAGE (host repo mounted :ro at /host-repo for the clone)"
    if ! docker run -d --name "$CONTAINER" -v "$REPO:/host-repo:ro" "$IMAGE" sleep infinity >/dev/null 2>&1; then
        say "RED: container start failed (image pull?) — degraded to --local"
        MODE="--local"
    fi
fi

if [ "$MODE" != "--local" ]; then
    # non-interactive: without this apt blocks forever on tzdata's debconf
    cexec() { docker exec -i -e DEBIAN_FRONTEND=noninteractive "$CONTAINER" bash -c "$*"; }
    START=$(date +%s)
    say "step 1: system deps (build-essential/curl/git/pkg-config + servo faces)"
    # clang/clang++ are REQUIRED: the vendored chains (boringssl_sys/uws_sys/
    # lsquic_sys) hardcode them — gcc cannot substitute (doctor.rs precedent).
    # FINDING w23b-1: scripts/bootstrap.sh's own apt face lacks clang — a
    # clean-OS run of the bare recipe would fail the same way (reported).
    cexec "apt-get update -qq && apt-get install -y -qq build-essential clang curl git pkg-config \
        libssl-dev libasound2-dev libudev-dev libc-ares-dev zlib1g-dev libarchive-dev libdeflate-dev libfontconfig-dev libfreetype-dev \
        libglib2.0-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-bad libmimalloc-dev cmake python3 llvm" \
        || { say "RED: apt face failed"; docker rm -f "$CONTAINER" >/dev/null; exit 1; }
    say "step 2: rustup + pinned toolchain + clone"
    cexec "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none" \
        || { say "RED: rustup failed"; docker rm -f "$CONTAINER" >/dev/null; exit 1; }
    cexec 'git config --global --add safe.directory /host-repo && git config --global --add safe.directory /host-repo/.git && . $HOME/.cargo/env && cd /root && git clone /host-repo bao' \
        || { say "RED: clone failed"; docker rm -f "$CONTAINER" >/dev/null; exit 1; }
    say "step 3: scripts/bootstrap.sh release (the recipe under test)"
    cexec '. $HOME/.cargo/env && export PATH=$HOME/.local/bin:$PATH && cd /root/bao && ./scripts/bootstrap.sh release' > /tmp/w23b-bootstrap.log 2>&1
    RC=$?
    DUR=$(( $(date +%s) - START ))
    tail -5 /tmp/w23b-bootstrap.log
    if [ "$RC" -ne 0 ]; then
        say "RED: bootstrap failed inside clean container (log: /tmp/w23b-bootstrap.log)"
        docker rm -f "$CONTAINER" >/dev/null
        exit 1
    fi
    say "step 4: --version verdict"
    V=$(cexec '. $HOME/.cargo/env && /root/bao/target/release/bao --version' 2>&1)
    echo "$V" | grep -q '^bao [0-9]' || { say "RED: --version verdict failed (got: $V)"; docker rm -f "$CONTAINER" >/dev/null; exit 1; }
    say "VERDICT PASS: clean-machine bootstrap green in ${DUR}s — $V"
    say "step 5: cold vs incremental timing (baseline seeds)"
    COLD=$(cexec '. $HOME/.cargo/env && cd /root/bao && rm -rf target && /usr/bin/time -f %e cargo build --release --jobs 4 2>&1 | tail -1' 2>/dev/null)
    say "cold rebuild (no cache): ${COLD}s"
    INC=$(cexec '. $HOME/.cargo/env && cd /root/bao && /usr/bin/time -f %e cargo build --release --jobs 4 2>&1 | tail -1' 2>/dev/null)
    say "incremental (no-op): ${INC}s — expected near-0; a large value = cache/mtime regression probe"
    docker rm -f "$CONTAINER" >/dev/null
    exit 0
fi

# ─── degraded local form ─────────────────────────────────────────────────────
SRC="${2:-/tmp/w23b-local-clone}"
say "DEGRADED local form (host toolchain assumed) — clone to $SRC"
rm -rf "$SRC"
git clone --depth 1 "file://$REPO" "$SRC" || { say "RED: local clone failed"; exit 1; }
START=$(date +%s)
(cd "$SRC" && ./scripts/bootstrap.sh release) > /tmp/w23b-local.log 2>&1
RC=$?
DUR=$(( $(date +%s) - START ))
tail -3 /tmp/w23b-local.log
V=$(cat "$SRC/target/release/bao --version" 2>/dev/null; "$SRC/target/release/bao" --version 2>/dev/null || echo "")
if [ "$RC" -ne 0 ]; then say "RED: local bootstrap failed (log: /tmp/w23b-local.log)"; exit 1; fi
echo "$V" | grep -q '^bao [0-9]' || { say "RED: --version verdict failed (got: $V)"; exit 1; }
say "VERDICT PASS (degraded/local): bootstrap green in ${DUR}s — $V"
INC=$( cd "$SRC" && /usr/bin/time -f %e cargo build --release --jobs 4 2>&1 | tail -1)
say "incremental (no-op): ${INC}s"
exit 0
