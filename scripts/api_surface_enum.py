#!/usr/bin/env python3
"""api-surface.sh's enumerator (W22a #17 stable-set gate).

Reads the rustdoc JSON emissions for the umbrella (bao-core) and the 7
transit crates, classifies every public item into the three freeze tiers, and
either rewrites the archive (--update) or diffs it against the archive
(--check, the gate mode).

Tiers:
  Stable       — the umbrella's top-level re-export list (the `pub use` items)
                 + the deprecated `BaoRuntime` alias. Contract: additions or
                 removals are breaking; semver-gate owns the break semantics.
  Experimental — the 7 namespaced module entries (umbrella) and every public
                 item reachable inside the 7 transit crates. minor-version
                 changelog commitment only.
  Internal     — `#[doc(hidden)]` pub items (grep-based: rustdoc JSON does
                 not serialize the hidden marker). Each must be registered in
                 the archive's hidden_manifest; unregistered new hidden items
                 = red.

Exit codes: 0 gate green, 1 gate red, 2 tool error.
"""
import json
import os
import re
import sys
import collections

DOC_DIR = os.environ.get("BAO_API_SURFACE_DOC_DIR", "")
REPO = os.environ.get("BAO_API_SURFACE_REPO", os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ARCHIVE = os.path.join(REPO, "docs", "api-surface.json")
SRC = os.path.join(REPO, "src")

TRANSIT = ["bao-browser", "bao_engine", "bun_runtime", "bao_cdp", "bao_cdp_client", "bao_stealth", "bao_uloop"]
EXPERIMENTAL_MODULES = ["browser", "cdp", "cdp_client", "engine", "runtime", "stealth", "uloop"]
STABLE_DEPRECATED = ["BaoRuntime"]

def item_kind(it):
    return next(iter((it.get("inner") or {}).keys()), "?")

def is_hidden(it):
    return any("doc(hidden)" in str(a) for a in (it.get("attrs") or []))

def enumerate_crate(path):
    """(top_items, pub_count, hidden_paths) for one rustdoc JSON."""
    d = json.load(open(path))
    idx = d["index"]
    root = idx[str(d["root"])]
    top_ids = root["inner"].get("module", {}).get("items", [])
    tops = []
    for iid in top_ids:
        it = idx.get(str(iid))
        if it is None:
            continue
        use = it.get("inner", {}).get("use") or {}
        # a `pub use a::b::Name` re-export renders name=None — the PUBLIC
        # name is the last path segment of the source.
        src = use.get("source")
        public_name = it.get("name") or (src.split("::")[-1] if src else None)
        tops.append({
            "name": public_name,
            "kind": item_kind(it),
            "glob": bool(use.get("is_glob")),
            "source": src,
            "deprecated": it.get("deprecation") is not None,
            "hidden": is_hidden(it),
        })
    # Reachable public items (Experimental census + hidden grep-augment).
    seen = set()
    stack = list(top_ids)
    pub = 0
    hidden_paths = []
    while stack:
        iid = stack.pop()
        if iid in seen:
            continue
        seen.add(iid)
        it = idx.get(str(iid))
        if it is None:
            continue
        pub += 1
        if is_hidden(it):
            hidden_paths.append(f"{it.get('name') or '?'}({item_kind(it)})")
        mod = (it.get("inner") or {}).get("module")
        if mod and mod.get("items"):
            stack.extend(mod["items"])
    return tops, pub, hidden_paths

def grep_hidden(crate_dir, crate):
    """Source-grep the doc(hidden) pub face (rustdoc JSON does not carry the
    marker). Counts `#[doc(hidden)]` attributes that are real attributes (not
    doc-comment mentions) and captures the next pub item line as the path."""
    hits = []
    for dirpath, _dirs, files in os.walk(crate_dir):
        for f in files:
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dirpath, f)
            rel = os.path.relpath(p, SRC)
            lines = open(p, encoding="utf-8", errors="ignore").read().splitlines()
            for i, l in enumerate(lines):
                s = l.strip()
                if s == "#[doc(hidden)]" or s.startswith("#[doc(hidden)] "):
                    # skip doc-COMMENT mentions (//! lines) — already filtered by strip form
                    # find the next `pub` item within 4 lines for the path hint
                    hint = ""
                    for j in range(i + 1, min(i + 5, len(lines))):
                        m = re.search(r"pub (?:use |fn |struct |enum |mod |trait |type |const |static )(\w+)", lines[j])
                        if m:
                            hint = m.group(1)
                            break
                    hits.append(f"{rel}:{i+1}:{hint or '?'}")
    return hits

def collect():
    umbrella_json = os.path.join(DOC_DIR, "bao.json")
    tops, _, _ = enumerate_crate(umbrella_json)
    crate_counts = {}
    hidden = collections.defaultdict(list)
    for c in TRANSIT:
        jp = os.path.join(DOC_DIR, c.replace("-", "_") + ".json")
        _, pub, _ = enumerate_crate(jp)
        crate_counts[c] = pub
        # package name → src directory (bun_runtime lives in src/bao_runtime).
        src_dir = {"bun_runtime": "bao_runtime"}.get(c, c.replace("-", "_"))
        crate_dir = os.path.join(SRC, src_dir, "src")
        hidden[c] = grep_hidden(crate_dir, c)
    return tops, crate_counts, hidden

