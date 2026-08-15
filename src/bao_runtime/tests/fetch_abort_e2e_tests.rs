// @trace TEST-ENG-FETCH-ABORT [req:REQ-ENG-001 REQ-ENG-006] [level:e2e]
// fetch() AbortSignal wiring end-to-end (WHATWG fetch init.signal /
// Request.signal), three-state coverage:
//   1. pre-aborted signal: the fetch rejects immediately with DOMException
//      AbortError ("The operation was aborted") and NO request reaches the
//      wire,
//   2. mid-flight abort (server delays its response): rejects with
//      AbortError; the request DID reach the server (abort raced the
//      in-flight socket, cancelled via Signals.aborted + HTTPThread
//      schedule_shutdown) and the server observes the connection reset,
//   3. no signal: plain fetch resolves unchanged (regression guard).
// Plus the Request-signal entry path (fetch(new Request(url, {signal}))).
//
// Wire-level: a per-connection-threaded TCP server with path-routed delay +
// connection-reset observation. Exit strategy mirrors fetch_init_e2e_tests
// (parked HTTPThread is a non-daemon thread; force-exit sidesteps the
// mimalloc atexit double-free).

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

/// True once `buf` holds a complete HTTP/1.1 request (full header block and,
/// when Content-Length is present, the full body).
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

/// Path-routed server: `/a-mid*` and `/a-req*` delay their response by
/// `SLOW_DELAY_MS` (giving the client room to abort mid-flight); everything
/// else responds immediately. Per-connection threads so one slow connection
/// never blocks the others.
const SLOW_DELAY_MS: u64 = 800;

fn start_abort_server() -> (u16, Captured, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    // Server-side cancellation visibility: count slow-path connections whose
    // post-delay write/read hit EPIPE/ECONNRESET (client tore the socket
    // down on abort) instead of a clean exchange.
    let resets = Arc::new(Mutex::new(0usize));
    let sink = Arc::clone(&captured);
    let reset_sink = Arc::clone(&resets);
    listener.set_nonblocking(true).ok();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let sink = Arc::clone(&sink);
                    let reset_sink = Arc::clone(&reset_sink);
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
                                Err(_) => break,
                            }
                        }
                        let text = String::from_utf8_lossy(&buf).to_lowercase();
                        let slow = text.contains("/a-mid") || text.contains("/a-req");
                        if !text.is_empty() {
                            sink.lock().unwrap().push(text);
                        }
                        // Cancellation evidence #1: on an aborted connection
                        // the client tears the socket down while the server
                        // is still holding the (delayed) response — a probe
                        // read right after the request completes returns
                        // EOF/error instead of staying open until the write.
                        let mut closed_early = false;
                        if slow {
                            let mut probe = [0u8; 16];
                            stream
                                .set_read_timeout(Some(Duration::from_millis(50)))
                                .ok();
                            match stream.read(&mut probe) {
                                Ok(0) => closed_early = true,
                                Err(ref e)
                                    if e.kind() == ErrorKind::ConnectionReset
                                        || e.kind() == ErrorKind::BrokenPipe =>
                                {
                                    closed_early = true
                                }
                                _ => {}
                            }
                            std::thread::sleep(Duration::from_millis(SLOW_DELAY_MS));
                        }
                        let resp =
                            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                        let write_ok = stream.write_all(resp.as_bytes()).is_ok();
                        let _ = stream.flush();
                        // Cancellation evidence #2: post-response probe —
                        // writing into a torn-down socket errors, or the
                        // follow-up read hits ECONNRESET instead of EOF.
                        let mut probe = [0u8; 16];
                        let probe_res = stream.read(&mut probe);
                        let probe_err = matches!(probe_res, Err(ref e) if e.kind()
                            == ErrorKind::ConnectionReset
                            || e.kind() == ErrorKind::BrokenPipe);
                        if slow && (closed_early || !write_ok || probe_err) {
                            *reset_sink.lock().unwrap() += 1;
                        }
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    (port, captured, resets)
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

