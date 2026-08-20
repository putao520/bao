// @trace TEST-ENG-ABORT-TIMEOUT [req:REQ-ENG-006 REQ-ENG-001] [level:e2e]
// AbortSignal.timeout(ms) end-to-end, two phases in one test (the
// HTTPThread force-exit strategy at the tail kills the process, so this
// file carries exactly one #[test]):
//   A. shape: timeout(50) aborts within the pump window with a
//      DOMException reason named TimeoutError ("signal timed out";
//      instanceof DOMException + Error both true; code 23 via the
//      DOMException code map), the abort listener receives
//      {type:'abort', target:signal}, the signal is NOT aborted before the
//      timer, and the argument validation mirrors Node (negative / NaN /
//      non-number ms throw TypeError; 0 is legal);
//   B. fetch integration: fetch(slow path, {signal: AbortSignal.timeout(50)})
//      cancels the request and rejects with the same DOMException
//      AbortError shape as AbortController-driven aborts (REQ-ENG-001
//      init.signal); a concurrent plain fetch is undisturbed.
//
// Wire-level: per-connection-threaded TCP server, path-routed delay
// (mirrors fetch_abort_e2e_tests; /a-slow delays 800ms so the 50ms signal
// timeout fires well inside the in-flight window).

use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;
use mozjs::rooted;

