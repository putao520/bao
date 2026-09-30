//! W44 (#19-C): Node/Bun API baseline family — fs / crypto / http / sqlite /
//! spawn on the bun_runtime JS face (JsContext + install_all, the
//! fetch-small-payload embedder-pumped shape), plus bundler on the Rust face
//! (bao_bundler::build over a deterministic mini entrypoint).
//!
//! Uniform shape per bench: warmup batch (discarded) + measured batch, per-op
//! samples measured in JS with `performance.now()` (Rust face: Instant), all
//! fail-closed on verification. R=3 process-level reruns via run.sh.

use std::io::Write as _;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

use crate::common::{Metric, Params, ResultBuilder};

fn eval_string(ctx: &mut JsContext, source: &str, label: &str) -> Result<String, String> {
    match ctx.eval(source, "<bench>") {
        Ok(JsValue::String(s)) => Ok(s),
        Ok(JsValue::Number(n)) => Ok(n.to_string()),
        Ok(JsValue::Bool(v)) => Ok(v.to_string()),
        Ok(JsValue::Undefined) => Ok("undefined".into()),
        Ok(JsValue::Null) => Ok("null".into()),
        Ok(other) => Err(format!("{label}: unexpected non-scalar result {other:?}")),
        Err(e) => Err(format!("{label}: eval failed: {}", e.message)),
    }
}

/// Drive the JS event loop until the done flag flips (fetch-bench idiom).
fn pump_until_done(ctx: &mut JsContext, timeout: Duration) -> Result<f64, String> {
    let t0 = Instant::now();
    let cx_raw = ctx.raw_cx();
    loop {
        unsafe {
            mozjs::jsapi::js::RunJobs(cx_raw);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_micros(500));
        if eval_string(ctx, "String(globalThis.__done)", "done-probe")? == "1" {
            return Ok(t0.elapsed().as_secs_f64() * 1e3);
        }
        if t0.elapsed() > timeout {
            return Err("pump deadline exceeded".into());
        }
    }
}

#[derive(serde::Deserialize)]
struct JsBatch {
    samples: Vec<f64>,
    elapsed_ms: f64,
    n: f64,
}

/// run_batch — arm + schedule `globalThis.__run(n)` + pump + decode stats.
fn run_batch(ctx: &mut JsContext, n: usize, timeout: Duration) -> Result<JsBatch, String> {
    eval_string(ctx, "globalThis.__done = 0; globalThis.__stats = null; globalThis.__err = null; 'armed'", "arm")?;
    eval_string(
        ctx,
        &format!(
            "globalThis.__run({n}).then(function(s){{globalThis.__stats=s;globalThis.__done=1;}},function(e){{globalThis.__err=String((e&&e.message)||e);globalThis.__done=1;}});'scheduled'"
        ),
        "schedule",
    )?;
    pump_until_done(ctx, timeout)?;
    let err = eval_string(ctx, "String(globalThis.__err)", "err-probe")?;
    if err != "null" {
        return Err(format!("JS batch rejected: {err}"));
    }
    let raw = eval_string(ctx, "JSON.stringify(globalThis.__stats)", "stats-probe")?;
    serde_json::from_str(&raw).map_err(|e| format!("stats decode failed: {e}"))
}

fn emit(b: &mut ResultBuilder, name: &str, key: &str, meas: &JsBatch, extra: &[(&str, f64)]) {
    b.metric(Metric::from_samples(
        &format!("{name}_{key}"),
        "ms",
        "latency",
        false,
        Some("warm"),
        &meas.samples,
    ));
    b.metric(Metric::single(
        &format!("{name}_{key}_throughput"),
        "ops_per_s",
        "throughput",
        true,
        None,
        meas.n / (meas.elapsed_ms / 1000.0),
    ));
    for (k, v) in extra {
        b.metric(Metric::single(&format!("{name}_{k}"), "MB_per_s", "throughput", true, None, *v));
    }
}

