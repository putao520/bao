// @trace TEST-ENG-006-EXIT [req:REQ-ENG-006] [level:integration]
// process.on('exit') dispatch — Node semantics (upstream 18391f652):
// listeners run in registration order at orderly exit, each receiving the
// exit code; a throwing listener does not stop later ones; process.exitCode
// set inside a listener (or by the script) steers the final exit code.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    // Initialize Output before any event-loop tick flushes stdout.
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    // Same wiring as BaoRuntime::new — drain loop, then 'exit' dispatch.
    ctx.set_post_eval_hook(bun_runtime::bun_api::post_eval_drain_then_exit);
    ctx
}

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        other => panic!("expected number result, got {:?}", other.map(|v| format!("{:?}", v))),
    }
}

#[test]
fn exit_listener_receives_code_and_exitcode_respected() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        process.on('exit', function (code) { process.exitCode = code + 4; });
        process.exit(3);
        'done'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    assert!(bun_runtime::should_exit(), "process.exit(3) must request exit");
    assert_eq!(
        bun_runtime::exit_code(),
        7,
        "listener must run with code 3 and its process.exitCode = 3 + 4 assignment must win"
    );
}

#[test]
fn natural_exit_dispatches_with_zero_and_honours_exitcode() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        process.on('exit', function (code) { process.exitCode = code + 5; });
        'done'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    assert!(
        !bun_runtime::should_exit(),
        "assigning process.exitCode alone must not request an exit"
    );
    assert_eq!(
        bun_runtime::exit_code(),
        5,
        "natural exit dispatches with code 0; the listener's 0 + 5 must be honoured"
    );
}

#[test]
fn multiple_listeners_run_in_registration_order() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        process.on('exit', function () { process.exitCode = 11; });
        process.on('exit', function () { process.exitCode = process.exitCode + 100; });
        process.exit(1);
        'done'
        "#,
        "<test>",
    );
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    assert_eq!(
        bun_runtime::exit_code(),
        111,
        "first listener sets 11; second must observe 11 (registration order) and set 111"
    );
}

#[test]
fn throwing_listener_does_not_block_later_listeners() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let r = ctx.eval(
        r#"
        process.on('exit', function () { throw new Error('boom'); });
        process.on('exit', function (code) { process.exitCode = 42; });
        process.exit(1);
        'done'
        "#,
        "<test>",
    );
    assert!(
        r.is_ok(),
        "listener throw must be swallowed, not surface as an eval error: {:?}",
        r.err()
    );
    assert_eq!(
        bun_runtime::exit_code(),
        42,
        "second listener must still run after the first one threw"
    );
}

#[test]
fn exitcode_property_roundtrip() {
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();
    let v = eval_number(&mut ctx, "process.exitCode = 9; process.exitCode");
    assert_eq!(v, 9.0, "getter must return the assigned value");
    assert_eq!(
        bun_runtime::exit_code(),
        9,
        "setter must land in the orderly-exit EXIT_CODE slot"
    );
    assert!(!bun_runtime::should_exit(), "no exit requested");
}
