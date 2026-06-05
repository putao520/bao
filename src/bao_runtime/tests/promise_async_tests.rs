// @trace TEST-ENG-008-ASYNC [req:REQ-ENG-001] [level:unit]
// Deep tests for Promise, async/await, microtask queue — single test for mozjs single-init.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_test_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
) {
    bao_runtime::globals::install_all(cx, global);
}

#[test]
fn test_promise_async_deep() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(install_test_globals);

    // =============================================
    // === Promise construction ===
    // =============================================

    // Promise.resolve basic
    let result = eval_string(&mut ctx, r#"
        var p = Promise.resolve(42);
        typeof p === "object" ? "object" : typeof p
    "#);
    assert_eq!(result, "object", "Promise.resolve should return object");

    // Promise.reject
    let result = eval_string(&mut ctx, r#"
        var p = Promise.reject("err");
        typeof p === "object" ? "object" : typeof p
    "#);
    assert_eq!(result, "object", "Promise.reject should return object");

    // new Promise constructor
    assert!(eval_bool(&mut ctx, r#"
        var resolved = false;
        new Promise(function(resolve) { resolve(1); });
        true
    "#), "new Promise constructor should work");

    // Promise with then chain
    let result = eval_number(&mut ctx, r#"
        var result = 0;
        Promise.resolve(10).then(function(v) { result = v * 2; });
        result
    "#);
    // Note: then callbacks are async, result may be 0 at sync eval time
    // This just tests that then() doesn't throw
    assert!(!result.is_nan(), "Promise.then should not throw");

    // Promise.all exists and is function
    assert!(eval_bool(&mut ctx, "typeof Promise.all === 'function'"), "Promise.all should exist");

    // Promise.allSettled exists
    assert!(eval_bool(&mut ctx, "typeof Promise.allSettled === 'function'"), "Promise.allSettled should exist");

    // Promise.race exists
    assert!(eval_bool(&mut ctx, "typeof Promise.race === 'function'"), "Promise.race should exist");

    // Promise.any exists
    assert!(eval_bool(&mut ctx, "typeof Promise.any === 'function'"), "Promise.any should exist");

    // =============================================
    // === Promise.prototype methods ===
    // =============================================

    // .then
    assert!(eval_bool(&mut ctx, r#"
        typeof Promise.resolve(1).then === 'function'
    "#), "Promise.then should be function");

    // .catch
    assert!(eval_bool(&mut ctx, r#"
        typeof Promise.reject(1).catch === 'function'
    "#), "Promise.catch should be function");

    // .finally
    assert!(eval_bool(&mut ctx, r#"
        typeof Promise.resolve(1).finally === 'function'
    "#), "Promise.finally should be function");

    // =============================================
    // === async/await syntax ===
    // =============================================

    // async function declaration
    assert!(eval_bool(&mut ctx, r#"
        async function foo() { return 1; }
        foo() instanceof Promise
    "#), "async function should return Promise");

    // async function expression
    assert!(eval_bool(&mut ctx, r#"
        var fn = async function() { return 42; };
        fn() instanceof Promise
    "#), "async function expression should return Promise");

    // async arrow function
    assert!(eval_bool(&mut ctx, r#"
        var fn = async () => 99;
        fn() instanceof Promise
    "#), "async arrow should return Promise");

    // await inside async
    let result = eval_number(&mut ctx, r#"
        var syncResult = 0;
        async function compute() {
            var x = await Promise.resolve(7);
            syncResult = x * 3;
            return syncResult;
        }
        compute();
        syncResult
    "#);
    // await is async, so syncResult might be 0 — just verify no error
    assert!(!result.is_nan(), "async/await should not throw");

    // =============================================
    // === try/catch with async ===
    // =============================================

    let result = eval_string(&mut ctx, r#"
        async function safeReject() {
            try {
                await Promise.reject("boom");
            } catch(e) {
                return "caught:" + e;
            }
        }
        "ok"
    "#);
    assert_eq!(result, "ok", "async function with try/catch should parse");

    // =============================================
    // === Promise combinators deep ===
    // =============================================

    // Promise.all with array
    let result = eval_string(&mut ctx, r#"
        var p = Promise.all([Promise.resolve(1), Promise.resolve(2), Promise.resolve(3)]);
        p instanceof Promise ? "promise" : typeof p
    "#);
    assert_eq!(result, "promise", "Promise.all should return Promise");

    // Promise.all with mixed values (non-Promise resolves automatically)
    assert!(eval_bool(&mut ctx, r#"
        Promise.all([1, Promise.resolve(2), 3]) instanceof Promise
    "#), "Promise.all with mixed values should work");

    // Promise.race
    assert!(eval_bool(&mut ctx, r#"
        Promise.race([Promise.resolve("fast"), Promise.resolve("slow")]) instanceof Promise
    "#), "Promise.race should return Promise");

    // Promise.allSettled
    assert!(eval_bool(&mut ctx, r#"
        Promise.allSettled([Promise.resolve(1), Promise.reject("err")]) instanceof Promise
    "#), "Promise.allSettled should return Promise");

    // Promise.any
    assert!(eval_bool(&mut ctx, r#"
        Promise.any([Promise.reject("a"), Promise.resolve("b")]) instanceof Promise
    "#), "Promise.any should return Promise");

    // =============================================
    // === Promise chaining patterns ===
    // =============================================

    // then returns new Promise (chainability)
    assert!(eval_bool(&mut ctx, r#"
        var p1 = Promise.resolve(1);
        var p2 = p1.then(function(x) { return x + 1; });
        p1 !== p2
    "#), "then should return new Promise");

    // catch returns new Promise
    assert!(eval_bool(&mut ctx, r#"
        var p1 = Promise.reject("err");
        var p2 = p1.catch(function() { return "recovered"; });
        p1 !== p2
    "#), "catch should return new Promise");

    // finally returns new Promise
    assert!(eval_bool(&mut ctx, r#"
        var p1 = Promise.resolve(1);
        var p2 = p1.finally(function() {});
        p1 !== p2
    "#), "finally should return new Promise");

    // =============================================
    // === Promise static utilities ===
    // =============================================

    // Promise.resolve with thenable
    assert!(eval_bool(&mut ctx, r#"
        var thenable = { then: function(resolve) { resolve(42); } };
        Promise.resolve(thenable) instanceof Promise
    "#), "Promise.resolve with thenable should work");

    // Promise constructor with reject
    assert!(eval_bool(&mut ctx, r#"
        new Promise(function(_, reject) { reject("nope"); }) instanceof Promise
    "#), "Promise constructor with reject should work");

    // =============================================
    // === Microtask/queue behavior ===
    // =============================================

    // queueMicrotask exists
    assert!(eval_bool(&mut ctx, "typeof queueMicrotask === 'function'"), "queueMicrotask should exist");

    // queueMicrotask callable
    let result = eval_string(&mut ctx, r#"
        var called = false;
        queueMicrotask(function() { called = true; });
        called ? "called" : "queued"
    "#);
    // Microtask is async — at sync eval time it may not have run
    assert!(result == "called" || result == "queued", "queueMicrotask should be callable, got: {}", result);

    // =============================================
    // === AsyncIterator / for await ===
    // =============================================

    // Symbol.asyncIterator exists
    assert!(eval_bool(&mut ctx, "typeof Symbol.asyncIterator === 'symbol'"), "Symbol.asyncIterator should exist");

    // Async generator syntax
    let result = eval_string(&mut ctx, r#"
        async function* asyncGen() { yield 1; yield 2; }
        var g = asyncGen();
        typeof g.next === "function" ? "ok" : "error"
    "#);
    assert_eq!(result, "ok", "async generator should be constructable");

    // async generator next() returns Promise
    assert!(eval_bool(&mut ctx, r#"
        async function* gen() { yield 42; }
        gen().next() instanceof Promise
    "#), "async generator next() should return Promise");

    // =============================================
    // === Error handling patterns ===
    // =============================================

    // Uncaught rejection doesn't crash
    assert!(eval_bool(&mut ctx, r#"
        Promise.reject("unhandled");
        true
    "#), "unhandled rejection should not crash");

    // Catch chain recovers
    assert!(eval_bool(&mut ctx, r#"
        Promise.reject("fail")
            .catch(function(e) { return "recovered:" + e; })
            instanceof Promise
    "#), "catch chain should recover");

    std::mem::forget(ctx);
}
