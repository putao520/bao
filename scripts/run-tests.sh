#!/usr/bin/env bash
#
# scripts/run-tests.sh — canonical headless 测试环境引导 + 一键真跑(Goal A「测试真执行 100%」验收载体)
#
# 用途:bao_browser/bao_runtime 等 suite 的 servo E2E 测试以
#   #[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97
#              Xvfb :99 配方; 真跑: ... cargo nextest run --run-ignored=all -p <crate>"]
# 显式标记(e-fakegreen 2026-10-16 假绿门控根治:无 env 静默 PASS 已根除,ignore 原因串即运维指令);
# 真外部网络面标 #[ignore = "[env-gate] 需要真外网/外部站点 + DISPLAY; ..."]。
# 本脚本把原因串里的环境前提一次备齐,再以 --run-ignored=all 放行全部测试真执行。
#
# Xvfb 配方镜像 scripts/local-smoke.sh:97(「servo WebRender 是 GL-based,headless 也要 DISPLAY」;
# 原 GitHub Actions smoke workflow 同配方,2026-08-22 随 Actions 弃用本地化)。
#
# 行为:
#   1. Xvfb 未在跑则自起 :99(自起不清:长驻无害,后续跑批复用;不 trap 清理——取简单者);
#   2. DISPLAY / BAO_TEST_NETWORK / BAO_TEST_REAL_SERVO 未设则补缺省(已设者尊重,可覆盖);
#   3. BAO_BIN 未设时在 CARGO_TARGET_DIR(缺省 <repo>/target,与 local-smoke.sh 同 idiom)
#      下探测 test-ci/bao——e2e 外部进程测试(bao_cli_*_e2e 等经 `bao browser` spawn 引擎)
#      需要;探测不到打提示后继续(纯进程内测试面不需要二进制);
#   4. exec cargo nextest run --cargo-profile test-ci --no-fail-fast --run-ignored=all "$@"
#      (-p/-E 原样透传;--no-fail-fast 保全面——单红不砍 2/3 测试面)。
#
# 用法:
#   scripts/run-tests.sh -p bao-browser                          # 整 crate
#   scripts/run-tests.sh -p bao-browser -E 'test(editing_e2e)'   # filterset 透传

set -euo pipefail

# 定位仓库根(与 local-smoke.sh / ensure-codegen.sh 同 idiom),任意 cwd 调用均可
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO}"

# 1) Xvfb:servo WebRender GL-based,headless 也要 DISPLAY(local-smoke.sh:97 同配方)
if ! pgrep -x Xvfb >/dev/null; then
    echo "[run-tests] starting Xvfb :99 (long-lived; left running on purpose)"
    Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp &
    sleep 2
    # 验证 Xvfb 在跑(local-smoke.sh 同判定)
    if ! pgrep -x Xvfb >/dev/null; then
        echo "ERROR: Xvfb failed to start" >&2
        exit 1
    fi
fi

# 2) env 补缺省(已设者尊重)
export DISPLAY="${DISPLAY:-:99}"
export BAO_TEST_NETWORK="${BAO_TEST_NETWORK:-1}"
export BAO_TEST_REAL_SERVO="${BAO_TEST_REAL_SERVO:-1}"

# 3) BAO_BIN 缺省定位(e2e 外部进程测试 spawn `bao` 引擎二进制;尊重已注入的 CARGO_TARGET_DIR)
if [ -z "${BAO_BIN:-}" ]; then
    TARGET_ROOT="${CARGO_TARGET_DIR:-${REPO}/target}"
    if [ -x "${TARGET_ROOT}/test-ci/bao" ]; then
        export BAO_BIN="${TARGET_ROOT}/test-ci/bao"
        echo "[run-tests] BAO_BIN=${BAO_BIN}"
    else
        echo "[run-tests] WARN: BAO_BIN unset and ${TARGET_ROOT}/test-ci/bao not found." >&2
        echo "           e2e external-process tests (bao_cli_*_e2e) will fail-closed." >&2
        echo "           Build it first: cargo build --profile test-ci -p bao_bin" >&2
    fi
fi

# 4) canonical 一键真跑(--run-ignored=all 放行 #[ignore] 面)
exec cargo nextest run --cargo-profile test-ci --no-fail-fast --run-ignored=all "$@"
