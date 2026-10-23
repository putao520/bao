#!/usr/bin/env python3
"""ev4 vendor 四宇宙归一 — 继承面全谱分析器(只读)。

扫描 vendor/{servo,stylo,ipc-channel,freetype-wrapper}(+ mozjs 总序参考),
产出:
  - inventory.json   机器可读全谱(每个 manifest 的继承键、根值、target 段)
  - summary.md       人读摘要(继承键频谱 / 根值表 / 冲突矩阵 / 孤儿 dep)
只读:绝不写仓库任何文件。
"""
from __future__ import annotations

import json
import tomllib
from pathlib import Path

REPO = Path("/home/putao/code/rust/bao")
OUT = Path("/tmp/ev4-design")

TREES = ["vendor/servo", "vendor/stylo", "vendor/ipc-channel", "vendor/freetype-wrapper"]
MOZJS = "vendor/mozjs"  # 总序参考,不展开


def toml_str(v) -> str:
    """把 workspace 根值渲染成单行 TOML 字面量(与 transformer 渲染规则一致)。"""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, str):
        return json.dumps(v)  # JSON 字符串转义与 TOML basic string 兼容(ASCII 场景)
    if isinstance(v, int):
        return str(v)
    if isinstance(v, list):
        return "[" + ", ".join(toml_str(x) for x in v) + "]"
    if isinstance(v, dict):
        return "{" + ", ".join(f"{k} = {toml_str(x)}" for k, x in v.items()) + "}"
    raise TypeError(f"unrenderable: {v!r}")


