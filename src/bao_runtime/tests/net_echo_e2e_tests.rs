// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:e2e]
// node:net echo over a REAL TCP roundtrip — server AND client entirely in JS
// (require('net')), driven by the unified event loop. Rooted from the
// IIFE/bridge sweep (bbe20a81), which left two gaps plus one masked bug this
// suite pins:
//
//   a. JS-idle tick: after Server.listen, an idle script (no pending timers)
//      never ticked the usockets loop — inbound connections sat un-accepted.
//      Fixed by registering the listen socket in the unified BCE-007 liveness
//      registry (node_http::register_active_app).
//   b. accept→JS bridge: usockets accept (vtable on_open, is_client == 0)
//      never created a JS net.Socket nor emitted Server 'connection'. Fixed
//      by dispatch_accept + __net_on_connection + the __net_make_socket
//      factory.
//   c. masked bugs the bridge uncovered: net_on_open wrote CONNECT_RESULT on
//      ACCEPTS too — during a same-thread connect spin the accept lands
//      first, so net_connect used to hand back the SERVER-side socket as the
//      client socket; and the JS poll chain tested `buf.length` on an
//      ArrayBuffer (only .byteLength exists) so 'data' never fired; and
//      __net_write silently wrote an empty payload for non-string arguments.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
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
/// `post_eval_drain_then_exit` the same way): the eval's tail loops this hook
/// INSIDE the AutoRealm, so timer callbacks dispatch with the realm entered.
/// Pumping from bare Rust outside any entered realm silently drops timer
/// callbacks (fire_js_callback_raw's fallback resolves the global but never
/// enters the realm) — the echo pipeline must be driven through here.
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

/// Eval `js_condition` (must yield 'y'/'n'), letting each eval's post-eval
/// hook pump the loop `budget` iterations; repeat until 'y'.
fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    for _ in 0..40 {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

/// One bounded pump pass (no condition) — for final settle assertions.
fn settle(ctx: &mut JsContext, budget: usize) {
    HOOK_BUDGET.with(|b| b.set(budget));
    let _ = eval_str(ctx, "'settle'");
}

/// Wire an echo server + client entirely in JS, echo a byte-exact payload
/// (including a non-UTF8 byte — the ArrayBuffer write path must be byte-faithful,
/// not string round-tripped), and shut everything down cleanly.
#[test]
fn net_echo_e2e_real_tcp_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var net = require('net');
        var log = [];
        globalThis.__done = false;

        var server = net.createServer(function(sock) {
            log.push('connection');
            log.push('remotePort=' + (sock.remotePort > 0));
            // Echo the received bytes straight back — byte-for-byte, via the
            // ArrayBuffer the data event delivers.
            sock.on('data', function(d) {
                sock.write(new Uint8Array(d));
            });
            // Synchronous same-object re-emit: end() emits 'end'+'close' on
            // this socket from inside the 'end' listener — the canonical Node
            // shape, safe since the node_events single-owner invariant fix.
            sock.on('end', function() { sock.end(); });
        });

        server.listen(0, '127.0.0.1', function() {
            var port = server.address().port;
            log.push('port=' + (port > 0));
            var payload = new Uint8Array([112, 105, 110, 103, 0xff, 0x00, 0x80]);
            // The connect callback fires synchronously inside net.connect
            // (before the `var client` assignment lands), so defer the first
            // write; 'data' delivery is poll-based and lossless either way.
            var client = net.connect(port, '127.0.0.1', function() {
                log.push('client_connected');
            });
            setTimeout(function() { client.write(payload); }, 0);
            client.on('data', function(d) {
                var got = Array.prototype.slice.call(new Uint8Array(d));
                var want = Array.prototype.slice.call(payload);
                var same = got.length === want.length && got.every(function(b, i) { return b === want[i]; });
                log.push('echo=' + same);
                // Synchronous same-object re-emit (end() emits 'end'/'close'
                // on this socket) — canonical Node shape, see the note above.
                client.end();
            });
            client.on('close', function() {
                server.close(function() {
                    log.push('server_closed');
                    globalThis.__done = true;
                });
            });
        });
        globalThis.__log = function() { return log.join('|'); };
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok", "echo wiring must eval cleanly");

    // Drive the loop through the production pump path: accept dispatch →
    // echo write → client data → teardown. The __net_connect spin inside the
    // same thread handles the connect; the setTimeout(0) poll chains carry
    // data; the liveness registration carries the idle windows between them.
    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    let diag = eval_str(&mut ctx, "globalThis.__log ? globalThis.__log() : '(no log)'");
    assert!(done, "echo roundtrip must complete; log so far: {diag}");

    let log = eval_str(&mut ctx, "globalThis.__log()");
    // 'connection' vs 'client_connected' order is epoll-batch dependent (the
    // accept and the connect completion land in the same wakeup on loopback)
    // — assert membership plus the deterministic anchors instead.
    for part in [
        "port=true",
        "client_connected",
        "connection",
        "remotePort=true",
        "echo=true",
        "server_closed",
    ] {
        assert!(
            log.split('|').any(|entry| entry == part),
            "echo log must contain '{part}' in order-independent position, got: {log}"
        );
    }
    assert!(
        log.starts_with("port=true"),
        "listening must be logged first, got: {log}"
    );
    assert!(
        log.ends_with("server_closed"),
        "server close must be the final event, got: {log}"
    );

    // Everything closed: the poll chains stopped and the liveness token was
    // dropped — the drain loop must be able to go idle again.
    assert!(
        !bun_runtime::node_http::has_active_servers(),
        "server.close() must unregister the net liveness token"
    );
    assert_eq!(
        eval_str(&mut ctx, "typeof require('net').Server === 'function' ? 'ok' : 'broken'"),
        "ok"
    );
}