def stable_names(tops):
    return sorted(t["name"] for t in tops if t["kind"] == "use" and not t["glob"] and t["name"])

def build_archive(tops, crate_counts, hidden):
    stable = stable_names(tops)
    alias = sorted(t["name"] for t in tops if t["kind"] == "type_alias")
    mods = sorted(t["name"] for t in tops if t["kind"] == "module")
    return {
        "_comment": "W22a (#17) API stable-set archive — the gate (scripts/api-surface.sh) diffs the live rustdoc enumeration against this file. Stable = the umbrella top-level re-export list (the consumer happy path; breaking = semver-gate major). Experimental = the 7 namespaced module entries + every public item in the 7 transit crates (minor changelog only). Internal = #[doc(hidden)] pub items (grep census — rustdoc JSON does not serialize the marker); every entry must carry a deliberate-hidden rationale at its definition.",
        "generated_by": "scripts/api-surface.sh --update",
        "stable_top_level": stable,
        "stable_deprecated_alias": alias,
        "experimental_modules": mods,
        "experimental_crate_pub_counts": crate_counts,
        "hidden_manifest": {c: sorted(v) for c, v in hidden.items()},
    }

def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "--check"
    tops, crate_counts, hidden = collect()
    if mode == "--update":
        arch = build_archive(tops, crate_counts, hidden)
        os.makedirs(os.path.dirname(ARCHIVE), exist_ok=True)
        json.dump(arch, open(ARCHIVE, "w"), indent=2, ensure_ascii=False)
        print(f"[api-surface] archive rewritten: {ARCHIVE}")
        print(f"[api-surface] stable={len(arch['stable_top_level'])} alias={arch['stable_deprecated_alias']} "
              f"modules={arch['experimental_modules']} pub_counts={arch['experimental_crate_pub_counts']} "
              f"hidden={sum(len(v) for v in arch['hidden_manifest'].values())}")
        return 0

    # --check gate mode
    if not os.path.exists(ARCHIVE):
        print(f"[api-surface] RED: archive missing ({ARCHIVE}) — run --update to create the first archive", file=sys.stderr)
        return 1
    arch = json.load(open(ARCHIVE))
    errors = []
    notes = []

    live_stable = stable_names(tops)
    arch_stable = arch.get("stable_top_level", [])
    added = sorted(set(live_stable) - set(arch_stable))
    removed = sorted(set(arch_stable) - set(live_stable))
    if added:
        errors.append(f"unarchived top-level pub item(s) (add to the Stable list deliberately or move under a namespace): {added}")
    if removed:
        errors.append(f"archived Stable item(s) removed from the umbrella top level: {removed}")

    live_alias = sorted(t["name"] for t in tops if t["kind"] == "type_alias")
    arch_alias = arch.get("stable_deprecated_alias", [])
    if set(live_alias) != set(arch_alias):
        errors.append(f"deprecated alias set drifted: live={live_alias} archived={arch_alias}")

    live_mods = sorted(t["name"] for t in tops if t["kind"] == "module")
    arch_mods = arch.get("experimental_modules", [])
    if set(live_mods) != set(arch_mods):
        errors.append(f"namespaced module set drifted: live={live_mods} archived={arch_mods}")

    for c, (pub, _) in [(c, (crate_counts[c], 0)) for c in crate_counts]:
        old = arch.get("experimental_crate_pub_counts", {}).get(c)
        if old is not None and pub != old:
            notes.append(f"EXPERIMENTAL surface change: {c} pub items {old} → {pub} (advisory — changelog per minor, not blocking)")

    arch_hidden = arch.get("hidden_manifest", {})
    for c in TRANSIT:
        live_h = set(hidden.get(c, []))
        arch_h = set(arch_hidden.get(c, []))
        new_hidden = sorted(live_h - arch_h)
        if new_hidden:
            errors.append(f"unregistered #[doc(hidden)] pub item(s) in {c} (register with a deliberate-hidden rationale): {new_hidden}")
        gone = sorted(arch_h - live_h)
        if gone:
            notes.append(f"hidden entries no longer present in {c} (consider pruning the manifest): {gone}")

    print(f"[api-surface] stable={len(live_stable)} alias={live_alias} modules={live_mods}")
    print(f"[api-surface] experimental pub counts: {crate_counts}")
    print(f"[api-surface] hidden census: {sum(len(v) for v in hidden.values())}")
    for n in notes:
        print(f"[api-surface] NOTE {n}")
    if errors:
        for e in errors:
            print(f"[api-surface] RED {e}", file=sys.stderr)
        print(f"[api-surface] GATE RED ({len(errors)} error(s))", file=sys.stderr)
        return 1
    print("[api-surface] GATE GREEN")
    return 0

if __name__ == "__main__":
    sys.exit(main())
