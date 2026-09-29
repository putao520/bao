#!/usr/bin/env bash
#
# scripts/publish-closure.sh — W23-⑥ release-closure automation (#18-⑥).
#
# Three modes, increasing commitment:
#   --plan [ref]     version triangle (local manifest ↔ sibling floors ↔
#                    registry curl, single-request-per-crate) + touched-crate
#                    closure (git diff since the last publish-closure commit)
#                    + reverse-dependency closure + bump suggestion table +
#                    topological publish order. ZERO network publishing.
#   --dry-run [ref]  plan + `cargo publish --dry-run --locked` per crate in
#                    topo order (first failure short-circuits).
#   --execute [ref]  DANGEROUS: real `cargo publish` per crate, single-flight
#                    with per-crate curl-200 verification after each upload
#                    (sparse-index phantom lesson: never bulk-verify); any
#                    failure short-circuits (no further publishes — §12 chain
#                    discipline). Requires BAO_PUBLISH_CONFIRM=yes.
#
# Human-reserved points (printed in every plan): ①bump-table sign-off
# ②crates.io token ③publish-window decision ④exception adjudication
# ⑤tag/commit closure message.
#
# Bump-level rule (face-transition-publish-discipline): a breaking face
# change on a 0.x crate requires the MINOR slot (never patch); with no
# semver-gate report available the suggestion is emitted as `minor?MANUAL`
# — the sign-off point resolves it.

set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"

MODE="${1:---plan}"
REF="${2:-}"
if [ -z "$REF" ]; then
    # default baseline: the most recent publish-closure commit
    REF=$(git log --oneline --grep='publish closure' -1 --format='%h')
    [ -z "$REF" ] && REF="HEAD~30"
fi

echo "[closure] baseline ref: $REF ($(git log -1 --format='%s' "$REF" 2>/dev/null | head -c 80))"

# ── workspace metadata (name/version/publish flag/manifest path/deps) ───────
# the full-graph metadata is ~100s of KB — pass via FILE, not argv/env
# (E2BIG on env inheritance).
META_FILE=$(mktemp /tmp/publish-closure-meta.XXXXXX.json)
cargo metadata --format-version 1 --no-deps 2>/dev/null > "$META_FILE"
export CLOSURE_META_FILE="$META_FILE"

python3 - "$MODE" "$REF" <<'PY_EOF'
import json, os, subprocess, sys, urllib.request, time

meta = json.load(open(os.environ["CLOSURE_META_FILE"]))
mode = sys.argv[1]
ref = sys.argv[2]
crates = {p["name"]: p for p in meta["packages"]}

# ── touched crates since REF (manifest or source under src/<crate>) ────────
diff = subprocess.run(["git", "diff", "--name-only", f"{ref}..HEAD"],
                      capture_output=True, text=True).stdout.splitlines()
name_by_path = {p["manifest_path"].replace(os.getcwd() + "/", ""): n for n, p in crates.items()}
touched = set()
for f in diff:
    if f.startswith("src/"):
        parts = f.split("/")
        if len(parts) >= 2:
            # crate dir = src/<dir>; map dir → package name via manifest_path
            for mp, n in name_by_path.items():
                if mp.startswith(f"src/{parts[1]}/"):
                    touched.add(n)
                    break

# ── reverse-dependency closure over the workspace dep graph ─────────────────
rev = {n: set() for n in crates}
for n, p in crates.items():
    for dep in p.get("dependencies", []):
        t = dep.get("name")
        if t in rev and dep.get("kind", "normal") == "normal":
            rev[t].add(n)
closure = set(touched)
frontier = set(touched)
while frontier:
    nxt = set()
    for t in frontier:
        for r in rev.get(t, ()): 
            if r not in closure:
                closure.add(r); nxt.add(r)
    frontier = nxt

