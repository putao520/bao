// @trace TEST-BUG-353 [req:REQ-ENG-006] [level:integration]
// Deep verification of BUG-353 fix: Bun.serve malloc corruption.
// Also verifies BUG-354 fix: server.stop() PrivateValue pointer preservation.
//
// BUG-353: Rust us_create_loop had no ext_size → C++ LoopData uninitialized.
// BUG-354: app_ptr stored as Int32Value truncated 64-bit pointer → SIGSEGV on stop().

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(v) => format!("{:?}", v),
        Err(e) => format!("ERROR: {:?}", e),
    }
}

#[test]
fn test_bug354_stop_does_not_crash() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── T1: Bun.serve + stop — BUG-354 pointer preservation ──
    eprintln!("== T1: serve+stop ==");
    let t1 = eval_str(
        &mut ctx,
        r#"
        var s = Bun.serve({ port: 0 });
        var portOk = typeof s.port === "number";
        var methodsOk = typeof s.stop === "function" && typeof s.ref === "function" && typeof s.unref === "function";
        s.stop();
        portOk && methodsOk ? "ok" : "fail"
    "#,
    );
    assert_eq!(t1, "ok", "T1 Bun.serve+stop: {}", t1);

    // ── T2: Second serve after stop — loop reuse ──
    eprintln!("== T2: second serve after stop ==");
    let t2 = eval_str(
        &mut ctx,
        r#"
        var s1 = Bun.serve({ port: 0 });
        s1.stop();
        var s2 = Bun.serve({ port: 0 });
        typeof s2 === "object" ? "ok" : "fail:" + typeof s2
    "#,
    );
    assert_eq!(t2, "ok", "T2 second serve after stop: {}", t2);
    // cleanup
    eprintln!("== T2 cleanup ==");
    eval_str(&mut ctx, "s2.stop();");

    // ── T3: 5 consecutive create+stop cycles ──
    eprintln!("== T3: 5 cycles ==");
    let t3 = eval_str(
        &mut ctx,
        r#"
        var count = 0;
        for (var i = 0; i < 5; i++) {
            var s = Bun.serve({ port: 0 });
            if (typeof s === "object") count++;
            s.stop();
        }
        count === 5 ? "ok" : "fail:" + count
    "#,
    );
    assert_eq!(t3, "ok", "T3 5 create-stop cycles: {}", t3);

    // ── T4: Bun.serve with fetch handler + stop ──
    eprintln!("== T4: fetch handler + stop ==");
    let t4 = eval_str(
        &mut ctx,
        r#"
        var sv = Bun.serve({
            port: 0,
            fetch: function(req) { return new Response("hello"); }
        });
        sv.stop();
        "ok"
    "#,
    );
    assert_eq!(t4, "ok", "T4 fetch handler + stop: {}", t4);

    // ── T5: http.createServer + listen + close ──
    eprintln!("== T5: http lifecycle ==");
    let t5 = eval_str(
        &mut ctx,
        r#"
        var http = require("http");
        var srv = http.createServer(function(req, res) { res.end("ok"); });
        srv.listen(0);
        var addr = srv.address();
        var addrOk = typeof addr === "object" && typeof addr.port === "number";
        srv.close();
        addrOk ? "ok" : "fail"
    "#,
    );
    assert_eq!(t5, "ok", "T5 http lifecycle: {}", t5);

    // ── T6: Multiple concurrent servers ──
    eprintln!("== T6: concurrent servers ==");
    let t6 = eval_str(
        &mut ctx,
        r#"
        var s1 = Bun.serve({ port: 0 });
        var s2 = Bun.serve({ port: 0 });
        var ok = typeof s1 === "object" && typeof s2 === "object";
        s1.stop(); s2.stop();
        ok ? "ok" : "fail"
    "#,
    );
    assert_eq!(t6, "ok", "T6 concurrent servers: {}", t6);

    // ── T7: BCE-005 — Bun.serve({port:0}) exposes actual bound port ──
    // @trace BCE-20260618-005 [req:REQ-ENG-006] [level:integration]
    // Regression: server.port must be > 0 after dynamic bind (port: 0).
    // The uWS listen callback fires synchronously inside App::listen and
    // captures the OS-assigned port into BunServeUserData.actual_port; the
    // JS server object exposes it via the port property.
    eprintln!("== T7: BCE-005 server.port > 0 ==");
    let t7 = eval_str(
        &mut ctx,
        r#"
        var s = Bun.serve({ port: 0 });
        var portVal = s.port;
        s.stop();
        (typeof portVal === "number" && portVal > 0) ? "ok:" + portVal : "fail:" + portVal
    "#,
    );
    assert!(
        t7.starts_with("ok:"),
        "T7 BCE-005 server.port > 0 after Bun.serve({{port:0}}): {}",
        t7
    );

    bun_runtime::shutdown_thread_sm();
}

