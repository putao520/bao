#!/usr/bin/env python3
"""SM-EVOLUTION #30 / UpstreamAudit ③Bao 使用映射(grep src/ → 两分类初判)

输入 inventory JSON,对每个 API symbol 在 Bao src/ 树做词边界使用计数
(单遍 tokenize corpus,等效 `command grep -rw` 且免 1300 次进程扫描),
输出 adoption JSON + 人类可读 adoption report:

  bao_status(初判两分类,#30 契约 v1):
    native-used            使用计数 > 0
    unused-pending-verdict 使用计数 = 0(裁决仍归人工账本
                           .claude/sm-capability-ledger.json,本脚本不裁定)
  usage_confidence(诚实度标记,防误报):
    high   名字带 JS_/js_ 前缀或命中 mozjs wrapper 面(名字即 API 面,误报低)
    medium 命中文件 import/提及 mozjs|bun_sm(上下文佐证)
    low    仅通用名命中且文件无 SM 上下文(如 EnterRealm/Evaluate 类短名,
           可能是无关标识符——人工裁决前必看 usage_files)

扫描面 = src/**/*.rs 全树(含 bun_* vendored crate 与 tests,与 S0-A census
同口径)。vendor/ 不扫(servo 上游代码非 Bao 采用面)。

用法:
    python3 scripts/sm-audit/usage_map.py [--repo PATH] [--inventory PATH]
        [--out-dir DIR]
    默认 inventory=.claude/sm-audit/ 下最新 inventory-*.json。

输出:
    <out>/adoption-<inventory label>.json
    <out>/adoption-report-<inventory label>.md
"""

import argparse
import json
import os
import re
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sm_audit_common import find_repo_root, now_iso  # noqa: E402

IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
SM_CONTEXT_HINT = re.compile(r"mozjs|bun_sm|bao_engine")


def latest_inventory(out_dir: str) -> str:
    cands = sorted(
        (f for f in os.listdir(out_dir)
         if f.startswith("inventory-") and f.endswith(".json")),
        key=lambda f: os.path.getmtime(os.path.join(out_dir, f)))
    if not cands:
        raise SystemExit(f"usage_map: no inventory-*.json under {out_dir}; "
                         "run extract_inventory.py first")
    return os.path.join(out_dir, cands[-1])


def build_corpus(repo: str):
    """src/**/*.rs 单遍 tokenize → {relpath: Counter} + 文件 SM 上下文标记。"""
    per_file = {}
    sm_ctx = {}
    src_root = os.path.join(repo, "src")
    for dirpath, _dirnames, filenames in os.walk(src_root):
        for fn in filenames:
            if not fn.endswith(".rs"):
                continue
            p = os.path.join(dirpath, fn)
            rel = os.path.relpath(p, repo)
            try:
                with open(p, "r", encoding="utf-8", errors="replace") as f:
                    text = f.read()
            except OSError as e:
                print(f"usage_map: WARN skip unreadable {rel}: {e}", file=sys.stderr)
                continue
            per_file[rel] = Counter(IDENT.findall(text))
            sm_ctx[rel] = bool(SM_CONTEXT_HINT.search(text))
    return per_file, sm_ctx


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--repo", default=find_repo_root())
    ap.add_argument("--inventory", default=None)
    ap.add_argument("--out-dir", default=None)
    args = ap.parse_args()

    repo = args.repo
    out_dir = args.out_dir or os.path.join(repo, ".claude", "sm-audit")
    inv_path = args.inventory or latest_inventory(out_dir)
    with open(inv_path, "r", encoding="utf-8") as f:
        inv = json.load(f)
    label = inv["label"]

    per_file, sm_ctx = build_corpus(repo)

    wrapper_bound = {
        e["symbol"] for e in inv["symbols"]
        if e["binding"].startswith("safe-wrapper")
    }
    rows = []
    for e in inv["symbols"]:
        name = e["symbol"]
        hits = [(rel, cnt[name]) for rel, cnt in per_file.items() if cnt.get(name)]
        hits.sort(key=lambda t: (-t[1], t[0]))
        count = sum(c for _, c in hits)
        status = "native-used" if count > 0 else "unused-pending-verdict"
        if count == 0:
            confidence = "n/a"
        elif name.startswith(("JS_", "js_")) or name in wrapper_bound:
            confidence = "high"
        elif any(sm_ctx.get(rel) for rel, _ in hits):
            confidence = "medium"
        else:
            confidence = "low"
        rows.append({
            "symbol": name,
            "rust_path": e["rust_path"],
            "category_guess": e["category_guess"],
            "surface": e["surface"],
            "stability_guess": e.get("stability_guess"),
            "binding": e["binding"],
            "usage_count": count,
            "usage_files": [rel for rel, _ in hits[:8]],
            "usage_confidence": confidence,
            "bao_status": status,
        })

    def rollup(key_field, status_field="bao_status"):
        out = {}
        for r in rows:
            k = r[key_field]
            bucket = out.setdefault(k, {"total": 0, "native-used": 0,
                                        "unused-pending-verdict": 0})
            bucket["total"] += 1
            bucket[r[status_field]] += 1
        return {k: out[k] for k in sorted(out)}

    summary = {
        "inventory": os.path.basename(inv_path),
        "inventory_label": label,
        "bao_commit": inv["bao_commit"],
        "src_files_scanned": len(per_file),
        "symbols": len(rows),
        "native-used": sum(1 for r in rows if r["bao_status"] == "native-used"),
        "unused-pending-verdict": sum(
            1 for r in rows if r["bao_status"] == "unused-pending-verdict"),
        "low_confidence_used": sum(
            1 for r in rows if r["usage_confidence"] == "low"),
        "by_category": rollup("category_guess"),
    }

    adoption = {
        "schema_version": 1,
        "tool": "scripts/sm-audit/usage_map.py",
        "generated": now_iso(),
        **summary,
        "api": sorted(rows, key=lambda r: (-r["usage_count"], r["symbol"])),
    }
    json_path = os.path.join(out_dir, f"adoption-{label}.json")
    with open(json_path, "w", encoding="utf-8") as f:
        json.dump(adoption, f, ensure_ascii=False, indent=1)
        f.write("\n")

    md_path = write_report(out_dir, label, adoption)
    print(f"OK {json_path}")
    print(f"OK {md_path}")
    print(f"  native-used={summary['native-used']} "
          f"unused-pending-verdict={summary['unused-pending-verdict']} "
          f"(low-confidence used={summary['low_confidence_used']})")


