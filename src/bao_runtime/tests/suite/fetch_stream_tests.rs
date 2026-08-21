// @trace TEST-ENG-STREAM [req:REQ-ENG-010] [level:system]
// Streaming response-body semantics (streaming default / W1):
//
//   1. chunked incremental delivery — per-chunk reader reads spread over
//      the server's inter-chunk delays (not one buffered terminal copy);
//   2. headers-arrival Promise resolve — the fetch Promise settles BEFORE
//      the body completes (the core W1 property);
//   3. byte-exact drain across the 256 KiB park high-water mark — park
//      engages with no reader, a later text() unparks and reproduces the
//      body exactly;
//   4. mid-body AbortSignal — the reader rejects with AbortError;
//   5. clone() on a streaming body throws TypeError;
//   6. endless body — early resolve + first chunk + park observability
//      (PENDING drained ⇒ the process is not kept alive by the parked,
//      unobserved stream), then reader.cancel() tears the stream down.
//
// The RSS-bounded endless-body variant (server blocked while parked) rides
// the W2 transport pause: park triggers
// `schedule_transport_pause_from_any_thread(id, Pause)` (h1 socket pause →
// kernel back-pressure; h2 stream window withholding); unpark resumes.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;
use mozjs::rooted;

/// Pin the streaming mode for this test process (restored on Drop).
fn streaming_guard() -> bun_runtime::fetch_api::FetchStreamingGuard {
    bun_runtime::fetch_api::set_fetch_streaming_override(true)
}

// ── Test servers ────────────────────────────────────────────────────────────

fn read_request_head(stream: &mut impl Read) {
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    while got.len() < 64 * 1024 {
        let Ok(n) = stream.read(&mut buf) else { return };
        if n == 0 {
            return;
        }
        got.extend_from_slice(&buf[..n]);
        if got.windows(4).any(|w| w == b"\r\n\r\n") {
            return;
        }
    }
}

/// Slow chunked HTTP/1.1 server: one chunk per `delay`, then the terminal.
/// Returns the port.
fn spawn_slow_chunked_server(chunks: Vec<Vec<u8>>, delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let _ = stream.write_all(&resp);
        let _ = stream.flush();
        for chunk in &chunks {
            std::thread::sleep(delay);
            let mut frame = format!("{:x}\r\n", chunk.len()).into_bytes();
            frame.extend_from_slice(chunk);
            frame.extend_from_slice(b"\r\n");
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
        }
        std::thread::sleep(delay / 2);
        let _ = stream.write_all(b"0\r\n\r\n");
        let _ = stream.flush();
    });
    port
}

/// Endless chunked server: streams `chunk` forever (until the connection
/// dies from cancel/abort). Returns the port.
fn spawn_endless_chunked_server(chunk: Vec<u8>, delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .and_then(|_| stream.flush());
        let mut frame = format!("{:x}\r\n", chunk.len()).into_bytes();
        frame.extend_from_slice(&chunk);
        frame.extend_from_slice(b"\r\n");
        loop {
            if stream.write_all(&frame).is_err() {
                return;
            }
            let _ = stream.flush();
            std::thread::sleep(delay);
        }
    });
    port
}

/// Headers-first server: head + first partial body immediately, then after
/// `mid_delay` the rest + terminal. Returns the port.
fn spawn_split_body_server(first: Vec<u8>, second: Vec<u8>, mid_delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        read_request_head(&mut stream);
        let mut open = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let mut f1 = format!("{:x}\r\n", first.len()).into_bytes();
        f1.extend_from_slice(&first);
        f1.extend_from_slice(b"\r\n");
        open.extend_from_slice(&f1);
        let _ = stream.write_all(&open);
        let _ = stream.flush();
        std::thread::sleep(mid_delay);
        let mut f2 = format!("{:x}\r\n", second.len()).into_bytes();
        f2.extend_from_slice(&second);
        f2.extend_from_slice(b"\r\n");
        let _ = stream.write_all(&f2);
        let _ = stream.write_all(b"0\r\n\r\n");
        let _ = stream.flush();
    });
    port
}

