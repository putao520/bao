#!/usr/bin/env python3
"""ev4 vendor 四宇宙归一 — 机械内联变换脚本。

把一个 vendor workspace 根([workspace] 虚拟 manifest)的
workspace.package / workspace.dependencies 继承,逐字面值内联进
树内每个成员 manifest,使整棵树在根删除后自洽(fail-closed:
遇到不可识别形态立即报错退出,绝不部分改写)。

语义规则(cargo workspace inheritance 的忠实展开):
  - package 字段: `field.workspace = true` -> `field = <根值字面>`
  - 依赖: 根值 base(member 可叠加 override):
      features        = base.features ∪ override.features
      default-features= override 有则用 override,否则 base
      optional        = override 有则用 override
      package/path/version = base(member 禁改,出现即 fail)
  - base 带 path 时按消费方 manifest 所在目录重算相对路径(绝对目标不变)。

幂等: 第二次运行零 diff(全部继承键已消失,自然收敛)。
默认 --dry-run 只输出 unified diff;--apply 才写盘(只写 --tree 指定的树)。
"""
from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import sys
import tomllib
from pathlib import Path

HDR = re.compile(r"^\[(.+)\]$")
DEP_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
OVERRIDE_KEYS = {"features", "default-features", "optional"}
FIELD_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_-]*)\.workspace[ \t]*=[ \t]*true[ \t]*(#.*)?$")
KEY_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_.-]*)[ \t]*=[ \t]*(.+?)[ \t]*(#.*)?$")


def render(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, str):
        return json.dumps(v)
    if isinstance(v, list):
        return "[" + ", ".join(render(x) for x in v) + "]"
    if isinstance(v, dict):
        return "{" + ", ".join(f"{k} = {render(x)}" for k, x in v.items()) + "}"
    raise TypeError(f"unrenderable {v!r}")


def render_dep(key: str, base, override: dict, consumer_dir: Path, root_dir: Path) -> str:
    if isinstance(base, str):
        merged: dict = {"version": base}
    elif isinstance(base, dict):
        merged = dict(base)
    else:
        raise SystemExit(f"FAIL: {key}: unsupported root value {base!r}")
    bad = set(override) - OVERRIDE_KEYS
    if bad:
        raise SystemExit(f"FAIL: {key}: override keys outside closed set {sorted(bad)}")
    if "features" in override:
        feats = list(merged.get("features", []))
        for f in override["features"]:
            if f not in feats:
                feats.append(f)
        merged["features"] = feats
    if "default-features" in override:
        merged["default-features"] = override["default-features"]
    if "optional" in override:
        merged["optional"] = override["optional"]
    if "path" in merged:
        target = (root_dir / merged["path"]).resolve()
        merged["path"] = os.path.relpath(target, consumer_dir).replace(os.sep, "/")
    order = ["package", "path", "version", "default-features", "features", "optional"]
    body = ", ".join(f"{k} = {render(merged[k])}" for k in order if k in merged)
    extra = [k for k in merged if k not in order]
    if extra:
        body += ", " + ", ".join(f"{k} = {render(merged[k])}" for k in extra)
    return f"{key} = {{{body}}}"


def _brace_depth(s: str) -> int:
    depth, in_str = 0, False
    for ch in s:
        if ch == '"':
            in_str = not in_str
        elif not in_str and ch in "{":
            depth += 1
        elif not in_str and ch in "}":
            depth -= 1
    return depth


def transform_manifest(text: str, mf_path: Path, root_dir: Path,
                       wpkg: dict, wdeps: dict) -> str:
    out_lines: list[str] = []
    section = ""
    lines = text.splitlines(keepends=True)
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.rstrip("\n")
        m = HDR.match(stripped.strip())
        if m:
            section = m.group(1)
            out_lines.append(line)
            i += 1
            continue
        is_pkg = section == "package"
        sec_tail = section.rsplit(".", 1)[-1] if "." in section else section
        is_dep = sec_tail in DEP_SECTIONS

        if is_pkg:
            fm = FIELD_RE.match(stripped)
            if fm:
                field = fm.group(1)
                if field not in wpkg:
                    raise SystemExit(f"FAIL {mf_path}: package field `{field}` not in workspace.package")
                comment = f"  {fm.group(2)}" if fm.group(2) else ""
                out_lines.append(f"{field} = {render(wpkg[field])}{comment}\n")
                i += 1
                continue
        if is_dep:
            dm = FIELD_RE.match(stripped)
            if dm:  # dotted 依赖简写(防御性;当前树中不存在)
                key = dm.group(1)
                if key not in wdeps:
                    raise SystemExit(f"FAIL {mf_path}: dep key `{key}` not in workspace.dependencies")
                out_lines.append(render_dep(key, wdeps[key], {}, mf_path.parent, root_dir) + "\n")
                i += 1
                continue
            km = KEY_RE.match(stripped)
            if km and km.group(2).startswith("{"):
                # 聚合多行 inline table(字符串感知的括号深度)
                start = i
                depth = _brace_depth(stripped)
                while depth > 0:
                    i += 1
                    if i >= len(lines):
                        raise SystemExit(f"FAIL {mf_path}:{start+1}: 未闭合的 inline table")
                    piece = lines[i].rstrip("\n")
                    if "#" in piece:
                        raise SystemExit(f"FAIL {mf_path}:{i+1}: 多行记录内含注释,拒绝静默丢弃")
                    depth += _brace_depth(piece)
                    stripped += " " + piece.strip()
                    line = line.rstrip("\n") + " " + piece.strip() + "\n"
                km = KEY_RE.match(stripped)
                if km and "workspace = true" in km.group(2):
                    key, inline = km.group(1), km.group(2)
                    comment = f"  {km.group(3)}" if km.group(3) else ""
                    try:
                        parsed = tomllib.loads(f"x = {inline}")["x"]
                    except tomllib.TOMLDecodeError as e:
                        raise SystemExit(f"FAIL {mf_path}: cannot parse inline table for `{key}`: {e}")
                    if parsed.get("workspace") is not True:
                        out_lines.append(line)
                        i += 1
                        continue
                    if key not in wdeps:
                        raise SystemExit(f"FAIL {mf_path}: dep key `{key}` not in workspace.dependencies")
                    override = {k: v for k, v in parsed.items() if k != "workspace"}
                    out_lines.append(render_dep(key, wdeps[key], override, mf_path.parent, root_dir) + comment + "\n")
                    i += 1
                    continue
        out_lines.append(line)
        i += 1
    return "".join(out_lines)


def sanity_no_inheritance_left(text: str, mf_path: Path) -> None:
    for i, line in enumerate(text.splitlines(), 1):
        if re.search(r"(?:^|[,{\[ ])\s*workspace\s*=\s*true|\w[\w-]*\.workspace\s*=", line):
            raise SystemExit(f"FAIL {mf_path}:{i}: inheritance survived: {line.strip()}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default="/home/putao/code/rust/bao")
    ap.add_argument("--tree", action="append",
                    default=["vendor/servo"],
                    help="vendor workspace 树(可多次;stylo/ipc-channel/freetype-wrapper 无继承,无需传)")
    ap.add_argument("--apply", action="store_true", help="写盘;默认 dry-run 输出 diff")
    ap.add_argument("--no-diff", action="store_true", help="dry-run 只打摘要不打 diff")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()

    for tree in args.tree:
        root_dir = repo / tree
        root_mf = root_dir / "Cargo.toml"
        raw = root_mf.read_text()
        data = tomllib.loads(raw)
        ws = data.get("workspace", {})
        if not ws:
            print(f"SKIP {tree}: 无 [workspace] 根,无需内联")
            continue
        wpkg, wdeps = ws.get("package", {}), ws.get("dependencies", {})
        changed = skipped = 0
        for mf in sorted(root_dir.rglob("Cargo.toml")):
            rel = mf.relative_to(repo)
            sp = str(rel)
            if "/target/" in f"/{sp}/" or "/.venv/" in f"/{sp}/":
                continue
            if mf == root_mf:
                continue
            text = mf.read_text()
            if "workspace = true" not in text and ".workspace" not in text:
                skipped += 1
                continue
            # 防多行 inline table:结构计数 vs 行计数必须一致
            n_line = len(re.findall(r"workspace = true", text))
            d = tomllib.loads(text)

            def count(o):
                c = 0
                for v in o.values() if isinstance(o, dict) else []:
                    if isinstance(v, dict):
                        if v.get("workspace") is True:
                            c += 1
                        c += count(v)
                return c
            if n_line != count(d):
                raise SystemExit(f"FAIL {rel}: 多行/异常 workspace 继承形态,拒绝机械改写")
            new = transform_manifest(text, mf, root_dir, wpkg, wdeps)
            sanity_no_inheritance_left(new, rel)
            if new != text:
                changed += 1
                if args.apply:
                    mf.write_text(new)
                elif not args.no_diff:
                    sys.stdout.writelines(difflib.unified_diff(
                        text.splitlines(keepends=True), new.splitlines(keepends=True),
                        fromfile=f"a/{rel}", tofile=f"b/{rel}", n=1))
        print(f"[{tree}] manifests rewritten: {changed}, no-inheritance skipped: {skipped}"
              f"{' (APPLIED)' if args.apply else ' (dry-run)'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