# ── registry versions (single request per crate) ────────────────────────────
def registry_version(name):
    url = f"https://crates.io/api/v1/crates/{name}"
    req = urllib.request.Request(url, headers={"User-Agent": "bao-publish-closure"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)["crate"]["max_stable_version"]
    except Exception as e:
        return f"UNVERIFIED({type(e).__name__})"

def semver_tuple(v):
    parts = v.split("-")[0].split(".")
    try:
        return tuple(int(x) for x in parts[:3])
    except Exception:
        return (0, 0, 0)

# ── bump suggestion ──────────────────────────────────────────────────────────
# semver-gate report (if present) contributes majors.
gate_report = ""
import glob
for f in sorted(glob.glob(".claude/semver-gate-report-*.md")):
    gate_report = open(f, encoding="utf-8", errors="ignore").read()
    break

def suggest_bump(name, local, reg, touched_flag):
    if local == reg:
        if not touched_flag:
            return "no-publish", "untouched since baseline"
        lvl = "minor?MANUAL"
        if name in gate_report and "major-required" in gate_report.split(name)[1][:400] if name in gate_report else False:
            lvl = "major?MANUAL"
        return "needs-bump", f"source changed since baseline; suggested {lvl} (sign-off resolves)"
    lt, rt = semver_tuple(local), semver_tuple(reg)
    if lt > rt:
        return "publish-ready", "manifest version already ahead of registry"
    return "no-publish", f"registry ahead ({reg} > {local}) — investigate"

# ── topological order over the closure subgraph (dependencies first) ────────
def deps_of(n):
    return [d.get("name") for d in crates[n].get("dependencies", []) if d.get("name") in closure]

order = []
state = {}
def visit(n):
    if state.get(n) == 2: return
    if state.get(n) == 1:
        print(f"[closure] WARN: cycle at {n}", file=sys.stderr); return
    state[n] = 1
    for d in deps_of(n): visit(d)
    state[n] = 2
    order.append(n)
for n in sorted(closure): visit(n)
# dev-dependency cycles in the workspace graph can leave members unordered —
# append them alphabetically (still after all their ordered deps).
missing = sorted(closure - set(order))
if missing:
    print(f"[closure] NOTE: {len(missing)} member(s) in a dev-dep cycle appended alphabetically: {missing}", file=sys.stderr)
    order.extend(missing)

# ── assemble the plan table ──────────────────────────────────────────────────
rows = []
pending = 0
levels = {"publish-ready": 0, "needs-bump": 0, "no-publish": 0}
for n in order:
    p = crates[n]
    local = p["version"]
    publishable = p.get("publish") != []
    reg = registry_version(n) if publishable else "unpublished-by-manifest"
    action, why = suggest_bump(n, local, reg, n in touched)
    if not publishable:
        action, why = "skip", "publish=false in manifest"
    levels[action] = levels.get(action, 0) + 1
    if action in ("publish-ready", "needs-bump"):
        pending += 1
    rows.append((n, local, reg, action, why))
    time.sleep(1.0)  # crates.io courtesy rate

print()
print("=== PUBLISH CLOSURE PLAN (baseline %s) ===" % ref)
print("%-22s %-12s %-14s %-14s %s" % ("crate", "local", "registry", "action", "why"))
for n, local, reg, action, why in rows:
    print("%-22s %-12s %-14s %-14s %s" % (n, local, reg, action, why))
print()
print("=== TOPOLOGICAL PUBLISH ORDER (publish-ready first; needs-bump enter after sign-off) ===")
seq = [n for n in order if dict((r[0], r[3]) for r in rows).get(n) in ("publish-ready", "needs-bump")]
print(" → ".join(seq) if seq else "(empty — no publish-ready/needs-bump crates in this closure)")
print()
print(f"[closure] closure size={len(closure)} (touched={len(touched)}) pending={pending} "
      f"levels={ {k: v for k, v in levels.items()} }")
print()
print("HUMAN-RESERVED POINTS:")
print("  ①bump-table sign-off  ②crates.io token  ③publish-window decision")
print("  ④exception adjudication  ⑤tag/commit closure message")

# persist the plan for --dry-run/--execute consumption
with open("/tmp/publish-closure-plan.json", "w") as f:
    json.dump({"ref": ref, "order": order,
               "rows": [{"name": n, "local": l, "registry": r, "action": a, "why": w} for n, l, r, a, w in rows]},
              f, indent=2)
print("[closure] plan persisted: /tmp/publish-closure-plan.json")

if mode == "--plan":
    sys.exit(0)

# ── dry-run / execute ────────────────────────────────────────────────────────
publish_list = [n for n in order if dict((r[0], r[3]) for r in rows).get(n) == "publish-ready"]
if mode == "--dry-run":
    print(f"[closure] DRY-RUN over {len(publish_list)} publish-ready crate(s), topo order")
    bad = 0
    for n in publish_list:
        r = subprocess.run(["cargo", "publish", "--dry-run", "--locked", "-p", n],
                           capture_output=True, text=True)
        ok = r.returncode == 0
        print(f"[closure]   {'OK ' if ok else 'RED'} {n}")
        if not ok:
            bad += 1
            print((r.stderr or r.stdout)[-800:])
            if bad >= 1:
                print("[closure] short-circuit: first dry-run failure aborts the pass")
                break
    sys.exit(1 if bad else 0)

if mode == "--execute":
    if os.environ.get("BAO_PUBLISH_CONFIRM") != "yes":
        print("[closure] RED: --execute requires BAO_PUBLISH_CONFIRM=yes (human sign-off point ①)")
        sys.exit(2)
    if not os.environ.get("CARGO_REGISTRY_TOKEN"):
        print("[closure] RED: CARGO_REGISTRY_TOKEN missing (human-reserved point ②)")
        sys.exit(2)
    print(f"[closure] EXECUTE over {len(publish_list)} crate(s), single-flight topo order")
    for n in publish_list:
        print(f"[closure] publishing {n} …")
        r = subprocess.run(["cargo", "publish", "--locked", "-p", n], capture_output=True, text=True)
        if r.returncode != 0:
            print(f"[closure] RED: publish {n} failed — SHORT CIRCUIT (no further publishes)\n{(r.stderr or r.stdout)[-800:]}")
            sys.exit(1)
        # single-request verification (sparse-index phantom lesson)
        ok = False
        for attempt in range(6):
            time.sleep(10)
            rv = registry_version(n)
            if rv == crates[n]["version"]:
                ok = True
                break
            print(f"[closure]   verify attempt {attempt+1}: registry={rv} (waiting for index)")
        if not ok:
            print(f"[closure] RED: {n} uploaded but registry verify failed after retries — SHORT CIRCUIT")
            sys.exit(1)
        print(f"[closure]   verified: registry {n}=={rv}")
    print(f"[closure] EXECUTE PASS: {len(publish_list)} crate(s) published and verified")
    sys.exit(0)
PY_EOF