// ── Harness ─────────────────────────────────────────────────────────────────

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<fetch_stream_test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Two-part pump (mirrors fetch_abort_e2e_tests): realm-entered
/// drain_and_check (timers fire) + unconditional MiniEventLoop
/// tick_without_idle + RunJobs (FetchTasklet ConcurrentTask dispatch).
fn pump_once(ctx: &mut JsContext) {
    let cx_raw = ctx.raw_cx();
    {
        let mut cxm = ctx.cx();
        let global = bao_engine::context::thread_realm_global();
        if let Some(g) = global {
            mozjs::rooted!(&in(cxm) let g_root = g);
            let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            timers::drain_and_check(realm_cx);
        } else {
            timers::drain_and_check(&mut cxm);
        }
    }
    unsafe {
        mozjs_sys::jsapi::js::RunJobs(cx_raw);
    }
    timers::with_event_loop(|loop_| {
        loop_.tick_without_idle(std::ptr::null_mut());
    });
}

fn pump_until(ctx: &mut JsContext, done_expr: &str, deadline: Duration) -> String {
    let end = Instant::now() + deadline;
    loop {
        pump_once(ctx);
        let state = eval_string(ctx, &format!("({done_expr})"));
        if state != "false" {
            return state;
        }
        if Instant::now() >= end {
            return eval_string(ctx, &format!("({done_expr})"));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ── 1. Incremental chunked delivery ─────────────────────────────────────────

#[test]
fn streaming_chunked_delivery_is_incremental() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let body = "alpha-beta-gamma-delta";
    let chunks: Vec<Vec<u8>> = ["alpha-", "beta-", "gamma-", "delta"]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
    let port = spawn_slow_chunked_server(chunks, Duration::from_millis(120));
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let js = format!(
        r#"
        (function() {{
            var times = [];
            var parts = [];
            var out = {{ err: null }};
            fetch("http://127.0.0.1:{}/").then(function(r) {{
                var reader = r.body.getReader();
                (function pump() {{
                    return reader.read().then(function(res) {{
                        if (res.done) {{
                            globalThis.__done = true;
                            globalThis.__times = times.join(",");
                            globalThis.__body = parts.join("");
                            return;
                        }}
                        times.push(Date.now());
                        parts.push(String.fromCharCode.apply(null, res.value));
                        return pump();
                    }}, function(e) {{
                        out.err = (e && e.message) || String(e);
                        globalThis.__done = true;
                        globalThis.__body = "ERR:" + out.err;
                    }});
                }})();
            }}, function(e) {{
                globalThis.__done = true;
                globalThis.__body = "REJECT:" + ((e && e.message) || String(e));
            }});
            return "scheduled";
        }})()
        "#,
        port
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);

    pump_until(&mut ctx, "globalThis.__done === true", Duration::from_secs(20));
    let body_out = eval_string(&mut ctx, "globalThis.__body");
    assert_eq!(
        body_out, body,
        "reassembled stream must equal the full body"
    );

    let times: Vec<u64> = eval_string(&mut ctx, "globalThis.__times")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or(0))
        .collect();
    assert!(
        times.len() >= 4,
        "expected >=4 separate chunk reads (one per server chunk), got {} — \
         a buffered one-shot delivery collapses to fewer",
        times.len()
    );
    let spread = times.last().unwrap() - times.first().unwrap();
    assert!(
        spread >= 150,
        "chunk reads must be spread over the server's inter-chunk delays \
         (4 chunks × 120ms), got {}ms first→last",
        spread
    );
}

// ── 2. Headers-arrival Promise resolve ──────────────────────────────────────

#[test]
fn streaming_headers_resolve_before_body_completes() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let port = spawn_split_body_server(
        b"first-half-".to_vec(),
        b"second-half".to_vec(),
        Duration::from_millis(350),
    );
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let js = format!(
        r#"
        (function() {{
            fetch("http://127.0.0.1:{}/").then(function(r) {{
                globalThis.__resolveTime = Date.now();
                return r.text();
            }}).then(function(t) {{
                globalThis.__textTime = Date.now();
                globalThis.__body = t;
                globalThis.__done = true;
            }}, function(e) {{
                globalThis.__body = "ERR:" + ((e && e.message) || String(e));
                globalThis.__done = true;
            }});
            return "scheduled";
        }})()
        "#,
        port
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);

    pump_until(&mut ctx, "globalThis.__done === true", Duration::from_secs(20));
    let body = eval_string(&mut ctx, "globalThis.__body");
    assert_eq!(body, "first-half-second-half", "full body must survive the pull loop");

    let resolve = eval_string(&mut ctx, "globalThis.__resolveTime").parse::<u64>().unwrap_or(0);
    let text_done = eval_string(&mut ctx, "globalThis.__textTime").parse::<u64>().unwrap_or(0);
    assert!(
        text_done - resolve >= 250,
        "the fetch Promise must resolve at headers arrival, well BEFORE the \
         350ms-delayed body completes (resolve={}, text done={}, delta={}ms)",
        resolve,
        text_done,
        text_done.saturating_sub(resolve)
    );
}

