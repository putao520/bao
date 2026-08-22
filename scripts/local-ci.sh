#!/usr/bin/env bash
#
# scripts/local-ci.sh — 本地 CI 持久入口(1:1 镜像原 GitHub Actions ci.yml 四 job)
#
# 来源:.github/workflows/ci.yml 已于 2026-08-22 随 GitHub Actions 弃用而退役
#       (用户裁决:费用原因,CI 本地化)。本脚本是该 workflow 四 job 的本地
#       等价物,cargo 命令逐字镜像,禁擅自增删改:
#       - `--jobs 1` 是项目铁律(mozjs EBUSY patch 语义,见根 CLAUDE.md);
#       - `cargo fmt` 禁 `--all`(--all 会递归 vendor/servo);
#       - 修正原 ci.yml 两处笔误(Commander 裁决):
#         ① check job 包名 `bao_browser` → `bao-browser`(真实 package 名,
#           原文与 lib target 名混淆;cargo -p 解析失败,CI 恒 exit 101);
#         ② clippy job 去掉多余 `--lib`(bao_bin 是纯 binary 包,无 lib
#           target,`error: no library targets found` exit 101)。
#
# 用法:./scripts/local-ci.sh
#
# job 清单(与原 ci.yml 一一对应):
#   fmt       非阻断  cargo fmt -- --check(bao_* 历史格式化债务 ~8000 处,
#                     原 CI 即 continue-on-error,仅报告)
#   check     阻断    ./scripts/ensure-codegen.sh
#                     cargo check --jobs 1 -p bao_bin
#                     cargo check --jobs 1 -p bao-browser
#   clippy    阻断    ./scripts/ensure-codegen.sh(幂等,镜像原 clippy job 前置 step)
#                     cargo clippy --jobs 1 -p bao_bin --no-deps --bins
#                     (warnings 暂不阻断:命令不带 -D warnings,退出码语义即契约)
#   bce-gate  阻断    make bce-check(本地门禁:覆盖原 CI 三扫且多一 catch-fallback)
#
# 退出码:任一阻断 job(check / clippy / bce-gate)失败 → 非 0;仅 fmt 失败 → 0。

set -uo pipefail

# 定位仓库根(与 ensure-codegen.sh 同 idiom),保证任意 cwd 调用均可
# (make 与 ./scripts/ensure-codegen.sh 均依赖仓库根 cwd)
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO}"

# 镜像原 ci.yml 顶层 env 块。CARGO_BUILD_JOBS=1 是铁律兜底:make bce-check
# 内部的 `cargo run -p bao_lints` 无显式 --jobs 1,靠本变量保住单线程语义。
export CARGO_BUILD_JOBS=1
export CARGO_NET_RETRY=3
export RUSTUP_MAX_RETRIES=3

FMT_RC=0
CHECK_RC=0
CLIPPY_RC=0
BCE_RC=0

# verdict <rc> → 输出 PASS / FAIL
verdict() {
    if [ "$1" -eq 0 ]; then echo PASS; else echo FAIL; fi
}

# ============================ [1/4] fmt(非阻断) ============================
# rustfmt --check 的 stdout 是全量 diff(历史债务 ~8000 处),直接透出会淹没
# 终端:落临时文件收敛,只报退出码 + 行数;短输出(≤50 行,通常是真实报错
# 而非格式债务)原样回显便于诊断。失败且 diff 巨大时保留临时文件供排查。
echo "==> [1/4] fmt (non-blocking): cargo fmt -- --check"
FMT_TMP="$(mktemp /tmp/bao-local-ci-fmt.XXXXXX)"
cargo fmt -- --check >"${FMT_TMP}" 2>&1
FMT_RC=$?
FMT_LINES="$(wc -l <"${FMT_TMP}")"
if [ "${FMT_RC}" -eq 0 ]; then
    rm -f "${FMT_TMP}"
    echo "PASS fmt"
elif [ "${FMT_LINES}" -le 50 ]; then
    cat "${FMT_TMP}"
    rm -f "${FMT_TMP}"
    echo "FAIL fmt (exit=${FMT_RC}, diff_lines=${FMT_LINES})"
else
    echo "FAIL fmt (exit=${FMT_RC}, diff_lines=${FMT_LINES}, 全量 diff 留存于 ${FMT_TMP})"
fi

# ============================ [2/4] check(阻断) ============================
# 镜像原 CI job 内 step 语义:按序执行,首败即 job 败、后续 step 跳过
# (子 shell 内 set -e 实现短路;外层无 set -e,job 间独立容错继续)。
echo "==> [2/4] check (blocking)"
(
    set -e
    echo '$ ./scripts/ensure-codegen.sh'
    ./scripts/ensure-codegen.sh
    echo '$ cargo check --jobs 1 -p bao_bin'
    cargo check --jobs 1 -p bao_bin
    echo '$ cargo check --jobs 1 -p bao-browser'
    cargo check --jobs 1 -p bao-browser
)
CHECK_RC=$?
if [ "${CHECK_RC}" -eq 0 ]; then echo "PASS check"; else echo "FAIL check (exit=${CHECK_RC})"; fi

# ============================ [3/4] clippy(阻断) ============================
echo "==> [3/4] clippy (blocking)"
(
    set -e
    echo '$ ./scripts/ensure-codegen.sh'
    ./scripts/ensure-codegen.sh
    echo '$ cargo clippy --jobs 1 -p bao_bin --no-deps --bins'
    cargo clippy --jobs 1 -p bao_bin --no-deps --bins
)
CLIPPY_RC=$?
if [ "${CLIPPY_RC}" -eq 0 ]; then echo "PASS clippy"; else echo "FAIL clippy (exit=${CLIPPY_RC})"; fi

# ============================ [4/4] bce-gate(阻断) ============================
echo "==> [4/4] bce-gate (blocking): make bce-check"
make bce-check
BCE_RC=$?
if [ "${BCE_RC}" -eq 0 ]; then echo "PASS bce-gate"; else echo "FAIL bce-gate (exit=${BCE_RC})"; fi

# ================================ 总表 ================================
echo
echo "==================== LOCAL-CI SUMMARY ===================="
echo "$(verdict "${FMT_RC}")     fmt       (non-blocking)"
echo "$(verdict "${CHECK_RC}")    check     (blocking)"
echo "$(verdict "${CLIPPY_RC}")   clippy    (blocking)"
echo "$(verdict "${BCE_RC}")      bce-gate  (blocking)"
echo "========================================================="

OVERALL_RC=0
if [ "${CHECK_RC}" -ne 0 ] || [ "${CLIPPY_RC}" -ne 0 ] || [ "${BCE_RC}" -ne 0 ]; then
    OVERALL_RC=1
fi

if [ "${OVERALL_RC}" -eq 0 ]; then
    echo "LOCAL-CI: PASS"
else
    echo "LOCAL-CI: FAIL"
fi
exit "${OVERALL_RC}"
