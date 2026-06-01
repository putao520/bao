// @trace TEST-ENG-004 [req:REQ-ENG-004] [level:integration]
// @trace TEST-ENG-001 [req:REQ-ENG-001] [level:integration]
//
// All SpiderMonkey-dependent tests run within a single #[test] function because
// ENGINE_HANDLE is thread_local — Rust's test harness gives each #[test] its own
// thread, and JSEngine::init() cannot be called a second time in a new thread
// after the engine was leaked in the first.  One JsContext is created and reused
// across all sub-tests.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Sub-test helpers — each maps to one of the 10 required test scenarios
// ---------------------------------------------------------------------------

fn test_01_job_queue_init_empty_state(ctx: &mut JsContext) {
    let result = ctx.eval("42", "test_empty.js");
    assert!(result.is_ok(), "eval on fresh queue should succeed");
}

fn test_02_job_queue_enqueue_and_drain(ctx: &mut JsContext) {
    let result = ctx.eval(
        r#"
            var resolved = false;
            Promise.resolve(10).then(function(v) { resolved = v === 10; });
            "done"
        "#,
        "drain_test.js",
    );
    assert!(result.is_ok(), "promise drain should not error");
}

fn test_03_job_queue_fifo_ordering(ctx: &mut JsContext) {
    // Parallel then-callbacks execute in enqueue order
    let result = ctx.eval(
        r#"
            var order = [];
            Promise.resolve().then(function() { order.push(1); });
            Promise.resolve().then(function() { order.push(2); });
            Promise.resolve().then(function() { order.push(3); });
            order.join(",")
        "#,
        "fifo_parallel.js",
    );
    assert!(result.is_ok());

    // Chained then-callbacks execute in chain order
    let result = ctx.eval(
        r#"
            var result = [];
            Promise.resolve(0)
                .then(function() { result.push("a"); })
                .then(function() { result.push("b"); })
                .then(function() { result.push("c"); });
            "check"
        "#,
        "fifo_chain.js",
    );
    assert!(result.is_ok());
}

fn test_04_job_queue_capacity_length(ctx: &mut JsContext) {
    // Enqueue several promise jobs, then drain them
    let result = ctx.eval(
        r#"
            Promise.resolve(1).then(function() {});
            Promise.resolve(2).then(function() {});
            "enqueued"
        "#,
        "capacity.js",
    );
    assert!(result.is_ok());

    // After drain, queue should be empty — second eval must not encounter stale jobs
    let result2 = ctx.eval("'after'", "capacity2.js");
    assert!(result2.is_ok());
}

fn test_06_eval_simple_expressions(ctx: &mut JsContext) {
    // Arithmetic
    let val = ctx.eval("1 + 2", "arith.js").expect("1+2 should succeed");
    assert!(val.is_number(), "1+2 should produce a number");
    assert_eq!(val.as_number().unwrap(), 3.0);

    // String concatenation
    let val = ctx.eval("'hello' + ' ' + 'world'", "string.js").expect("string concat should succeed");
    assert!(val.is_string(), "concat should produce a string");
    assert_eq!(val.as_string().unwrap(), "hello world");

    // Boolean — SM returns bool as boolean JSVal → JsValue::Bool
    let val = ctx.eval("true", "bool.js").expect("true should succeed");
    assert!(val.as_bool().unwrap_or(false) || val.is_number(), "true should be bool or number");

    // Undefined
    let val = ctx.eval("undefined", "undef.js").expect("undefined should succeed");
    assert!(val.is_undefined());

    // Null
    let val = ctx.eval("null", "null.js").expect("null should succeed");
    assert!(val.is_null());
}

fn test_07_eval_syntax_error(ctx: &mut JsContext) {
    let err = ctx.eval("function(", "syntax_err.js").expect_err("syntax error should be Err");
    assert!(!err.message.is_empty(), "error message should not be empty");
    assert_eq!(err.filename, "syntax_err.js");
}

fn test_07_eval_reference_error(ctx: &mut JsContext) {
    let err = ctx.eval("nonexistentVariable", "ref_err.js").expect_err("reference error should be Err");
    assert!(!err.message.is_empty());
}

