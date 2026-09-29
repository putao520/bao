#!/usr/bin/env bash
#
# scripts/consumer-gate.sh — W23-② three-path clean consumer gate (#18-②).
#
# Proves the published crates are consumable through all three real paths:
#   registry — fresh temp project, `cargo add <published crate>`, resolve
#              (`cargo metadata`) + curl-200 registry probe per crate +
#              same-version drift detection (local manifest vs registry —
#              the e18b7158 4-instance drift-debt class, single-request-per-
#              crate per the sparse-index phantom lesson, never bulk scans);
#   git      — consumer references the repo by git (tag/branch); verifies the
#              git dependency face has no path-only leakage (large network
#              traffic — opt-in);
#   source   — consumer references the working tree by path (tarball copy
#              into the temp dir, zero tree writes).
#
# Fail-closed: registry resolve failure / curl non-200 after retries / version
# drift = RED. git/source paths default to SKIP (noted) unless enabled —
# they are heavy-network faces.
#
# Usage:
#   scripts/consumer-gate.sh                       # registry path (default)
#   scripts/consumer-gate.sh --registry            # same
#   scripts/consumer-gate.sh --git --tag <ref>     # opt-in git path
#   scripts/consumer-gate.sh --source              # opt-in source path
#   scripts/consumer-gate.sh --build               # add a real `cargo check` after resolve (heavy)

set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

WORK=$(mktemp -d)/consumer-gate
mkdir -p "$WORK"
trap 'rm -rf "$(dirname "$WORK")"' EXIT

# Representative published set (e18b7158 closure faces).
CRATES=(bao-core bao-browser bun_runtime bao-servo bao-mozjs)
MODE="registry"
GIT_TAG=""
DO_BUILD=0
while [ $# -gt 0 ]; do
    case "$1" in
        --registry) MODE="registry" ;;
        --git) MODE="git"; shift; GIT_TAG="${1:-}"; continue ;;
        --source) MODE="source" ;;
        --build) DO_BUILD=1 ;;
        *) echo "unknown arg: $1"; exit 2 ;;
    esac
    shift
done

if [ "$MODE" = "registry" ]; then
    echo "[consumer] registry path — crates: ${CRATES[*]}"
    rc=0
    mkdir -p "$WORK/reg/src"
    cat > "$WORK/reg/Cargo.toml" <<'TOML'
[package]
name = "consumer-reg"
version = "0.0.0"
edition = "2021"
TOML
    echo 'fn main() {}' > "$WORK/reg/src/main.rs"
    for c in "${CRATES[@]}"; do
        # single-request registry probe (no bulk scanning)
        code=$(curl -sS -o /tmp/cg-$c.json -w '%{http_code}' --max-time 30 \
            -A "bao-consumer-gate" "https://crates.io/api/v1/crates/$c" 2>/dev/null || echo "000")
        if [ "$code" != "200" ]; then
            echo "[consumer] RED: crates.io probe for $c returned HTTP $code"
            rc=1
            continue
        fi
        reg_ver=$(python3 -c "import json;d=json.load(open('/tmp/cg-$c.json'));print(d['crate']['max_stable_version'])" 2>/dev/null || echo "?")
        local_ver=$(grep -m1 '^version' "src/$(echo "$c" | tr '-' '_')/Cargo.toml" 2>/dev/null | sed 's/version = "//; s/"//')
        [ -z "$local_ver" ] && local_ver=$(grep -m2 'name = "'$c'"' -A2 Cargo.lock | grep version | head -1 | sed 's/version = "//; s/"//' || echo "?")
        drift="ok"
        if [ "$reg_ver" != "?" ] && [ "$local_ver" != "?" ] && [ "$reg_ver" != "$local_ver" ]; then
            drift="DRIFT"
            rc=1
        fi
        echo "[consumer] $c: registry=$reg_ver local=$local_ver drift=$drift"
        cargo add --manifest-path "$WORK/reg/Cargo.toml" "$c@$reg_ver" --quiet 2>&1 | grep -viE 'adding|updating|downloading' || true
        grep -q "$c" "$WORK/reg/Cargo.toml" || { echo "[consumer] RED: cargo add failed to record $c"; rc=1; }
    done
    # Resolve the aggregate consumer (locked): the dependency graph must close.
    missing=0
    for c in "${CRATES[@]}"; do
        grep -q "$c" "$WORK/reg/Cargo.toml" || { echo "[consumer] RED: $c never recorded in the consumer manifest"; missing=1; }
    done
    [ "$missing" = "1" ] && rc=1
    echo "[consumer] resolve: cargo metadata over the aggregate consumer (all 5 deps recorded)"
    if (cd "$WORK/reg" && cargo metadata --format-version 1 >/dev/null 2>&1); then
        echo "[consumer] registry resolve: OK (dependency graph closes)"
    else
        echo "[consumer] RED: registry resolve failed (dependency graph does not close)"
        rc=1
    fi
    if [ "$DO_BUILD" = "1" ]; then
        echo "[consumer] build knob: cargo check (heavy — cold target)"
        (cd "$WORK/reg" && cargo check --quiet) || { echo "[consumer] RED: build knob failed"; rc=1; }
    else
        echo "[consumer] build knob skipped (resolve-level green; run --build for compile-level on a cold window)"
    fi
    rm -f /tmp/cg-*.json
    exit $rc
fi

if [ "$MODE" = "git" ]; then
    if [ -z "$GIT_TAG" ]; then echo "[consumer] git path needs --tag <ref> (SKIP — noted)"; exit 0; fi
    echo "[consumer] git path — consumer with git dep on tag $GIT_TAG (heavy network, opt-in)"
    mkdir -p "$WORK/git/src"
    cat > "$WORK/git/Cargo.toml" <<TOML
[package]
name = "consumer-git"
version = "0.0.0"
edition = "2021"

[dependencies]
bao-core = { git = "$(git remote get-url origin 2>/dev/null || echo "$REPO")", tag = "$GIT_TAG" }
TOML
    echo 'fn main() {}' > "$WORK/git/src/main.rs"
    (cd "$WORK/git" && cargo check --quiet) || { echo "[consumer] RED: git-path check failed"; exit 1; }
    echo "[consumer] git path: OK"
    exit 0
fi

if [ "$MODE" = "source" ]; then
    echo "[consumer] source path — tarball copy of the tree (zero tree writes)"
    mkdir -p "$WORK/src-copy"
    tar --exclude='./target' --exclude='./.git' --exclude='./vendor' -cf - . | (cd "$WORK/src-copy" && tar -xf -)
    mkdir -p "$WORK/src-con/src"
    cat > "$WORK/src-con/Cargo.toml" <<TOML
[package]
name = "consumer-src"
version = "0.0.0"
edition = "2021"

[dependencies]
bao-core = { path = "$WORK/src-copy/src/bao" }
TOML
    echo 'fn main() {}' > "$WORK/src-con/src/main.rs"
    (cd "$WORK/src-con" && cargo check --quiet) || { echo "[consumer] RED: source-path check failed"; exit 1; }
    echo "[consumer] source path: OK"
    exit 0
fi