def main() -> None:
    # ── 主根 workspace(对照真源)─────────────────────────────────────────
    main_root = tomllib.loads((REPO / "Cargo.toml").read_text())
    main_pkg = main_root.get("workspace", {}).get("package", {})
    main_deps = main_root.get("workspace", {}).get("dependencies", {})
    main_patch = main_root.get("patch", {}).get("crates-io", {})
    main_exclude = main_root["workspace"].get("exclude", [])

    inventory: dict = {
        "main_root": {
            "exclude": main_exclude,
            "workspace_package": main_pkg,
            "patch_crates_io": {k: toml_str(v) for k, v in main_patch.items()},
            "workspace_deps_key_count": len(main_deps),
        },
        "trees": {},
        "mozjs": {},
    }

    # ── 每个 vendor 树 ───────────────────────────────────────────────────
    for tree in TREES:
        root_manifest = REPO / tree / "Cargo.toml"
        data = tomllib.loads(root_manifest.read_text())
        is_ws_root = "[workspace]" in root_manifest.read_text().split("[", 1)[-1] or "workspace" in data
        ws = data.get("workspace", {})
        wdeps = ws.get("dependencies", {})
        wpkg = ws.get("package", {})
        wpatch = data.get("patch", {}).get("crates-io", {})

        entry = {
            "root_manifest": tree,
            "is_workspace_root": "workspace" in data and "members" in ws,
            "members": ws.get("members", []),
            "default_members": ws.get("default-members"),
            "exclude": ws.get("exclude", []),
            "workspace_package": wpkg,
            "workspace_deps": wdeps,
            "patch_crates_io": {k: toml_str(v) for k, v in wpatch.items()},
            "manifests": [],
        }
        # profiles 位于根 manifest 顶层 [profile.*]
        entry["profiles"] = sorted(data.get("profile", {}).keys())

        # 工具链 / lock / target 现状
        entry["rust_toolchain"] = [
            str(p.relative_to(REPO))
            for p in sorted((REPO / tree).rglob("rust-toolchain*"))
            if "target" not in p.parts
        ]
        entry["cargo_lock"] = (REPO / tree / "Cargo.lock").exists()
        entry["target_dir"] = (REPO / tree / "target").exists()

        # 继承消费谱:逐 manifest
        dep_usage: dict[str, list] = {k: [] for k in wdeps}  # key -> [consumer:section]
        pkg_field_usage: dict[str, list] = {k: [] for k in wpkg}
        manifests = []
        for mf in sorted((REPO / tree).rglob("Cargo.toml")):
            rel = str(mf.relative_to(REPO))
            if "/target/" in rel:
                continue
            text = mf.read_text()
            d = tomllib.loads(text)
            pkg = d.get("package", {})
            rec = {
                "manifest": rel,
                "package": pkg.get("name"),
                "version": pkg.get("version"),
                "uses_workspace_inheritance": ".workspace" in text or "workspace = true" in text,
                "inherited_package_fields": {},
                "inherited_deps": {},      # section -> {key: local_override}
                "own_lints_tables": sorted(d.get("lints", {}).keys()),
            }
            # package 字段继承: "edition.workspace = true" 在 tomllib 里就是 key "edition.workspace"
            for k, v in pkg.items():
                if isinstance(v, dict) and v.get("workspace") is True:
                    field = k
                    rec["inherited_package_fields"][field] = toml_str(wpkg.get(field, "<MISSING>"))
                    pkg_field_usage.setdefault(field, []).append(rel)
            for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                sec = d.get(section, {})
                for k, v in sec.items():
                    if isinstance(v, dict) and v.get("workspace") is True:
                        override = {kk: vv for kk, vv in v.items() if kk != "workspace"}
                        rec["inherited_deps"].setdefault(section, {})[k] = toml_str(override) if override else "{}"
                        dep_usage.setdefault(k, []).append(f"{rel}:{section}")
                    elif k in wdeps and isinstance(v, dict) and "workspace" in str(v):
                        # { workspace = true, features = [...] } 已被上支捕获;这里兜底记录异常形态
                        rec["inherited_deps"].setdefault(section, {})[k] = "ABNORMAL:" + toml_str(v)
            # target 段继承
            for tname, tsec in d.get("target", {}).items():
                for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                    for k, v in tsec.get(section, {}).items():
                        if isinstance(v, dict) and v.get("workspace") is True:
                            override = {kk: vv for kk, vv in v.items() if kk != "workspace"}
                            rec["inherited_deps"].setdefault(f"target:{tname}:{section}", {})[k] = (
                                toml_str(override) if override else "{}"
                            )
                            dep_usage.setdefault(k, []).append(f"{rel}:target[{tname}]:{section}")
            manifests.append(rec)

        entry["manifests"] = manifests
        entry["dep_usage"] = {k: v for k, v in dep_usage.items() if v}
        entry["orphan_workspace_deps"] = sorted(k for k, v in dep_usage.items() if not v)
        entry["pkg_field_usage"] = {k: v for k, v in pkg_field_usage.items() if v}
        inventory["trees"][tree] = entry

    # ── 主根 vs servo 根 workspace.dependencies 冲突矩阵(吸收不可走继承合并的证据)──
    servo = inventory["trees"]["vendor/servo"]
    sdeps = servo["workspace_deps"]
    conflicts = []
    for k in sorted(set(sdeps) & set(main_deps)):
        if toml_str(sdeps[k]) != toml_str(main_deps[k]):
            conflicts.append({"key": k, "servo": toml_str(sdeps[k]), "main": toml_str(main_deps[k])})
    inventory["inheritance_merge_conflicts"] = conflicts

    # ── patch 链 parity(②)───────────────────────────────────────────────
    inventory["patch_parity"] = {
        "main_root": inventory["main_root"]["patch_crates_io"],
        "vendor/servo": servo["patch_crates_io"],
        "identical_target": all(
            inventory["main_root"]["patch_crates_io"].get(k) == servo["patch_crates_io"].get(k)
            for k in set(inventory["main_root"]["patch_crates_io"]) | set(servo["patch_crates_io"])
        ),
    }

    # ── lock 对照(追加证据)──────────────────────────────────────────────
    def lock_set(p: Path) -> dict[str, list[str]]:
        if not p.exists():
            return {}
        d = tomllib.loads(p.read_text())
        out: dict[str, list[str]] = {}
        for pkg in d.get("package", []):
            out.setdefault(pkg["name"], []).append(pkg["version"])
        return out

    mainlock = lock_set(REPO / "Cargo.lock")
    servo_lock = lock_set(REPO / "vendor/servo/Cargo.lock")
    ipc_lock = lock_set(REPO / "vendor/ipc-channel/Cargo.lock")
    crown_lock = lock_set(REPO / "vendor/servo/support/crown/Cargo.lock")
    inventory["locks"] = {
        "main_packages": sum(len(v) for v in mainlock.values()),
        "servo_packages": sum(len(v) for v in servo_lock.values()),
        "ipc_packages": sum(len(v) for v in ipc_lock.values()),
        "crown_packages": sum(len(v) for v in crown_lock.values()),
        "servo_only_packages": sorted(n for n in servo_lock if n not in mainlock),
        "shared_version_drift": sorted(
            (n, mainlock[n], servo_lock[n])
            for n in set(mainlock) & set(servo_lock)
            if sorted(mainlock[n]) != sorted(servo_lock[n])
        ),
    }

    # ── mozjs 总序参考(仅根形态)────────────────────────────────────────
    mj = tomllib.loads((REPO / MOZJS / "Cargo.toml").read_text())
    inventory["mozjs"] = {
        "is_workspace_root": "workspace" in mj,
        "members": mj.get("workspace", {}).get("members"),
        "exclude_in_main": "vendor/mozjs" in main_exclude,
    }

    (OUT / "inventory.json").write_text(json.dumps(inventory, indent=2, ensure_ascii=False))

    # ── 人读摘要 ────────────────────────────────────────────────────────
    lines = ["# ev4 继承面全谱 — 分析器输出摘要\n"]
    for tree, e in inventory["trees"].items():
        n_inheriting = sum(1 for m in e["manifests"] if m["uses_workspace_inheritance"])
        lines.append(f"## {tree}\n")
        lines.append(f"- workspace root: {e['is_workspace_root']}; members={e['members']}")
        lines.append(f"- manifests scanned: {len(e['manifests'])}; using inheritance: {n_inheriting}")
        lines.append(f"- patch.crates-io: {e['patch_crates_io']}")
        lines.append(f"- rust-toolchain: {e['rust_toolchain']}; own Cargo.lock: {e['cargo_lock']}; target/: {e['target_dir']}")
        lines.append(f"- orphan workspace deps (根里声明但零消费): {e['orphan_workspace_deps']}")
        lines.append(f"- dep keys consumed: {len(e['dep_usage'])}; workspace.package fields: {list(e['pkg_field_usage'])}\n")
    lines.append(f"## 继承合并冲突(主根 vs servo 根同 key 异值): {len(conflicts)}")
    for c in conflicts:
        lines.append(f"- `{c['key']}`: servo={c['servo']}  main={c['main']}")
    lines.append("")
    pp = inventory["patch_parity"]
    lines.append(f"## patch parity: identical={pp['identical_target']} main={pp['main_root']} servo={pp['vendor/servo']}")
    lk = inventory["locks"]
    lines.append(f"## locks: main={lk['main_packages']} servo={lk['servo_packages']} ipc={lk['ipc_packages']} crown={lk['crown_packages']}")
    lines.append(f"- shared-key version drift (main vs servo lock): {lk['shared_version_drift']}")
    (OUT / "summary.md").write_text("\n".join(lines) + "\n")
    print(f"written: {OUT/'inventory.json'} ({len(json.dumps(inventory))} bytes), {OUT/'summary.md'}")


if __name__ == "__main__":
    main()
