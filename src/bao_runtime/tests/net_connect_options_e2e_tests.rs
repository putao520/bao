// @trace TEST-ENG-007-NET-OPTIONS [req:REQ-ENG-007] [level:e2e]
// node:net options-form regression (SIGSEGV class):
//
//   1. `net.connect({ port, host }, cb)` — the options object used to be
//      handed to `__net_connect` verbatim, whose native side read the raw
//      JSVal payload with `to_int32()`: a JSObject tag meant a heap pointer
//      reinterpreted as the port (garbage connect or SIGSEGV); a
//      double-tagged Number meant f64 bits truncated to i32. The JS layer
//      now unwraps the options form (Node normalizeArgs semantics) and the
//      native extracts the port tag-safely with explicit throws.
//   2. Malformed ports must THROW (Node: TypeError / RangeError family) —
//      never coerce garbage into a connect.
//   3. `server.listen({ port })` used to match none of the typeof branches
//      (an object is neither function/number/string), silently binding a
//      RANDOM port instead of the requested one.
//
// Driving: the bounded post-eval hook (the PRODUCTION pump path — same as
// net_echo_e2e_tests); the echo roundtrip's timers/polls all fire through it.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<net-options-e2e>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

thread_local! {
    /// Iteration budget for `bounded_drain_hook` (fn-pointer hooks cannot
    /// capture state; tests run single-threaded per context).
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

/// Bounded post-eval drain hook — the PRODUCTION pump path (the CLI installs
/// `post_eval_drain_then_exit` the same way). See net_echo_e2e_tests.rs for
/// why driving through the entered realm is load-bearing.
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

fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    for _ in 0..60 {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

// ─── 1. options-form connect: real TCP echo roundtrip ─────────────────────

#[test]
fn net_connect_options_form_echo_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    let setup = eval_str(
        &mut ctx,
        r#"
        var net = require('net');
        globalThis.__done = false;
        globalThis.__stage = 'wiring';

        var server = net.createServer(function(sock) {
            sock.on('data', function(d) { sock.write(new Uint8Array(d)); });
            sock.on('end', function() { sock.end(); });
        });
        server.listen(0, '127.0.0.1', function() {
            globalThis.__stage = 'listening';
            // OPTIONS FORM — pre-fix this passed the raw object into
            // __net_connect and SIGSEGV'd / connected to a garbage port.
            var client = net.connect(
                { port: server.address().port, host: '127.0.0.1' },
                function() { globalThis.__stage = 'connected'; }
            );
            setTimeout(function() { client.write('options-form-ping'); }, 0);
            client.on('data', function(d) {
                globalThis.__stage = 'echo:' + String.fromCharCode.apply(null, new Uint8Array(d));
                client.end();
            });
            client.on('close', function() {
                server.close(function() { globalThis.__done = true; });
            });
        });
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok", "options-form wiring must eval cleanly");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    assert!(done, "options-form net.connect roundtrip must settle (stage: {})",
        eval_str(&mut ctx, "globalThis.__stage"));
    let stage = eval_str(&mut ctx, "globalThis.__stage");
    assert_eq!(
        stage, "echo:options-form-ping",
        "echo payload must roundtrip byte-exact through the options-form connect"
    );
}

// ─── 2. malformed ports throw (never coerce into a connect) ───────────────

#[test]
fn net_connect_malformed_port_throws() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    // All synchronous throws — no loop driving needed.
    let r = eval_str(
        &mut ctx,
        r#"
        var net = require('net');
        var out = [];
        function attempt(label, fn) {
            try { fn(); out.push(label + ':no-throw'); }
            catch (e) { out.push(label + ':threw:' + String(e && e.message).substring(0, 40)); }
        }
        // Non-numeric string port: Number('abc') = NaN → native rejects (not finite).
        attempt('nan-string', function() { net.connect({ port: 'abc' }); });
        // Out-of-range numbers → RangeError family.
        attempt('too-big', function() { net.connect({ port: 70000 }); });
        attempt('negative', function() { net.connect({ port: -1 }); });
        // Missing port entirely.
        attempt('no-port', function() { net.connect({ host: '127.0.0.1' }); });
        // Raw object form straight into the native (defense-in-depth path).
        attempt('raw-object', function() { globalThis.__net_connect({}, '127.0.0.1'); });
        // Direct numeric-string port IS accepted by Node — coerced by the JS
        // layer before the native; only refused-connection behavior follows.
        // (Binding a live server for it is covered by the listen test below.)
        out.join('|')
    "#,
    );
    assert!(r.contains("nan-string:threw:"), "NaN port must throw: {r}");
    assert!(r.contains("too-big:threw:"), "port 70000 must throw: {r}");
    assert!(r.contains("negative:threw:"), "port -1 must throw: {r}");
    assert!(r.contains("no-port:threw:"), "missing port must throw: {r}");
    assert!(r.contains("raw-object:threw:"), "raw object port must throw at the native boundary: {r}");
    for part in r.split('|') {
        assert!(!part.contains("no-throw"), "every malformed port must throw: {r}");
    }
    // Range-style message family for the out-of-range case (Node parity).
    assert!(r.contains("Port should be"), "out-of-range message must be Node-style: {r}");
}

// ─── 3. server.listen({port}) binds the REQUESTED port ────────────────────

#[test]
fn server_listen_options_form_binds_requested_port() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = make_ctx();

    // Reserve a free port, then release it for the JS server.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
    let wanted = probe.local_addr().unwrap().port();
    drop(probe);

    let setup = eval_str(
        &mut ctx,
        &format!(
            r#"
            var net = require('net');
            globalThis.__done = false;
            globalThis.__got = -1;
            var server = net.createServer(function() {{}});
            // OPTIONS FORM — pre-fix the object matched no typeof branch and
            // the server bound a RANDOM port (0) instead of {wanted}.
            server.listen({{ port: {wanted}, host: '127.0.0.1' }}, function() {{
                globalThis.__got = server.address().port;
                server.close(function() {{ globalThis.__done = true; }});
            }});
            'setup-ok'
        "#
        ),
    );
    assert_eq!(setup, "setup-ok", "listen options-form wiring must eval cleanly");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    assert!(done, "listen({{port}}) must settle");
    let got = eval_str(&mut ctx, "String(globalThis.__got)");
    assert_eq!(
        got,
        wanted.to_string(),
        "listen({{port: {wanted}}}) must bind the REQUESTED port (pre-fix: random port 0)"
    );
}