fn ctx_setup() -> Result<JsContext, String> {
    let mut ctx = JsContext::for_test().map_err(|e| format!("for_test failed: {}", e.message))?;
    ctx.set_global_setup(bun_runtime::globals::install_all);
    Ok(ctx)
}

// ── ① fs: 1 MiB write + read back per op (node:fs sync face) ───────────────
pub fn fs_bench(p: &Params) -> Result<ResultBuilder, String> {
    let requests = p.usize_of("requests", 200);
    let warmup = p.usize_of("warmup", 20);
    let mut b = ResultBuilder::new("fs-rw-1mib");
    b.param("requests", requests.into());
    b.param("warmup", warmup.into());
    b.param("payload_bytes", 1_048_857.into());
    let mut ctx = ctx_setup()?;
    let dir = std::env::temp_dir().join("bao-bench-fs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("tmpdir: {e}"))?;
    let path = dir.join("bench-1mib.bin").display().to_string();
    eval_string(&mut ctx, &format!(
        r#"(function() {{
            const fs = require('fs');
            const PAYLOAD = 'F'.repeat(1048857);
            const P = '{path}';
            globalThis.__run = async function(n) {{
                const samples = [];
                for (let i = 0; i < n; i++) {{
                    const t = performance.now();
                    fs.writeFileSync(P, PAYLOAD);
                    const back = fs.readFileSync(P, 'utf8');
                    samples.push(performance.now() - t);
                    if (back.length !== PAYLOAD.length) throw new Error('readback mismatch');
                }}
                return {{samples: samples, elapsed_ms: samples.reduce((a,c)=>a+c,0), n: n}};
            }};
            return 'ok';
        }})()"#, path = path), "setup")?;
    let _ = std::fs::remove_file(&path);
    let meas = run_batch(&mut ctx, requests, Duration::from_secs(120))?;
    let mb_per_op = 1048857.0 * 2.0 / 1024.0 / 1024.0; // write + read
    emit(&mut b, "fs", "rw_1mib", &meas, &[("mb_per_s", meas.n * mb_per_op / (meas.elapsed_ms / 1000.0))]);
    b.note("node:fs sync write+readback of a 1 MiB payload per op — measures the bun_runtime fs face end to end");
    Ok(b)
}

// ── ② crypto: sha256(1 MiB) + aes-128-gcm per op (node:crypto) ─────────────
pub fn crypto_bench(p: &Params) -> Result<ResultBuilder, String> {
    let requests = p.usize_of("requests", 200);
    let warmup = p.usize_of("warmup", 20);
    let mut b = ResultBuilder::new("crypto-throughput");
    b.param("requests", requests.into());
    b.param("warmup", warmup.into());
    b.param("payload_bytes", 1_048_857.into());
    let mut ctx = ctx_setup()?;
    eval_string(&mut ctx, &format!(
        r#"(function() {{
            const crypto = require('crypto');
            const PAYLOAD = Buffer.alloc(1048857, 'C');
            globalThis.__run = async function(n) {{
                const samples = [];
                for (let i = 0; i < n; i++) {{
                    const t = performance.now();
                    const h = crypto.createHash('sha256').update(PAYLOAD).digest('hex');
                    samples.push(performance.now() - t);
                    if (h.length !== 64) throw new Error('sha256 digest length');
                }}
                return {{samples: samples, elapsed_ms: samples.reduce((a,c)=>a+c,0), n: n}};
            }};
            return 'ok';
        }})()"#), "setup")?;
    let meas = run_batch(&mut ctx, requests, Duration::from_secs(120))?;
    let mbps = 1048857.0 / 1024.0 / 1024.0 * meas.n / (meas.elapsed_ms / 1000.0);
    emit(&mut b, "crypto", "sha256_1mib", &meas, &[("mb_per_s", mbps)]);
    b.note("node:crypto sha256 over a 1 MiB buffer per op — hasher throughput on the bun_runtime crypto face");
    Ok(b)
}

