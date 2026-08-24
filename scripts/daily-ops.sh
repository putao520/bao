#!/usr/bin/env bash
# bao daily-ops launcher: systemd -> claude headless
set -euo pipefail
REPO="/home/putao/code/rust/bao"
RUNDIR="$REPO/.claude/daily-ops"
MODE="${MODE:-dry-run}"
MAX_SECONDS="${MAX_SECONDS:-604800}"
# node 绝对路径双通道解析(2026-08-24 根因:systemd 服务 PATH 无 nvm,MCP spawn "node" 失败)
NODE_BIN="$(command -v node || ls -d "$HOME"/.nvm/versions/node/*/bin/node 2>/dev/null | sort -V | tail -1 || true)"
NODE_DIR="$(dirname "${NODE_BIN:-$(ls -d "$HOME"/.nvm/versions/node/*/bin/node 2>/dev/null | sort -V | tail -1 || echo /usr/bin/node)}")"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin${NODE_DIR:+:$NODE_DIR}:$PATH"
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
# CARGO_BUSY 有限等待:最多 6 轮 × 5min(2026-08-24 教训:06:11 busy 06:24 清零,差 13min)
BUSY_ROUNDS=0
while pgrep -x cargo >/dev/null 2>&1 && [ "$BUSY_ROUNDS" -lt 6 ]; do
  sleep 300
  BUSY_ROUNDS=$((BUSY_ROUNDS + 1))
done
pgrep -x cargo >/dev/null 2>&1 && export DAILY_OPS_CARGO_BUSY=1
[ -n "${CARGO_REGISTRY_TOKEN:-}" ] || export DAILY_OPS_PUBLISH=failed
GIT_PRE="$(git -C "$REPO" rev-parse HEAD)"
LOG_FILE="$RUNDIR/logs-$(date +%F).log"
# headless 挂载 gsc-spec 插件 MCP(file_lock 互斥,2026-08-24 根治:--mcp-config 显式注入;
# command 用 node 绝对路径 spawn——服务 PATH 无 nvm,字面 "node" 启动失败即工具集缺 file_lock)
BOOTSTRAP="$(ls -d "$HOME"/.claude/plugins/cache/gsc-spec/gsc-spec/*/mcp/src/bootstrap.mjs 2>/dev/null | sort -V | tail -1 || true)"
MCP_FLAG=()
if [ -n "$BOOTSTRAP" ] && [ -n "$NODE_BIN" ]; then
  PLUGIN_ROOT="$(dirname "$(dirname "$(dirname "$BOOTSTRAP")")")"
  MCPCONF="$(mktemp /tmp/daily-ops-mcp-XXXXXX.json)"
  printf '{"mcpServers":{"arch":{"command":"%s","args":["%s"],"env":{"CLAUDE_PLUGIN_ROOT":"%s"}}}}' \
    "$NODE_BIN" "$BOOTSTRAP" "$PLUGIN_ROOT" > "$MCPCONF"
  MCP_FLAG=(--mcp-config "$MCPCONF")
else
  echo "WARN: node/bootstrap not found, MCP mount skipped" >&2
fi
set +e
timeout --signal=TERM --kill-after=60 "$MAX_SECONDS" \
  claude -p "$(cat "$REPO/.claude/prompts/daily-ops.md")" "${MCP_FLAG[@]}" \
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
