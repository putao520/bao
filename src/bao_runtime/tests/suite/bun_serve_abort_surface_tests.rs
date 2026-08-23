// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// domain-check a4ed5948b8(own-idiom fix): Bun.serve abort surface + in-spin
// timer settle. Two regressions guarded:
//
// 1. `await new Promise(r => setTimeout(r, 50))` inside a fetch handler used
//    to NEVER settle: the promise spin drained microtasks + ConcurrentTasks
//    but never fired BAO_REGISTRY timers, hit SERVE_PROMISE_POLL_MAX_ITERS,
//    and the caller wrote 404 "Not Found" to a live client. The spin now
//    drives `timers::drain_one_pass` per iteration (fires due wall-clock
//    timers) — the handler's 50ms timer resolves in-spin and the client gets
//    the handler's real 200 body.
// 2. A connection closing mid-dispatch had no abort surface at all: no
//    on_aborted registration existed, so the spin burned the JS thread to
//    the cap and then wrote 404 to a dead socket. The route handler now
//    registers an on_aborted latch (node_http2.rs:2046 pattern); the spin
//    polls it every iteration, exits early, and the dispatch ABANDONS the
//    response (never writes to the dead socket) — observable via the
//    `bun_api::SERVE_ABORT_COUNT` counter. The follow-up request on a fresh
//    connection proves the server recovered (not wedged in the cap spin).

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

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

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

/// Drive the JS thread's MiniEventLoop (fetch e2e pattern).
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One raw HTTP/1.1 GET, pumping the JS-thread MiniEventLoop between read
/// attempts (the uWS dispatch only runs when the loop ticks — a blocking
/// read on this thread would starve it and time out).
fn http_get(ctx: &mut JsContext, port: u16, path: &str) -> Option<Vec<u8>> {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(25))).ok();
    sock.set_nonblocking(false).ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    sock.write_all(req.as_bytes()).ok()?;
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        drive_event_loop(ctx, 3);
        match sock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
        // Complete responses end with connection close (end(…, close=true))
        // — but stop early once headers say Content-Length is satisfied.
        if let Some(sep) = out.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&out[..sep]).to_ascii_lowercase();
            if let Some(cl) = head.lines().find_map(|l| {
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            }) {
                if out.len() >= sep + 4 + cl {
                    break;
                }
            }
        }
    }
    Some(out)
}

/// Split a raw response into (headers_block, body_bytes) at the first
/// `\r\n\r\n`.
fn split_response(raw: &[u8]) -> (String, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let pos = raw
        .windows(sep.len())
        .position(|w| w == sep)
        .expect("response must contain header/body separator");
    let head = String::from_utf8_lossy(&raw[..pos]).into_owned();
    let body = raw[pos + sep.len()..].to_vec();
    (head, body)
}

#[test]
fn test_serve_async_handler_timer_settles() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        r#"
        var srv = Bun.serve({
          port: 0,
          hostname: "127.0.0.1",
          fetch: async function(req) {
            // The exact shape that used to 404: the promise can only settle
            // via a BAO_REGISTRY wall-clock timer, which the old spin never
            // fired.
            await new Promise(function(resolve) { setTimeout(resolve, 50); });
            return new Response("settled", { status: 200 });
          },
        });
        globalThis.__srv = srv;
        "ok"
    "#,
    );
    assert_eq!(setup, "ok", "serve setup must eval");
    drive_event_loop(&mut ctx, 30);

    let port = eval_number(&mut ctx, "globalThis.__srv.port") as u16;
    assert!(port > 0, "serve must bind an ephemeral port, got {port}");
    drive_event_loop(&mut ctx, 10);

    let raw = http_get(&mut ctx, port, "/delayed")
        .unwrap_or_else(|| panic!("request must get a response"));
    let (head, body) = split_response(&raw);
    let status_line = head.lines().next().unwrap_or("");
    assert!(
        status_line.starts_with("HTTP/1.1 200"),
        "async handler awaiting setTimeout(50) must settle in-spin and answer 200, got: {status_line}"
    );
    assert_eq!(
        body, b"settled".to_vec(),
        "body must be the handler's real response (old behavior: 404 'Not Found')"
    );

    let _ = eval_string(&mut ctx, "globalThis.__srv.stop(), 'stopped'");
    drive_event_loop(&mut ctx, 10);
}

#[test]
fn test_serve_connection_abort_exits_spin_early() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        r#"
        var srv = Bun.serve({
          port: 0,
          hostname: "127.0.0.1",
          fetch: function(req) {
            if (req.url.split("?")[0] === "/fast") {
              return new Response("fast-ok", { status: 200 });
            }
            // /slow: resolve only after 1200ms. If the client disconnects
            // mid-dispatch, the abort latch must end the spin long before
            // this timer ever fires.
            return new Promise(function(resolve) {
              setTimeout(function() { resolve(new Response("late", { status: 200 })); }, 1200);
            });
          },
        });
        globalThis.__srv = srv;
        "ok"
    "#,
    );
    assert_eq!(setup, "ok", "serve setup must eval");
    drive_event_loop(&mut ctx, 30);

    let port = eval_number(&mut ctx, "globalThis.__srv.port") as u16;
    assert!(port > 0, "serve must bind an ephemeral port, got {port}");
    drive_event_loop(&mut ctx, 10);

    let aborts_before = bun_runtime::bun_api::SERVE_ABORT_COUNT
        .load(std::sync::atomic::Ordering::Relaxed);

    // Client sends GET /slow, then closes from a helper thread 30ms later
    // (the dispatch spin blocks this thread inside the uWS tick, so the
    // close cannot come from here).
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect /slow");
    let req = format!(
        "GET /slow HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        port
    );
    sock.write_all(req.as_bytes()).expect("write /slow");
    let closer = sock.try_clone().expect("clone socket");
    let closer_thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let _ = closer.shutdown(Shutdown::Both);
    });

    // Pump the loop: the dispatch tick enters the spin; after the helper's
    // FIN lands, uWS fires on_aborted and the spin bails within a few
    // iterations (~tens of ms — the 1200ms timer must NOT be waited out).
    let spin_started = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        drive_event_loop(&mut ctx, 3);
        let aborts_now = bun_runtime::bun_api::SERVE_ABORT_COUNT
            .load(std::sync::atomic::Ordering::Relaxed);
        if aborts_now > aborts_before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SERVE_ABORT_COUNT must increment within 2s of the client close (before={aborts_before})"
        );
    }
    let elapsed = spin_started.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "abort must exit the spin early ({}ms elapsed; the pending timer is 1200ms)",
        elapsed.as_millis()
    );
    closer_thread.join().expect("closer thread");

    // Server recovery: a fresh connection must still be served (the dispatch
    // loop is not wedged in the old 10k-iteration cap spin).
    let raw = http_get(&mut ctx, port, "/fast")
        .unwrap_or_else(|| panic!("recovery request must get a response"));
    let (head, body) = split_response(&raw);
    let status_line = head.lines().next().unwrap_or("");
    assert!(
        status_line.starts_with("HTTP/1.1 200"),
        "server must serve a follow-up request after the abort, got: {status_line}"
    );
    assert_eq!(body, b"fast-ok".to_vec(), "recovery body must match");

    // The abandoned /slow promise's 1200ms timer is still in BAO_REGISTRY —
    // let it fire so it does not pin anything past this test (the resolve
    // lands on an already-abandoned promise; no response is written).
    drive_event_loop(&mut ctx, 1300);

    let _ = eval_string(&mut ctx, "globalThis.__srv.stop(), 'stopped'");
    drive_event_loop(&mut ctx, 10);
}