type Captured = Arc<Mutex<Vec<String>>>;

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// True once `buf` holds a complete HTTP/1.1 request (full header block
/// and, when Content-Length is present, the full body).
fn request_complete(buf: &[u8]) -> bool {
    let Some(pos) = find_sub(buf, b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
    let clen = head.lines().find_map(|l| {
        l.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    match clen {
        Some(n) => buf.len() >= pos + 4 + n,
        None => true,
    }
}

const SLOW_DELAY_MS: u64 = 800;

/// Path-routed server: `/a-slow*` delays its response by `SLOW_DELAY_MS`
/// (the client's 50ms timeout fires well inside that window); everything
/// else responds immediately with body "ok". Per-connection threads so one
/// slow connection never blocks the fast-path control fetch.
fn start_slow_server() -> (u16, Captured) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    listener.set_nonblocking(true).ok();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let sink = Arc::clone(&sink);
                    std::thread::spawn(move || {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(500)))
                            .ok();
                        stream.set_nonblocking(false).ok();
                        let mut buf: Vec<u8> = Vec::new();
                        let mut chunk = [0u8; 4096];
                        loop {
                            match stream.read(&mut chunk) {
                                Ok(0) => break,
                                Ok(n) => {
                                    buf.extend_from_slice(&chunk[..n]);
                                    if request_complete(&buf) {
                                        break;
                                    }
                                }
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                                    // Read timeout / transient: stop collecting.
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                        let text = String::from_utf8_lossy(&buf).to_lowercase();
                        let slow = text.contains("/a-slow");
                        if !text.is_empty() {
                            sink.lock().unwrap().push(text);
                        }
                        if slow {
                            std::thread::sleep(Duration::from_millis(SLOW_DELAY_MS));
                        }
                        let resp =
                            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                        stream.write_all(resp.as_bytes()).ok();
                        let _ = stream.flush();
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    (port, captured)
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Two-part pump from fetch_abort_e2e_tests:
///   1. realm-entered drain_and_check (timer wheels fire — the timeout
///      static's abort included; timer callbacks resolve through
///      CurrentGlobalOrNull);
///   2. MiniEventLoop tick_without_idle + RunJobs (dispatches the
///      FetchTasklet ConcurrentTasks).
/// Polls `poll_js` until it holds no "PENDING" marker or the deadline
/// passes; returns the last poll value.
fn pump_until_settled(ctx: &mut JsContext, poll_js: &str, timeout: Duration) -> String {
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + timeout;
    let mut final_poll = String::new();
    while Instant::now() < deadline {
        {
            let mut cxm = ctx.cx();
            let global = bao_engine::context::thread_realm_global();
            if let Some(g) = global {
                rooted!(&in(cxm) let g_root = g);
                let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                timers::drain_and_check(realm_cx);
            } else {
                timers::drain_and_check(&mut cxm);
            }
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        std::thread::sleep(Duration::from_millis(2));
        final_poll = eval_string(ctx, poll_js);
        if !final_poll.contains("PENDING") {
            break;
        }
    }
    final_poll
}

#[test]
fn test_abort_signal_timeout_shape_and_fetch() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    // ── Phase A: AbortSignal.timeout(50) shape (no network) ──
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup_a = eval_string(
        &mut ctx,
        r#"
        (function() {
            var out = { pre: "PENDING", reason: "PENDING", code: "", inst: "",
                       event: "", notEarly: "", throws: "" };
            var s = AbortSignal.timeout(50);
            out.notEarly = String(s.aborted === false && s.reason === undefined);
            s.addEventListener('abort', function(ev) {
                out.event = ev.type + ":" + (ev.target === s);
            });
            setTimeout(function() {
                out.pre = String(s.aborted);
                out.reason = (s.reason && s.reason.name) + ":" + (s.reason && s.reason.message);
                out.code = String(s.reason && s.reason.code);
                out.inst = String(s.reason instanceof DOMException) +
                    ":" + String(s.reason instanceof Error);
            }, 120);
            // Validation mirrors Node: negative / NaN / non-number throw
            // TypeError; 0 is legal.
            var t = "";
            try { AbortSignal.timeout(-1); t += "neg-ok"; } catch (e) { t += "neg:" + e.name; }
            try { AbortSignal.timeout(NaN); t += "|nan-ok"; } catch (e) { t += "|nan:" + e.name; }
            try { AbortSignal.timeout("50"); t += "|str-ok"; } catch (e) { t += "|str:" + e.name; }
            try { AbortSignal.timeout(0); t += "|zero-ok"; } catch (e) { t += "|zero:" + e.name; }
            out.throws = t;
            globalThis.__pollA = function() {
                return [out.pre, out.reason, out.code, out.inst,
                        out.event, out.notEarly, out.throws].join("|");
            };
            return "scheduled";
        })()
        "#,
    );
    assert!(
        setup_a.contains("scheduled"),
        "AbortSignal.timeout setup failed: {}",
        setup_a
    );

    let poll_a = pump_until_settled(&mut ctx, r#"globalThis.__pollA()"#, Duration::from_secs(10));
    assert!(
        !poll_a.contains("PENDING"),
        "AbortSignal.timeout(50) did not settle within the pump window: {}",
        poll_a
    );
    assert_eq!(
        poll_a,
        "true|TimeoutError:signal timed out|23|true:true|abort:true|true|\
         neg:TypeError|nan:TypeError|str:TypeError|zero-ok",
        "AbortSignal.timeout shape must match Node semantics (TimeoutError \
         DOMException, listener dispatch, argument validation)"
    );

    // ── Phase B: fetch(slow, {signal: AbortSignal.timeout(50)}) ──
    let (port, _captured) = start_slow_server();
    std::thread::sleep(Duration::from_millis(50));

    let js_b = format!(
        r#"
        (function() {{
            var base = "http://127.0.0.1:{port}";
            var out = {{ tmo: "PENDING", ctl: "PENDING" }};
            fetch(base + "/a-slow", {{ signal: AbortSignal.timeout(50) }})
                .then(function() {{ out.tmo = "resolved"; }},
                      function(e) {{
                          out.tmo = "rejected:" + e.name + ":" + e.message +
                              ":" + (e instanceof DOMException) +
                              ":" + (e instanceof Error);
                      }});
            // Control: a concurrent plain fetch against the fast path must
            // be undisturbed by the timed-out fetch's abort.
            fetch(base + "/a-fast")
                .then(function(r) {{ return r.text(); }})
                .then(function(b) {{ out.ctl = "resolved:" + b; }},
                      function(e) {{ out.ctl = "rejected:" + (e && e.message); }});
            globalThis.__pollB = function() {{ return out.tmo + "|" + out.ctl; }};
            return "scheduled";
        }})()
        "#
    );
    let setup_b = eval_string(&mut ctx, &js_b);
    assert!(
        setup_b.contains("scheduled"),
        "timeout fetch setup failed: {}",
        setup_b
    );

    let poll_b = pump_until_settled(&mut ctx, r#"globalThis.__pollB()"#, Duration::from_secs(30));
    assert!(
        !poll_b.contains("PENDING"),
        "timeout fetch promises did not settle: {}",
        poll_b
    );
    let parts: Vec<&str> = poll_b.split('|').collect();
    assert_eq!(
        parts[0],
        "rejected:AbortError:The operation was aborted:true:true",
        "fetch with AbortSignal.timeout must reject with DOMException AbortError \
         (instanceof DOMException + Error both true): {}",
        poll_b
    );
    assert_eq!(
        parts[1],
        "resolved:ok",
        "concurrent plain fetch must be undisturbed by the timed-out fetch: {}",
        poll_b
    );

    eprintln!(
        "[PASS] TEST-ENG-ABORT-TIMEOUT e2e: timeout(50) TimeoutError DOMException shape \
         + validation + fetch cancellation (AbortError) + concurrent-fetch regression"
    );

    // Mirror fetch_abort_e2e_tests exit strategy: park HTTPThread,
    // force-exit.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
