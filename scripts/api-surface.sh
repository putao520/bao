#!/usr/bin/env bash
#
# scripts/api-surface.sh — W22a #17 stable-set gate (semver-gate.sh 同族脚本门;
# build-in-test 反模式规避——rustdoc 生成分钟级,不进 nextest)。
#
# 三档冻结语义(docs/api.md 为声明文档,docs/api-surface.json 为机械归档):
#   Stable       伞顶层精选再导出(40 项)+ deprecated BaoRuntime alias
#                —— break 由 semver-gate 阻断(major 语义),本门管「未声明新顶层 pub」。
#   Experimental 7 命名空间入口 + 7 透传 crate 全 pub 面 —— 变化=advisory 报告(minor changelog)。
#   Internal     #[doc(hidden)] pub 项(grep 基,rustdoc JSON 不序列化 hidden 标记)
#                —— 新增未登记 hidden = 红(每处须有 deliberate-hidden 理由)。
#
# semver-gate 联动:本门与 semver-gate.sh 互补——semver-gate 检「已有项的 break」,
# 本门检「新 pub 未入档」与「hidden 未登记」。Stable 档新增即红(消歧:要么显式
# 升 Stable 改归档,要么挪命名空间降 Experimental)。
#
# 用法:
#   scripts/api-surface.sh --check            门模式(默认):枚举+对照归档,红=1
#   scripts/api-surface.sh --update           重写归档 docs/api-surface.json(评审 diff 后提交)
#   scripts/api-surface.sh --help
#
# 环境依赖:nightly 工具链(rust-toolchain.toml 钉);RUSTDOCFLAGS 自动加
# --cap-lints allow(伞 doc 的 @trace [level:..] intra-doc link 在 -D warnings
# 下会 deny rustdoc——cap 掉属脚本面规避,非产品码改动)。
#
# 已知边界(如实):
#   - rustdoc JSON(format 60)不序列化 doc(hidden) 标记 → Internal 档走源码
#     grep(同 stop 预案:TOOL-ERROR 类降级源码枚举,如实标注基)。
#   - Experimental 计数变化为 advisory(设计 §1:minor changelog,不阻断)。

set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO}"

DOC_DIR="${BAO_API_SURFACE_DOC_DIR:-$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/doc}"
export BAO_API_SURFACE_DOC_DIR
export BAO_API_SURFACE_REPO

CRATES=(bao-core bao-browser bao_engine bun_runtime bao_cdp bao_cdp_client bao_stealth bao_uloop)

mode="--check"
case "${1:-}" in
    --check|--update|--help) mode="${1:-}" ;;
    "") mode="--check" ;;
    *) echo "unknown arg: $1 (use --check | --update | --help)"; exit 2 ;;
esac

if [ "$mode" = "--help" ]; then
    sed -n '2,30p' "$0"
    exit 0
fi

echo "[api-surface] rustdoc JSON generation (8 crates; deps are pre-built — ~1min)..."
export RUSTDOCFLAGS="--cap-lints allow"
for c in "${CRATES[@]}"; do
    if ! timeout "${BAO_API_SURFACE_TIMEOUT:-300}" cargo rustdoc -p "$c" --output-format json -Z unstable-options >/dev/null 2>&1; then
        echo "[api-surface] TOOL-ERROR: rustdoc JSON failed for $c (toolchain blocked?) — 留痕,非 API break" >&2
        exit 2
    fi
done

exec python3 scripts/api_surface_enum.py "$mode"
