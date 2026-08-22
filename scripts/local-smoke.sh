#!/usr/bin/env bash
#
# scripts/local-smoke.sh — 本地 smoke 持久入口(双模式,镜像原 GitHub Actions 两份 smoke workflow)
#
# 来源:.github/workflows/{browser,cdp}-smoke.yml 已于 2026-08-22 随 GitHub Actions
#       弃用而退役(用户裁决:费用原因,能力本地化)。本脚本是两份 workflow 的本地
#       等价物,smoke 断言步骤与原 workflow 语义逐字一致,禁擅自增删改
#       (原文可查:`git show da0f051e~1:.github/workflows/browser-smoke.yml` /
#        `git show da0f051e~1:.github/workflows/cdp-smoke.yml`)。
#
# 用法:./scripts/local-smoke.sh [browser|cdp|all]    # 默认 all
#
# 模式清单(与原两份 workflow 一一对应,all = 顺序跑两个,各自独立起清 Xvfb/bao):
#   browser  ← browser-smoke.yml:Xvfb → bao doctor(信息性)→ bao browser
#              (--cdp-port 9222 --url https://example.com)→ CDP /json/list →
#              bao run evaluate(typeof document)
#   cdp      ← cdp-smoke.yml:Xvfb → 临时目录 npm install playwright-core →
#              bao browser(--cdp-port 9222)→ Playwright connectOverCDP →
#              goto example.com → title 非空 + screenshot 非空断言 → 产物校验
#
# 与原 CI 的差异(仅环境适配,断言语义不动):
#   - 二进制定位 $CARGO_TARGET_DIR/debug/bao(本机 CARGO_TARGET_DIR 由环境注入,
#     repo 内 target/ 是残迹;原 CI 固定 ./target 是因为 Actions 无注入 env)
#   - 工作产物(bao.log / bao.pid / cdp-smoke.mjs / 截图 / node_modules)全部落
#     mktemp 临时目录并在退出时清理 — 禁在仓库根落地任何产物
#   - Xvfb 自起自清(原 CI 靠 VM teardown);npm install 在临时目录执行
#     (原 CI 在仓库根,本地禁 node_modules 污染仓库)
#   - 每模式输出 PASS|FAIL smoke-<mode>,末行 LOCAL-SMOKE: PASS|FAIL + 聚合退出码
#
# @trace REQ-BRW-001 (servo 浏览器集成)
# @trace REQ-CDP-001 (CDP Server 基础)
# @trace REQ-CDP-008 (Playwright 连接 / connectOverCDP)

set -uo pipefail

# 定位仓库根(与 ensure-codegen.sh / local-ci.sh 同 idiom),保证任意 cwd 调用均可
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO}"

# 镜像原两份 workflow 顶层 env 块。CARGO_BUILD_JOBS=1 是铁律兜底:
# --jobs 1 是 mozjs EBUSY patch 语义(见根 CLAUDE.md)
export CARGO_BUILD_JOBS=1
export CARGO_NET_RETRY=3
export RUSTUP_MAX_RETRIES=3

# 二进制定位(本地适配:尊重注入的 CARGO_TARGET_DIR)
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target}"
BAO_BIN="$TARGET_DIR/debug/bao"

usage() {
    cat >&2 <<'USAGE'
usage: ./scripts/local-smoke.sh [browser|cdp|all]    (default: all)
  browser  bao doctor + CDP /json/list + bao run evaluate      (mirrors browser-smoke.yml)
  cdp      Playwright connectOverCDP -> navigate -> screenshot (mirrors cdp-smoke.yml)
  all      run browser then cdp sequentially
USAGE
}

# ---------------- 模式内运行期状态(mode 子 shell 局部;防御性 ${:-} 读取)----------------
BAO_PID=""
XVFB_PID=""
BAO_LOG=""
BAO_PID_FILE=""

# mode_cleanup — 模式子 shell 的 EXIT trap:
# kill bao(先 TERM、sleep 1、再 KILL,逐字镜像原 workflow Cleanup step)+
# 杀自己起的 Xvfb(原 CI 靠 VM teardown,本地必须自清);
# 失败时回显 bao.log 尾部(本地等价物:原 CI 失败时 upload bao.log artifact)
mode_cleanup() {
    local exit_code=$?
    if [ -n "${BAO_PID:-}" ]; then
        kill "${BAO_PID}" 2>/dev/null || true
        sleep 1
        kill -9 "${BAO_PID}" 2>/dev/null || true
    fi
    if [ -n "${XVFB_PID:-}" ] && kill -0 "${XVFB_PID}" 2>/dev/null; then
        kill "${XVFB_PID}" 2>/dev/null || true
        sleep 1
        kill -9 "${XVFB_PID}" 2>/dev/null || true
    fi
    if [ "${exit_code}" -ne 0 ] && [ -n "${BAO_LOG:-}" ] && [ -f "${BAO_LOG}" ]; then
        echo "=== bao.log tail (${BAO_LOG}) ==="
        tail -n 50 "${BAO_LOG}" || true
    fi
}

