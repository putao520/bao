// @trace TEST-ENG-P0-REFUSED-NET-CLOSE [req:REQ-ENG-006 REQ-ENG-007 REQ-ENG-010] [level:e2e]
// P0 regression tests (e-p0-http transfer batch B): the three dispatch-chain
// hangs found by the binary probes, locked as driven e2e tests.
//
//   1. fetch('http://127.0.0.1:9/') / fetch('https://127.0.0.1:9/') — a
//      connection-refused fetch must reject PROMPTLY (previously: the
//      HTTPThread failed the task with ConnectionRefused but the JS thread
//      never drained the resolve_tasklet ConcurrentTask — a fetch-only script
//      has no loop-tick liveness — so the Promise hung forever). Shape:
//      TypeError("fetch failed") with .cause.code ECONNREFUSED.
//   2. net.connect(port, host, cb) — the connect callback fires on a LATER
//      tick (Node semantics), after `var c = net.connect(...)` assigned and
//      after listeners registered in the same block. Also covers the
//      'connect'-before-'data' ordering and the refused-port 'error' shape
//      (deferred, c assigned).
//   3. http server close — srv.close(cb) invokes cb, the server emits
//      'close', a re-close delivers Error("Server is not running"), and with
//      an unconsumed response in flight the loop still settles (liveness
//      token released → has_active_servers() false).
//
// Driving: the bounded post-eval hook (the PRODUCTION pump path — same as
// net_echo_e2e_tests). Bare-Rust pumping outside the entered realm silently
// drops timer callbacks; every assertion here depends on setTimeout-delivered
// events, so the hook form is load-bearing.

use std::cell::Cell;
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

thread_local! {
    /// Iteration budget for `bounded_drain_hook` (fn-pointer hooks cannot
    /// capture state; tests run single-threaded per context).
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook — drives `drain_and_check` (timers, loop
/// tick, ConcurrentTask drain) inside the eval's AutoRealm, `budget` times
/// per eval call.
fn bounded_drain_hook(cx: &mut mozjs::context::JSContext) -> bool {
    let exhausted = HOOK_BUDGET.with(|b| {
        let n = b.get();
        if n == 0 {
            return true;
        }
        b.set(n - 1);
        false
    });
    if exhausted {
        return false;
    }
    bun_runtime::timers::drain_and_check(cx)
}

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<p0-refused-net-close>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

/// Eval `js_condition` ('y'/'n') with `budget` hook iterations per eval;
/// repeat until 'y' or timeout. Returns elapsed, or None on timeout.
fn wait_until(
    ctx: &mut JsContext,
    js_condition: &str,
    budget: usize,
    timeout: Duration,
) -> Option<Duration> {
    let start = Instant::now();
    loop {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return Some(start.elapsed());
        }
        if start.elapsed() >= timeout {
            return None;
        }
    }
}

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

// ─── 1. fetch refused: prompt reject + error shape ────────────────────────

#[test]
fn fetch_http_refused_rejects_promptly_with_typeerror() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.outcome = 'pending';
        var p = fetch('http://127.0.0.1:9/');
        p.then(function (v) { globalThis.outcome = 'resolved(unexpected):' + v.status; },
               function (e) {
                   globalThis.errInstanceOf = (e instanceof TypeError);
                   globalThis.errMsg = String(e && e.message);
                   globalThis.causeCode = e && e.cause && e.cause.code;
                   globalThis.outcome = 'rejected';
               });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "fetch eval");

    // Port 9 on loopback is kernel-refused instantly; the whole budget is the
    // JS-thread drain cadence. Pre-fix this hung forever (the timeout always
    // won).
    let elapsed = wait_until(
        &mut ctx,
        "globalThis.outcome !== 'pending' ? 'y' : 'n'",
        50,
        Duration::from_secs(6),
    );
    assert!(
        elapsed.is_some(),
        "refused fetch must settle (pre-fix: silent hang forever)"
    );
    assert!(
        elapsed.unwrap() < Duration::from_secs(5),
        "refused fetch must reject in seconds, not hang"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.outcome)"),
        "rejected",
        "refused fetch must reject, not resolve"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.errInstanceOf)"),
        "true",
        "network failure must reject with a real TypeError (instanceof holds)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.errMsg)"),
        "fetch failed",
        "TypeError message must be the fetch network-error message"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.causeCode)"),
        "ECONNREFUSED",
        "cause must carry the transport code ECONNREFUSED"
    );
}

#[test]
fn fetch_https_refused_rejects_promptly() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.outcome = 'pending';
        fetch('https://127.0.0.1:9/').then(
            function (v) { globalThis.outcome = 'resolved(unexpected):' + v.status; },
            function (e) {
                globalThis.errInstanceOf = (e instanceof TypeError);
                globalThis.causeCode = e && e.cause && e.cause.code;
                globalThis.outcome = 'rejected';
            });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "https fetch eval");

    let ok = wait_until(
        &mut ctx,
        "globalThis.outcome !== 'pending' ? 'y' : 'n'",
        50,
        Duration::from_secs(6),
    );
    assert!(
        ok.is_some(),
        "refused https fetch must settle (same hang class as http)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.outcome)"),
        "rejected",
        "refused https fetch must reject"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.errInstanceOf)"),
        "true",
        "https network failure must reject with TypeError"
    );
}