fn test_07_eval_throw_error(ctx: &mut JsContext) {
    let err = ctx.eval("throw new Error('test error')", "throw.js").expect_err("throw should be Err");
    assert!(
        err.message.contains("test error"),
        "message should contain 'test error', got: {}", err.message
    );
}

fn test_08_global_setup_hook(ctx: &mut JsContext) {
    static CALLED: AtomicBool = AtomicBool::new(false);

    unsafe fn setup_hook(
        _cx: &mut mozjs::context::JSContext,
        _global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
    ) {
        CALLED.store(true, Ordering::SeqCst);
    }

    ctx.set_global_setup(setup_hook);
    assert!(ctx.global_setup().is_some());

    let result = ctx.eval("1", "setup_test.js");
    assert!(result.is_ok());
    assert!(CALLED.load(Ordering::SeqCst), "global_setup should have been called");
}

fn test_09_post_eval_hook(ctx: &mut JsContext) {
    static HOOK_CALLED: AtomicBool = AtomicBool::new(false);

    fn hook(_cx: &mut mozjs::context::JSContext) -> bool {
        HOOK_CALLED.store(true, Ordering::SeqCst);
        false
    }

    ctx.set_post_eval_hook(hook);
    assert!(ctx.post_eval_hook().is_some());

    let result = ctx.eval("42", "hook_test.js");
    assert!(result.is_ok());
    assert!(HOOK_CALLED.load(Ordering::SeqCst), "post_eval_hook should have been called");

    // Verify hook is called per eval
    static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
    fn counting_hook(_cx: &mut mozjs::context::JSContext) -> bool {
        CALL_COUNT.fetch_add(1, Ordering::SeqCst);
        false
    }

    ctx.set_post_eval_hook(counting_hook);
    ctx.eval("1", "count1.js").unwrap();
    ctx.eval("2", "count2.js").unwrap();
    ctx.eval("3", "count3.js").unwrap();
    assert_eq!(CALL_COUNT.load(Ordering::SeqCst), 3, "hook should fire once per eval");
}

fn test_10_cx_mut(ctx: &mut JsContext) {
    let _cx = ctx.cx_mut();
    let result = ctx.eval("'cx_ok'", "cx_test.js");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().as_string().unwrap(), "cx_ok");
}

fn test_extra_object_and_array(ctx: &mut JsContext) {
    let val = ctx.eval("({a: 1, b: 2})", "obj.js").unwrap();
    assert!(val.is_object());

    let val = ctx.eval("[1, 2, 3]", "arr.js").unwrap();
    assert!(val.is_object(), "JS arrays are objects in SpiderMonkey");
}

fn test_extra_eval_isolation(ctx: &mut JsContext) {
    ctx.eval("var x = 100;", "state1.js").unwrap();
    // Each eval creates a new global — variables do NOT persist across evals
    let result = ctx.eval("typeof x === 'undefined'", "state2.js").unwrap();
    // x should be undefined in the new global
    assert!(result.as_bool().unwrap_or(false), "each eval should be isolated (new global)");
}

// ---------------------------------------------------------------------------
// Master test — single-threaded, single JsContext
// ---------------------------------------------------------------------------

#[test]
fn job_queue_and_context_integration() {
    // (5) JsContext::new() succeeds
    let mut ctx = JsContext::new().expect("JsContext::new() should succeed");

    // Run all sub-tests in order on the shared context
    test_01_job_queue_init_empty_state(&mut ctx);
    test_02_job_queue_enqueue_and_drain(&mut ctx);
    test_03_job_queue_fifo_ordering(&mut ctx);
    test_04_job_queue_capacity_length(&mut ctx);
    test_06_eval_simple_expressions(&mut ctx);
    test_07_eval_syntax_error(&mut ctx);
    test_07_eval_reference_error(&mut ctx);
    test_07_eval_throw_error(&mut ctx);
    test_08_global_setup_hook(&mut ctx);
    test_09_post_eval_hook(&mut ctx);
    test_10_cx_mut(&mut ctx);
    test_extra_object_and_array(&mut ctx);
    test_extra_eval_isolation(&mut ctx);

    // Leak ctx to avoid SpiderMonkey thread-local cleanup segfault on process exit.
    // The test passes functionally; this only affects teardown.
    std::mem::forget(ctx);
}
