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
# CARGO_BUSY 收窄为 bao 进程(2026-08-24 裁定:机器级 pgrep 误伤他项目 cargo;bao 独立 target dir 零锁竞争)
bao_cargo_busy() {
  local pid
  for pid in $(pgrep -x cargo 2>/dev/null); do
    [ "$(readlink "/proc/$pid/cwd" 2>/dev/null || true)" = "$REPO" ] && return 0
  done
  return 1
}
# 有限等待:最多 6 轮 × 5min(2026-08-24 教训:06:11 busy 06:24 清零,差 13min)
BUSY_ROUNDS=0
while bao_cargo_busy && [ "$BUSY_ROUNDS" -lt 6 ]; do
  sleep 300
  BUSY_ROUNDS=$((BUSY_ROUNDS + 1))
done
bao_cargo_busy && export DAILY_OPS_CARGO_BUSY=1
[ -n "${CARGO_REGISTRY_TOKEN:-}" ] || export DAILY_OPS_PUBLISH=failed
GIT_PRE="$(git -C "$REPO" rev-parse HEAD)"
LOG_FILE="$RUNDIR/logs-$(date +%F).log"
# ── issue 作者 allowlist 门禁(launcher 侧确定性过滤,2026-09-01 用户裁决)──────────────
# issue 仅响应 allowlist 内作者;其余由 launcher 在 live 下确定性 canned close。
# non_owner 条目只保留 number/author 两字段——不可信文本(title/body)绝不进入无头会话上下文(注入防护)。
ALLOW_FILE="$RUNDIR/issue-authors.allow"
[ -f "$ALLOW_FILE" ] || printf '# issue author allowlist for daily-ops intake (one login per line)\nputao520\n' > "$ALLOW_FILE"
INBOX="$RUNDIR/inbox-$(date +%F).json"
rm -f "$RUNDIR/inbox-raw.json"
if [ -z "${DAILY_OPS_GH:-}" ]; then
  gh issue list --repo putao520/bao --state open --json number,title,body,labels,author \
    > "$RUNDIR/inbox-raw.json" 2>/dev/null || export DAILY_OPS_GH=failed
