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
  # ops-notify 标签 issue 先于分桶排除(2026-09-16 用户裁决):它们是 launcher 自己的升级通知
  # 载体,非工单——不进 owner/non_owner、不 canned close、不进会话 intake。
  jq -c --rawfile allow "$ALLOW_FILE" '
    ($allow | split("\n") | map(gsub("^\\s+|\\s+$"; ""))
      | map(select(length > 0 and (startswith("#") | not)))) as $o
    | [ .[] | select([.labels[]?.name] | index("ops-notify") == null) ] as $items
    | {
        owner: [$items[] | select((.author.login // "") as $l | ($o | index($l)) != null)],
        non_owner: [$items[] | select((.author.login // "") as $l | ($o | index($l)) == null)
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
# headless 挂载 gsc-spec 插件 MCP(2026-08-24 根治:--mcp-config 显式注入;
# command 用 node 绝对路径 spawn——服务 PATH 无 nvm,字面 "node" 启动失败即工具集全灭)
# 2026-09-01 加固:插件缓存目录由自动更新器并发写入,原 "sort -V | tail -1" 可能选中刚落盘的坏版本
# (当日 06:08 选中 6.8.1618——携带 spec-tools 顶层加载 ReferenceError 的切割版,加载即崩 → MCP 零挂载
#  → 按无头约束 fail-closed 整轮降级只读)。修复:30 分钟稳定窗 + 两段握手探测,失败逐级回退。
# 探针两段断言(上游 gsc ISSUE #23「initialize 假绿」类:initialize 正常返回 serverInfo 但
# tool registry 全灭——initialize-only 探针对该类恒过,回退永不触发,必须拦截):
# ① id=1 initialize 响应须含 serverInfo;② id=2 tools/list 响应的 result.tools 须非空且含
#   commit_gate(工具面健康锚点,2026-09-03 起;原锚 file_lock 已随 gsc DEC-GSC-WORKSPACE-
#   TOPOLOGY-1 W3b 退役,不再断言锁工具;无 commit_gate 即 fail-closed 降级)——缺任一/超时/无响应均判
#   该候选坏,回退下一候选。捕获上限 200000B(68 工具的 tools/list 响应远超 400B,截断会误杀)。
# 2026-09-16 时序放宽:sleep4/timeout8 窗在 tools/list 大响应(~153KB)流至 ~95% 处击杀 node
#   → 尾行 JSON 截断 → jq 无 id==2 匹配 → 好版本被误判 probe-fail(间歇 3/6 轮假阴性,
#   escalation_ledger environment_pending 2026-09-04 起挂起)→ 放宽为 sleep8/timeout15。
probe_bootstrap() {
  local candidate="$1" resp
  [ -f "$candidate" ] || return 1
  resp="$((printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"dailyops-probe","version":"0"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
sleep 8) | timeout 15 "$NODE_BIN" "$candidate" 2>/dev/null | head -c 200000 || true)"
  printf '%s' "$resp" | jq -e 'select(.id == 1) | .result.serverInfo != null' >/dev/null 2>&1 || return 1
  printf '%s' "$resp" | jq -e 'select(.id == 2) | .result.tools | length > 0 and any(.[]; .name == "commit_gate")' >/dev/null 2>&1
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
# ── 升级推送通知段(升级信号出口,用户裁决 2026-09-16 ②)────────────────────────────────
# 此前升级项(escalated>0)与连续 SKIPPED_BUSY 空转只写进报告文件,无任何主动出口——用户不主动
# 翻报告系统就沉默空转(2026-09-11..15 soak 死锁 6 轮无人察觉的教训)。两条件任一命中即推送;
# 位于下方 RC 分支之前,RC=0/124/其他全部路径都经过本段。
# 通道=GitHub @mention 邮件(2026-09-16 用户裁决:pt-worker headless 无图形会话,桌面通知
# notify-send 物理不可达——实测 ServiceUnknown rc=1)。载体=putao520/bao 的 ops-notify 标签
# issue:当日已存在则追加 @putao520 comment 去重,否则建新 issue 并 @putao520 触发邮件;
# 该标签 issue 已被 intake 段排除,不会回流为当轮工单。
# gh 失败(未认证/网络/权限)显式 WARN 降级(journal 可见),|| 形态吸收失败,不改变 launcher 退出码。
# 判定面 pipe-swallow 根治(2026-09-16,范式 2039eb940):报告读取/提取进程内零管道;gh 去重查询
# 失败与「查询为空」显式区分——失败=无法判定去重,跳过发送(误走 create 会重复建通知 issue),
# 不静默降级为空;SKIPPED_BUSY 扫描失败显式 WARN 且该条件不触发,不冒充「已判定未命中」。
ESC_N=""
if [ -f "$REPORT" ]; then
  # bash 5.2 实证:$(<file) 重定向失败在 if/|| 内均为 shell 级致命(非命令退出态),
  # 不可读判定必须前置 -r 门,禁依赖 $(<) 自身退出码。
  if [ -r "$REPORT" ]; then
    esc_max=0
    content=""
    content="$(<"$REPORT")"
    # 提取域保持 ^SUMMARY: 行(报告正文另有散文 escalated=N,放宽域会改变触发语义);
    # 逐行 BASH_REMATCH 提取全部 escalated=<N> 取最大,零 fork 零管道。
    while IFS= read -r line || [ -n "$line" ]; do
      [[ "$line" == SUMMARY:* ]] || continue
      rest="$line"
      while [[ "$rest" =~ escalated=([0-9]+) ]]; do
        esc_val=$((10#${BASH_REMATCH[1]}))
        if [ "$esc_val" -gt "$esc_max" ]; then esc_max=$esc_val; fi
        rest="${rest#*escalated=${BASH_REMATCH[1]}}"
      done
    done <<< "$content"
    ESC_N="$esc_max"
  else
    echo "[daily-ops] WARN: escalation scan degraded, report unreadable: $REPORT" >&2
  fi
fi
ESC_TRIGGER=""
if [ -n "$ESC_N" ] && [ "$ESC_N" -gt 0 ]; then
  ESC_TRIGGER="escalated=$ESC_N"
fi
# 条件 b:最近 3 份报告(按 mtime,含当日)的「- 执行:」行均含 SKIPPED_BUSY = 连续 3 轮空转。
# 列表失败/单份报告不可读 → 显式 WARN 且该条件不触发(<3 份属正常的「轮数不足」,不 WARN)。
BUSY_SEEN=0
BUSY_HITS=0
BUSY_SCAN_OK=1
BUSY_LIST=""
BUSY_LIST="$(ls -t "$RUNDIR"/reports/*.md 2>/dev/null)" || BUSY_SCAN_OK=0
if [ "$BUSY_SCAN_OK" -eq 0 ]; then
  echo "[daily-ops] WARN: skipped_busy scan degraded, report listing failed — busy condition not evaluated" >&2
else
  while IFS= read -r rf; do
    [ -n "$rf" ] || continue
    BUSY_SEEN=$((BUSY_SEEN + 1))
    grep_rc=0
    command grep -q '^-[[:space:]]*执行:.*SKIPPED_BUSY' "$rf" 2>/dev/null || grep_rc=$?
    if [ "$grep_rc" -eq 0 ]; then
      BUSY_HITS=$((BUSY_HITS + 1))
    elif [ "$grep_rc" -ne 1 ]; then
      BUSY_SCAN_OK=0
      echo "[daily-ops] WARN: skipped_busy scan degraded, report unreadable: $rf — busy condition not evaluated" >&2
    fi
    [ "$BUSY_SEEN" -lt 3 ] || break
  done <<< "$BUSY_LIST"
fi
if [ "$BUSY_SCAN_OK" -eq 1 ] && [ "$BUSY_SEEN" -eq 3 ] && [ "$BUSY_HITS" -eq 3 ]; then
  ESC_TRIGGER="skipped_busy_x3${ESC_TRIGGER:+ + $ESC_TRIGGER}"
fi
if [ -n "$ESC_TRIGGER" ]; then
  ESC_MSG="$(date +%F) $ESC_TRIGGER — see .claude/daily-ops/reports/$(basename "$REPORT")"
  if [ "$MODE" = "live" ]; then
    # 去重:当日已存 ops-notify 通知 issue(标题含当日日期)则 comment 追加,否则 create 新 issue。
    # 查询失败≠查询为空:失败跳过本次发送(loud WARN),禁误走 create 重复建通知 issue。
    ESC_TODAY=""
    if ESC_TODAY="$(gh issue list --repo putao520/bao --state open --label ops-notify \
      --search "in:title $(date +%F)" --json number --jq '.[0].number' 2>/dev/null)"; then
      if [ -n "$ESC_TODAY" ]; then
        gh issue comment "$ESC_TODAY" --repo putao520/bao --body "@putao520 $ESC_MSG" >/dev/null 2>&1 \
          || echo "[daily-ops] WARN: escalation notify via gh failed (trigger=$ESC_TRIGGER)" >&2
      else
        gh issue create --repo putao520/bao --title "[OPS-NOTIFY] $(date +%F) $ESC_TRIGGER" \
          --body "@putao520 $ESC_MSG(report: .claude/daily-ops/reports/$(basename "$REPORT"))" \
          --label ops-notify >/dev/null 2>&1 \
          || echo "[daily-ops] WARN: escalation notify via gh failed (trigger=$ESC_TRIGGER)" >&2
      fi
    else
      echo "[daily-ops] WARN: escalation dedup query failed, skip send (trigger=$ESC_TRIGGER)" >&2
    fi
  else
    echo "[daily-ops] WARN: escalation notify skipped (dry-run, trigger=$ESC_TRIGGER)" >&2
  fi
fi
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
