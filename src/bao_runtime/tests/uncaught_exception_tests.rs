// @trace TEST-ENG-006-UNCAUGHT [req:REQ-ENG-006] [level:integration]
//
// Unified uncaught-exception / unhandled-rejection routing — Node semantics:
//   - timer throw (setTimeout/setImmediate callback)          → timers path
//   - microtask throw / promise reaction rejection             → job_queue +
//     SM rejection-tracker path (queueMicrotask = Promise.then)
//   - EventEmitter listener throw (streams dispatch via emit)   → ee_emit path
//
// Contract (mirrors Node):
//   uncaughtException handler registered → handler receives the Error; the
//   process keeps running (no exit request, exit code untouched).
//   No handler → full stack printed to stderr (captured here via
//   uncaught::begin_capture) + exit code 1 + exit requested.
//   unhandledRejection handler → receives (reason, promise).
//   No unhandledRejection handler → escalation to the uncaught path
//   (Node default --unhandled-rejections=throw).
//   Handler itself throws → fatal: report + exit 1, no recursion.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    // Same wiring as BaoRuntime::new — drain loop, then 'exit' dispatch.
    ctx.set_post_eval_hook(bun_runtime::bun_api::post_eval_drain_then_exit);
    ctx
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        other => panic!("expected string result, got {:?}", other.map(|v| format!("{v:?}"))),
    }
}

// ── Path 1: timers (setTimeout via fire_js_callback_raw) ───────────────────

#[test]
fn timer_throw_without_handler_prints_stack_and_exits_1() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();
    let r = ctx.eval(
        r#"
        setTimeout(function () { throw new Error('boom-timer'); }, 0);
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let out = bun_runtime::uncaught::take_capture();
    assert!(
        out.contains("boom-timer"),
        "default report must carry the error message + stack, got: {out}"
    );
    assert!(
        out.contains("Error:"),
        "report must carry the Error framing, got: {out}"
    );
    assert!(
        bun_runtime::should_exit(),
        "uncaught timer throw without handler must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "uncaught timer throw without handler must exit 1"
    );
}

#[test]
fn timer_throw_with_handler_receives_error_and_process_continues() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        globalThis.got = null;
        process.on('uncaughtException', function (e) { globalThis.got = e.message; });
        setTimeout(function () { throw new Error('handled-timer-boom'); }, 0);
        setTimeout(function () { globalThis.after = 'ran'; }, 5);
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    assert_eq!(
        eval_string(&mut ctx, "globalThis.got"),
        "handled-timer-boom",
        "handler must receive the thrown Error object"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.after"),
        "ran",
        "later timers must still run after a handled uncaught exception"
    );
    assert!(
        !bun_runtime::should_exit(),
        "handled uncaught exception must not request exit (handler decides)"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        0,
        "handled uncaught exception must leave the exit code untouched"
    );
}

// ── Path 2: promise jobs / rejections (job_queue + SM tracker) ─────────────

#[test]
fn microtask_throw_without_handler_prints_and_exits_1() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();
    let r = ctx.eval(
        r#"
        queueMicrotask(function () { throw new Error('qm-boom'); });
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let out = bun_runtime::uncaught::take_capture();
    assert!(
        out.contains("qm-boom"),
        "queueMicrotask throw must surface in the default report, got: {out}"
    );
    assert!(
        bun_runtime::should_exit(),
        "unhandled microtask rejection must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "unhandled microtask rejection must exit 1"
    );
}

#[test]
fn then_throw_without_handlers_escalates_to_uncaught_and_exits_1() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();
    let r = ctx.eval(
        r#"
        Promise.resolve().then(function () { throw new Error('then-boom'); });
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let out = bun_runtime::uncaught::take_capture();
    assert!(
        out.contains("then-boom"),
        "rejected reaction without any handler must surface, got: {out}"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "unhandled rejection without handlers must exit 1 (Node throw mode)"
    );
}

