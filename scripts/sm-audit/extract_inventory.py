#!/usr/bin/env python3
"""SM-EVOLUTION #30 / UpstreamAudit ①capability extractor(可重复跑)

从 repo 现状抽取 SpiderMonkey embedding API 面,输出 version-bound JSON inventory:

  源 A(全量面): bindgen jsapi.rs(bao-mozjs-sys build out)的全部 extern "C" pub fn
      —— symbol / rust_path(命名空间)/ mangled link_name / 签名(类型归一化)/
         surface(public|friend|internal|support)/ category_guess / stability_guess
  源 B(已绑定面): vendor/mozjs/mozjs/src 安全层
      —— wrappers2(jsapi2_wrappers.in.rs,现行)> wrappers1(jsapi_wrappers.in.rs,
         deprecated)> mozjs-layer(rust.rs/realm.rs/context.rs/... 引用)> raw-only

输出绑定 commit hash(bao HEAD + dirty 标记)+ mozjs crate 版本 + jsapi md5,
作为 drift 基线(#30:下次 mozjs 前移跑 drift.py 对比)。

注意(与 #30 契约对齐):
  - 本 inventory 是 symbol 级 FACTS,不是 capability 裁决;capability 级状态
    (used-native/wrapped/emulated/missing/deliberately-unused)仍由人工账本
    .claude/sm-capability-ledger.json 持有。category_guess 仅启发式。
  - glue_*.in.rs 绑定 crate::glue(gluebindings.rs,mozjs 自产 glue 面,非 SM
    上游 API),不入 jsapi inventory,只在 summary 计数;jsapi.rs 内 root::glue
    命名空间(C++ glue)是另一套,保留在 inventory 里(surface=glue)。

自检(fail-closed,#30 stop 条件的前哨):
  - 解析出的 extern fn 数必须 == 文件内 `extern "C" {` 块数(bindgen 一 fn 一块);
  - 阳性对照:JS_NewContext(root)、EnterRealm(root::JS)、ReportOutOfMemory(root::js)
    必须在结果中(记忆教训:ugrep 桥假阴性——一切零命中结论须带阳性对照)。
  自检失败 = bindgen 结构漂移,退出码 2,降级路径见脚本尾部说明。

用法:
    python3 scripts/sm-audit/extract_inventory.py [--repo PATH] [--jsapi PATH]
        [--out-dir DIR] [--label LABEL]
    默认 out=.claude/sm-audit/,label=<YYYY-MM-DD>-<HEAD short8>。

输出:
    <out>/inventory-<label>.json
"""

import argparse
import hashlib
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sm_audit_common import (  # noqa: E402
    find_repo_root, git_head, mozjs_baseline, discover_jsapi,
    experimental_terms, classify_surface, categorize, now_iso, today,
)

STRIP_STRINGS = re.compile(r'"(?:[^"\\]|\\.)*"')
ATTR_LINK_NAME = re.compile(r'#\[link_name\s*=\s*"((?:[^"\\]|\\.)*)"\]')
ATTR_DOC = re.compile(r'#\[doc\s*=\s*"((?:[^"\\]|\\.)*)"\]')
ATTR_DEPRECATED = re.compile(r'#\[deprecated')
MOD_OPEN = re.compile(r'^\s*pub mod (\w+)\s*\{')
EXTERN_OPEN = re.compile(r'^\s*(?:unsafe\s+)?extern "C"\s*\{')
PUB_FN = re.compile(r'^\s*pub fn (\w+)\s*\(')
WRAP_ENTRY = re.compile(r'wrap!\(\s*(\w+)\s*:\s*pub fn (\w+)\s*\(')

MOZJS_SAFE_LAYER_FILES = [
    "lib.rs", "rust.rs", "cell.rs", "consts.rs", "context.rs",
    "conversions.rs", "error.rs", "panic.rs", "realm.rs", "typedarray.rs",
]
WRAPPER_FILES = {
    "wrappers2": "jsapi2_wrappers.in.rs",
    "wrappers1": "jsapi_wrappers.in.rs",
    "glue2": "glue2_wrappers.in.rs",
    "glue1": "glue_wrappers.in.rs",
}

IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def parse_jsapi(path: str):
    """解析 bindgen jsapi.rs → [(entry dict)]。

    行级状态机:剥离字符串字面量后计大括号深度;`pub mod` 入栈维护 rust_path;
    仅在 extern "C" 块内收集 `pub fn`;签名跨行收集至括号平衡的 `;`。
    """
    symbols = []
    mod_stack = []          # [(name, parent_depth)]
    extern_stack = []       # [depth_at_open]
    depth = 0
    pending = {}            # 等待挂到下一个 fn 的属性
    sig = None              # {"name","lines","paren"}
    extern_blocks = 0
    extern_statics = 0      # extern 块内 pub static(如 mozilla 常量;非 fn 面)

    def rust_path():
        return "::".join(n for n, _ in mod_stack) if mod_stack else "root"

    def emit_if_closed(sig):
        """签名括号平衡且行尾 `;` → 落袋。返回 True 表示已消费。"""
        if sig["paren"] <= 0 and sig["lines"][-1].rstrip().endswith(";"):
            symbols.append(_finalize(sig, sig.pop("attrs"), rust_path()))
            return True
        return False

    with open(path, "r", encoding="utf-8") as f:
        for raw in f:
            if sig is None:
                m = ATTR_LINK_NAME.search(raw)
                if m:
                    pending["link_name"] = m.group(1)
                m = ATTR_DOC.search(raw)
                if m:
                    pending["doc"] = (pending.get("doc", "") + "\n" + m.group(1)).strip()
                if ATTR_DEPRECATED.search(raw):
                    pending["deprecated_attr"] = True

            line = STRIP_STRINGS.sub('""', raw)

            if sig is not None:
                sig["lines"].append(line)
                sig["paren"] += line.count("(") - line.count(")")
                if emit_if_closed(sig):
                    sig = None
                # 签名行不含大括号;深度无需在此更新
                continue

            m = MOD_OPEN.match(line)
            if m:
                mod_stack.append((m.group(1), depth))
                depth += line.count("{") - line.count("}")
                continue
            # extern 标记必须在剥字符串前对 raw 行匹配(STRIP 会把 "C" 变 "")
            if EXTERN_OPEN.match(raw):
                extern_stack.append(depth)
                extern_blocks += 1
                depth += line.count("{") - line.count("}")
                continue
            if extern_stack:
                m = PUB_FN.match(line)
                if m and depth == extern_stack[-1] + 1:
                    sig = {"name": m.group(1), "lines": [line],
                           "paren": line.count("(") - line.count(")"),
                           "attrs": pending}
                    pending = {}
                    if emit_if_closed(sig):  # 单行签名(bindgen 常见形态)
                        sig = None
                    continue
                if (depth == extern_stack[-1] + 1
                        and re.match(r'^\s*pub static \w+', line)):
                    extern_statics += 1
                    continue
            depth += line.count("{") - line.count("}")
            if depth < 0:
                raise SystemExit("extract_inventory: brace underflow — bindgen form changed?")
            while mod_stack and depth <= mod_stack[-1][1]:
                mod_stack.pop()
            while extern_stack and depth <= extern_stack[-1]:
                extern_stack.pop()

    if sig is not None:
        raise SystemExit("extract_inventory: truncated signature at EOF — parse unstable")
    return symbols, extern_blocks, extern_statics


def _split_top_level(s: str, sep: str = ","):
    parts, buf, depth_p, depth_a = [], [], 0, 0
    for ch in s:
        if ch in "([":
            depth_p += 1
        elif ch in ")]":
            depth_p -= 1
        elif ch == "<":
            depth_a += 1
        elif ch == ">":
            depth_a -= 1
        if ch == sep and depth_p == 0 and depth_a == 0:
            parts.append("".join(buf))
            buf = []
        else:
            buf.append(ch)
    if buf:
        parts.append("".join(buf))
    return parts