fi
if [ -z "${DAILY_OPS_GH:-}" ] && [ -s "$RUNDIR/inbox-raw.json" ]; then
  jq -c --rawfile allow "$ALLOW_FILE" '
    ($allow | split("\n") | map(gsub("^\\s+|\\s+$"; ""))
      | map(select(length > 0 and (startswith("#") | not)))) as $o
    | {
        owner: [.[] | select((.author.login // "") as $l | ($o | index($l)) != null)],
        non_owner: [.[] | select((.author.login // "") as $l | ($o | index($l)) == null)
                    | {number: .number, author: (.author.login // "")}]
      }' "$RUNDIR/inbox-raw.json" > "$INBOX"
  rm -f "$RUNDIR/inbox-raw.json"
  export DAILY_OPS_INBOX="$INBOX"
  # 确定性 canned close 仅 live;dry-run 零 gh 写(硬禁四写不变)
  if [ "$MODE" = "live" ]; then
    CANNED="Thanks for filing. This repository currently triages issues only from allowlisted maintainer accounts, so outside submissions are closed unreviewed. If this is a genuine Bao bug report, please reach the maintainer through the channels in the README."
    while read -r n; do
      [ -n "$n" ] || continue
      if gh issue close "$n" --repo putao520/bao --reason "not planned" --comment "$CANNED" >/dev/null 2>&1; then
        echo "[daily-ops] issue-gate: closed #$n (author not in allowlist)" >&2
        echo "[daily-ops] issue-gate: closed #$n (author not in allowlist)" >> "$LOG_FILE"
      else
        echo "[daily-ops] issue-gate: close FAILED #$n (author not in allowlist)" >&2
        echo "[daily-ops] issue-gate: close FAILED #$n (author not in allowlist)" >> "$LOG_FILE"
      fi
    done < <(jq -r '.non_owner[].number' "$INBOX")
  fi
fi
rm -f "$RUNDIR/inbox-raw.json"
# headless 挂载 gsc-spec 插件 MCP(file_lock 互斥,2026-08-24 根治:--mcp-config 显式注入;
# command 用 node 绝对路径 spawn——服务 PATH 无 nvm,字面 "node" 启动失败即工具集缺 file_lock)
# 2026-09-01 加固:插件缓存目录由自动更新器并发写入,原 "sort -V | tail -1" 可能选中刚落盘的坏版本
# (当日 06:08 选中 6.8.1618——携带 spec-tools 顶层加载 ReferenceError 的切割版,加载即崩 → MCP 零挂载
#  → 按无头约束 fail-closed 整轮降级只读)。修复:30 分钟稳定窗 + initialize 握手探测,失败逐级回退。
probe_bootstrap() {
  local candidate="$1" resp
  [ -f "$candidate" ] || return 1
  resp="$((printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"dailyops-probe","version":"0"}}}'; sleep 3) \
    | timeout 6 "$NODE_BIN" "$candidate" 2>/dev/null | head -c 400 || true)"
  printf '%s' "$resp" | command grep -q '"serverInfo"'
}
BOOTSTRAP=""
if [ -z "$NODE_BIN" ]; then
  echo "WARN: node not found, MCP mount skipped" >&2
else
  for candidate in $(ls -d "$HOME"/.claude/plugins/cache/gsc-spec/gsc-spec/*/mcp/src/bootstrap.mjs 2>/dev/null | sort -r -V || true); do
    CAND_DIR="$(dirname "$(dirname "$(dirname "$candidate")")")"  # .../gsc-spec/<版本>
    CAND_VER="$(basename "$CAND_DIR")"
    # 30 分钟稳定窗:跳过刚写入的版本目录(自动更新器随时回收/替换,选中即可能读到一半被删)
    if [ -d "$CAND_DIR" ] && [ "$(( $(date +%s) - $(stat -c %Y "$CAND_DIR") ))" -lt 1800 ]; then
      echo "[daily-ops] mcp-pin: skip version=$CAND_VER reason=fresh(<30min)" >&2
      continue
    fi
    if probe_bootstrap "$candidate"; then
      BOOTSTRAP="$candidate"
      echo "[daily-ops] mcp-pin: version=$CAND_VER probe=ok" >&2
      echo "[daily-ops] mcp-pin: version=$CAND_VER probe=ok" >> "$LOG_FILE"
      break
    fi
    echo "[daily-ops] mcp-pin: fallback from=$CAND_VER reason=probe-fail" >&2
    echo "[daily-ops] mcp-pin: fallback from=$CAND_VER reason=probe-fail" >> "$LOG_FILE"
  done
  [ -n "$BOOTSTRAP" ] || echo "WARN: no stable bootstrap passed probe, MCP mount skipped" >&2
fi
MCP_FLAG=()
if [ -n "$BOOTSTRAP" ] && [ -n "$NODE_BIN" ]; then
  PLUGIN_ROOT="$(dirname "$(dirname "$(dirname "$BOOTSTRAP")")")"
  MCPCONF="$(mktemp /tmp/daily-ops-mcp-XXXXXX.json)"
  trap 'rm -f "$MCPCONF"' EXIT
  printf '{"mcpServers":{"arch":{"command":"%s","args":["%s"],"env":{"CLAUDE_PLUGIN_ROOT":"%s"}}}}' \
    "$NODE_BIN" "$BOOTSTRAP" "$PLUGIN_ROOT" > "$MCPCONF"
  MCP_FLAG=(--mcp-config "$MCPCONF")
fi
# 冷启动 bootstrap 含环境检测 + runtime-server 自举,默认 MCP 超时不足(2026-09-01 教训)
export MCP_TIMEOUT=120000 MCP_TOOL_TIMEOUT=300000
# 无头值班不需要外部网页抓取/图像分析(2026-09-01 注入防护:切断不可信 issue 文本可驱动的出网外带通道)
DISALLOW_FLAG=()
if claude --help 2>/dev/null | command grep -q -- --disallowedTools; then
  DISALLOW_FLAG=(--disallowedTools "WebFetch" "WebSearch" "mcp__web_reader__webReader" "mcp__4_5v_mcp__analyze_image")
else
  echo "WARN: claude CLI has no --disallowedTools, egress lockdown skipped" >&2
fi
set +e
timeout --signal=TERM --kill-after=60 "$MAX_SECONDS" \
  claude -p "$(cat "$REPO/.claude/prompts/daily-ops.md")" "${MCP_FLAG[@]}" "${DISALLOW_FLAG[@]}" \
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
