# Bench REPORT (auto-generated — do not hand-edit)

Regenerate with `python3 scripts/bench-report.py` (gate: `--check` fails on drift).
Good-and-bad side by side: failed runs are first-class rows below (§4), never dropped.

- run-dates: 20  ·  distinct benches: 13
- sources: `bench/results/<date>-<commit>/<bench>.run-<k>.json` (schema v1)

## 1. Coverage matrix (bench × run-date)

| bench | 2026-09-10-03396a13 | 2026-09-10-27dbb606 | 2026-09-10-a4a6738c | 2026-09-10-b8d8f5e5 | 2026-09-10-ec4a0e5a | 2026-09-11-508502de | 2026-09-12-508502de | 2026-09-13-508502de | 2026-09-14-508502de | 2026-09-15-508502de | 2026-09-16-d61f26f1 | 2026-09-17-3e2d5a6f | 2026-09-19-66bdb414 | 2026-09-20-1a2670e5 | 2026-09-24-0f731b93 | 2026-09-25-d4aa9e51 | 2026-09-26-6f74d61a | 2026-09-29-5388d9c4 | 2026-09-29-7e22a1fe | 2026-09-30-c106f6b6 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| bundler-bench | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | ✓ ×3 |
| crypto-bench | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | ✓ ×3 |
| fetch-small-payload | — | — | — | ✓ ×3 | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |
| fs-bench | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | ✓ ×3 |
| http-bench | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | ✓ ×3 |
| page-churn | — | — | — | ✓ ×1 | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |
| realm-create-drop | — | — | — | ✓ ×3 | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |
| rss-sample | — | — | — | ✓ ×3 | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |
| runtime-create-drop | — | — | — | ✓ ×3 | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |
| soak | ✓ ×1 | — | ✓ ×1 | — | ERROR | ✓ ×1 | ERROR | ✓ ×1 | ERROR | ERROR | ERROR | ✓ ×1 | ✓ ×1 | ✓ ×1 | ✓ ×1 | ✓ ×1 | ✓ ×1 | ✓ ×1 | ✓ ×1 | — |
| spawn-bench | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | ✓ ×3 |
| sqlite-bench | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | ✓ ×3 |
| stencil-cost | ✓ ×3 | ✓ ×3 | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |

## 2. Latest key metrics (per bench, newest run-date)

### bundler-bench — 2026-09-30-c106f6b6 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'iterations': 20}`

### crypto-bench — 2026-09-30-c106f6b6 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'requests': 200}`

### fetch-small-payload — 2026-09-10-b8d8f5e5 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| rtt p50 | 0.86 | ms | lower-better |
| throughput | 1,041.21 | ops_per_s | higher-better |

parameters: `{'concurrency': 1, 'requests': 400}`

### fs-bench — 2026-09-30-c106f6b6 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'requests': 200}`

### http-bench — 2026-09-30-c106f6b6 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'requests': 300}`

### page-churn — 2026-09-10-b8d8f5e5 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| cycle p50 | 95.49 | ms | lower-better |
| pages/s | 5.98 | ops_per_s | higher-better |
| rss slope | 64,170.06 | KiB_per_s | lower-better |

parameters: `{'iterations': 15}`

### realm-create-drop — 2026-09-10-b8d8f5e5 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'iterations': 100}`

### rss-sample — 2026-09-10-b8d8f5e5 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

### runtime-create-drop — 2026-09-10-b8d8f5e5 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| create p50 | 6.47 | ms | lower-better |

parameters: `{'iterations': 50}`

### soak — 2026-09-29-7e22a1fe (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| pages/s | 8.47 | ops_per_s | higher-better |
| steady slope | 218.91 | KiB_per_s | lower-better |

parameters: `{'duration_mins': 60, 'executed_cycles': 30480, 'scenario': 'page-churn'}`

### spawn-bench — 2026-09-30-c106f6b6 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'requests': 100}`

### sqlite-bench — 2026-09-30-c106f6b6 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'requests': 60}`

### stencil-cost — 2026-09-10-27dbb606 (environment: pt-worker)

| metric | value | unit | direction |
|---|---|---|---|
| n/a (no known key metric) | — |  |  |

parameters: `{'iterations': 60}`

## 3. History (latest vs previous measurement)