def _normalize_signature(joined: str):
    """`name(a: T1, b: T2) -> R;` → (arg_types, ret)。arg 名剥离(改名不构成漂移)。"""
    joined = re.sub(r"\s+", " ", joined).strip().rstrip(";").strip()
    p_open = joined.find("(")
    p_close = joined.rfind(")")
    if p_open == -1 or p_close == -1:
        return "", ""
    args_raw = joined[p_open + 1:p_close]
    ret = joined[p_close + 1:].strip()
    if ret.startswith("->"):
        ret = ret[2:].strip()
    types = []
    for part in _split_top_level(args_raw):
        part = part.strip()
        if not part:
            continue
        pieces = _split_top_level(part, ":")
        types.append(pieces[-1].strip() if len(pieces) > 1 else part)
    return ", ".join(types), ret


def _finalize(sig, attrs, path):
    joined = " ".join(sig["lines"])
    arg_types, ret = _normalize_signature(joined)
    name = sig["name"]
    sig_norm = f"({arg_types}) -> {ret}"
    return {
        "symbol": name,
        "rust_path": path,
        "key": f"{path}::{name}",
        "mangled": attrs.get("link_name"),
        "args": 0 if arg_types == "" else len(_split_top_level(arg_types)),
        "ret": ret,
        "signature": sig_norm,
        "signature_hash": hashlib.md5(sig_norm.encode()).hexdigest()[:12],
        "deprecated_attr": bool(attrs.get("deprecated_attr")),
    }


def parse_wrappers(mozjs_src: str):
    """4 个 wrapper .in.rs 的 wrap! 条目 → {layer: set(names)} + 未匹配清单。"""
    out = {}
    for layer, fname in WRAPPER_FILES.items():
        path = os.path.join(mozjs_src, fname)
        names = set()
        if os.path.isfile(path):
            with open(path, "r", encoding="utf-8") as f:
                for m in WRAP_ENTRY.finditer(f.read()):
                    names.add((m.group(1), m.group(2)))
        out[layer] = names
    return out


