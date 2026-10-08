// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:integration]
//
// e148 / e143 registration: node:tls unhandled-'error' Node crash-semantics
// lock. In Node, `emit('error')` on a TLSSocket with no 'error' listener
// THROWS from the emit call site ("Emitted 'error' event on TLSSocket
// without a listener") and, in an I/O-driven dispatch (which the tls
// driver's tasklet is), that throw is an uncaught exception: it must reach
// the unified uncaught router (process.on('uncaughtException') or stderr +
// exit 1) — NOT be swallowed by the Rust emit bridge.
//
// The scenario: tls.connect() to a guaranteed-refused 127.0.0.1 port with
// no 'error' listener. The legacy promise forwarder IS silenced (a rejection
// handler is attached) so the only possible crash source is the 'error'
// emit throw itself — without that silencing the unhandled-rejection
// escalation path would crash too and the lock could not distinguish the
// two (the emit bridge is the face under test).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use mozjs::rooted;
use std::time::{Duration, Instant};

fn make_ctx() -> JsContext {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// One pump pass: realm-entered timer drain + MiniEventLoop tick (TLS
/// tasklet dispatch) + microjobs — same combined shape as
/// tls_socket_wrap_tests::pump_once (the tls suite needs both faces).
fn pump_once(ctx: &mut JsContext) {
    let cx_raw = ctx.raw_cx();
    {
        let mut cxm = ctx.cx();
        let global = bao_engine::context::thread_realm_global();
        if let Some(g) = global {
            rooted!(&in(cxm) let g_root = g);
            let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            bun_runtime::timers::drain_and_check(realm_cx);
        } else {
            bun_runtime::timers::drain_and_check(&mut cxm);
        }
    }
    bun_runtime::timers::with_event_loop(|loop_| {
        loop_.tick_without_idle(std::ptr::null_mut());
    });
    unsafe {
        mozjs_sys::jsapi::js::RunJobs(cx_raw);
    }
    std::thread::sleep(Duration::from_millis(2));
}

/// Deadline-based pump until `cond` holds (bounded wait, never infinite).
fn pump_until(ctx: &mut JsContext, timeout: Duration, cond: impl Fn(&mut JsContext) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while !cond(ctx) {
        if Instant::now() >= deadline {
            return false;
        }
        pump_once(ctx);
    }
    true
}

#[test]
fn tls_connect_refused_unhandled_error_routes_to_uncaught_crash() {
    // Guaranteed-refused port: bind a listener, read its port, drop it.
    let port = {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        l.local_addr().unwrap().port()
    };

    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();

    let mut ctx = make_ctx();
    let source = format!(
        r#"
        var tls = require('tls');
        var s = tls.connect({{ host: '127.0.0.1', port: {port} }});
        // Silence ONLY the legacy promise path: with a rejection handler
        // attached, an unhandled rejection can never be the crash source,
        // so any exit request below is attributable to the 'error' emit
        // throw alone (the face under lock).
        s.then(function () {{}}, function () {{}});
        'started';
        "#
    );
    let started = ctx.eval(&source, "<test>");
    assert!(
        matches!(&started, Ok(JsValue::String(s)) if s.as_str() == "started"),
        "tls.connect dispatch must succeed synchronously, got: {started:?}"
    );

    // Pump until the refused connect lands as ClientError → emit('error')
    // with no listener → throw → uncaught router → request_exit(1).
    let arrived = pump_until(&mut ctx, Duration::from_secs(10), |_| {
        bun_runtime::should_exit()
    });
    let report = bun_runtime::uncaught::take_capture();

    assert!(
        arrived,
        "unhandled 'error' on a client TLSSocket must reach the uncaught \
         router and request process exit (Node crash semantics) — captured \
         report: {report:?}"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "unhandled 'error' crash must request exit code 1"
    );
    assert!(
        report.contains("uncaught exception"),
        "default report must label the crash as an uncaught exception, got: {report:?}"
    );
    // The routed value is the emitted error object (tls_build_error_js
    // message), not a bare undefined.
    assert!(
        !report.contains("<unprintable value>"),
        "routed error must render, got: {report:?}"
    );

    bun_runtime::clear_exit();
}

#[test]
fn tls_connect_refused_with_error_listener_does_not_crash() {
    // Complement of the crash lock: WITH an 'error' listener the emit is
    // handled — no uncaught routing, no exit request, and the listener sees
    // the error object (Node: the listener swallows the failure).
    let port = {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        l.local_addr().unwrap().port()
    };

    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();

    let mut ctx = make_ctx();
    let source = format!(
        r#"
        var tls = require('tls');
        var s = tls.connect({{ host: '127.0.0.1', port: {port} }});
        s.on('error', function (e) {{ globalThis.__tlsErr = String(e && e.message || e); }});
        s.then(function () {{}}, function () {{}});
        'started';
        "#
    );
    assert!(
        matches!(ctx.eval(&source, "<test>"), Ok(JsValue::String(_))),
        "tls.connect dispatch must succeed synchronously"
    );

    let seen = pump_until(&mut ctx, Duration::from_secs(10), |c| {
        matches!(
            c.eval("globalThis.__tlsErr || ''", "<probe>"),
            Ok(JsValue::String(s)) if !s.is_empty()
        )
    });
    let report = bun_runtime::uncaught::take_capture();

    assert!(
        seen,
        "the 'error' listener must fire for the refused connect (report was: {report:?})"
    );
    assert!(
        !bun_runtime::should_exit(),
        "a HANDLED 'error' emit must not request process exit — crash routing \
         is only for the unhandled surface (report: {report:?})"
    );
    assert!(
        report.is_empty(),
        "a handled 'error' emit must produce no uncaught report, got: {report:?}"
    );

    bun_runtime::clear_exit();
}
