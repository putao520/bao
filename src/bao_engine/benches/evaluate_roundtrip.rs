// @trace REQ-ENG-001
//! evaluate() JS↔native round-trip latency bench (GitHub issue #2).
//!
//! Measures the full round trip: JS source → `JsContext::eval` (compile +
//! execute + RunJobs) → `JsValue` back in native Rust. This is the same
//! SpiderMonkey evaluate surface that CDP `Runtime.evaluate` /
//! `Runtime.callFunctionOn` reach through the servo bridge
//! (`BridgeCommand::EvaluateJs` → script thread → SM evaluate) — the bench
//! drives the engine directly, so no browser/servo runtime is started.
//!
//! Run (fast: opt-level 2, reuses the bulk-test profile):
//! ```text
//! cargo bench -p bao_engine --bench evaluate_roundtrip --profile test-ci
//! ```
//! Full fidelity (fat-LTO release profile; much longer build):
//! ```text
//! cargo bench -p bao_engine --bench evaluate_roundtrip
//! ```
//!
//! Cases:
//! - `engine_init` — `JsContext::for_test()` (JSEngine + Runtime + TLS setup)
//! - `cold_first_eval` — first eval in the process (realm global + first compile)
//! - `cold_window_100` — evals #2..#100, JIT/IC warm-up trajectory
//! - `warm_simple_1p1` — `1+1` steady state (simple expression)
//! - `warm_json_stringify` — `JSON.stringify({...})` (medium: alloc + serialize)
//! - `warm_fn_call` — `__bench_fn(21)` on a pre-defined function
//!   (`Runtime.callFunctionOn` shape)
//! - `fresh_realm_first_eval` — first eval on a brand-new `JsContext` (engine
//!   and JIT warm, realm cold; closest analog to a fresh CDP page's first
//!   evaluate)
//!
//! Measurement notes: every iteration is timed individually with
//! `Instant::now()` (vDSO clock, ~2×20-40 ns overhead per sample, outside the
//! eval window only the call itself is inside). Upper percentiles include
//! SpiderMonkey GC pauses — that is honest round-trip latency, not noise.

use std::hint::black_box;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

/// Untimed iterations before the measured window of each warm case.
const WARMUP_ITERS: usize = 2_000;
/// Upper bound on measured samples per warm case.
const MAX_SAMPLES: usize = 20_000;
/// Wall-clock budget per warm case (whichever limit hits first stops the case).
const CASE_BUDGET: Duration = Duration::from_secs(3);
/// Individually-timed evals right after the first one (cold trajectory).
const COLD_WINDOW: usize = 100;

const SRC_SIMPLE: &str = "1+1";
const SRC_FN_DEF: &str = "globalThis.__bench_fn = (x) => x * 2;";
const SRC_FN_CALL: &str = "__bench_fn(21)";
const SRC_JSON: &str = "JSON.stringify({a:1,b:\"x\",c:[1,2,3],d:{e:true,f:[3,1,4,1,5,9,2,6,5,3,5]},g:\"hello world 0123456789\"})";
const EXPECT_JSON: &str = "{\"a\":1,\"b\":\"x\",\"c\":[1,2,3],\"d\":{\"e\":true,\"f\":[3,1,4,1,5,9,2,6,5,3,5]},\"g\":\"hello world 0123456789\"}";

fn fatal(label: &str, detail: String) -> ! {
    eprintln!("FATAL [{label}]: {detail}");
    std::process::exit(1);
}

/// eval with fail-closed error handling — no fake data on engine failure.
fn eval_checked(ctx: &mut JsContext, source: &str, label: &str) -> JsValue {
    match ctx.eval(black_box(source), "<bench>") {
        Ok(v) => v,
        Err(e) => fatal(label, format!("eval failed: {}", e.message)),
    }
}

fn verify_number(label: &str, v: &JsValue, expected: f64) {
    match v {
        JsValue::Number(n) if *n == expected => {}
        other => fatal(
            label,
            format!("expected Number({expected}), got {other:?}"),
        ),
    }
}

fn verify_json(label: &str, v: &JsValue) {
    match v {
        JsValue::String(s) if s == EXPECT_JSON => {}
        other => fatal(label, format!("expected JSON string mismatch: {other:?}")),
    }
}

struct Stats {
    n: usize,
    min_ns: u64,
    mean_ns: f64,
    p50_ns: u64,
    p90_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    max_ns: u64,
    cv: f64,
}

/// Nearest-rank percentile over sorted nanosecond samples.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    debug_assert!(!sorted.is_empty());
    let idx = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = idx.clamp(1, sorted.len()) - 1;
    sorted[idx]
}

fn stats(mut samples: Vec<u64>) -> Stats {
    debug_assert!(!samples.is_empty());
    samples.sort_unstable();
    let n = samples.len();
    let min_ns = samples[0];
    let max_ns = samples[n - 1];
    let mean_ns = samples.iter().map(|&s| s as f64).sum::<f64>() / n as f64;
    // Coefficient of variation (bench/METHODLOGY.md §3: cv > 5% ⇒ unstable).
    // Per-call latency distributions are tail-heavy by nature (GC pauses in
    // the upper tail), so cv here reflects distribution shape, not run-to-run
    // flakiness.
    let var = samples
        .iter()
        .map(|&s| {
            let d = s as f64 - mean_ns;
            d * d
        })
        .sum::<f64>()
        / n as f64;
    let cv = var.sqrt() / mean_ns;
    Stats {
        n,
        min_ns,
        mean_ns,
        p50_ns: percentile(&samples, 50.0),
        p90_ns: percentile(&samples, 90.0),
        p95_ns: percentile(&samples, 95.0),
        p99_ns: percentile(&samples, 99.0),
        max_ns,
        cv,
    }
}

