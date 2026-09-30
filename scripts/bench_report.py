#!/usr/bin/env python3
"""bench/REPORT.md generator (W41 / #19-I) — zero-dep python.

Scans bench/results/<date>-<commit>/<bench>.run-<k>.json and assembles
bench/REPORT.md:
  1. coverage matrix (run-date × bench),
  2. per-bench latest key metrics (p50/p95/mean, direction-aware),
  3. history table (latest vs previous measurement, Δ%),
  4. failures & notes verbatim (good-and-bad side by side — errors are
     first-class rows, never hidden).

Modes:
  (default)  rewrite bench/REPORT.md
  --check    regenerate in memory and diff against the on-disk REPORT.md —
             any drift (results changed without regenerating the report) = red.

Zero third-party deps; schema per bench/schema/bench-result.schema.json v1.
"""
import glob
import json
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(REPO, "bench", "results")
REPORT = os.path.join(REPO, "bench", "REPORT.md")

# Per-bench key metrics for the detail/history tables (name, label, unit).
KEY_METRICS = {
    "page-churn": [("churn_cycle", "cycle p50", "ms"), ("churn_pages_per_s_wall", "pages/s", "ops_per_s"),
                   ("vm_rss_slope_over_churn", "rss slope", "KiB_per_s")],
    "fetch-small-payload": [("fetch_rtt", "rtt p50", "ms"), ("fetch_throughput", "throughput", "ops_per_s")],
    "realm-create-drop": [("realm_create", "create p50", "ms")],
    "runtime-create-drop": [("runtime_create", "create p50", "ms")],
    "rss-sample": [("vm_rss", "rss p50", "KiB")],
    "stencil-cost": [("stencil_first_compile", "first compile p50", "ms")],
    "soak": [("churn_pages_per_s_wall", "pages/s", "ops_per_s"),
             ("vm_rss_slope_steady", "steady slope", "KiB_per_s")],
    "page-stress": [("pages_completed_per_s", "pages/s", "ops_per_s")],
    "fs-rw-1mib": [("fs_rw_1mib", "rw 1MiB p50", "ms"), ("fs_rw_1mib_throughput", "rw throughput", "ops_per_s"),
                   ("fs_mb_per_s", "mb/s", "MB_per_s")],
    "crypto-throughput": [("crypto_sha256_1mib", "sha256 1MiB p50", "ms"),
                          ("crypto_mb_per_s", "mb/s", "MB_per_s")],
    "http-serve-echo": [("http_serve_echo", "echo p50", "ms"), ("http_serve_echo_throughput", "req/s", "ops_per_s")],
    "sqlite-insert-select": [("sqlite_insert10_select", "ins10+sel p50", "ms"),
                             ("sqlite_insert10_select_throughput", "ops/s", "ops_per_s")],
    "spawn-echo-sync": [("spawn_echo_sync", "spawnSync p50", "ms"),
                        ("spawn_echo_sync_throughput", "spawns/s", "ops_per_s")],
    "bundler-mini": [("bundler_mini_build", "mini build p50", "ms"),
                     ("bundler_mini_throughput", "builds/s", "ops_per_s")],
}
FALLBACK = [("churn_cycle", "cycle p50", "ms")]

def load_runs():
    """{(date, bench): [run-json,…]} sorted by run index."""
    out = {}
    for d in sorted(glob.glob(os.path.join(RESULTS, "*"))):
        if not os.path.isdir(d):
            continue
        date = os.path.basename(d)
        for f in sorted(glob.glob(os.path.join(d, "*.run-*.json"))):
            bench = os.path.basename(f).split(".run-")[0]
            try:
                doc = json.load(open(f, encoding="utf-8"))
            except Exception as e:
                doc = {"benchmark": bench, "error": f"UNREADABLE: {e}", "metrics": []}
            out.setdefault((date, bench), []).append(doc)
    return out

def metric_map(doc):
    return {m["name"]: m for m in doc.get("metrics", [])}

def fmt(v):
    if v is None:
        return "—"
    if isinstance(v, float):
        return f"{v:,.2f}"
    return str(v)

def key_rows(doc, bench):
    mm = metric_map(doc)
    rows = []
    for name, label, unit in KEY_METRICS.get(bench, FALLBACK):
        m = mm.get(name)
        if m:
            rows.append((label, fmt(m.get("p50", m.get("mean"))), unit, m.get("higher_is_better")))
    if not rows:
        # honest N/A when the bench schema carries none of the known keys
        rows = [("n/a (no known key metric)", "—", "", None)]
    return rows