| bench | metric | prev | latest | Δ | note |
|---|---|---|---|---|---|
| bundler-bench | — | — | — | — | single run-date |
| crypto-bench | — | — | — | — | single run-date |
| fetch-small-payload | — | — | — | — | single run-date |
| fs-bench | — | — | — | — | single run-date |
| http-bench | — | — | — | — | single run-date |
| page-churn | — | — | — | — | single run-date |
| realm-create-drop | — | — | — | — | single run-date |
| rss-sample | — | — | — | — | single run-date |
| runtime-create-drop | — | — | — | — | single run-date |
| soak | pages/s | 8.52 | 8.47 | -0.7% | ~ |
| soak | steady slope | 115.10 | 218.91 | +90.2% | ⚠ worse |
| spawn-bench | — | — | — | — | single run-date |
| sqlite-bench | — | — | — | — | single run-date |
| stencil-cost | first compile p50 | — | — | — | metric absent on one side |

## 4. Failures & notes (verbatim, never dropped)

- 2026-09-10-27dbb606/stencil-cost.run-1 note: tiny_1p1: A1(realm+compile+exec) p50=218.3us A2(compile+exec) p50=55.6us B(stencil compile) p50=3.2us C(instantiate+exec) p50=49.7us D(cached path) p50=56.7us → stencil saves 5.8us/realm; wired cache speedup 0.98x, residual overhead beyond C 12.3% (of D) / 12.5% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-1 note: stealth: A1(realm+compile+exec) p50=1607.4us A2(compile+exec) p50=1301.5us B(stencil compile) p50=938.9us C(instantiate+exec) p50=248.1us D(cached path) p50=250.0us → stencil saves 1053.4us/realm; wired cache speedup 5.21x, residual overhead beyond C 0.8% (of D) / 0.2% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-1 note: stealth_x10: A1(realm+compile+exec) p50=11010.3us A2(compile+exec) p50=10853.3us B(stencil compile) p50=10443.3us C(instantiate+exec) p50=1200.5us D(cached path) p50=1102.8us → stencil saves 9652.8us/realm; wired cache speedup 9.84x, residual overhead beyond C -8.9% (of D) / 0.0% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-2 note: tiny_1p1: A1(realm+compile+exec) p50=220.5us A2(compile+exec) p50=55.0us B(stencil compile) p50=3.1us C(instantiate+exec) p50=47.5us D(cached path) p50=55.8us → stencil saves 7.5us/realm; wired cache speedup 0.99x, residual overhead beyond C 14.9% (of D) / 15.1% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-2 note: stealth: A1(realm+compile+exec) p50=1430.6us A2(compile+exec) p50=1272.5us B(stencil compile) p50=904.7us C(instantiate+exec) p50=248.6us D(cached path) p50=244.4us → stencil saves 1023.9us/realm; wired cache speedup 5.21x, residual overhead beyond C -1.7% (of D) / 0.0% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-2 note: stealth_x10: A1(realm+compile+exec) p50=12725.7us A2(compile+exec) p50=10875.7us B(stencil compile) p50=9421.6us C(instantiate+exec) p50=1241.4us D(cached path) p50=1548.6us → stencil saves 9634.3us/realm; wired cache speedup 7.02x, residual overhead beyond C 19.8% (of D) / 2.8% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-3 note: tiny_1p1: A1(realm+compile+exec) p50=224.8us A2(compile+exec) p50=57.6us B(stencil compile) p50=4.1us C(instantiate+exec) p50=52.3us D(cached path) p50=55.1us → stencil saves 5.4us/realm; wired cache speedup 1.05x, residual overhead beyond C 5.1% (of D) / 4.9% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-3 note: stealth: A1(realm+compile+exec) p50=1504.5us A2(compile+exec) p50=2351.0us B(stencil compile) p50=894.4us C(instantiate+exec) p50=241.1us D(cached path) p50=212.9us → stencil saves 2109.9us/realm; wired cache speedup 11.05x, residual overhead beyond C -13.3% (of D) / 0.0% (of A2)
- 2026-09-10-27dbb606/stencil-cost.run-3 note: stealth_x10: A1(realm+compile+exec) p50=11431.8us A2(compile+exec) p50=10247.8us B(stencil compile) p50=8917.0us C(instantiate+exec) p50=1190.5us D(cached path) p50=1186.6us → stencil saves 9057.3us/realm; wired cache speedup 8.64x, residual overhead beyond C -0.3% (of D) / 0.0% (of A2)
- 2026-09-10-b8d8f5e5/fetch-small-payload.run-1 note: per-request RTT measured in JS via performance.now; pump tick interval 500 µs bounds embedder overhead below the RTT floor
- 2026-09-10-b8d8f5e5/fetch-small-payload.run-2 note: per-request RTT measured in JS via performance.now; pump tick interval 500 µs bounds embedder overhead below the RTT floor
- 2026-09-10-b8d8f5e5/fetch-small-payload.run-3 note: per-request RTT measured in JS via performance.now; pump tick interval 500 µs bounds embedder overhead below the RTT floor
- 2026-09-10-b8d8f5e5/realm-create-drop.run-1 note: realm = lazily-created global object per JsContext wrapper over the shared thread SM Runtime (realm-per-context model); SM JSContext itself is not destroyed per iteration
- 2026-09-10-b8d8f5e5/realm-create-drop.run-2 note: realm = lazily-created global object per JsContext wrapper over the shared thread SM Runtime (realm-per-context model); SM JSContext itself is not destroyed per iteration
- 2026-09-10-b8d8f5e5/realm-create-drop.run-3 note: realm = lazily-created global object per JsContext wrapper over the shared thread SM Runtime (realm-per-context model); SM JSContext itself is not destroyed per iteration
- **2026-09-10-ec4a0e5a/soak.run-1: ERROR** — soak aborted after 2195 cycles / 0 full segments at cycle 2195: iter 2195: marker verification failed (got "null") — fail-closed, no green numbers on wrong results — per-cycle series preserved in bench/results/2026-09-10-ec4a0e5a/soak.run-1.segments.jsonl
- 2026-09-10-ec4a0e5a/soak.run-1 note: bench failed — fail-closed; this document carries the error, not a partial baseline
- **2026-09-12-508502de/soak.run-1: ERROR** — soak aborted after 20895 cycles / 4 full segments at cycle 20895: iter 20895: marker verification failed (got "null") — fail-closed, no green numbers on wrong results — per-cycle series preserved in bench/results/2026-09-12-508502de/soak.run-1.segments.jsonl
- 2026-09-12-508502de/soak.run-1 note: bench failed — fail-closed; this document carries the error, not a partial baseline
- **2026-09-14-508502de/soak.run-1: ERROR** — soak aborted after 17684 cycles / 3 full segments at cycle 17684: iter 17684: marker verification failed (got "null") — fail-closed, no green numbers on wrong results — per-cycle series preserved in bench/results/2026-09-14-508502de/soak.run-1.segments.jsonl
- 2026-09-14-508502de/soak.run-1 note: bench failed — fail-closed; this document carries the error, not a partial baseline
- **2026-09-15-508502de/soak.run-1: ERROR** — soak aborted after 5662 cycles / 1 full segments at cycle 5662: iter 5662: marker verification failed (got "null") — fail-closed, no green numbers on wrong results — per-cycle series preserved in bench/results/2026-09-15-508502de/soak.run-1.segments.jsonl
- 2026-09-15-508502de/soak.run-1 note: bench failed — fail-closed; this document carries the error, not a partial baseline
- **2026-09-16-d61f26f1/soak.run-1: ERROR** — soak aborted after 25013 cycles / 4 full segments at cycle 25013: iter 25013: marker verification failed (got "null") — fail-closed, no green numbers on wrong results [#41 diag: embedder-url=data:text/html,<h1 id=b>benchmark-25013</h1> realm={"url":"data:text/html,<h1 id=b>benchmark-25013</h1>","rs"
- 2026-09-16-d61f26f1/soak.run-1 note: bench failed — fail-closed; this document carries the error, not a partial baseline
- 2026-09-30-c106f6b6/sqlite-bench.run-1 note: bun:sqlite: 10 prepared inserts + 1 indexed select per op (file DB, not :memory:) — synchronous face
- 2026-09-30-c106f6b6/sqlite-bench.run-2 note: bun:sqlite: 10 prepared inserts + 1 indexed select per op (file DB, not :memory:) — synchronous face
- 2026-09-30-c106f6b6/sqlite-bench.run-3 note: bun:sqlite: 10 prepared inserts + 1 indexed select per op (file DB, not :memory:) — synchronous face

## 5. Regression gate

`scripts/bench-regression-gate.sh` compares the two newest run-dates per bench on
the §3 key metrics: ≥20% adverse Δ = RED (direction-aware); soak slope-class
metrics are advisory-only (noise-dominated). Wire-in: local-ci optional segment
(`BAO_BENCH_REGRESSION=1`).

