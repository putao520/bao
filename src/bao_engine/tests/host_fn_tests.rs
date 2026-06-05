// @trace TEST-ENG-003-HOSTFN [req:REQ-ENG-003] [level:unit]
// Unit tests for bao_engine host_fn: ArgReader, define_host_fn!, console, call_function

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

unsafe fn install_test_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
) {
    bao_engine::host_fn::install_console(cx, global);
}

#[test]
fn test_host_fn_all() {
    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");
    ctx.set_global_setup(install_test_globals);

    // === console.time / console.timeEnd ===
    let result = eval_string(&mut ctx, r#"
        var results = [];
        console.time('test-timer');
        console.timeEnd('test-timer');
        results.push("timer_ok");
        results.join("|")
    "#);
    assert!(result.contains("timer_ok"), "console timer should work");

    // === console.count / console.countReset ===
    let result = eval_string(&mut ctx, r#"
        var results = [];
        console.count('mycount');
        console.count('mycount');
        console.count('mycount');
        console.countReset('mycount');
        console.count('mycount');
        results.push("count_ok");
        results.join("|")
    "#);
    assert!(result.contains("count_ok"), "console count should work");

    // === console.assert (should not throw) ===
    let result = eval_string(&mut ctx, r#"
        console.assert(true, 'should not appear');
        console.assert(false, 'should appear');
        "assert_ok"
    "#);
    assert_eq!(result, "assert_ok");

    // === console.trace ===
    let result = eval_string(&mut ctx, r#"
        console.trace('trace-test');
        "trace_ok"
    "#);
    assert_eq!(result, "trace_ok");

    // === console.dir ===
    let result = eval_string(&mut ctx, r#"
        console.dir({a: 1});
        "dir_ok"
    "#);
    assert_eq!(result, "dir_ok");

    // === console.clear ===
    let result = eval_string(&mut ctx, r#"
        console.clear();
        "clear_ok"
    "#);
    assert_eq!(result, "clear_ok");

    // === console.table ===
    let result = eval_string(&mut ctx, r#"
        console.table([{name: 'a', val: 1}]);
        "table_ok"
    "#);
    assert_eq!(result, "table_ok");

    // === typeof checks on console methods ===
    let result = eval_string(&mut ctx, r#"
        var results = [];
        results.push(typeof console.log);
        results.push(typeof console.error);
        results.push(typeof console.warn);
        results.push(typeof console.info);
        results.push(typeof console.debug);
        results.push(typeof console.dir);
        results.push(typeof console.time);
        results.push(typeof console.timeEnd);
        results.push(typeof console.trace);
        results.push(typeof console.assert);
        results.push(typeof console.clear);
        results.push(typeof console.count);
        results.push(typeof console.countReset);
        results.push(typeof console.table);
        results.join(",")
    "#);
    let parts: Vec<&str> = result.split(',').collect();
    assert_eq!(parts.len(), 14, "should have 14 console methods");
    for part in &parts {
        assert_eq!(*part, "function", "console method should be function, got: {}", part);
    }

    // === Error construction and message extraction ===
    let result = eval_string(&mut ctx, r#"
        try {
            throw new Error('test error message');
        } catch(e) {
            e.message
        }
    "#);
    assert_eq!(result, "test error message");

    // === TypeError ===
    let result = eval_string(&mut ctx, r#"
        try {
            null.foo;
        } catch(e) {
            e instanceof TypeError ? "TypeError" : e.message
        }
    "#);
    assert_eq!(result, "TypeError");

    // === SyntaxError from eval ===
    let result = eval_string(&mut ctx, r#"
        try {
            eval("function(");
        } catch(e) {
            e instanceof SyntaxError ? "SyntaxError" : e.message
        }
    "#);
    assert_eq!(result, "SyntaxError");

    // === RangeError ===
    let result = eval_string(&mut ctx, r#"
        try {
            new Array(-1);
        } catch(e) {
            e instanceof RangeError ? "RangeError" : e.message
        }
    "#);
    assert_eq!(result, "RangeError");

    // === Promise rejection handling ===
    let result = eval_string(&mut ctx, r#"
        typeof Promise.reject("err").catch(function(e){ return e; })
    "#);
    assert_eq!(result, "object");

    // === try-catch-finally ===
    let result = eval_string(&mut ctx, r#"
        var result = "";
        try {
            result += "try ";
            throw "err";
        } catch(e) {
            result += "catch ";
        } finally {
            result += "finally";
        }
        result
    "#);
    assert_eq!(result, "try catch finally");

    // === Error stack trace ===
    let result = eval_string(&mut ctx, r#"
        try {
            throw new Error("stack test");
        } catch(e) {
            typeof e.stack
        }
    "#);
    assert_eq!(result, "string", "Error.stack should be a string");

    // === Custom error class ===
    let result = eval_string(&mut ctx, r#"
        class MyError extends Error {
            constructor(msg) { super(msg); this.name = "MyError"; }
        }
        try { throw new MyError("custom"); }
        catch(e) { e.name + ":" + e.message; }
    "#);
    assert_eq!(result, "MyError:custom");

    // === Arrow functions ===
    assert_eq!(eval_number(&mut ctx, "((x) => x * 2)(21)"), 42.0);

    // === Destructuring ===
    let result = eval_string(&mut ctx, r#"
        var {a, b} = {a: "hello", b: "world"};
        a + " " + b
    "#);
    assert_eq!(result, "hello world");

    // === Spread operator ===
    assert_eq!(eval_number(&mut ctx, "var arr = [1,2,3]; var [a,...rest] = arr; rest.length"), 2.0);

    // === Template literals ===
    let result = eval_string(&mut ctx, "var x = 42; `value is ${x}`");
    assert_eq!(result, "value is 42");

    // === for...of ===
    assert_eq!(eval_number(&mut ctx, "var sum = 0; for (var x of [1,2,3,4,5]) sum += x; sum"), 15.0);

    // === Symbol ===
    let result = eval_string(&mut ctx, "typeof Symbol('test')");
    assert_eq!(result, "symbol");

    // === WeakMap/WeakSet ===
    let result = eval_string(&mut ctx, r#"
        typeof new WeakMap() === 'object' && typeof new WeakSet() === 'object' ? "yes" : "no"
    "#);
    assert_eq!(result, "yes");

    // === Proxy ===
    let result = eval_string(&mut ctx, "typeof Proxy");
    assert_eq!(result, "function");

    // === Reflect ===
    let result = eval_string(&mut ctx, "typeof Reflect");
    assert_eq!(result, "object");

    std::mem::forget(ctx);
}