// ── 3. Byte-exact drain across the park high-water mark ─────────────────────

#[test]
fn streaming_drain_across_park_is_byte_exact() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    // 48 × 16 KiB = 768 KiB total — well past the 256 KiB park mark.
    const CHUNKS: usize = 48;
    const CHUNK_LEN: usize = 16 * 1024;
    // ASCII-only pattern (< 0x80): text() decodes UTF-8, which is lossy
    // for arbitrary binary — the byte-exactness proof must stay in the
    // identity-decode range.
    let chunk: Vec<u8> = (0..CHUNK_LEN).map(|i| ((i * 7 + 11) % 128) as u8).collect();
    let all: Vec<Vec<u8>> = (0..CHUNKS).map(|_| chunk.clone()).collect();
    let port = spawn_slow_chunked_server(all, Duration::from_millis(4));
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Stage 1: fetch, let the WHOLE body arrive while NOBODY reads (the
    // staging crosses the 256 KiB park mark with no pull).
    let js = format!(
        r#"
        (function() {{
            fetch("http://127.0.0.1:{}/").then(function(r) {{
                globalThis.__resp = r;
                globalThis.__resolved = true;
            }}, function(e) {{
                globalThis.__resolved = true;
                globalThis.__fail = (e && e.message) || String(e);
            }});
            return "scheduled";
        }})()
        "#,
        port
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);
    pump_until(&mut ctx, "globalThis.__resolved === true", Duration::from_secs(20));
    // Let the remainder of the body stream in unread (park territory).
    let end = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < end {
        pump_once(&mut ctx);
        std::thread::sleep(Duration::from_millis(5));
    }

    // Stage 2: text() unparks and drains everything.
    eval_string(
        &mut ctx,
        r#"
        globalThis.__resp.text().then(function(t) {
            globalThis.__body = t;
            globalThis.__done = true;
        }, function(e) {
            globalThis.__body = "ERR:" + ((e && e.message) || String(e));
            globalThis.__done = true;
        });
        "#,
    );
    pump_until(&mut ctx, "globalThis.__done === true", Duration::from_secs(30));
    let body = eval_string(&mut ctx, "globalThis.__body");
    assert!(
        !body.starts_with("ERR:"),
        "text() across the park mark failed: {}",
        body
    );

    // Byte-exactness in Rust (the expected body is 768 KiB — comparing in
    // JS avoids ferrying it through the eval string boundary).
    let check = eval_string(
        &mut ctx,
        r#"
        (function() {
            var expected = "";
            var chunk = new Uint8Array(16384);
            for (var i = 0; i < chunk.length; i++) chunk[i] = (i * 7 + 11) % 128;
            var s = String.fromCharCode.apply(null, chunk);
            for (var j = 0; j < 48; j++) expected += s;
            var got = globalThis.__body;
            if (got.length !== expected.length) return "LEN:" + got.length + ":" + expected.length;
            for (var k = 0; k < expected.length; k += 4096) {
                if (got.substring(k, k + 4096) !== expected.substring(k, k + 4096)) {
                    return "MISMATCH@" + k;
                }
            }
            return "EXACT";
        })()
        "#,
    );
    assert_eq!(check, "EXACT", "park→unpark drain must be byte-exact: {}", check);
}

