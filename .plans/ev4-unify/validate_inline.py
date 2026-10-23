#!/usr/bin/env python3
"""ev4 内联变换独立验证器(实现轮 --apply 之后必跑)。

与 transformer 不同代码路径:从"正本根 manifest + 正本成员 manifest"重算每个
继承点的期望字面值,与变换后 manifest 逐点对照。任何一处不等即非零退出。

用法:
  python3 validate_inline.py --origin <干净树>/vendor/servo --transformed <变换树>/vendor/servo

  - 干净树: 按 §10 worktree 律取 `git worktree add <tmp> HEAD` 的检出
    (或 rehearsal 布局的正本副本)。
  - 变换树: inline_vendor_workspace.py --apply 后的工作树。

验收判据: `checked N inheritance sites: ERR=0` 且 `unchanged manifests: M`(应=4:
根 + crown + python/tidy 两夹具)。
"""
from __future__ import annotations

import argparse
import os
import tomllib
from pathlib import Path

DEP_SECS = ("dependencies", "dev-dependencies", "build-dependencies")


def expect_dep(base, override):
    m = {"version": base} if isinstance(base, str) else dict(base)
    feats = list(m.get("features", []))
    for f in override.get("features", []):
        if f not in feats:
            feats.append(f)
    if "features" in override:
        m["features"] = feats
    if "default-features" in override:
        m["default-features"] = override["default-features"]
    if "optional" in override:
        m["optional"] = override["optional"]
    return m


def rebase(exp: dict, copy_root: Path, mf: Path) -> dict:
    if "path" in exp:
        exp = dict(exp)
        target = (copy_root / exp["path"]).resolve()
        exp["path"] = os.path.relpath(target, mf.parent)
    return exp


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--origin", required=True, help="干净(未变换)vendor/servo 目录")
    ap.add_argument("--transformed", required=True, help="变换后 vendor/servo 目录")
    args = ap.parse_args()
    origin, transformed = Path(args.origin).resolve(), Path(args.transformed).resolve()
    root = tomllib.loads((origin / "Cargo.toml").read_text())
    ws = root.get("workspace", {})
    wpkg, wdeps = ws.get("package", {}), ws.get("dependencies", {})

    errs = ok = unchanged = 0
    for mf in sorted(transformed.rglob("Cargo.toml")):
        rel = mf.relative_to(transformed)
        o_mf = origin / rel
        if not o_mf.exists():
            print(f"ERR {rel}: 变换树多出 manifest(禁止)"); errs += 1; continue
        orig = tomllib.loads(o_mf.read_text())
        new = tomllib.loads(mf.read_text())
        if orig == new:
            unchanged += 1
            continue

        def has_ws(d):
            for v in d.values() if isinstance(d, dict) else []:
                if isinstance(v, dict):
                    if v.get("workspace") is True:
                        return True
                    if has_ws(v):
                        return True
            return False

        ws_left = has_ws(new.get("package", {})) or has_ws(new.get("dependencies", {})) \
            or has_ws(new.get("dev-dependencies", {})) or has_ws(new.get("build-dependencies", {})) \
            or any(has_ws(t.get(s, {})) for t in new.get("target", {}).values() for s in DEP_SECS)
        if ws_left:
            print(f"ERR {rel}: 继承残留"); errs += 1

        for sec in DEP_SECS:
            osec, nsec = orig.get(sec, {}), new.get(sec, {})
            if set(osec) != set(nsec):
                print(f"ERR {rel}:{sec}: key 集不同 {set(osec) ^ set(nsec)}"); errs += 1; continue
            for k in osec:
                ov = osec[k]
                if isinstance(ov, dict) and ov.get("workspace") is True:
                    ok += 1
                    exp = rebase(expect_dep(wdeps[k], {a: b for a, b in ov.items() if a != "workspace"}),
                                 transformed, mf)
                    if nsec[k] != exp:
                        print(f"ERR {rel}:{sec}:{k}:\n  exp {exp}\n  got {nsec[k]}"); errs += 1
                elif ov != nsec[k]:
                    print(f"ERR {rel}:{sec}:{k}: 非继承键被改动"); errs += 1
        for tn, ts in orig.get("target", {}).items():
            for sec in DEP_SECS:
                osec = ts.get(sec, {})
                nsec = new.get("target", {}).get(tn, {}).get(sec, {})
                for k, ov in osec.items():
                    if isinstance(ov, dict) and ov.get("workspace") is True:
                        ok += 1
                        exp = rebase(expect_dep(wdeps[k], {a: b for a, b in ov.items() if a != "workspace"}),
                                     transformed, mf)
                        if nsec.get(k) != exp:
                            print(f"ERR {rel}:target[{tn}]:{sec}:{k}:\n  exp {exp}\n  got {nsec.get(k)}"); errs += 1
        op, npkg = orig.get("package", {}), new.get("package", {})
        for k in set(op) | set(npkg):
            ov = op.get(k)
            if isinstance(ov, dict) and ov.get("workspace") is True:
                ok += 1
                if npkg.get(k) != wpkg[k]:
                    print(f"ERR {rel}:package.{k}: exp {wpkg[k]!r} got {npkg.get(k)!r}"); errs += 1
            elif op.get(k) != npkg.get(k):
                print(f"ERR {rel}:package.{k}: 非继承字段被改动"); errs += 1

    print(f"\nchecked {ok} inheritance sites: ERR={errs}; unchanged manifests: {unchanged}")
    return 1 if errs else 0


if __name__ == "__main__":
    raise SystemExit(main())