/// One warm case: warmup untimed, then per-iteration timed evals with
/// verification (mismatch is fatal — no green numbers on wrong results).
fn warm_case(ctx: &mut JsContext, source: &str, label: &str, verify: impl Fn(&JsValue)) {
    for _ in 0..WARMUP_ITERS {
        let v = eval_checked(ctx, source, label);
        verify(&v);
    }
    let mut samples: Vec<u64> = Vec::with_capacity(MAX_SAMPLES);
    let budget_start = Instant::now();
    while samples.len() < MAX_SAMPLES && budget_start.elapsed() < CASE_BUDGET {
        let t0 = Instant::now();
        let v = match ctx.eval(black_box(source), "<bench>") {
            Ok(v) => v,
            Err(e) => fatal(label, format!("eval failed mid-run: {}", e.message)),
        };
        let dt = t0.elapsed();
        verify(&v);
        samples.push(dt.as_nanos() as u64);
    }
    report(label, &stats(samples));
}

fn report(label: &str, s: &Stats) {
    println!(
        "{:<24} n={:<6} min={:>9.3} p50={:>9.3} p90={:>9.3} p95={:>9.3} p99={:>9.3} max={:>10.3} mean={:>9.3} cv={:>5.1}% ops/s(p50)={:>8.0} (µs)",
        label,
        s.n,
        s.min_ns as f64 / 1000.0,
        s.p50_ns as f64 / 1000.0,
        s.p90_ns as f64 / 1000.0,
        s.p95_ns as f64 / 1000.0,
        s.p99_ns as f64 / 1000.0,
        s.max_ns as f64 / 1000.0,
        s.mean_ns / 1000.0,
        s.cv * 100.0,
        1e9 / s.p50_ns as f64,
    );
}

fn report_single(label: &str, ns: u64) {
    let s = stats(vec![ns]);
    report(label, &s);
}

fn main() {
    println!("bao evaluate() round-trip bench (issue #2) — bao_engine engine-direct drive");
    println!("per-call Instant timing; upper percentiles include SM GC pauses");
    println!();

    // ── Engine + context init (JSEngine, Runtime, TLS) ─────────────────────
    let t0 = Instant::now();
    let mut ctx = JsContext::for_test()
        .unwrap_or_else(|e| fatal("engine_init", format!("for_test failed: {}", e.message)));
    let engine_init_ns = t0.elapsed().as_nanos() as u64;
    report_single("engine_init", engine_init_ns);

    // ── Cold: very first eval (realm global creation + first compile) ──────
    let t0 = Instant::now();
    let v = eval_checked(&mut ctx, SRC_SIMPLE, "cold_first_eval");
    let cold_first_ns = t0.elapsed().as_nanos() as u64;
    verify_number("cold_first_eval", &v, 2.0);
    report_single("cold_first_eval", cold_first_ns);

    // ── Cold window: evals #2..#100, individually timed ────────────────────
    let mut cold_samples: Vec<u64> = Vec::with_capacity(COLD_WINDOW);
    for i in 0..COLD_WINDOW {
        let t0 = Instant::now();
        let v = eval_checked(&mut ctx, SRC_SIMPLE, "cold_window_100");
        let dt = t0.elapsed();
        if let JsValue::Number(n) = v {
            if n != 2.0 {
                fatal("cold_window_100", format!("iter {i}: expected 2.0, got {n}"));
            }
        } else {
            fatal("cold_window_100", format!("iter {i}: non-number result"));
        }
        cold_samples.push(dt.as_nanos() as u64);
    }
    report("cold_window_100", &stats(cold_samples));

    println!();

    // ── Warm cases ──────────────────────────────────────────────────────────
    warm_case(&mut ctx, SRC_SIMPLE, "warm_simple_1p1", |v| {
        verify_number("warm_simple_1p1", v, 2.0)
    });

    // Runtime.callFunctionOn shape: define the function once, then each
    // iteration is a pure call round-trip. The definition's completion value
    // is the assigned function object (assignment expression semantics) —
    // matched on discriminant only, never dereferenced or held (JsValue::Object
    // is a GC-unsafe transient variant).
    let v = eval_checked(&mut ctx, SRC_FN_DEF, "fn_def");
    match v {
        JsValue::Object(_) => {}
        other => fatal("fn_def", format!("expected function Object, got {other:?}")),
    }
    warm_case(&mut ctx, SRC_FN_CALL, "warm_fn_call", |v| {
        verify_number("warm_fn_call", v, 42.0)
    });

    warm_case(&mut ctx, SRC_JSON, "warm_json_stringify", |v| {
        verify_json("warm_json_stringify", v)
    });

    // ── Fresh realm: brand-new JsContext (engine + JIT warm, realm cold) ───
    // `for_test()` parasitizes the live thread Runtime, so this measures realm
    // global creation + first script on a fresh realm — the closest analog to
    // a fresh CDP page's first evaluate.
    let t0 = Instant::now();
    let mut ctx2 = JsContext::for_test()
        .unwrap_or_else(|e| fatal("fresh_realm", format!("second for_test failed: {}", e.message)));
    let realm_create_ns = t0.elapsed().as_nanos() as u64;
    report_single("fresh_realm_ctx_create", realm_create_ns);

    let t0 = Instant::now();
    let v = eval_checked(&mut ctx2, SRC_SIMPLE, "fresh_realm_first_eval");
    let fresh_first_ns = t0.elapsed().as_nanos() as u64;
    verify_number("fresh_realm_first_eval", &v, 2.0);
    report_single("fresh_realm_first_eval", fresh_first_ns);

    println!();
    println!("all cases verified (result values checked every iteration)");
}