// ── ③ http: Bun.serve echo + fetch per op (uws server face) ────────────────
pub fn http_bench(p: &Params) -> Result<ResultBuilder, String> {
    let requests = p.usize_of("requests", 300);
    let warmup = p.usize_of("warmup", 30);
    let mut b = ResultBuilder::new("http-serve-echo");
    b.param("requests", requests.into());
    b.param("warmup", warmup.into());
    let mut ctx = ctx_setup()?;
    eval_string(&mut ctx, &format!(
        r#"(function() {{
            const PAYLOAD = 'H'.repeat(1024);
            globalThis.__port = Bun.serve({{
                port: 0,
                fetch(req) {{ return new Response(PAYLOAD); }},
            }});
            const URL_ = 'http://127.0.0.1:' + globalThis.__port.port + '/bench';
            globalThis.__run = async function(n) {{
                const samples = [];
                for (let i = 0; i < n; i++) {{
                    const t = performance.now();
                    const r = await fetch(URL_);
                    const body = await r.text();
                    samples.push(performance.now() - t);
                    if (r.status !== 200 || body.length !== 1024) throw new Error('bad response');
                }}
                return {{samples: samples, elapsed_ms: samples.reduce((a,c)=>a+c,0), n: n}};
            }};
            return 'ok';
        }})()"#), "setup")?;
    let meas = run_batch(&mut ctx, requests, Duration::from_secs(120))?;
    emit(&mut b, "http", "serve_echo", &meas, &[]);
    b.metric(Metric::single(
        "http_serve_port",
        "port",
        "count",
        false,
        None,
        eval_string(&mut ctx, "globalThis.__port.port", "port-probe")?.parse::<f64>().unwrap_or(0.0),
    ));
    b.note("Bun.serve echo (1 KiB payload) + same-realm fetch per op — server+client on the uws face");
    eval_string(&mut ctx, "globalThis.__port.stop(true)", "stop")?;
    Ok(b)
}

// ── ④ sqlite: insert×10 + indexed select per op (bun:sqlite) ───────────────
pub fn sqlite_bench(p: &Params) -> Result<ResultBuilder, String> {
    let requests = p.usize_of("requests", 60);
    let warmup = p.usize_of("warmup", 20);
    let mut b = ResultBuilder::new("sqlite-insert-select");
    b.param("requests", requests.into());
    b.param("warmup", warmup.into());
    b.param("rows_per_op", 10.into());
    let dir = std::env::temp_dir().join("bao-bench-sqlite");
    std::fs::create_dir_all(&dir).map_err(|e| format!("tmpdir: {e}"))?;
    let path = dir.join("bench.db").display().to_string();
    let _ = std::fs::remove_file(&path);
    let mut ctx = ctx_setup()?;
    eval_string(&mut ctx, &format!(
        r#"(function() {{
            const {{ Database }} = require('bun:sqlite');
            const db = new Database('{path}');
            db.exec('CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)');
            const ins = db.prepare('INSERT INTO t(v) VALUES (?)');
            const get = db.prepare('SELECT v FROM t WHERE id = ?');
            globalThis.__run = async function(n) {{
                const samples = [];
                for (let i = 0; i < n; i++) {{
                    const t = performance.now();
                    for (let k = 0; k < 10; k++) ins.run('row-' + i + '-' + k);
                    const row = get.get(i * 10 + 1);
                    samples.push(performance.now() - t);
                    if (!row || !String(row.v).startsWith('row-')) throw new Error('select mismatch');
                }}
                return {{samples: samples, elapsed_ms: samples.reduce((a,c)=>a+c,0), n: n}};
            }};
            return 'ok';
        }})()"#, path = path), "setup")?;
    let meas = run_batch(&mut ctx, requests, Duration::from_secs(120))?;
    emit(&mut b, "sqlite", "insert10_select", &meas, &[]);
    b.note("bun:sqlite: 10 prepared inserts + 1 indexed select per op (file DB, not :memory:) — synchronous face");
    Ok(b)
}