fn count_for(captured: &Captured, path: &str) -> usize {
    captured
        .lock()
        .unwrap()
        .iter()
        .filter(|req| req.contains(path))
        .count()
}

#[test]
fn test_fetch_abort_signal_three_states() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let (port, captured, resets) = start_abort_server();
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let js = format!(
        r#"
        (function() {{
            var base = "http://127.0.0.1:{port}";
            var out = {{ pre: null, mid: null, reqsig: null, plain: null }};

            // 1. Pre-aborted signal: reject immediately, no wire request.
            var c1 = new AbortController();
            c1.abort();
            fetch(base + "/a-pre", {{ signal: c1.signal }})
                .then(function() {{ out.pre = "resolved"; }},
                      function(e) {{
                          out.pre = "rejected:" + e.name + ":" + e.message +
                              ":" + (e instanceof DOMException) +
                              ":" + (e instanceof Error);
                      }});

            // 2. Mid-flight abort via init.signal (server delays 800ms).
            //    Deterministic ordering: the abort fires only after the Rust
            //    driver observed the request on the wire (it sets
            //    globalThis.__midSeen once the capture server recorded
            //    "/a-mid"). A blind setTimeout(40) assumed 40ms always covers
            //    connect+send+accept+record; under CPU oversubscription that
            //    is false and the abort raced ahead of the send. The re-arm
            //    poll keeps the abort strictly inside the server's 800ms
            //    response-delay window, which is the actual mid-flight
            //    property under test.
            var c2 = new AbortController();
            fetch(base + "/a-mid", {{ signal: c2.signal }})
                .then(function() {{ out.mid = "resolved"; }},
                      function(e) {{ out.mid = "rejected:" + e.name + ":" + e.message; }});
            function armAbortWhenSeen(ctrl, flag) {{
                function tryAbort() {{
                    if (globalThis[flag]) {{ ctrl.abort(); }}
                    else {{ setTimeout(tryAbort, 5); }}
                }}
                setTimeout(tryAbort, 40);
            }}
            armAbortWhenSeen(c2, "__midSeen");

            // 3. Mid-flight abort via Request.signal (Request base path) —
            //    same observed-then-abort gating as phase 2.
            var c3 = new AbortController();
            var req = new Request(base + "/a-req", {{ signal: c3.signal }});
            fetch(req)
                .then(function() {{ out.reqsig = "resolved"; }},
                      function(e) {{ out.reqsig = "rejected:" + e.name + ":" + e.message; }});
            armAbortWhenSeen(c3, "__reqSeen");

            // 4. No signal: plain fetch regression guard.
            fetch(base + "/a-plain")
                .then(function(r) {{ return r.text(); }})
                .then(function() {{ out.plain = "resolved"; }},
                      function(e) {{ out.plain = "rejected:" + (e && e.message); }});

            globalThis.__poll = function() {{
                // Array join stringifies null as "" — map to a sentinel so
                // the Rust driver can distinguish pending from settled.
                return [out.pre, out.mid, out.reqsig, out.plain]
                    .map(function(v) {{ return v === null ? "PENDING" : v; }})
                    .join("|");
            }};
            return "scheduled";
        }})()
        "#
    );

    let setup_out = eval_string(&mut ctx, &js);
    assert!(
        setup_out.contains("scheduled"),
        "fetch abort setup failed: {}",
        setup_out
    );

    // Drive the event loop until all four fetches settle. Two-part pump:
    //   1. realm-entered drain_and_check (timer wheel fires the setTimeout
    //      abort triggers; timer callbacks resolve through
    //      CurrentGlobalOrNull) — same contract as web_socket_async_tests;
    //   2. unconditional MiniEventLoop tick_without_idle + RunJobs, which
    //      dispatches the FetchTasklet ConcurrentTasks (resolve_tasklet).
    //      drain_and_check's own I/O tick is gated on active node HTTP
    //      servers, which this test has none of (the capture server is a
    //      raw std::net listener) — without this leg the fetch promises
    //      would never settle (mirrors the fetch_init_e2e_tests pump).
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut final_poll = String::new();
    let (mut mid_seen, mut req_seen) = (false, false);
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
        final_poll = eval_string(&mut ctx, r#"globalThis.__poll()"#);
        // Publish the wire-observation flags the gated aborts poll for. The
        // capture server records "/a-mid"/"/a-req" the moment its connection
        // thread reads the full request — before its 800ms response delay —
        // so flipping the flag here keeps each abort strictly inside the
        // mid-flight window instead of racing the send under load.
        if !mid_seen && count_for(&captured, "/a-mid http") >= 1 {
            mid_seen = true;
            eval_string(&mut ctx, "globalThis.__midSeen = true");
        }
        if !req_seen && count_for(&captured, "/a-req http") >= 1 {
            req_seen = true;
            eval_string(&mut ctx, "globalThis.__reqSeen = true");
        }
        if !final_poll.contains("PENDING") {
            break;
        }
    }
    assert!(
        !final_poll.contains("PENDING"),
        "fetch abort promises did not all settle: {}",
        final_poll
    );

    // 1. Pre-aborted: AbortError DOMException with the WHATWG message; the
    //    shim's DOMException inherits Error (instanceof Error true too).
    assert_eq!(
        final_poll.split('|').next().unwrap_or(""),
        "rejected:AbortError:The operation was aborted:true:true",
        "pre-aborted signal must reject with DOMException AbortError immediately: {}",
        final_poll
    );
    // No wire request for the pre-aborted fetch.
    assert_eq!(
        count_for(&captured, "/a-pre"),
        0,
        "pre-aborted fetch must not reach the network"
    );

    // 2. Mid-flight (init.signal): AbortError rejection, request reached the
    //    server (abort raced the in-flight socket, not the send).
    let parts: Vec<&str> = final_poll.split('|').collect();
    assert_eq!(
        parts[1], "rejected:AbortError:The operation was aborted",
        "mid-flight abort (init.signal) must reject with AbortError: {}",
        final_poll
    );
    assert_eq!(
        count_for(&captured, "/a-mid http"),
        1,
        "mid-flight abort must happen AFTER the request reached the server"
    );

    // 3. Mid-flight (Request.signal): same contract through the Request path.
    assert_eq!(
        parts[2], "rejected:AbortError:The operation was aborted",
        "mid-flight abort (Request.signal) must reject with AbortError: {}",
        final_poll
    );
    assert_eq!(
        count_for(&captured, "/a-req http"),
        1,
        "Request-signal abort must happen AFTER the request reached the server"
    );

    // 4. No-signal regression: plain fetch still resolves.
    assert_eq!(
        parts[3], "resolved",
        "no-signal fetch regressed: {}",
        final_poll
    );
    assert_eq!(
        count_for(&captured, "/a-plain http"),
        1,
        "no-signal fetch must complete exactly once"
    );

    // Server-side cancellation visibility (尽力断言): at least one aborted
    // slow-path connection must show a client-side teardown (early FIN or
    // reset/broken pipe) instead of a clean exchange. The server threads
    // observe this asynchronously (probe window + delayed response), so poll
    // for the observation with a bounded wait.
    let observe_deadline = Instant::now() + Duration::from_secs(5);
    let mut resets_seen = *resets.lock().unwrap();
    while resets_seen < 1 && Instant::now() < observe_deadline {
        std::thread::sleep(Duration::from_millis(50));
        resets_seen = *resets.lock().unwrap();
    }
    assert!(
        resets_seen >= 1,
        "abort should be observable server-side as a connection teardown (got {})",
        resets_seen
    );

    eprintln!(
        "[PASS] TEST-ENG-FETCH-ABORT e2e: pre-aborted (no wire request) + mid-flight abort init.signal/Request.signal (AbortError DOMException, server-side reset) + no-signal regression"
    );

    // Mirror fetch_init_e2e_tests exit strategy: park HTTPThread, force-exit.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
