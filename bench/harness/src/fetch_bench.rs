//! Bench ④: small-payload `fetch` throughput on the Node stack
//! (`JsContext` + full `bun_runtime` globals + local Rust HTTP server).
//!
//! This is the embedder-pumped shape proven by the h2 e2e suite
//! (`h2_fetch_node_stack_e2e_tests.rs`): the fetch tasklets are driven by
//! RunJobs + `timers::with_event_loop(tick_without_idle)`. The CLI's own
//! post-eval drain currently parks on network waits (2026-09-10 probe —
//! recorded in results notes and issue #19), so the bench drives the loop
//! explicitly. Requests are sequential (await per request); per-request RTT
//! is measured inside JS with `performance.now()` and verified fail-closed
//! (status + length + content byte) on every response.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

use crate::common::{Metric, Params, ResultBuilder};

/// Fixed small response served to every request (deterministic body).
fn spawn_server(body: &str) -> std::io::Result<(u16, std::sync::Arc<std::sync::atomic::AtomicU64>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let served = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let served_thread = served.clone();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
        body.len(),
        body
    );
    let response = response.into_bytes();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let response = response.clone();
            let served = served_thread.clone();
            std::thread::spawn(move || {
                // keep-alive: serve until the client closes (read EOF)
                let mut buf = [0u8; 4096];
                loop {
                    // drain the request head (GET, no body)
                    loop {
                        match stream.read(&mut buf) {
                            Ok(0) => return,
                            Ok(n) => {
                                // naive: request heads are far below 4 KiB;
                                // a split head across reads just costs one
                                // extra pass through this loop.
                                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") || n < buf.len() {
                                    break;
                                }
                            }
                            Err(_) => return,
                        }
                    }
                    if stream.write_all(&response).is_err() {
                        return;
                    }
                    served.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
    });
    Ok((port, served))
}

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

/// Drive the JS thread's event loop (microtasks + network tasklets) until the
/// JS-side done flag is set. Mirrors the h2 e2e driving idiom; the 500 µs
/// sleep bounds pump overhead to well under the per-request RTT floor while
/// not busy-burning a whole core for the whole run.
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
            return Err(format!("pump deadline exceeded ({timeout:?})"));
        }
    }
}

#[derive(serde::Deserialize)]
struct JsBatch {
    samples: Vec<f64>,
    bytes: f64,
    elapsed_ms: f64,
    n: f64,
}

fn run_batch(
    ctx: &mut JsContext,
    n: usize,
    timeout: Duration,
) -> Result<JsBatch, String> {
    eval_string(
        ctx,
        "globalThis.__done = 0; globalThis.__stats = null; globalThis.__err = null; 'armed'",
        "arm",
    )?;
    eval_string(
        ctx,
        &format!(
            "globalThis.__run({n}).then(function(s){{globalThis.__stats = s; globalThis.__done = 1;}}, function(e){{globalThis.__err = String((e && e.message) || e); globalThis.__done = 1;}}); 'scheduled'"
        ),
        "schedule",
    )?;
    pump_until_done(ctx, timeout)?;
    let err = eval_string(ctx, "String(globalThis.__err)", "err-probe")?;
    if err != "null" {
        return Err(format!("JS batch rejected: {err}"));
    }
    let raw = eval_string(ctx, "JSON.stringify(globalThis.__stats)", "stats-probe")?;
    serde_json::from_str::<JsBatch>(&raw).map_err(|e| format!("stats JSON decode failed: {e}"))
}

