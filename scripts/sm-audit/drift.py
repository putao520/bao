#!/usr/bin/env python3
"""SM-EVOLUTION #30 / UpstreamAudit ②drift detector(两版 inventory diff)

对比两份 extract_inventory.py 产物,回答「SM 新增/删除/改变了哪些 embedding
capability?」(#30):

  三分类(主分类,任务合同口径):
    added    新版有、旧版无(按 rust_path+symbol 键)
    removed  旧版有、新版无
    renamed  removed 与 added 同命名空间且名字相似度 ≥0.75 → 配对
             (old → new;C++ 改名后 mangled link_name 同步变化,故以名字+命名
              空间配对,不猜 mangled)
  追加检测(服务于 #30「改变了哪些」目标,不计入三分类 headline):
    signature_changed 同名同键但归一化签名 hash 变化(参数类型/返回值漂移)
    stability_flip    experimental→public / public→experimental / deprecated
                      翻转
  归并(#30 禁令:不得只 diff bindgen 巨量输出):全部变更按 category_guess
  域级聚合输出 rollup;symbol 级明细保留供人工裁决。

  Bao 使用标注:旧版同名 adoption JSON(--old-adoption,默认按 label 同目录
  自动发现)存在时,removed/signature_changed 标注 bao_usage_count——非零即
  升级波阻断候选(daily-ops 消费点)。

用法:
    python3 scripts/sm-audit/drift.py OLD.json NEW.json [--out FILE]
        [--old-adoption ADOPTION.json]
    自检:OLD==NEW 时三分类必须全零(基线无漂移)。

输出:JSON 报告(--out 缺省打印 stdout)+ stdout 摘要(三分类计数 + 域 rollup)。
"""

import argparse
import glob
import json
import os
import sys
from difflib import SequenceMatcher

RENAME_THRESHOLD = 0.75