def write_report(out_dir: str, label: str, adoption: dict) -> str:
    """人类可读 adoption report 样张(#30 报告形态,非 capability 裁决)。"""
    s = adoption
    L = []
    L.append(f"# SM capability adoption report — {label}")
    L.append("")
    L.append(f"- inventory: `{s['inventory']}`(bao `{s['bao_commit'][:12]}`)")
    L.append(f"- 扫描面: src/**/*.rs 全树({s['src_files_scanned']} 文件);"
             f"symbols={s['symbols']}")
    L.append(f"- 初判两分类: **native-used = {s['native-used']}** / "
             f"**unused-pending-verdict = {s['unused-pending-verdict']}**")
    L.append(f"- low-confidence used(通用名误报风险,裁决前必看 usage_files): "
             f"{s['low_confidence_used']}")
    L.append("")
    L.append("> 本报告是 symbol 级事实映射(#30 v1)。capability 裁决"
             "(used-native/wrapped/emulated/missing/deliberately-unused)仍由"
             "人工账本 `.claude/sm-capability-ledger.json` 持有;"
             "unused-pending-verdict ≠ 该弃用。")
    L.append("")
    L.append("## 按域汇总")
    L.append("")
    L.append("| category_guess | total | native-used | unused-pending-verdict |")
    L.append("|---|---:|---:|---:|")
    for cat, b in s["by_category"].items():
        L.append(f"| {cat} | {b['total']} | {b['native-used']} | "
                 f"{b['unused-pending-verdict']} |")
    L.append("")
    L.append("## Top 40 使用")
    L.append("")
    L.append("| symbol | category | binding | count | conf |")
    L.append("|---|---|---|---:|---|")
    for r in s["api"][:40]:
        if r["usage_count"] == 0:
            break
        L.append(f"| `{r['symbol']}` | {r['category_guess']} | {r['binding']} | "
                 f"{r['usage_count']} | {r['usage_confidence']} |")
    L.append("")
    L.append("## 已绑定但 Bao 零使用的 safe-wrapper(adoption-gap 候选,供 #30 裁决)")
    L.append("")
    gap = [r for r in s["api"]
           if r["usage_count"] == 0 and r["binding"].startswith("safe-wrapper")]
    L.append(f"共 {len(gap)} 项;样例(按域聚合后全表见 adoption JSON):")
    L.append("")
    L.append("| symbol | category | stability |")
    L.append("|---|---|---|")
    for r in gap[:25]:
        L.append(f"| `{r['symbol']}` | {r['category_guess']} | "
                 f"{r['stability_guess']} |")
    L.append("")
    md_path = os.path.join(out_dir, f"adoption-report-{label}.md")
    with open(md_path, "w", encoding="utf-8") as f:
        f.write("\n".join(L) + "\n")
    return md_path


if __name__ == "__main__":
    main()
