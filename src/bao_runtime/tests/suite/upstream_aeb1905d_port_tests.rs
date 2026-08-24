// @trace TEST-ENG-005 [req:REQ-ENG-005 REQ-ENG-007] [level:e2e]
// Problem-domain ports of upstream bun aeb1905d0a "node: stop after a JS
// throw in HTTPParser callbacks, X509 getters, Buffer.from(BigInt64Array),
// structured-clone deserialize" (verified via
// `git -C ~/code/rust/bun show aeb1905d0a`):
//   1. Buffer.from(new BigInt64Array([1n])) / BigUint64Array /
//      new Buffer(typedarray) throw TypeError("Cannot convert a BigInt value
//      to a number") instead of returning a truncated/zero-filled buffer;
//      an empty BigInt view still yields an empty Buffer (upstream
//      buffer.test.js pins all four shapes).
//   2. http server_close: a throwing close callback must not leave its
//      exception pending across the subsequent 'close' emit re-entry — the
//      captured throw routes to process 'uncaughtException' and the 'close'
//      event still fires.
//   3. http upgrade dispatch: a throw from the listener (routed inside
//      ee_emit) AND from an overridden emit surface (routed by the native
//      capture-clear-route) are both observable via 'uncaughtException',
//      and the uWS crash-class guard still answers 426 because nobody
//      accepted the upgrade.
//
// Driving: the bounded post-eval hook (the PRODUCTION pump path — same as
// p0_refused_net_close_tests); server lifecycle assertions depend on
// pump-delivered events.

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
    match ctx.eval(code, "<upstream-aeb1905d-port>") {
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
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

// ─── 1. Buffer.from(BigInt64Array) throws ────────────────────────────────

#[test]
fn buffer_from_bigint_typed_arrays_throw_typeerror() {
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        function probe(fn) {
            try { var b = fn(); return 'no-throw:len=' + b.length; }
            catch (e) {
                return (e instanceof TypeError ? 'TypeError' : 'other') + ':' + String(e && e.message);
            }
        }
        [
            probe(function () { return Buffer.from(new BigInt64Array([1n, 2n])); }),
            probe(function () { return Buffer.from(new BigUint64Array([1n])); }),
            probe(function () { return new Buffer(new BigInt64Array([1n])); }),
            'empty:' + Buffer.from(new BigInt64Array(0)).length,
            'u32:' + Buffer.from(new Uint32Array([256, 257])).join(','),
            'f64len:' + Buffer.from(new Float64Array([1.5, 2.5])).length
        ].join('|')
        "#,
    );
    // Upstream aeb1905d0a test pins: both BigInt kinds throw TypeError via
    // Buffer.from AND the legacy `new Buffer(typedarray)` entry, an empty
    // view has nothing to copy (length 0), and number-coercible element
    // copies keep their low-byte semantics.
    assert_eq!(
        r,
        "TypeError:Cannot convert a BigInt value to a number\
         |TypeError:Cannot convert a BigInt value to a number\
         |TypeError:Cannot convert a BigInt value to a number\
         |empty:0|u32:0,1|f64len:2",
        "Buffer.from BigInt element-copy contract"
    );
}

// ─── 2. server_close: throwing close callback ─────────────────────────────

#[test]
fn http_server_close_callback_throw_routed_and_close_event_still_fires() {
    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.log = [];
        globalThis.uncaught = 'none';
        process.on('uncaughtException', function (e) {
            globalThis.uncaught = String(e && e.message);
        });
        var http = require('http');
        var srv = http.createServer(function (req, res) { res.end('ok'); });
        srv.on('close', function () { globalThis.log.push('close-event'); });
        srv.listen(0, '127.0.0.1', function () {
            var port = srv.address().port;
            var rq = http.get('http://127.0.0.1:' + port + '/', function (res) {
                res.on('data', function () {});
                res.on('end', function () {
                    srv.close(function () { throw new Error('close-cb-boom'); });
                });
            });
            rq.on('error', function () {});
        });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "server close wiring eval");

    let ok = wait_until(
        &mut ctx,
        "globalThis.uncaught === 'close-cb-boom' && globalThis.log.indexOf('close-event') >= 0 ? 'y' : 'n'",
        50,
        Duration::from_secs(8),
    );
    assert!(
        ok.is_some(),
        "throwing close callback must route to uncaughtException AND the 'close' event must still fire"
    );
    // The 'close' emit ran AFTER the throwing callback with nothing pending —
    // the context is clean for subsequent evaluation.
    assert_eq!(eval_str(&mut ctx, "6 * 7"), "42");
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "srv.close() must release the liveness token"
    );
}

