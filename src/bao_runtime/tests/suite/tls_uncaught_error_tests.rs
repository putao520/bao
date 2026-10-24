// @trace TEST-ENG-7 [req:REQ-ENG-007] [level:integration]
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
#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

fn make_ctx() -> JsContext {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// @trace REQ-ENG-010
/// Settle-pump delegate: unified suite pump (`common::pump_until`) evaluating
/// `Boolean(cond_js)` until truthy or timeout expiry.
fn pump_until(ctx: &mut JsContext, timeout: Duration, cond_js: &str) -> bool {
    common::pump_until(
        ctx,
        &format!("Boolean({})", cond_js),
        timeout,
        common::eval_string,
        |p| p == "true",
    ) == "true"
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
    // The exit flag is a Rust-side observable (not JS-reachable), so it is
    // the settle predicate; the poll expression is a harmless sentinel whose
    // value is ignored by the closure.
    let _ = common::pump_until(
        &mut ctx,
        "globalThis.__uncaughtSentinel",
        Duration::from_secs(10),
        common::eval_string,
        |_poll: &str| bun_runtime::should_exit(),
    );
    let arrived = bun_runtime::should_exit();
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

    let seen = pump_until(
        &mut ctx,
        Duration::from_secs(10),
        "(globalThis.__tlsErr || '').length > 0",
    );
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