// ── 4. Mid-body AbortSignal ─────────────────────────────────────────────────

#[test]
fn streaming_abort_mid_body_rejects_reader_with_abort_error() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let port = spawn_endless_chunked_server(vec![0x41u8; 8192], Duration::from_millis(20));
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let js = format!(
        r#"
        (function() {{
            var ctrl = new AbortController();
            fetch("http://127.0.0.1:{}/", {{ signal: ctrl.signal }}).then(function(r) {{
                globalThis.__ctrl = ctrl;
                globalThis.__reader = r.body.getReader();
                return globalThis.__reader.read();
            }}).then(function(first) {{
                globalThis.__firstLen = first.value ? first.value.byteLength : 0;
                globalThis.__abortTime = Date.now();
                globalThis.__ctrl.abort();
                // The read racing the abort settles the outcome; a second
                // read (post-abort) must surface the AbortError.
                return globalThis.__reader.read();
            }}).then(function(res) {{
                globalThis.__body = "UNEXPECTED-READ:" + (res && res.done);
                globalThis.__done = true;
            }}, function(e) {{
                globalThis.__body = (e && e.name) + ":" + ((e && e.message) || "");
                globalThis.__done = true;
            }});
            return "scheduled";
        }})()
        "#,
        port
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);

    pump_until(&mut ctx, "globalThis.__done === true", Duration::from_secs(20));
    let first_len = eval_string(&mut ctx, "globalThis.__firstLen || 0")
        .parse::<usize>()
        .unwrap_or(0);
    assert!(first_len > 0, "first chunk must arrive before the abort");
    let outcome = eval_string(&mut ctx, "globalThis.__body");
    assert!(
        outcome.starts_with("AbortError"),
        "post-abort read must reject with AbortError, got: {}",
        outcome
    );
}

// ── 5. clone() on a streaming body throws ───────────────────────────────────

#[test]
fn streaming_clone_throws_type_error() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let port = spawn_slow_chunked_server(vec![b"clone-me".to_vec()], Duration::from_millis(5));
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let js = format!(
        r#"
        (function() {{
            fetch("http://127.0.0.1:{}/").then(function(r) {{
                try {{
                    r.clone();
                    globalThis.__clone = "no-throw";
                }} catch (e) {{
                    globalThis.__clone = e instanceof TypeError ? "TypeError" : e.name;
                }}
                return r.text();
            }}).then(function(t) {{
                globalThis.__body = t;
                globalThis.__done = true;
            }}, function(e) {{
                globalThis.__body = "ERR:" + ((e && e.message) || String(e));
                globalThis.__done = true;
            }});
            return "scheduled";
        }})()
        "#,
        port
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);

    pump_until(&mut ctx, "globalThis.__done === true", Duration::from_secs(20));
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__clone"),
        "TypeError",
        "clone() on a streaming body must throw TypeError"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__body"),
        "clone-me",
        "the stream must remain consumable after the failed clone"
    );
}

// ── 6. Endless body: early resolve + park observability + cancel ────────────