#[test]
fn unhandledrejection_handler_receives_reason_and_promise() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        globalThis.got = null;
        globalThis.gotPromise = false;
        process.on('unhandledRejection', function (reason, promise) {
            globalThis.got = reason.message;
            globalThis.gotPromise = promise instanceof Promise;
        });
        Promise.reject(new Error('rej-plain'));
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    assert_eq!(
        eval_string(&mut ctx, "globalThis.got"),
        "rej-plain",
        "unhandledRejection handler must receive the rejection reason"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.gotPromise)"),
        "true",
        "unhandledRejection handler must receive the rejected Promise"
    );
    assert!(
        !bun_runtime::should_exit(),
        "handled rejection must not request exit"
    );
    assert_eq!(bun_runtime::exit_code(), 0, "handled rejection exits 0");
}

#[test]
fn late_catch_cancels_pending_unhandled_rejection() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();
    let r = ctx.eval(
        r#"
        globalThis.fired = false;
        process.on('unhandledRejection', function () { globalThis.fired = true; });
        const p = Promise.reject(new Error('late-catch'));
        Promise.resolve().then(function () { p.catch(function () { globalThis.caught = 'yes'; }); });
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let out = bun_runtime::uncaught::take_capture();
    assert!(
        !out.contains("late-catch"),
        "a catch attached in a later microtask must cancel the pending rejection, got: {out}"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.fired)"),
        "false",
        "unhandledRejection must not fire when a handler arrived before the flush"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.caught"),
        "yes",
        "the late catch must still run"
    );
    assert_eq!(bun_runtime::exit_code(), 0, "no unhandled rejection remains");
}

// ── Path 3: EventEmitter listener throws (stream callbacks dispatch here) ──

#[test]
fn emitter_listener_throw_routes_to_uncaught_handler() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        globalThis.got = null;
        const EventEmitter = require('events').EventEmitter;
        const ee = new EventEmitter();
        process.on('uncaughtException', function (e) { globalThis.got = e.message; });
        ee.on('data', function () { throw new Error('ee-boom'); });
        ee.emit('data', 1);
        globalThis.done = 'yes';
        'ok'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    assert_eq!(
        eval_string(&mut ctx, "globalThis.got"),
        "ee-boom",
        "emitter listener throw must route to the uncaughtException handler"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.done"),
        "yes",
        "code after a handled emitter throw keeps running"
    );
    assert!(!bun_runtime::should_exit(), "handled → no exit request");
    assert_eq!(bun_runtime::exit_code(), 0, "handled → exit code 0");
}

#[test]
fn emitter_listener_throw_without_handler_prints_and_exits_1() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();
    let r = ctx.eval(
        r#"
        const EventEmitter = require('events').EventEmitter;
        const ee = new EventEmitter();
        ee.on('data', function () { throw new Error('ee-fatal'); });
        ee.emit('data', 1);
        'ok'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let out = bun_runtime::uncaught::take_capture();
    assert!(
        out.contains("ee-fatal"),
        "emitter throw without handler must surface, got: {out}"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "emitter throw without handler must exit 1"
    );
    assert!(bun_runtime::should_exit(), "emitter throw requests exit");
}

// ── Fatal: the handler itself throws ────────────────────────────────────────

#[test]
fn throwing_uncaught_handler_is_fatal_exits_1() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    bun_runtime::uncaught::begin_capture();
    let r = ctx.eval(
        r#"
        process.on('uncaughtException', function () { throw new Error('inner-boom'); });
        setTimeout(function () { throw new Error('outer-boom'); }, 0);
        'scheduled'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let out = bun_runtime::uncaught::take_capture();
    assert!(
        out.contains("inner-boom"),
        "a throwing uncaughtException handler must be reported as fatal, got: {out}"
    );
    assert!(
        bun_runtime::should_exit(),
        "throwing handler must request exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        1,
        "throwing handler must exit 1"
    );
}