def build_report():
    runs = load_runs()
    dates = sorted({d for d, _ in runs})
    benches = sorted({b for _, b in runs})

    L = []
    L.append("# Bench REPORT (auto-generated — do not hand-edit)")
    L.append("")
    L.append("Regenerate with `python3 scripts/bench-report.py` (gate: `--check` fails on drift).")
    L.append("Good-and-bad side by side: failed runs are first-class rows below (§4), never dropped.")
    L.append("")
    L.append(f"- run-dates: {len(dates)}  ·  distinct benches: {len(benches)}")
    L.append(f"- sources: `bench/results/<date>-<commit>/<bench>.run-<k>.json` (schema v1)")
    L.append("")

    # §1 coverage matrix
    L.append("## 1. Coverage matrix (bench × run-date)")
    L.append("")
    L.append("| bench | " + " | ".join(dates) + " |")
    L.append("|---|" + "---|" * len(dates))
    for b in benches:
        cells = []
        for d in dates:
            rs = runs.get((d, b))
            if not rs:
                cells.append("—")
            elif any(r.get("error") for r in rs):
                cells.append("ERROR")
            else:
                cells.append(f"✓ ×{len(rs)}")
        L.append(f"| {b} | " + " | ".join(cells) + " |")
    L.append("")

    # §2 latest key metrics per bench
    L.append("## 2. Latest key metrics (per bench, newest run-date)")
    L.append("")
    for b in benches:
        b_dates = [d for d in dates if (d, b) in runs]
        latest = runs[(b_dates[-1], b)][-1]
        if latest.get("error"):
            L.append(f"### {b} — ERROR ({b_dates[-1]})")
            L.append("")
            L.append(f"```")
            L.append(str(latest["error"])[:400])
            L.append("```")
            L.append("")
            continue
        params = latest.get("parameters", {})
        L.append(f"### {b} — {b_dates[-1]} (environment: {latest.get('environment', {}).get('host', 'n/a')})")
        L.append("")
        L.append("| metric | value | unit | direction |")
        L.append("|---|---|---|---|")
        for label, value, unit, hib in key_rows(latest, b):
            direction = "higher-better" if hib else ("lower-better" if hib is not None else "")
            L.append(f"| {label} | {value} | {unit} | {direction} |")
        # selected parameters line
        sel = {k: params[k] for k in sorted(params) if k in (
            "scenario", "executed_cycles", "duration_mins", "duration_secs", "concurrency",
            "requests", "iterations", "mode", "pages_completed", "stale_events")}
        if sel:
            L.append("")
            L.append(f"parameters: `{sel}`")
        L.append("")

    # §3 history (latest vs previous, per bench, per key metric)
    L.append("## 3. History (latest vs previous measurement)")
    L.append("")
    L.append("| bench | metric | prev | latest | Δ | note |")
    L.append("|---|---|---|---|---|---|")
    for b in benches:
        b_dates = [d for d in dates if (d, b) in runs]
        if len(b_dates) < 2:
            L.append(f"| {b} | — | — | — | — | single run-date |")
            continue
        cur_doc = runs[(b_dates[-1], b)][-1]
        prev_doc = runs[(b_dates[-2], b)][-1]
        cm, pm = metric_map(cur_doc), metric_map(prev_doc)
        for name, label, unit in KEY_METRICS.get(b, FALLBACK):
            c, p = cm.get(name), pm.get(name)
            if not c or not p:
                L.append(f"| {b} | {label} | — | — | — | metric absent on one side |")
                continue
            cv, pv = c.get("p50", c.get("mean")), p.get("p50", p.get("mean"))
            if not (isinstance(cv, (int, float)) and isinstance(pv, (int, float))) or pv == 0:
                L.append(f"| {b} | {label} | {fmt(pv)} | {fmt(cv)} | — | non-numeric |")
                continue
            delta = (cv - pv) / pv * 100.0
            hib = c.get("higher_is_better")
            worse = (delta < 0) if hib else (delta > 0)
            flag = "⚠ worse" if worse and abs(delta) >= 20 else ("improved" if abs(delta) >= 20 else "~")
            L.append(f"| {b} | {label} | {fmt(pv)} | {fmt(cv)} | {delta:+.1f}% | {flag} |")
    L.append("")

    # §4 failures & notes (good-and-bad side by side)
    L.append("## 4. Failures & notes (verbatim, never dropped)")
    L.append("")
    err_any = False
    for (d, b), rs in sorted(runs.items()):
        for i, r in enumerate(rs, 1):
            if r.get("error"):
                err_any = True
                L.append(f"- **{d}/{b}.run-{i}: ERROR** — {str(r['error'])[:300]}")
            for note in r.get("notes", []):
                if any(k in note.lower() for k in ("red", "fail", "degraded", "finding", "violat")):
                    err_any = True
                    L.append(f"- {d}/{b}.run-{i} note: {note[:300]}")
    if not err_any:
        L.append("(no failed runs and no red-flagged notes in the archived set)")
    L.append("")

    # §5 regression-gate hint
    L.append("## 5. Regression gate")
    L.append("")
    L.append("`scripts/bench-regression-gate.sh` compares the two newest run-dates per bench on")
    L.append("the §3 key metrics: ≥20% adverse Δ = RED (direction-aware); soak slope-class")
    L.append("metrics are advisory-only (noise-dominated). Wire-in: local-ci optional segment")
    L.append("(`BAO_BENCH_REGRESSION=1`).")
    L.append("")
    return "\n".join(L) + "\n"

def main():
    report = build_report()
    if "--check" in sys.argv:
        ondisk = open(REPORT, encoding="utf-8").read() if os.path.exists(REPORT) else ""
        if ondisk != report:
            sys.stderr.write("[bench-report] RED: bench/REPORT.md drifted from the regenerated "
                             "content — rerun `python3 scripts/bench-report.py` and review the diff\n")
            import difflib
            for line in list(difflib.unified_diff(ondisk.splitlines(), report.splitlines(),
                                                  "REPORT.md(disk)", "REPORT.md(regen)", lineterm=""))[:40]:
                sys.stderr.write(line + "\n")
            return 1
        print("[bench-report] GATE GREEN (regeneration byte-equal)")
        return 0
    open(REPORT, "w", encoding="utf-8").write(report)
    print(f"[bench-report] REPORT.md regenerated ({report.count(chr(10))} lines)")
    return 0

if __name__ == "__main__":
    sys.exit(main())
