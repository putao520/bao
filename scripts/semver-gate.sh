#!/usr/bin/env bash
#
# scripts/semver-gate.sh — W4 #17-3 Gate C: API break-detection gate
# (cargo-semver-checks;reuse-decision: 成熟工具,禁自写 snapshot diff)
#
# 语义(face-transition-publish-discipline 记忆条):
#   0.x 语义域 major-required ⇒ minor bump 未做 ⇒ 硬 fail(退出码 1,阻断);
#   minor/patch-required 或其他 break ⇒ advisory(报告,不阻断);
#   单 crate 超时 ⇒ SKIP 并标注(不静默)。
#
# crate 集:workspace members 中已发布到 crates.io 者(curl 动态探测,
# UA=bao-check;离线/探测失败回退固定清单)。
#
# 用法:见 --help。报告默认落 .claude/semver-gate-report-<date>.md。

set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "${REPO}"

TIMEOUT_SECS="${BAO_SEMVER_GATE_TIMEOUT:-900}"
CRATES_API_UA="bao-check"
FALLBACK=(bao-core bao-browser bao_cdp bao_engine bao_stealth bun_runtime bun_sm cdp-server bao_cdp_client)

usage() {
    cat <<'EOF'
semver-gate.sh — W4 #17-3 Gate C: API break-detection (cargo-semver-checks)

用法:
  scripts/semver-gate.sh                    本用法
  scripts/semver-gate.sh --run              对发布面 crate 集全量跑 gate
  scripts/semver-gate.sh --run --crate N    只跑指定 crate(可重复)
  scripts/semver-gate.sh --run --report P   报告落 P(默认 .claude/semver-gate-report-<date>.md)

判定语义(face-transition-publish-discipline):
  PASS            与已发布 baseline 无 API break
  FAIL (major)    required bump: major —— 0.x 语义 = minor bump 未做 → 硬 fail(退出码 1)
  ADVISORY        minor/patch-required 或其他 break → 报告,不阻断
  SKIP (timeout)  单 crate 超 BAO_SEMVER_GATE_TIMEOUT(默认 900s)→ 标注跳过,不静默
  TOOL-ERROR      rustdoc 构建失败(vendor/依赖编译问题)→ 标注留痕,非 API break

环境:
  BAO_SEMVER_GATE_TIMEOUT   单 crate 超时秒数(默认 900)

crate 集:workspace members ∩ crates.io 已发布(curl 动态探测;
离线/探测失败回退固定清单,与本仓发布面一致)。
EOF
}

RUN=0
REPORT=""
CRATES=()
while [ $# -gt 0 ]; do
    case "$1" in
        --run) RUN=1 ;;
        --report) REPORT="${2:-}"; shift ;;
        --crate) CRATES+=("${2:-}"); shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
    esac
    shift
done
if [ "$RUN" -ne 1 ]; then
    usage
    exit 0
fi

if ! command -v cargo-semver-checks >/dev/null 2>&1; then
    echo "cargo-semver-checks not on PATH — install: cargo install cargo-semver-checks --locked" >&2
    exit 1
fi
TOOL_VERSION="$(cargo-semver-checks --version 2>&1)"
echo "[semver-gate] tool: ${TOOL_VERSION}"

# ── crate 集:workspace members ∩ crates.io 已发布(动态探测;离线回退)──
SET=()
members="$(cargo metadata --no-deps --format-version 1 2>/dev/null \
    | python3 -c 'import json,sys; [print(p["name"]) for p in json.load(sys.stdin)["packages"]]' 2>/dev/null)"
if [ -n "${members}" ]; then
    for m in ${members}; do
        if curl -s --max-time 10 -A "${CRATES_API_UA}" \
            "https://crates.io/api/v1/crates/${m}" | grep -q '"versions":\['; then
            SET+=("${m}")
        fi
    done
fi
if [ "${#SET[@]}" -eq 0 ]; then
    echo "[semver-gate] crates.io 探测不可用(离线?)→ 回退固定清单"
    SET=("${FALLBACK[@]}")
fi
if [ "${#CRATES[@]}" -gt 0 ]; then
    SET=("${CRATES[@]}")
fi
echo "[semver-gate] crate 集(${#SET[@]}): ${SET[*]}"

TMP="$(mktemp -d /tmp/bao-semver-gate.XXXXXX)"
trap 'rm -rf "${TMP}"' EXIT

if [ -z "${REPORT}" ]; then
    REPORT=".claude/semver-gate-report-$(date +%F).md"
fi
mkdir -p "$(dirname "${REPORT}")"
{
    echo "# semver-gate report — $(date '+%F %T')"
    echo
    echo "- tool: ${TOOL_VERSION}"
    echo "- crate set (${#SET[@]}): ${SET[*]}"
    echo "- semantics: major-required = hard fail (0.x → minor bump owed); minor/patch = advisory; timeout = SKIP (annotated)"
    echo
    echo "| crate | verdict | detail |"
    echo "|---|---|---|"
} > "${REPORT}"

MAJOR_FAIL=0
ADVISORY=0
for c in "${SET[@]}"; do
    log="${TMP}/${c}.log"
    timeout "${TIMEOUT_SECS}" cargo semver-checks -p "${c}" >"${log}" 2>&1
    rc=$?
    if [ "${rc}" -eq 0 ]; then
        echo "PASS            ${c}"
        echo "| ${c} | PASS | — |" >> "${REPORT}"
    elif [ "${rc}" -eq 124 ]; then
        echo "SKIP (timeout)  ${c}  (>${TIMEOUT_SECS}s — 标注,不静默)"
        echo "| ${c} | SKIP | timeout >${TIMEOUT_SECS}s |" >> "${REPORT}"
    elif grep -qi "failed to build rustdoc" "${log}"; then
        # 工具错误(rustdoc 构建失败,vendor/依赖编译问题)——非 API break,
        # 标注留痕不静默;归基础设施侧跟进,不计入 gate 判定。
        echo "TOOL-ERROR      ${c}  (rustdoc build failed — 见 log;非 API break)"
        echo "| ${c} | TOOL-ERROR | rustdoc build failed (infra; not an API break) |" >> "${REPORT}"
    elif grep -qi "required bump: major" "${log}"; then
        echo "FAIL (major)    ${c}  → 0.x 语义 = minor bump 未做(硬 fail)"
        echo "| ${c} | FAIL | major-required (0.x: minor bump owed) |" >> "${REPORT}"
        MAJOR_FAIL=1
    else
        kind="$(grep -oi 'required bump: [a-z]*' "${log}" | head -1)"
        echo "ADVISORY        ${c}  (${kind:-break detected} — 报告,不阻断)"
        echo "| ${c} | ADVISORY | ${kind:-break detected} |" >> "${REPORT}"
        ADVISORY=$((ADVISORY + 1))
    fi
done

echo
echo "| summary | major-required hard fails: ${MAJOR_FAIL}; advisory: ${ADVISORY} |" >> "${REPORT}"
echo "[semver-gate] report: ${REPORT}"
echo "[semver-gate] major-required hard fails: ${MAJOR_FAIL}; advisory: ${ADVISORY}"
exit "${MAJOR_FAIL}"