// ─── 2. net.connect: deferred callback (Node semantics) ───────────────────

#[test]
fn net_connect_callback_fires_after_assignment_and_registration() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.log = [];
        globalThis.cbSawAssigned = null;
        var net = require('net');
        var server = net.createServer(function (sock) {
            sock.on('data', function (d) { sock.write('pong'); });
        });
        server.listen(0, '127.0.0.1', function () {
            var port = server.address().port;
            var c;
            // The canonical registration shape: the connect callback captured
            // `c` via the closure — pre-fix the callback ran synchronously
            // inside net.connect() while `c` was still undefined.
            c = net.connect(port, '127.0.0.1', function () {
                globalThis.cbSawAssigned = (typeof c !== 'undefined');
                globalThis.log.push('connected');
                c.write('ping');
            });
            c.on('data', function (d) {
                globalThis.log.push('data');
            });
            globalThis.log.push('assigned');
        });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "net.connect wiring eval");

    let ok = wait_until(
        &mut ctx,
        "globalThis.log.indexOf('data') >= 0 ? 'y' : 'n'",
        50,
        Duration::from_secs(8),
    );
    assert!(
        ok.is_some(),
        "connect + echo data must flow (log: {})",
        eval_str(&mut ctx, "globalThis.log.join('|')")
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.cbSawAssigned)"),
        "true",
        "connect callback must run AFTER `var c = net.connect(...)` assigned"
    );
    let log = eval_str(&mut ctx, "globalThis.log.join('|')");
    assert!(
        log.starts_with("assigned|connected"),
        "callback deferral: 'assigned' must precede 'connected', got: {}",
        log
    );
}

#[test]
fn net_connect_refused_error_deferred_and_socket_closed() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.outcome = 'pending';
        globalThis.errSawAssigned = null;
        globalThis.errMsg = '';
        var net = require('net');
        var c2;
        c2 = net.connect(9, '127.0.0.1', function () {
            globalThis.outcome = 'connected(unexpected)';
        });
        c2.on('error', function (e) {
            globalThis.errSawAssigned = (typeof c2 !== 'undefined');
            globalThis.errMsg = String(e && e.message);
            globalThis.outcome = 'errored';
        });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "refused connect eval");

    let ok = wait_until(
        &mut ctx,
        "globalThis.outcome !== 'pending' ? 'y' : 'n'",
        50,
        Duration::from_secs(6),
    );
    assert!(
        ok.is_some(),
        "refused net.connect must fire 'error' (no silent hang)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.outcome)"),
        "errored",
        "refused connect must error, never connect"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.errSawAssigned)"),
        "true",
        "'error' must fire after assignment (deferred delivery)"
    );
    let msg = eval_str(&mut ctx, "globalThis.errMsg");
    assert!(
        msg.contains("ECONNREFUSED"),
        "error message must name ECONNREFUSED, got: {}",
        msg
    );
}

// ─── 3. srv.close(): callback, 'close' event, liveness released ──────────

#[test]
fn http_server_close_delivers_callback_event_and_settles_loop() {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.log = [];
        globalThis.__done = false;
        var http = require('http');
        var srv = http.createServer(function (req, res) {
            res.writeHead(200, { 'Content-Type': 'text/plain' });
            res.end('hello-unconsumed');
        });
        srv.on('close', function () { globalThis.log.push('close-event'); });
        srv.listen(0, '127.0.0.1', function () {
            var port = srv.address().port;
            // Client fetches but never consumes the response body.
            var req = http.get('http://127.0.0.1:' + port + '/', function (res) {
                globalThis.log.push('got-headers');
            });
            req.on('error', function () {});
            setTimeout(function () {
                srv.close(function () { globalThis.log.push('close-cb'); });
                // Node shape: a second close() delivers the not-running error.
                srv.close(function (e) {
                    globalThis.log.push('reclose:' + (e && e.message));
                    globalThis.__done = true;
                });
            }, 150);
        });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "server close wiring eval");

    let ok = wait_until(
        &mut ctx,
        "globalThis.__done === true ? 'y' : 'n'",
        50,
        Duration::from_secs(8),
    );
    assert!(ok.is_some(), "close + re-close callbacks must both fire");
    let log = eval_str(&mut ctx, "globalThis.log.join('|')");
    for part in ["got-headers", "close-cb", "close-event"] {
        assert!(
            log.split('|').any(|e| e == part),
            "close lifecycle must contain '{}' in: {}",
            part,
            log
        );
    }
    assert!(
        log.contains("reclose:Server is not running"),
        "re-close must deliver Node's ERR_SERVER_NOT_RUNNING error, got: {}",
        log
    );
    // Liveness released: the loop can go idle after close (the unconsumed
    // response must not hold the process hostage).
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "srv.close() must release the liveness token so the process can exit"
    );
}