#[test]
fn streaming_endless_body_resolves_early_parks_and_cancels() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let port = spawn_endless_chunked_server(vec![0x42u8; 64 * 1024], Duration::from_millis(5));
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let js = format!(
        r#"
        (function() {{
            fetch("http://127.0.0.1:{}/").then(function(r) {{
                globalThis.__resp = r;
                globalThis.__reader = r.body.getReader();
                return globalThis.__reader.read();
            }}).then(function(first) {{
                globalThis.__firstLen = first.value ? first.value.byteLength : 0;
                globalThis.__resolved = true;
            }}, function(e) {{
                globalThis.__resolved = true;
                globalThis.__fail = (e && e.message) || String(e);
            }});
            return "scheduled";
        }})()
        "#,
        port
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);

    pump_until(&mut ctx, "globalThis.__resolved === true", Duration::from_secs(20));
    let first_len = eval_string(&mut ctx, "globalThis.__firstLen || 0")
        .parse::<usize>()
        .unwrap_or(0);
    assert!(first_len > 0, "first chunk must arrive (early resolve + first read)");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__fail || \"\""),
        "",
        "endless-body fetch must resolve cleanly at headers"
    );

    // Let the unobserved staging cross the 256 KiB park mark (server keeps
    // sending; nobody reads).
    let end = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < end {
        pump_once(&mut ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !bun_runtime::fetch_async::has_pending(),
        "a parked unobserved stream must leave the PENDING registry (the \
         fetch no longer keeps the event loop ticking — process-exit capable)"
    );

    // reader.cancel() tears the stream down (Canceled + transport abort).
    eval_string(
        &mut ctx,
        r#"
        globalThis.__reader.cancel().then(function() {
            globalThis.__cancelled = true;
        }, function() {
            globalThis.__cancelled = true;
        });
        "#,
    );
    let state = pump_until(&mut ctx, "globalThis.__cancelled === true", Duration::from_secs(20));
    assert_eq!(state, "true", "reader.cancel() must settle");
}

// ── 7. RSS-bounded endless body (W2 transport pause wired) ──────────────────

/// 10 concurrent endless bodies: each fetch resolves, reads ONE chunk, then
/// parks (staging ≥ 256 KiB, no reader). With the W2 transport pause the
/// h1 sockets stop reading (kernel back-pressure blocks the in-process
/// servers' writes); without it the aggregate ingress is ~640 KiB/ms —
/// gigabytes over the 3s window. Budget per parked stream: staging
/// ≤ high-water (256 KiB) + one lookahead chunk + both kernel buffers;
/// the bound allows ~6 MiB per stream, >30× below the unbounded rate.
#[test]
fn streaming_endless_body_rss_bounded() {
    let _guard = streaming_guard();
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    const STREAMS: usize = 10;
    let mut ports = Vec::with_capacity(STREAMS);
    for _ in 0..STREAMS {
        ports.push(spawn_endless_chunked_server(
            vec![0x43u8; 64 * 1024],
            Duration::from_millis(1),
        ));
    }
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let url_list = ports
        .iter()
        .map(|p| format!("\"http://127.0.0.1:{}\"", p))
        .collect::<Vec<_>>()
        .join(",");
    let js = format!(
        r#"
        (function() {{
            var urls = [{urls}];
            var doneCount = 0;
            globalThis.__readers = [];
            for (var i = 0; i < urls.length; i++) {{
                (function(url) {{
                    fetch(url).then(function(r) {{
                        var reader = r.body.getReader();
                        globalThis.__readers.push(reader);
                        return reader.read();
                    }}).then(function() {{
                        doneCount++;
                        if (doneCount === urls.length) globalThis.__parked = true;
                    }}, function() {{
                        doneCount++;
                        if (doneCount === urls.length) globalThis.__parked = true;
                    }});
                }})(urls[i]);
            }}
            return "scheduled";
        }})()
        "#,
        urls = url_list
    );
    let out = eval_string(&mut ctx, &js);
    assert!(out.contains("scheduled"), "setup failed: {}", out);
    pump_until(&mut ctx, "globalThis.__parked === true", Duration::from_secs(30));
    // Every stream must have resolved and read its first chunk.
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__readers.length"),
        format!("{}", STREAMS),
        "all {} endless fetches must resolve and read a first chunk",
        STREAMS
    );

    fn rss_kb() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest.trim_end_matches(" kB").trim().parse().unwrap_or(0);
            }
        }
        0
    }

    let before = rss_kb();
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        pump_once(&mut ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let after = rss_kb();
    assert!(
        after.saturating_sub(before) < 64 * 1024,
        "3s of 10 parked unobserved endless bodies must keep RSS bounded \
         (park: staging ≤ high-water per stream + W2 transport pause), \
         grew {} KB → {} KB",
        before,
        after
    );
}
