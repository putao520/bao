// @trace REQ-ENG-006 [entity:JSContext]
/// Module evaluation error surfacing tests — verify that a module whose
/// evaluation fails reports an explicit error to the caller instead of
/// returning success.
///
/// SM contract: `JS::ModuleEvaluate` returns `true` even when the module
/// body throws — the error is captured into the module record and exposed
/// only as a REJECTED evaluation promise. Before the fix, `eval_module*`
/// never inspected that promise, so `bao run foo.mjs` exited 0 on a
/// top-level throw, worker bootstrap failures were invisible, and test
/// files passed vacuously. These tests pin the fixed contract.

use bao_engine::context::{JsContext, thread_realm_global};
use bao_engine::module_loader::ModuleLoader;
use mozjs::rooted;

/// Build a test JsContext with the full Node/Bun globals installed
/// (same harness as realm_persistence_tests).
fn make_ctx() -> JsContext {
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// A synchronous top-level `throw` must surface as an explicit `Err`
/// carrying the thrown message — not a swallowed success.
#[test]
fn module_top_level_throw_surfaces_error() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    let result = ModuleLoader::eval_module_in_realm(
        &mut cx,
        "throw new Error('boom-top-level');",
        "<throw>.mjs",
        None,
        global.handle(),
    );

    let err = result.expect_err("top-level throw must surface as Err, not Ok");
    assert!(
        err.message.contains("boom-top-level"),
        "error message must contain the thrown content, got: {}",
        err.message
    );
}

/// A rejected top-level await settles during the job-queue drain and must
/// surface as an explicit `Err` (Node: uncaught rejection in module scope).
#[test]
fn module_top_level_await_rejection_surfaces_error() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    let result = ModuleLoader::eval_module_in_realm(
        &mut cx,
        "await Promise.reject(new Error('tla-reject'));",
        "<tla-reject>.mjs",
        None,
        global.handle(),
    );

    let err = result.expect_err("rejected top-level await must surface as Err");
    assert!(
        err.message.contains("tla-reject"),
        "error message must contain the rejection reason, got: {}",
        err.message
    );
}

/// A successful module is unaffected: returns Ok and its side effects are
/// visible in the shared realm.
#[test]
fn module_success_path_unchanged() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "globalThis.fromOkModule = 'present';",
        "<ok>.mjs",
        None,
        global.handle(),
    )
    .expect("successful module must still evaluate to Ok");

    let r = ctx
        .eval("globalThis.fromOkModule;", "<verify>")
        .expect("verify eval");
    assert_eq!(
        r.as_string(),
        Some("present"),
        "successful module side effect must be observable"
    );
}

/// A top-level await that never settles is NOT an error — the evaluation
/// outlives the call and the eval returns success (boundary semantics).
#[test]
fn module_pending_top_level_await_is_not_an_error() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "await new Promise(() => {});",
        "<tla-pending>.mjs",
        None,
        global.handle(),
    )
    .expect("pending (never-settling) top-level await must not be an error");
}

/// `eval_module_in_realm_then` must NOT run `after_eval` when the module
/// evaluation failed (a failed module must not proceed to downstream work —
/// e.g. `bao test` must not run suites registered by a broken module).
#[test]
fn module_then_skips_after_eval_on_throw() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    let mut after_eval_ran = false;
    let result = ModuleLoader::eval_module_in_realm_then(
        &mut cx,
        "throw new Error('boom-then');",
        "<throw-then>.mjs",
        None,
        global.handle(),
        |_realm_cx| {
            after_eval_ran = true;
        },
    );

    let err = result.expect_err("throwing module in _then variant must Err");
    assert!(
        err.message.contains("boom-then"),
        "error message must contain the thrown content, got: {}",
        err.message
    );
    assert!(
        !after_eval_ran,
        "after_eval must not run after a failed module evaluation"
    );
}

/// After a surfaced module error the context must not be poisoned: the
/// pending exception is consumed by the error extraction, so subsequent
/// evals on the same realm succeed.
#[test]
fn realm_still_usable_after_module_error() {
    let mut ctx = make_ctx();
    ctx.eval("void 0;", "<realm-init>").expect("realm init");
    let global_ptr = thread_realm_global().expect("realm global");
    let mut cx = ctx.cx();
    rooted!(&in(cx) let global = global_ptr);

    let _ = ModuleLoader::eval_module_in_realm(
        &mut cx,
        "throw new Error('boom-poison-check');",
        "<poison>.mjs",
        None,
        global.handle(),
    )
    .expect_err("first eval must fail");

    // Same realm, next eval: must succeed cleanly (no stale exception).
    ModuleLoader::eval_module_in_realm(
        &mut cx,
        "globalThis.recovered = true;",
        "<recovered>.mjs",
        None,
        global.handle(),
    )
    .expect("realm must remain usable after a surfaced module error");

    let r = ctx
        .eval("globalThis.recovered;", "<verify>")
        .expect("verify eval");
    assert_eq!(r.as_bool(), Some(true));
}