/// Peer-FIN lifecycle: when the client ends first, the server-side socket
/// must observe 'end' (half-close) — the vtable on_end → __net_poll_state==2
/// path — and the loop must settle after close with no spinning poll chain.
#[test]
fn net_peer_fin_delivers_end_event() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var net = require('net');
        var log = [];
        globalThis.__done = false;
        var server = net.createServer(function(sock) {
            // Synchronous same-object re-emit on 'end' — canonical shape.
            sock.on('end', function() { log.push('saw_end'); sock.end(); });
            sock.on('close', function() { log.push('sock_closed'); });
            sock.on('data', function(d) { sock.write(new Uint8Array(d)); });
        });
        server.listen(0, '127.0.0.1', function() {
            var client = net.connect(server.address().port, '127.0.0.1', function() {});
            setTimeout(function() { client.write('fin-probe'); }, 0);
            // Synchronous same-object re-emit (end() emits 'end'/'close').
            client.on('data', function() { client.end(); });
        });
        var watcher = setInterval(function() {
            if (log.indexOf('sock_closed') >= 0) {
                clearInterval(watcher);
                server.close(function() { globalThis.__done = true; });
            }
        }, 0);
        globalThis.__log = function() { return log.join('|'); };
        'setup-ok'
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = wait_until(&mut ctx, "globalThis.__done === true ? 'y' : 'n'", 50);
    let diag = eval_str(&mut ctx, "globalThis.__log ? globalThis.__log() : '(no log)'");
    assert!(done, "peer-FIN lifecycle must settle; log: {diag}");

    let log = eval_str(&mut ctx, "globalThis.__log()");
    assert!(
        log.contains("saw_end") && log.contains("sock_closed"),
        "peer FIN must deliver 'end' then 'close' on the server-side socket, got: {log}"
    );

    // Let any residual poll chain observe the closed sockets and stop.
    settle(&mut ctx, 30);
    assert!(!bun_runtime::node_http::has_active_servers());

    // No poll chain left spinning: with no timers and no servers,
    // drain_and_check reports no pending work.
    let mut cxm = ctx.cx();
    assert!(
        !bun_runtime::timers::drain_and_check(&mut cxm),
        "after full teardown the loop must have no pending work (no spinning poll chain)"
    );
}