def load(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def index(inv):
    return {e["key"]: e for e in inv["symbols"]}


def pair_renames(removed, added, old_idx, new_idx):
    """同 rust_path 内贪心配对相似名(≥阈值)→ [(old_key, new_key, ratio)]。"""
    pairs = []
    rem = sorted(removed)
    add = sorted(added)
    for rk in rem:
        old = old_idx[rk]
        best, best_ratio = None, 0.0
        for ak in add:
            new = new_idx[ak]
            if new["rust_path"] != old["rust_path"]:
                continue
            ratio = SequenceMatcher(
                None, old["symbol"].lower(), new["symbol"].lower()).ratio()
            if ratio > best_ratio:
                best, best_ratio = ak, ratio
        if best is not None and best_ratio >= RENAME_THRESHOLD:
            pairs.append((rk, best, round(best_ratio, 3)))
            add.remove(best)
    renamed_old = {p[0] for p in pairs}
    renamed_new = {p[1] for p in pairs}
    return (pairs,
            [k for k in rem if k not in renamed_old],
            [k for k in add if k not in renamed_new])


def rollup(entries):
    out = {}
    for e in entries:
        out[e["category_guess"]] = out.get(e["category_guess"], 0) + 1
    return dict(sorted(out.items(), key=lambda kv: -kv[1]))


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--out", default=None)
    ap.add_argument("--old-adoption", default=None,
                    help="旧版 adoption JSON(bao 使用标注;默认按 label 自动发现)")
    args = ap.parse_args()

    old_inv, new_inv = load(args.old), load(args.new)
    old_idx, new_idx = index(old_inv), index(new_inv)
    old_keys, new_keys = set(old_idx), set(new_idx)

    removed_raw = sorted(old_keys - new_keys)
    added_raw = sorted(new_keys - old_keys)
    pairs, removed, added = pair_renames(
        removed_raw, added_raw, old_idx, new_idx)

    signature_changed = [
        k for k in sorted(old_keys & new_keys)
        if old_idx[k]["signature_hash"] != new_idx[k]["signature_hash"]
    ]
    stability_flip = [
        (k, old_idx[k].get("stability_guess"), new_idx[k].get("stability_guess"))
        for k in sorted(old_keys & new_keys)
        if old_idx[k].get("stability_guess") != new_idx[k].get("stability_guess")
    ]

    # Bao 使用标注(阻断候选信号)
    adoption_counts = {}
    if args.old_adoption:
        adoption_counts = {
            r["symbol"]: r["usage_count"]
            for r in load(args.old_adoption)["api"]}
    elif old_inv["label"]:
        cand = glob.glob(os.path.join(
            os.path.dirname(os.path.abspath(args.old)),
            f"adoption-{old_inv['label']}.json"))
        if cand:
            adoption_counts = {
                r["symbol"]: r["usage_count"] for r in load(cand[0])["api"]}

    def bao_used(key):
        return adoption_counts.get(key.rsplit("::", 1)[-1], 0)

    report = {
        "schema_version": 1,
        "tool": "scripts/sm-audit/drift.py",
        "old": {
            "label": old_inv["label"], "bao_commit": old_inv["bao_commit"],
            "mozjs": {k: old_inv["mozjs"][k] for k in ("crate", "sys_crate",
                                                      "jsapi_md5")},
        },
        "new": {
            "label": new_inv["label"], "bao_commit": new_inv["bao_commit"],
            "mozjs": {k: new_inv["mozjs"][k] for k in ("crate", "sys_crate",
                                                      "jsapi_md5")},
        },
        "summary": {
            "added": len(added),
            "removed": len(removed),
            "renamed": len(pairs),
            "signature_changed": len(signature_changed),
            "stability_flip": len(stability_flip),
            "removed_bao_used": sum(1 for k in removed if bao_used(k) > 0),
        },
        "added": [
            {"key": k, **{f: new_idx[k][f] for f in
                         ("rust_path", "symbol", "category_guess",
                          "stability_guess", "binding", "signature")}}
            for k in added],
        "removed": [
            {"key": k, "bao_usage_count": bao_used(k),
             **{f: old_idx[k][f] for f in
                ("rust_path", "symbol", "category_guess", "stability_guess",
                 "binding")}}
            for k in removed],
        "renamed": [
            {"old_key": o, "new_key": n, "similarity": r,
             "old_symbol": old_idx[o]["symbol"], "new_symbol": new_idx[n]["symbol"],
             "rust_path": old_idx[o]["rust_path"],
             "category_guess": new_idx[n]["category_guess"],
             "old_bao_usage_count": bao_used(o)}
            for o, n, r in pairs],
        "signature_changed": [
            {"key": k, "bao_usage_count": bao_used(k),
             "old_signature": old_idx[k]["signature"],
             "new_signature": new_idx[k]["signature"],
             "category_guess": new_idx[k]["category_guess"]}
            for k in signature_changed],
        "stability_flip": [
            {"key": k, "old": o, "new": n} for k, o, n in stability_flip],
        "rollup_by_category": {
            "added": rollup([new_idx[k] for k in added]),
            "removed": rollup([old_idx[k] for k in removed]),
            "renamed": rollup([new_idx[n] for _, n, _ in pairs]),
            "signature_changed": rollup([new_idx[k] for k in signature_changed]),
        },
    }

    s = report["summary"]
    print(f"drift {report['old']['label']} -> {report['new']['label']}")
    print(f"  three-way: added={s['added']} removed={s['removed']} "
          f"renamed={s['renamed']}")
    print(f"  extra: signature_changed={s['signature_changed']} "
          f"stability_flip={s['stability_flip']} "
          f"removed_bao_used={s['removed_bao_used']} (升级波阻断候选)")
    for axis in ("added", "removed", "renamed", "signature_changed"):
        top = list(report["rollup_by_category"][axis].items())[:6]
        if top:
            print(f"  {axis} by domain: " +
                  ", ".join(f"{c}={c_}" for c, c_ in top))

    out = args.out
    if out:
        with open(out, "w", encoding="utf-8") as f:
            json.dump(report, f, ensure_ascii=False, indent=1)
            f.write("\n")
        print(f"OK {out}")

    # 自检:同基线对比必须零漂移
    if (old_inv["bao_commit"] == new_inv["bao_commit"]
            and old_inv["mozjs"]["jsapi_md5"] == new_inv["mozjs"]["jsapi_md5"]
            and (s["added"] or s["removed"] or s["renamed"]
                 or s["signature_changed"])):
        print("drift: SELF-CHECK FAILED — identical baseline must diff to zero",
              file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
