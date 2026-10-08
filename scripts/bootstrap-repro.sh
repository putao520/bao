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
    # --init(#54 同类横扫):docker-init(tini)作 PID1 收割 exec 孤儿子进程,防僵尸累积
    if ! docker run -d --init --name "$CONTAINER" -v "$REPO:/host-repo:ro" "$IMAGE" sleep infinity >/dev/null 2>&1; then
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
    # BAO_BOOTSTRAP_OVERLAY=1: clone gives HEAD (committed recipe); overlay
    # the WORKING-TREE bootstrap.sh to verify an uncommitted recipe patch
    # before commit (labelled in output).
    if [ "${BAO_BOOTSTRAP_OVERLAY:-0}" = "1" ]; then
        docker cp "$REPO/scripts/bootstrap.sh" "$CONTAINER":/root/bao/scripts/bootstrap.sh
        say "overlay: working-tree bootstrap.sh copied over HEAD (uncommitted-recipe mode)"
    fi
    say "step 3: scripts/bootstrap.sh release (the recipe under test)"
    cexec '. $HOME/.cargo/env && export PATH=$HOME/.local/bin:$PATH && cd /root/bao && ./scripts/bootstrap.sh release' > /tmp/w23b-bootstrap.log 2>&1
    RC=$?
    DUR=$(( $(date +%s) - START ))
    tail -5 /tmp/w23b-bootstrap.log
    if [ "$RC" -ne 0 ]; then
        # Export the build log BEFORE tearing the container down (observability).
        docker cp "$CONTAINER":/tmp/w23b-bootstrap.log /tmp/w23b-bootstrap.log 2>/dev/null || true
        say "RED: bootstrap failed inside clean container (log exported: /tmp/w23b-bootstrap.log)"
        docker rm -f "$CONTAINER" >/dev/null
        exit 1
    fi
    say "step 4: doctor verdict (the CLI has no --version flag — the doctor subcommand is the health face)"
    V=$(cexec '. $HOME/.cargo/env && /root/bao/target/release/bao doctor' 2>&1)
    echo "$V" | grep -q 'Bao doctor' || { say "RED: doctor verdict failed (got: $V)"; docker rm -f "$CONTAINER" >/dev/null; exit 1; }
    say "VERDICT PASS: clean-machine bootstrap green in ${DUR}s (doctor face OK)"
    say "step 5: cold vs incremental timing (baseline seeds)"
    T0=$(date +%s)
    cexec '. $HOME/.cargo/env && cd /root/bao && rm -rf target && cargo build --release --jobs 4 >/dev/null 2>&1'
    COLD=$(( $(date +%s) - T0 ))
    say "cold rebuild (no cache): ${COLD}s"
    T1=$(date +%s)
    cexec '. $HOME/.cargo/env && cd /root/bao && cargo build --release --jobs 4 >/dev/null 2>&1'
    INC=$(( $(date +%s) - T1 ))
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
V=$("$SRC/target/release/bao" doctor 2>/dev/null | head -1)
if [ "$RC" -ne 0 ]; then say "RED: local bootstrap failed (log: /tmp/w23b-local.log)"; exit 1; fi
echo "$V" | grep -q 'Bao doctor' || { say "RED: doctor verdict failed (got: $V)"; exit 1; }
say "VERDICT PASS (degraded/local): bootstrap green in ${DUR}s (doctor face OK)"
INC=$( cd "$SRC" && /usr/bin/time -f %e cargo build --release --jobs 4 2>&1 | tail -1)
say "incremental (no-op): ${INC}s"
exit 0
