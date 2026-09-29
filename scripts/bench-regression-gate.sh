#!/usr/bin/env bash
#
# scripts/bench-regression-gate.sh — W41 / #19-H regression gate
# (semver-gate / api-surface 同族脚本门;local-ci 可选段形态)。
#
# 语义:对 results/ 中**最新两个 run-date** 的同名 bench,取关键指标
# (scripts/bench_report.py 的 KEY_METRICS 同表——单一真源 import)做
# 方向感知对照: adverse Δ ≥ 阈值(默认 20%,BAO_BENCH_REGRESSION_PCT 可调)=红。
# soak 的 vm_rss_slope_steady(泄漏斜率)类=advisory-only(噪声主导,只报告)。
#
# 用法:
#   scripts/bench-regression-gate.sh            # 门(默认 20%)
#   scripts/bench-regression-gate.sh --pct 15   # 自定阈值
#   scripts/bench-regression-gate.sh --help
#
# 依赖:python3(零三方);同目录 bench_report.py 的 KEY_METRICS 表。

set -uo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
export BAO_SCRIPTS_DIR="$REPO/scripts"

PCT="${BAO_BENCH_REGRESSION_PCT:-20}"
[ "${1:-}" = "--pct" ] && { PCT="${2:-20}"; }
[ "${1:-}" = "--help" ] && { sed -n '2,16p' "$0"; exit 0; }

python3 - "$PCT" <<'PY_EOF'
import json, os, sys, glob

sys.path.insert(0, os.environ.get("BAO_SCRIPTS_DIR", "/home/putao/code/rust/bao/scripts"))
from bench_report import load_runs, KEY_METRICS  # single source: the key-metric table

pct = float(sys.argv[1])
runs = load_runs()
dates = sorted({d for d, _ in runs})
if len(dates) < 2:
    print("[bench-regression] SKIP: fewer than two run-dates archived")
    sys.exit(0)
cur_d, prev_d = dates[-1], dates[-2]

errors, advisories, compared = [], [], 0
for bench, keys in KEY_METRICS.items():
    if (cur_d, bench) not in runs or (prev_d, bench) not in runs:
        continue
    cur = runs[(cur_d, bench)][-1]
    prev = runs[(prev_d, bench)][-1]
    if cur.get("error") or prev.get("error"):
        errors.append(f"{bench}: a run on {'cur' if cur.get('error') else 'prev'} date is an ERROR document")
        continue
    cm = {m["name"]: m for m in cur.get("metrics", [])}
    pm = {m["name"]: m for m in prev.get("metrics", [])}
    for name, label, _unit in keys:
        c, p = cm.get(name), pm.get(name)
        if not c or not p:
            continue
        cv, pv = c.get("p50", c.get("mean")), p.get("p50", p.get("mean"))
        if not (isinstance(cv, (int, float)) and isinstance(pv, (int, float))) or pv == 0:
            continue
        compared += 1
        delta = (cv - pv) / pv * 100.0
        hib = c.get("higher_is_better")
        adverse = (delta < 0) if hib else (delta > 0)
        slope_class = "slope" in name or "rss" in name and "slope" in name
        if adverse and abs(delta) >= pct:
            if slope_class:
                advisories.append(f"{bench}/{name}: {pv:.2f}→{cv:.2f} ({delta:+.1f}%) — slope-class advisory")
            else:
                errors.append(f"{bench}/{name}: {pv:.2f}→{cv:.2f} ({delta:+.1f}%, {pct:.0f}% threshold, direction={'higher-better' if hib else 'lower-better'})")

print(f"[bench-regression] window: {prev_d} → {cur_d} (threshold {pct:.0f}%, direction-aware; slope-class advisory)")
print(f"[bench-regression] compared {compared} key-metric pairs")
for a in advisories:
    print(f"[bench-regression] NOTE {a}")
if errors:
    for e in errors:
        print(f"[bench-regression] RED {e}", file=sys.stderr)
    print(f"[bench-regression] GATE RED ({len(errors)} regression(s))", file=sys.stderr)
    sys.exit(1)
print("[bench-regression] GATE GREEN")
PY_EOF