pub fn run(p: &Params) -> Result<ResultBuilder, String> {
    let requests = p.usize_of("requests", 400);
    let warmup = p.usize_of("warmup", 50);
    let body_bytes = p.usize_of("body-bytes", 256);
    let budget_ms = p.f64_of("budget-ms", 120_000.0);

    let mut b = ResultBuilder::new("fetch-small-payload");
    b.param("requests", requests.into());
    b.param("warmup", warmup.into());
    b.param("body_bytes", body_bytes.into());
    b.param("budget_ms", budget_ms.into());
    b.param("server", "127.0.0.1 std::TcpListener, thread-per-conn keep-alive".into());
    b.param("concurrency", 1.into());

    let body = "B".repeat(body_bytes);
    let (port, served) = spawn_server(&body).map_err(|e| format!("server spawn failed: {e}"))?;
    let url = format!("http://127.0.0.1:{port}/bench");
    b.param("url_path", "http://127.0.0.1:<port>/bench".into());

    let mut ctx = JsContext::for_test()
        .map_err(|e| format!("for_test failed: {}", e.message))?;
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Probe the clock; performance.now is required for sub-ms per-request RTT.
    let clock = eval_string(&mut ctx, "typeof performance !== 'undefined' ? 'performance' : 'date'", "clock-probe")?;
    if clock != "performance" {
        return Err(
            "performance.now unavailable in runtime realm — per-request RTT would be clock-resolution-limited; refusing to emit fake-precision numbers".into(),
        );
    }

    let setup = format!(
        r#"(function() {{
            globalThis.__samples = null;
            const URL_ = '{url}';
            const BODY_LEN = {body_bytes};
            const MARK = 'B'.charCodeAt(0);
            globalThis.__run = async function(n) {{
                const samples = [];
                let bytes = 0;
                const t0 = performance.now();
                for (let i = 0; i < n; i++) {{
                    const s = performance.now();
                    const r = await fetch(URL_);
                    const t = await r.text();
                    if (r.status !== 200) throw new Error('status ' + r.status);
                    if (t.length !== BODY_LEN) throw new Error('body length ' + t.length + ' != ' + BODY_LEN);
                    if (t.charCodeAt(0) !== MARK) throw new Error('body content mismatch');
                    samples.push(performance.now() - s);
                    bytes += t.length;
                }}
                return {{samples: samples, bytes: bytes, elapsed_ms: performance.now() - t0, n: n}};
            }};
            return 'ok';
        }})()"#
    );
    let setup_ok = eval_string(&mut ctx, &setup, "setup")?;
    if setup_ok != "ok" {
        return Err(format!("setup returned {setup_ok:?}"));
    }

    let start = Instant::now();
    // Warmup batch (discarded) — JIT + connection + resolver warm.
    let warm_n = warmup;
    let warm = run_batch(&mut ctx, warm_n, Duration::from_secs(60))?;
    if warm.n as usize != warm_n {
        return Err(format!("warmup batch n mismatch: {} != {warm_n}", warm.n));
    }

    // Measured batch.
    let meas = run_batch(&mut ctx, requests, Duration::from_secs(90))?;
    let elapsed_total_ms = start.elapsed().as_secs_f64() * 1e3;
    if meas.n as usize != requests {
        return Err(format!("measured batch n mismatch: {} != {requests}", meas.n));
    }
    if meas.bytes as usize != requests * body_bytes {
        return Err(format!(
            "byte accounting mismatch: {} != {}",
            meas.bytes,
            requests * body_bytes
        ));
    }
    let served_count = served.load(std::sync::atomic::Ordering::Relaxed);
    let expected_served = (warm_n + requests) as u64;
    if served_count != expected_served {
        return Err(format!(
            "server saw {served_count} requests, expected {expected_served} — fail-closed"
        ));
    }

    b.param("warmup_served", warm_n.into());
    b.param("measured_served", served_count.into());
    b.metric(Metric::from_samples(
        "fetch_rtt",
        "ms",
        "latency",
        false,
        Some("warm"),
        &meas.samples,
    ));
    let js_ops = meas.n / (meas.elapsed_ms / 1000.0);
    b.metric(Metric::single("fetch_throughput", "ops_per_s", "throughput", true, None, js_ops));
    b.metric(Metric::single(
        "fetch_throughput_wall",
        "ops_per_s",
        "throughput",
        true,
        None,
        (warm_n + requests) as f64 / (elapsed_total_ms / 1000.0),
    ));
    b.metric(Metric::single("fetch_payload_mb_per_s", "MB_per_s", "throughput", true, None, meas.bytes / 1024.0 / 1024.0 / (meas.elapsed_ms / 1000.0)));
    b.note("sequential single-flight fetch (await per request) against loopback std::TcpListener — measures the bao fetch pipeline (resolver + client + promise plumbing), not kernel/network limits");
    b.note("per-request RTT measured in JS via performance.now; pump tick interval 500 µs bounds embedder overhead below the RTT floor");
    b.note("known separate defect: `bao -e` / `bao run *.mjs` top-level-await fetch parks in the CLI's own drain (2026-09-10 probe) — this bench uses the embedder-pumped shape instead; the CLI defect needs its own issue");
    Ok(b)
}
