// @trace TEST-ENG-FETCH-NB [req:REQ-ENG-001] [level:e2e]
// Upstream bun 77afa71e9 absorption probe: fetch() responses that cannot
// have a body (204, 205, 304, and any response to HEAD) must expose
// `body === null`; text() resolves "" without setting bodyUsed; clone()
// after text() must not throw; a real 200 body is unchanged.
//
// Raw-socket server mirrors the upstream wire shapes (bun
// test/js/web/fetch/body.test.ts "a fetch() Response that cannot have a
// body"), as an honest keep-alive origin (see start_wire_server for why
// `Connection: close` answers are avoided). Exit strategy mirrors
// fetch_headers_e2e_tests (parked HTTPThread is non-daemon; force-exit
// after shutdown).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

/// Raw-wire server: answers by path (HEAD requests get the HEAD wire shape).
/// Honest keep-alive — one connection may carry several sequential probes
/// (the client pools), each answer is followed by reading the next request
/// on the same socket until EOF/idle-timeout. This deliberately avoids
/// `Connection: close` answers so the probes stay independent of close
/// framing — that interaction (2xx + forced Content-Length 0 + close ⇒
/// ContinueStreaming ⇒ EOF) is root-cured and locked by
/// bun_http's connection_close_guard_tests. Serves for 60s max.
fn start_wire_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).ok();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(500)))
                        .ok();
                    stream.set_nonblocking(false).ok();
                    // Keep-alive request loop: serve every request on this
                    // connection until EOF or a read timeout.
                    loop {
                        let mut req = Vec::new();
                        let mut chunk = [0u8; 4096];
                        // Read until the request head is complete.
                        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut chunk) {
                                Ok(0) => break,
                                Ok(n) => req.extend_from_slice(&chunk[..n]),
                                Err(_) => break,
                            }
                        }
                        if req.is_empty() {
                            break;
                        }
                        let first = String::from_utf8_lossy(&req);
                        let first_line = first.lines().next().unwrap_or("").to_lowercase();
                        let answer: &[u8] = if first_line.starts_with("head ") {
                            // 200 to a HEAD request: the GET body's
                            // Content-Length, no body bytes follow (RFC 9110
                            // §9.3.2).
                            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n"
                        } else if first_line.contains("/205") {
                            // RFC 9110 §15.3.6 forbids content on a 205, but
                            // framing can carry it; the bytes must be received
                            // and dropped.
                            b"HTTP/1.1 205 Reset Content\r\nContent-Length: 5\r\n\r\nhello"
                        } else if first_line.contains("/304") {
                            b"HTTP/1.1 304 Not Modified\r\nETag: \"x\"\r\n\r\n"
                        } else if first_line.contains("/200") {
                            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"
                        } else {
                            // 204: header block ends the message.
                            b"HTTP/1.1 204 No Content\r\n\r\n"
                        };
                        if stream.write_all(answer).is_err() {
                            break;
                        }
                        let _ = stream.flush();
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    port
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<nb>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true".into() } else { "false".into() },
        Ok(JsValue::Null) => "null".into(),
        Ok(JsValue::Undefined) => "undefined".into(),
        Ok(JsValue::Object(_)) => "[object]".into(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Drive the event loop until `__nb_done` is set (fetch e2e pump pattern).
fn pump_until_done(ctx: &mut JsContext, timeout: Duration) -> bool {
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(2));
        let status = eval_string(ctx, r#"globalThis.__nb_done ? "DONE" : "PEND""#);
        if status == "DONE" {
            return true;
        }
    }
    false
}

