#!/usr/bin/env bash
# bao daily-ops launcher: systemd -> claude headless
set -euo pipefail
REPO="/home/putao/code/rust/bao"
RUNDIR="$REPO/.claude/daily-ops"
MODE="${MODE:-dry-run}"
MAX_SECONDS="${MAX_SECONDS:-5400}"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
mkdir -p "$RUNDIR/reports"
exec 9>"$RUNDIR/lock"
flock -n 9 || { echo "busy, skip"; exit 0; }
REPORT="$RUNDIR/reports/$(date +%F).md"
[ -e "$REPORT" ] && REPORT="$RUNDIR/reports/$(date +%F).$(date +%H%M).md"
export DAILY_OPS_MODE="$MODE" DAILY_OPS_REPORT="$REPORT"
# 预检:失败不阻断,注入标志给会话消费
gh auth status >/dev/null 2>&1 || export DAILY_OPS_GH=failed
git -C "$REPO" diff --quiet >/dev/null 2>&1 || export DAILY_OPS_DIRTY=1
jq -e '.upstreams.bun.baseline and .upstreams.servo.baseline' "$REPO/.claude/upstream-baseline.json" >/dev/null 2>&1 || export DAILY_OPS_BASELINE=invalid
pgrep -x cargo >/dev/null 2>&1 && export DAILY_OPS_CARGO_BUSY=1
[ -n "${CARGO_REGISTRY_TOKEN:-}" ] || export DAILY_OPS_PUBLISH=failed
GIT_PRE="$(git -C "$REPO" rev-parse HEAD)"
LOG_FILE="$RUNDIR/logs-$(date +%F).log"
set +e
timeout --signal=TERM --kill-after=60 "$MAX_SECONDS" \
  claude -p "$(cat "$REPO/.claude/prompts/daily-ops.md")" \
  --dangerously-skip-permissions 2>&1 | tee -a "$LOG_FILE"
RC=${PIPESTATUS[0]}
set -e
if [ "$RC" -eq 0 ]; then
  if [ "$MODE" = "dry-run" ] && [ "$(git -C "$REPO" rev-parse HEAD)" != "$GIT_PRE" ]; then
    echo "VIOLATION: dry-run made commits ($(git -C "$REPO" rev-parse --short HEAD))" >> "$REPORT"
    exit 1
  fi
  exit 0
elif [ "$RC" -eq 124 ]; then
  echo "SUMMARY: timeout" >> "$REPORT"
  exit 124
else
  echo "SUMMARY: failed rc=$RC" >> "$REPORT"
  echo "claude log: $LOG_FILE" >> "$REPORT"
  exit "$RC"
fi