// ─── 3. upgrade dispatch: throw routed, 426 guard intact ─────────────────

#[test]
fn http_upgrade_throw_routed_and_426_guard_fires() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let mut ctx = make_ctx();

    let r = eval_str(
        &mut ctx,
        r#"
        globalThis.uncaught = 'none';
        process.on('uncaughtException', function (e) {
            globalThis.uncaught = String(e && e.message);
        });
        var http = require('http');
        var srv = http.createServer(function (req, res) { res.end('plain'); });
        srv.on('upgrade', function (req, socket, head) {
            throw new Error('upgrade-boom');
        });
        srv.listen(0, '127.0.0.1', function () {
            globalThis.port = srv.address().port;
        });
        'issued'
        "#,
    );
    assert_eq!(r, "issued", "upgrade wiring eval");

    let ok = wait_until(
        &mut ctx,
        "globalThis.port ? 'y' : 'n'",
        50,
        Duration::from_secs(8),
    );
    assert!(ok.is_some(), "server must listen");
    let port: u16 = eval_str(&mut ctx, "globalThis.port")
        .parse()
        .expect("port number");

    // Raw WS-upgrade GET on the real wire path. The node:http route handler
    // (and therefore the 426 write) runs on the JS thread during pump
    // drain, so the read must interleave with bounded pump evals — a bare
    // blocking read starves the server forever.
    fn send_upgrade_get(ctx: &mut JsContext, port: u16) -> String {
        use std::io::ErrorKind;
        let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let req = format!(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            port
        );
        sock.write_all(req.as_bytes()).expect("write upgrade request");
        sock.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 2048];
        loop {
            match sock.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => panic!("read upgrade response: {:?}", e),
            }
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            HOOK_BUDGET.with(|b| b.set(5));
            let _ = eval_str(ctx, "'pump'");
            assert!(
                Instant::now() < deadline,
                "server never answered the upgrade request"
            );
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    // Listener throw: routed to uncaughtException (observable), and the
    // crash-class guard answers 426 because nobody accepted the upgrade.
    let resp = send_upgrade_get(&mut ctx, port);
    assert!(
        resp.starts_with("HTTP/1.1 426"),
        "throwing upgrade listener must still hit the 426 guard, got: {}",
        resp
    );
    let ok = wait_until(
        &mut ctx,
        "globalThis.uncaught === 'upgrade-boom' ? 'y' : 'n'",
        50,
        Duration::from_secs(8),
    );
    assert!(
        ok.is_some(),
        "upgrade listener throw must route to uncaughtException, not be swallowed"
    );

    // Override the emit surface itself with a throwing JS function — the
    // native dispatch must capture-clear-route instead of continuing with
    // the exception pending (drives the node_http.rs route_pending_exception
    // fix directly; ee_emit's internal listener routing is bypassed).
    let r = eval_str(
        &mut ctx,
        r#"
        srv.emit = function (ev) { throw new Error('emit-override-boom'); };
        'overridden'
        "#,
    );
    assert_eq!(r, "overridden");

    let resp2 = send_upgrade_get(&mut ctx, port);
    assert!(
        resp2.starts_with("HTTP/1.1 426"),
        "emit-surface throw must still hit the 426 guard, got: {}",
        resp2
    );
    let ok = wait_until(
        &mut ctx,
        "globalThis.uncaught === 'emit-override-boom' ? 'y' : 'n'",
        50,
        Duration::from_secs(8),
    );
    assert!(
        ok.is_some(),
        "emit-surface throw must route to uncaughtException, not stay pending"
    );
}