# final_cleanup — 顶层 EXIT trap:清理临时工作目录(bao/Xvfb 进程由各 mode 子 shell 自清)
final_cleanup() {
    local exit_code=$?
    trap - EXIT
    rm -rf "${WORK_ROOT}"
    exit "${exit_code}"
}

# ---------------- 公共前置步骤函数(镜像两份 workflow 的同名 step)----------------

# Start Xvfb(servo WebRender 是 GL-based,headless 也要 DISPLAY;镜像原 step 原文)
start_xvfb() {
    Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp &
    XVFB_PID=$!
    sleep 2
    # 验证 Xvfb 在跑(逐字镜像原 workflow 判定)
    if ! pgrep -x Xvfb >/dev/null; then
        echo "ERROR: Xvfb failed to start"
        return 1
    fi
    echo "Xvfb started on :99 (PID=${XVFB_PID})"
    export DISPLAY=:99
}

# Launch bao browser + wait for CDP(镜像原同名 step:后台起 bao,日志落 bao.log,
# PID 落文件;轮询 /json/version 最长 60s,期间进程死 → cat 日志失败退出)
launch_bao_wait_cdp() {
    "$BAO_BIN" browser --cdp-port 9222 "$@" >"$BAO_LOG" 2>&1 &
    BAO_PID=$!
    echo "bao browser PID=${BAO_PID}"
    echo "${BAO_PID}" >"${BAO_PID_FILE}"

    local i
    for i in $(seq 1 60); do
        if curl -sf http://127.0.0.1:9222/json/version >/dev/null 2>&1; then
            echo "CDP Server up after ${i}s"
            return 0
        fi
        # 进程崩了直接退出
        if ! kill -0 "${BAO_PID}" 2>/dev/null; then
            echo "ERROR: bao browser exited before CDP came up. Log:"
            cat "${BAO_LOG}"
            return 1
        fi
        sleep 1
    done
    echo "ERROR: CDP Server did not come up within 60s. Log:"
    cat "${BAO_LOG}"
    return 1
}

# ============================ [browser] ← browser-smoke.yml ============================

run_browser_mode() {
    local mode_dir="$WORK_ROOT/browser"
    mkdir -p "$mode_dir"
    BAO_LOG="$mode_dir/bao.log"
    BAO_PID_FILE="$mode_dir/bao.pid"

    start_xvfb

    # Smoke — bao doctor(环境自检,informational;原 workflow 在 launch 前跑)
    "$BAO_BIN" doctor || true

    # Launch bao browser + wait for CDP(browser 模式带 --url,镜像原 workflow)
    launch_bao_wait_cdp --url https://example.com

    # Smoke — /json/list (target 列表)
    echo "=== /json/list ==="
    curl -sf http://127.0.0.1:9222/json/list | head -c 1000
    echo

    # Smoke — bao evaluate(等几秒让 navigate 完成;输出信息性回显,镜像原 step)
    sleep 5
    TITLE=$("$BAO_BIN" run -e 'console.log(typeof document)' 2>&1 || echo "n/a")
    echo "script output: ${TITLE}"
}

# ============================ [cdp] ← cdp-smoke.yml ============================