def mozjs_layer_refs(repo: str):
    """mozjs 安全层(wrapper .in.rs 之外)每个 .rs 文件的标识符集。"""
    mozjs_src = os.path.join(repo, "vendor", "mozjs", "mozjs", "src")
    files = []
    for fname in MOZJS_SAFE_LAYER_FILES:
        p = os.path.join(mozjs_src, fname)
        if os.path.isfile(p):
            files.append(p)
    gc_dir = os.path.join(mozjs_src, "gc")
    if os.path.isdir(gc_dir):
        files.extend(sorted(
            os.path.join(gc_dir, n) for n in os.listdir(gc_dir) if n.endswith(".rs")))
    per_file = {}
    for p in files:
        with open(p, "r", encoding="utf-8", errors="replace") as f:
            per_file[os.path.relpath(p, mozjs_src)] = set(IDENT.findall(f.read()))
    return mozjs_src, per_file


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--repo", default=find_repo_root())
    ap.add_argument("--jsapi", default=None,
                    help="bindgen jsapi.rs 路径(默认自动发现最新 build out)")
    ap.add_argument("--out-dir", default=None, help="默认 <repo>/.claude/sm-audit")
    ap.add_argument("--label", default=None, help="默认 <YYYY-MM-DD>-<HEAD short8>")
    args = ap.parse_args()

    repo = args.repo
    if args.jsapi:
        jsapi_path = os.path.abspath(args.jsapi)
        if not os.path.isfile(jsapi_path):
            raise SystemExit(f"--jsapi not found: {jsapi_path}")
        jsapi_md5, uniform = None, None
    else:
        jsapi_path, jsapi_md5, uniform = discover_jsapi(repo)

    raw, extern_blocks, extern_statics = parse_jsapi(jsapi_path)

    # ---- fail-closed 自检(阳性对照 + 结构计数) ----
    names = {r["symbol"] for r in raw}
    controls = {
        "JS_NewContext": "JS_NewContext" in names,
        "EnterRealm@root::JS": any(r["symbol"] == "EnterRealm"
                                   and r["rust_path"] == "root::JS" for r in raw),
        "ReportOutOfMemory@root::js": any(r["symbol"] == "ReportOutOfMemory"
                                          and r["rust_path"] == "root::js" for r in raw),
        "extern_block_count_matches": len(raw) + extern_statics == extern_blocks,
        "min_volume(len>=800)": len(raw) >= 800,
    }
    failed = [k for k, ok in controls.items() if not ok]
    if failed:
        print("extract_inventory: SELF-CHECK FAILED — bindgen form may have changed:",
              file=sys.stderr)
        for k, ok in controls.items():
            print(f"  {'PASS' if ok else 'FAIL'}  {k}", file=sys.stderr)
        print("降级路径(#30 stop 条件):以 vendor/mozjs/mozjs/src/rust.rs + "
              "*.in.rs wrapper 面为单源重写 parse_jsapi,勿硬猜新结构。",
              file=sys.stderr)
        sys.exit(2)

    mozjs_src, layer_files = mozjs_layer_refs(repo)
    wrappers = parse_wrappers(mozjs_src)
    jsapi_wrapper_names = {n for mod, n in wrappers["wrappers2"] if mod == "jsapi"} | \
                          {n for mod, n in wrappers["wrappers1"] if mod == "jsapi"}
    exp_terms = experimental_terms(repo)

    matched = set()
    for entry in raw:
        name = entry["symbol"]
        entry["surface"] = classify_surface(entry["rust_path"])
        entry["category_guess"] = categorize(entry["rust_path"], name)
        base = name.rstrip("0123456789")  # C++ 重载尾数不影响 experimental 词命中
        if base in exp_terms or name in exp_terms:
            entry["stability_guess"] = "experimental"
        elif entry["surface"] == "friend":
            entry["stability_guess"] = "friend(jsfriendapi)"
        elif entry["surface"] == "glue":
            entry["stability_guess"] = "glue"
        else:
            entry["stability_guess"] = "public"
        if name in {n for _, n in wrappers["wrappers2"]}:
            entry["binding"] = "safe-wrapper(wrappers2)"
            matched.add(name)
        elif name in {n for _, n in wrappers["wrappers1"]}:
            entry["binding"] = "safe-wrapper(wrappers1-deprecated)"
            matched.add(name)
        else:
            refs = [f for f, words in layer_files.items() if name in words]
            if refs:
                entry["binding"] = "mozjs-layer-reference"
                entry["mozjs_layer_refs"] = sorted(refs)
            else:
                entry["binding"] = "raw-only"

    unmatched = sorted(n for n in jsapi_wrapper_names - {r["symbol"] for r in raw})

    def rollup(field):
        out = {}
        for e in raw:
            out[e[field]] = out.get(e[field], 0) + 1
        return dict(sorted(out.items()))

    full, short, dirty, branch = git_head(repo)
    label = args.label or f"{today()}-{short}"
    out_dir = args.out_dir or os.path.join(repo, ".claude", "sm-audit")
    os.makedirs(out_dir, exist_ok=True)

    inventory = {
        "schema_version": 1,
        "tool": "scripts/sm-audit/extract_inventory.py",
        "generated": now_iso(),
        "label": label,
        "bao_commit": full,
        "bao_commit_short": short,
        "bao_branch": branch,
        "bao_tree_dirty": dirty,
        "mozjs": {
            **mozjs_baseline(repo),
            "jsapi_source": os.path.abspath(jsapi_path),
            "jsapi_md5": jsapi_md5,
            "jsapi_copies_uniform": uniform,
            "jsapi_extern_blocks": extern_blocks,
        },
        "self_check": {k: bool(v) for k, v in controls.items()},
        "summary": {
            "symbols_total": len(raw),
            "by_surface": rollup("surface"),
            "by_binding": rollup("binding"),
            "by_category_guess": rollup("category_guess"),
            "by_stability_guess": rollup("stability_guess"),
            "wrappers_parsed": {k: len(v) for k, v in wrappers.items()},
            "jsapi_wrapper_names_unmatched_in_jsapi_rs": unmatched,
        },
        "symbols": sorted(raw, key=lambda e: e["key"]),
    }

    out_path = os.path.join(out_dir, f"inventory-{label}.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(inventory, f, ensure_ascii=False, indent=1)
        f.write("\n")

    s = inventory["summary"]
    print(f"OK {out_path}")
    print(f"  symbols={s['symbols_total']} extern_blocks={extern_blocks} "
          f"(uniform_md5={uniform})")
    print(f"  surface={s['by_surface']}")
    print(f"  binding={s['by_binding']}")
    print(f"  unmatched_wrapper_names={len(unmatched)}")


if __name__ == "__main__":
    main()
