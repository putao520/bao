#!/usr/bin/env bash
# battery-v11-20261001.sh — 2026-10-01 大波波末门(W52 roundRect/W54 stylo 0.22/W55+v7 CDP frameId 族/v8 Input/v9 锚)
# 五节独立退出码;支持 --only N;由 C 在波末执行(全部在途合同收口、树静点后)。
# 用法: bash .plans/battery-v11-20261001.sh [--only 1|2|3|4|5]
set -uo pipefail
REPO=${REPO:-/home/putao/code/rust/bao}
JOBS=4
ONLY=""
[ "${1:-}" = "--only" ] && ONLY="${2:-}"
cd "$REPO" || { echo "FATAL: repo unreachable"; exit 97; }
declare -A GATE_RC

nt() { cargo nextest run --cargo-profile test-ci -j "$JOBS" "$@"; }

# 门① W54:stylo 0.22 链 — bao-servo 编译面 + fonts scoped + workspace check
gate1() {
  local rc=0
  cargo check -p bao-servo --jobs "$JOBS" >/dev/null 2>&1 || { echo "G1-FAIL: bao-servo check"; rc=1; }
  cargo check --workspace --jobs "$JOBS" >/dev/null 2>&1 || { echo "G1-FAIL: workspace check"; rc=1; }
  nt -p bao-servo-fonts || rc=1
  GATE_RC[1]=$rc
}

# 门② W52:canvas roundRect 单元面
gate2() {
  local rc=0
  nt -p bao-servo-canvas-traits || rc=1
  GATE_RC[2]=$rc
}

# 门③ CDP 族(v7 单源/v8 Input/v9 锚):全 suite + 真客户端双锚
gate3() {
  local rc=0
  nt -p bao_cdp || rc=1
  nt -p bao_cdp_client || rc=1
  nt -p bao-browser || rc=1
  # 真客户端 oracle:v8/v9 收口后必须默认绿(锚门已摘,不带 --run-ignored)
  nt -p bao-browser -E 'test(puppeteer_real_lifecycle_e2e) or test(playwright_style_full_flow)' || { echo "G3-FAIL: real-client anchors"; rc=1; }
  GATE_RC[3]=$rc
}

# 门④ runtime 面:cdp_server + engine scoped
gate4() {
  local rc=0
  nt -p bun_runtime -E 'test(cdp_server)' || rc=1
  nt -p bao_engine || rc=1
  GATE_RC[4]=$rc
}

# 门⑤ 外部探针:frameId 一致性(mini.py 断言面)+ WPT driver 三态基线硬门(低于基线=红)
gate5() {
  local rc=0
  local wpt_baseline=${WPT_BASELINE:-35}
  cargo build -p bao_bin --jobs "$JOBS" >/dev/null 2>&1 || { echo "G5-FAIL: bao_bin build"; rc=1; }
  timeout 90 python3 /tmp/w40-probe/mini.py --iso --ctx --with-script 2>&1 | command grep -q "VERDICT: OK" || { echo "G5-FAIL: mini frameId consistency"; rc=1; }
  if [ -f /tmp/wpt_first_run.py ] && [ -r /tmp/wpt-manifest.txt ]; then
    timeout 1800 python3 /tmp/wpt_first_run.py /tmp/wpt-manifest.txt /tmp/wpt-v11.md >/dev/null 2>&1
    local passes
    passes=$(command grep -cE '^\| .* \| OK \|' /tmp/wpt-v11.md 2>/dev/null || echo 0)
    if [ "${passes:-0}" -lt "$wpt_baseline" ]; then
      echo "G5-FAIL: WPT passes ${passes:-0} < baseline ${wpt_baseline} (see /tmp/wpt-v11.md)"
      rc=1
    fi
  else
    echo "G5-FAIL: WPT driver/manifest missing (/tmp/wpt_first_run.py, /tmp/wpt-manifest.txt)"
    rc=1
  fi
  GATE_RC[5]=$rc
}

for i in 1 2 3 4 5; do
  [ -n "$ONLY" ] && [ "$ONLY" != "$i" ] && continue
  "gate$i"; echo "GATE-$i RC=${GATE_RC[$i]}"
done
FAILS=0; for i in 1 2 3 4 5; do [ "${GATE_RC[$i]:-0}" -ne 0 ] && FAILS=$((FAILS+1)); done
echo "BATTERY-V11: $((5-FAILS))/5 gates green"
exit "$FAILS"