run_cdp_mode() {
    local mode_dir="$WORK_ROOT/cdp"
    local npm_dir="$mode_dir/npm"
    mkdir -p "$npm_dir"
    BAO_LOG="$mode_dir/bao.log"
    BAO_PID_FILE="$mode_dir/bao.pid"

    start_xvfb

    # Install playwright-core(不下载自带 chromium,connectOverCDP 连 Bao;原 CI
    # 在仓库根 install,本地落临时目录 — 禁 node_modules 污染仓库)
    (cd "$npm_dir" && npm install playwright-core)

    # Launch bao browser + wait for CDP(cdp 模式不带 --url,镜像原 workflow)
    launch_bao_wait_cdp

    # Playwright connectOverCDP → navigate → title → screenshot
    # (mjs 逐字取自原 cdp-smoke.yml 内嵌 heredoc,禁改)
    cat >"$npm_dir/cdp-smoke.mjs" <<'EOF'
import { chromium } from 'playwright-core';

const CDP_ENDPOINT = 'http://127.0.0.1:9222';
const TARGET_URL = 'https://example.com';
const SHOT_PATH = 'cdp-smoke-screenshot.png';

async function main() {
  console.log(`[cdp-smoke] connecting to ${CDP_ENDPOINT} ...`);
  const browser = await chromium.connectOverCDP(CDP_ENDPOINT, { timeout: 30000 });
  console.log('[cdp-smoke] connected.');

  // Bao 的 CDP Server 暴露已存在的 context(像 Chrome)
  const context = browser.contexts()[0] || await browser.newContext();
  const page = await context.newPage();

  console.log(`[cdp-smoke] navigating to ${TARGET_URL} ...`);
  await page.goto(TARGET_URL, { waitUntil: 'domcontentloaded', timeout: 30000 });

  const title = await page.title();
  console.log(`[cdp-smoke] title="${title}"`);

  await page.screenshot({ path: SHOT_PATH });
  console.log(`[cdp-smoke] screenshot saved: ${SHOT_PATH}`);

  if (!title || title.trim().length === 0) {
    throw new Error(`[cdp-smoke] FAIL: title is empty`);
  }

  const fs = await import('node:fs');
  if (!fs.existsSync(SHOT_PATH) || fs.statSync(SHOT_PATH).size === 0) {
    throw new Error(`[cdp-smoke] FAIL: screenshot missing or empty`);
  }

  console.log('[cdp-smoke] PASS: title non-empty + screenshot saved');
  await browser.close();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
EOF

    (cd "$npm_dir" && node cdp-smoke.mjs)

    # Verify screenshot artifact(镜像原 step:ls + file)
    (cd "$npm_dir" && ls -la cdp-smoke-screenshot.png && file cdp-smoke-screenshot.png)
}

# ============================ 参数与依赖预检 ============================

if [ "$#" -gt 1 ]; then
    echo "ERROR: too many arguments ($#, expected <= 1)" >&2
    usage
    exit 2
fi
MODE="${1:-all}"
case "$MODE" in
    browser|cdp|all) ;;
    *) echo "ERROR: unknown mode '${MODE}'" >&2; usage; exit 2 ;;
esac

# 快速失败:依赖缺失直接报,避免跑到一半 127 / launch 轮询 60s 盲等
for tool in curl Xvfb pgrep; do
    command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: missing dependency: $tool" >&2; exit 2; }
done
if [ "$MODE" != "browser" ]; then
    for tool in node npm file; do
        command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: missing dependency: $tool (cdp 模式需要)" >&2; exit 2; }
    done
fi

# 临时工作根(所有工作产物落此,退出清理)
WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bao-local-smoke.XXXXXX")"
trap final_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "work dir: ${WORK_ROOT} (退出时自动清理)"

# ============================ 公共前置(两模式同,镜像原 workflow steps) ============================

echo "==> [preamble] ensure-codegen + cargo build --jobs 1 -p bao_bin"
(
    set -euo pipefail
    echo '$ ./scripts/ensure-codegen.sh'
    ./scripts/ensure-codegen.sh
    echo '$ cargo build --jobs 1 -p bao_bin'
    cargo build --jobs 1 -p bao_bin
    if [ ! -x "$BAO_BIN" ]; then
        echo "ERROR: bao binary not found at $BAO_BIN (CARGO_TARGET_DIR=${TARGET_DIR})" >&2
        exit 1
    fi
)
BUILD_RC=$?
if [ "${BUILD_RC}" -ne 0 ]; then
    echo "FAIL build (exit=${BUILD_RC})"
    echo "LOCAL-SMOKE: FAIL"
    exit 1
fi
echo "PASS build"

# ============================ 跑模式(各自独立 errexit 子 shell + EXIT trap 清理) ============================

STATUS_BROWSER=0
STATUS_CDP=0

if [ "$MODE" = "browser" ] || [ "$MODE" = "all" ]; then
    echo
    echo "==> [smoke-browser] bao doctor + /json/list + bao run evaluate"
    (
        set -euo pipefail
        trap mode_cleanup EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        run_browser_mode
    )
    STATUS_BROWSER=$?
    if [ "${STATUS_BROWSER}" -eq 0 ]; then echo "PASS smoke-browser"; else echo "FAIL smoke-browser (exit=${STATUS_BROWSER})"; fi
fi

if [ "$MODE" = "cdp" ] || [ "$MODE" = "all" ]; then
    echo
    echo "==> [smoke-cdp] Playwright connectOverCDP -> navigate -> screenshot"
    (
        set -euo pipefail
        trap mode_cleanup EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        run_cdp_mode
    )
    STATUS_CDP=$?
    if [ "${STATUS_CDP}" -eq 0 ]; then echo "PASS smoke-cdp"; else echo "FAIL smoke-cdp (exit=${STATUS_CDP})"; fi
fi

# ============================ 聚合 ============================

OVERALL_RC=0
if [ "${STATUS_BROWSER}" -ne 0 ] || [ "${STATUS_CDP}" -ne 0 ]; then
    OVERALL_RC=1
fi

if [ "${OVERALL_RC}" -eq 0 ]; then
    echo "LOCAL-SMOKE: PASS"
else
    echo "LOCAL-SMOKE: FAIL"
fi
exit "${OVERALL_RC}"