// @trace BCE-20260618-006-DEEP [req:REQ-ENG-006] [level:integration]
// Deep regression for the double-stop use-after-free class of bug.
//
// Root cause: `server_stop` (Bun.serve) and `server_close` (http.createServer)
// destroyed the underlying `*mut App::<false>` via `App::destroy()` but left
// the `_appPtr` private slot on the JS server object pointing at the freed
// memory. A second `stop()`/`close()` call read the stale pointer and
// re-invoked `close()`/`destroy()` on freed memory → SIGSEGV.
//
// Fix: clear `_appPtr` (and `_udPtr` for node_http) to UndefinedValue after
// the destroy, so re-entry takes the null path and is a no-op.
//
// This test exercises all four idempotent-close paths:
//   T8: Bun.serve().stop().stop()
//   T9: http.createServer().listen().close().close()
//   T10: Bun.serve + fetch round-trip then double-stop (the
//        test_http_depth.js finishTests path)
//   T11: try/finally with stop in finally (double-stop via catch+finally)
#[test]
fn test_bce006_double_stop_idempotent() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── T8: Bun.serve().stop().stop() — no SIGSEGV ──
    eprintln!("== T8: Bun.serve double-stop ==");
    let t8 = eval_str(
        &mut ctx,
        r#"
        var s = Bun.serve({ port: 0 });
        s.stop();
        s.stop();
        "ok"
    "#,
    );
    assert_eq!(t8, "ok", "T8 Bun.serve double-stop: {}", t8);

    // ── T9: http.createServer().listen().close().close() — no SIGSEGV ──
    eprintln!("== T9: http.createServer double-close ==");
    let t9 = eval_str(
        &mut ctx,
        r#"
        var http = require("http");
        var srv = http.createServer(function(req, res) { res.end("ok"); });
        srv.listen(0);
        srv.close();
        srv.close();
        "ok"
    "#,
    );
    assert_eq!(t9, "ok", "T9 http.createServer double-close: {}", t9);

    // ── T10: Bun.serve with fetch handler + double-stop (finishTests pattern) ──
    // Verifies the double-stop idempotency holds even when a fetch handler is
    // registered (the fetch_cb_key path that exercises the full route handler
    // dispatch). Doesn't actually fetch — just verifies the server with a
    // handler can be safely stopped twice (the test_http_depth.js finishTests
    // path that originally hit the use-after-free).
    eprintln!("== T10: Bun.serve with fetch + double-stop ==");
    let t10 = eval_str(
        &mut ctx,
        r#"
        var sv = Bun.serve({
            port: 0,
            fetch: function(req) { return new Response("hello"); }
        });
        sv.stop();
        sv.stop();
        "ok"
    "#,
    );
    assert_eq!(t10, "ok", "T10 Bun.serve with fetch + double-stop: {}", t10);

    // ── T11: try/finally with stop (catch+finally double-stop) ──
    eprintln!("== T11: try/finally double-stop ==");
    let t11 = eval_str(
        &mut ctx,
        r#"
        var sv = Bun.serve({ port: 0, fetch: function(r) { return new Response("x"); } });
        var outcome = "ok";
        try {
            sv.stop();
        } catch (e) {
            outcome = "catch:" + e;
        } finally {
            try { sv.stop(); } catch (e2) { outcome = "finally:" + e2; }
        }
        outcome
    "#,
    );
    assert_eq!(t11, "ok", "T11 try/finally double-stop: {}", t11);

    bun_runtime::shutdown_thread_sm();
}