/// Run the five null-body probes against a fresh wire server and realm.
/// Returns (completed, report, bad_count). Shared by the streaming-default
/// and buffered-pinned tests — both delivery paths must expose the same
/// null-body Response shape.
fn run_null_body_probes() -> (bool, String, String) {
    let port = start_wire_server();
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Each probe records one line: "OK <label> | <raw>" or "BAD <label> | <judgement>".
    // Order: fast/EOF-delimited cases first, the 205-with-content (whose body
    // must be dropped) last so a wedge there cannot hide the other evidence.
    let js = format!(
        r#"
        (function() {{
            var base = "http://127.0.0.1:{port}";
            var lines = [];
            function record(ok, label, detail) {{
                lines.push((ok ? "OK " : "BAD ") + label + " | " + detail);
            }}
            function probe(label, path, init, expect) {{
                return fetch(base + path, init).then(function(res) {{
                    var info = {{
                        status: res.status,
                        bodyIsNull: res.body === null,
                        bodyType: res.body === null ? "null" : typeof res.body,
                        headersConn: res.headers.get("connection"),
                        headCl: res.headers.get("content-length"),
                        bodyUsed0: res.bodyUsed
                    }};
                    return res.text().then(function(t) {{
                        info.text = t;
                        info.bodyUsedAfterText = res.bodyUsed;
                        try {{
                            var c = res.clone();
                            info.cloneThrew = false;
                            info.cloneBodyIsNull = c.body === null;
                        }} catch (e) {{
                            info.cloneThrew = true;
                            info.cloneErr = (e && e.message) || String(e);
                        }}
                        // Repeatable read is a null-body semantic only (a used
                        // real body rejects — spec behavior, probed separately).
                        var second;
                        if (expect.nullBody) {{
                            second = res.text().then(function(t2) {{
                                info.textAgain = t2;
                                return info;
                            }}, function(e2) {{
                                info.textAgain = "REJECTED: " + ((e2 && e2.message) || String(e2));
                                return info;
                            }});
                        }} else {{
                            info.textAgain = "skipped";
                            second = Promise.resolve(info);
                        }}
                        return second;
                    }}).then(function(info) {{
                        var bad = [];
                        if (info.status !== expect.status) bad.push("status=" + info.status);
                        if (expect.nullBody && !info.bodyIsNull) bad.push("body=" + info.bodyType);
                        if (!expect.nullBody && info.bodyIsNull) bad.push("body=null");
                        if (info.text !== expect.text) bad.push("text=" + JSON.stringify(info.text));
                        if (info.bodyUsedAfterText !== expect.bodyUsedAfterText)
                            bad.push("bodyUsed=" + info.bodyUsedAfterText);
                        // clone() after text() must succeed for a null body;
                        // for a real (used) body it must THROW — spec behavior
                        // (upstream 77afa71e9 repro: "a real body, unchanged").
                        if (expect.nullBody && info.cloneThrew) bad.push("cloneThrew: " + info.cloneErr);
                        if (!expect.nullBody && !info.cloneThrew) bad.push("clone-after-used-text did not throw");
                        if (expect.cloneBodyIsNull !== undefined && info.cloneBodyIsNull !== expect.cloneBodyIsNull)
                            bad.push("cloneBody=" + info.cloneBodyIsNull);
                        if (expect.nullBody && info.textAgain !== "")
                            bad.push("textAgain=" + JSON.stringify(info.textAgain));
                        record(bad.length === 0, label, bad.length ? bad.join("; ") : JSON.stringify(info));
                    }});
                }}).catch(function(e) {{
                    record(false, label, "fetch rejected: " + (e && e.name) + ": " + (e && e.message) +
                        " cause=" + (e && e.cause && e.cause.code));
                }});
            }}
            var cases = [
                ["200-real-body",  "/200", {{}},                       {{ status: 200, nullBody: false, text: "hello", bodyUsedAfterText: true }}],
                ["204",            "/204", {{}},                       {{ status: 204, nullBody: true,  text: "",     bodyUsedAfterText: false, cloneBodyIsNull: true  }}],
                ["304",            "/304", {{}},                       {{ status: 304, nullBody: true,  text: "",     bodyUsedAfterText: false, cloneBodyIsNull: true  }}],
                ["200-to-HEAD",    "/200", {{ method: "HEAD" }},       {{ status: 200, nullBody: true,  text: "",     bodyUsedAfterText: false, cloneBodyIsNull: true  }}],
                ["205-with-content", "/205", {{}},                     {{ status: 205, nullBody: true,  text: "",     bodyUsedAfterText: false, cloneBodyIsNull: true  }}]
            ];
            var chain = Promise.resolve();
            cases.forEach(function(c) {{
                chain = chain.then(function() {{ return probe(c[0], c[1], c[2], c[3]); }});
            }});
            chain.then(function() {{
                globalThis.__nb_report = lines.join("\n");
                globalThis.__nb_bad = lines.filter(function(l) {{ return l.indexOf("BAD ") === 0; }}).length;
                globalThis.__nb_done = true;
            }});
            return "scheduled";
        }})()
        "#
    );

    let setup_out = eval_string(&mut ctx, &js);
    assert!(
        setup_out.contains("scheduled"),
        "fetch null-body probe setup failed: {}",
        setup_out
    );

    let done = pump_until_done(&mut ctx, Duration::from_secs(20));
    let report = eval_string(&mut ctx, r#"globalThis.__nb_report || "<no report>""#);
    let bad_count = eval_string(&mut ctx, r#"String(globalThis.__nb_bad)"#);

    eprintln!(
        "=== fetch null-body probe (port {}) ===\n{}\n=== bad_count={} done={} ===",
        port, report, bad_count, done
    );
    (done, report, bad_count)
}

#[test]
fn test_fetch_null_body_statuses() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let (done, report, bad_count) = run_null_body_probes();
    assert!(done, "probe never completed within 20s — report so far:\n{}", report);
    assert!(
        bad_count == "0",
        "null-body semantics violated ({} BAD lines):\n{}",
        bad_count,
        report
    );

    // Park HTTPThread, force-exit (same strategy as fetch_headers_e2e_tests).
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}

/// Buffered-delivery mode (the legacy single-outcome flow, pinned via the
/// fetch_api test hook): the same null-body Response shape must hold when
/// the head+body arrive as one collected outcome.
#[test]
fn test_fetch_null_body_statuses_buffered_mode() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let _streaming_off = bun_runtime::fetch_api::set_fetch_streaming_override(false);
    let (done, report, bad_count) = run_null_body_probes();
    assert!(done, "buffered probe never completed within 20s — report so far:\n{}", report);
    assert!(
        bad_count == "0",
        "null-body semantics violated in buffered mode ({} BAD lines):\n{}",
        bad_count,
        report
    );

    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