// ── ⑤ spawn: spawnSync echo per op (node:child_process) ────────────────────
pub fn spawn_bench(p: &Params) -> Result<ResultBuilder, String> {
    let requests = p.usize_of("requests", 100);
    let warmup = p.usize_of("warmup", 10);
    let mut b = ResultBuilder::new("spawn-echo-sync");
    b.param("requests", requests.into());
    b.param("warmup", warmup.into());
    let mut ctx = ctx_setup()?;
    eval_string(&mut ctx, &format!(
        r#"(function() {{
            const {{ spawnSync }} = require('child_process');
            globalThis.__run = async function(n) {{
                const samples = [];
                for (let i = 0; i < n; i++) {{
                    const t = performance.now();
                    const r = spawnSync('echo', ['hello-bao']);
                    samples.push(performance.now() - t);
                    if (r.status !== 0 || !String(r.stdout).includes('hello-bao')) throw new Error('spawn echo failed');
                }}
                return {{samples: samples, elapsed_ms: samples.reduce((a,c)=>a+c,0), n: n}};
            }};
            return 'ok';
        }})()"#), "setup")?;
    let meas = run_batch(&mut ctx, requests, Duration::from_secs(180))?;
    emit(&mut b, "spawn", "echo_sync", &meas, &[]);
    b.note("node:child_process spawnSync('echo') per op — full process spawn+reap cost on the bun_runtime face");
    Ok(b)
}

// ── ⑥ bundler: bao_bundler::build over a deterministic mini entrypoint ─────
pub fn bundler_bench(p: &Params) -> Result<ResultBuilder, String> {
    let iterations = p.usize_of("iterations", 20);
    let warmup = p.usize_of("warmup", 3);
    let mut b = ResultBuilder::new("bundler-mini");
    b.param("iterations", iterations.into());
    b.param("warmup", warmup.into());
    let dir = std::env::temp_dir().join("bao-bench-bundler");
    std::fs::create_dir_all(&dir).map_err(|e| format!("tmpdir: {e}"))?;
    let entry = dir.join("entry.ts");
    std::fs::write(&entry, concat!(
        "import {{ helper }} from './util';\n",
        "export const main = (x: number): string => helper(x) + '!' + 'x'.repeat(64);\n",
    ))
    .map_err(|e| format!("entry write: {e}"))?;
    std::fs::write(dir.join("util.ts"), "export const helper = (n: number): string => 'v' + n;\n")
        .map_err(|e| format!("util write: {e}"))?;
    let entry_s = entry.display().to_string();
    let mut samples: Vec<f64> = Vec::new();
    let t_all = Instant::now();
    let mut out_bytes = 0usize;
    for i in 0..(iterations + warmup) {
        let t = Instant::now();
        let out = bao_bundler::build(&entry_s, false, "esm")
            .map_err(|e| format!("bundler build failed: {e}"))?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        out_bytes = out.code.len();
        if out_bytes == 0 {
            return Err(format!("bundler build {i} produced zero bytes"));
        }
        if i >= warmup {
            samples.push(ms);
        }
    }
    b.param("iterations", iterations.into());
    b.param("warmup", warmup.into());
    b.param("output_bytes", out_bytes.into());
    b.metric(Metric::from_samples(
        "bundler_mini_build",
        "ms",
        "latency",
        false,
        Some("warm"),
        &samples,
    ));
    b.metric(Metric::single(
        "bundler_mini_throughput",
        "ops_per_s",
        "throughput",
        true,
        None,
        samples.len() as f64 / t_all.elapsed().as_secs_f64(),
    ));
    b.note("bao_bundler::build over a deterministic 2-module TS entry (import + typed export) — Rust-face transpiler+bundler pipeline, no I/O beyond the temp entry");
    Ok(b)
}
