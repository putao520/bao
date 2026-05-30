// @trace TEST-ENG-001-ERR [req:REQ-ENG-001] [level:unit]
// Deep tests for bao_engine error handling: error types, try/catch, async errors, stack traces
// Single test function due to mozjs single-init constraint.

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

unsafe fn install_test_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
) {
    bao_engine::host_fn::install_console(cx, global);
}

#[test]
fn test_error_handling_deep() {
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(install_test_globals);

    // =============================================
    // === Native Error types ===
    // =============================================

    // Error
    let result = eval_string(&mut ctx, r#"
        try { throw new Error("native error"); } catch(e) { e.message }
    "#);
    assert_eq!(result, "native error", "Error message should match");

    // TypeError
    let result = eval_string(&mut ctx, r#"
        try { null.foo; } catch(e) { e.constructor.name }
    "#);
    assert_eq!(result, "TypeError", "null access should throw TypeError");

    // RangeError
    let result = eval_string(&mut ctx, r#"
        try { new Array(-1); } catch(e) { e.constructor.name }
    "#);
    assert!(result.contains("RangeError"), "negative Array length should throw RangeError, got: {}", result);

    // SyntaxError
    let result = eval_string(&mut ctx, r#"
        try { eval("function("); } catch(e) { e.constructor.name }
    "#);
    assert_eq!(result, "SyntaxError", "bad eval should throw SyntaxError");

    // ReferenceError
    let result = eval_string(&mut ctx, r#"
        try { undeclaredVariable; } catch(e) { e.constructor.name }
    "#);
    assert_eq!(result, "ReferenceError", "undeclared variable should throw ReferenceError");

    // =============================================
    // === Error properties ===
    // =============================================

    // Error has .message
    let result = eval_string(&mut ctx, r#"
        var e = new Error("test_msg"); e.message
    "#);
    assert_eq!(result, "test_msg");

    // Error has .name
    let result = eval_string(&mut ctx, r#"
        var e = new Error("x"); e.name
    "#);
    assert_eq!(result, "Error");

    // Error has .stack
    assert!(eval_bool(&mut ctx, r#"
        var e = new Error("stack_test"); typeof e.stack === 'string' && e.stack.length > 0
    "#), "Error should have non-empty stack");

    // Custom error subclass
    let result = eval_string(&mut ctx, r#"
        class CustomError extends Error {
            constructor(msg) { super(msg); this.name = "CustomError"; }
        }
        try { throw new CustomError("custom"); } catch(e) { e.name + ':' + e.message }
    "#);
    assert_eq!(result, "CustomError:custom");

    // =============================================
    // === try/catch/finally ===
    // =============================================

    // try/catch returns value
    let result = eval_string(&mut ctx, r#"
        var r = "";
        try { r += "try"; throw "err"; } catch(e) { r += "_" + e; }
        r
    "#);
    assert_eq!(result, "try_err");

    // finally executes
    let result = eval_string(&mut ctx, r#"
        var r = "";
        try { r += "try"; } finally { r += "_finally"; }
        r
    "#);
    assert_eq!(result, "try_finally");

    // try/catch/finally combination
    let result = eval_string(&mut ctx, r#"
        var r = "";
        try { r += "try"; throw "x"; } catch(e) { r += "_catch"; } finally { r += "_finally"; }
        r
    "#);
    assert_eq!(result, "try_catch_finally");

    // Nested try/catch
    let result = eval_string(&mut ctx, r#"
        var r = "";
        try {
            try { throw "inner"; } catch(e) { r += e; }
            throw "outer";
        } catch(e) { r += "_" + e; }
        r
    "#);
    assert_eq!(result, "inner_outer");

    // =============================================
    // === Error from engine API (Rust side) ===
    // =============================================

    // eval with syntax error returns Err
    let result = ctx.eval(")))", "<syntax-test>");
    assert!(result.is_err(), "syntax error should return Err");
    let err = result.unwrap_err();
    assert!(!err.message.is_empty(), "error message should be non-empty");
    assert_eq!(err.filename, "<syntax-test>");

    // eval with runtime error returns Err
    let result = ctx.eval("throw new Error('from engine')", "<runtime-test>");
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("from engine"));

    // =============================================
    // === Error propagation patterns ===
    // =============================================

    // Error in function call
    let result = eval_string(&mut ctx, r#"
        function thrower() { throw new Error("func_error"); }
        try { thrower(); } catch(e) { e.message }
    "#);
    assert_eq!(result, "func_error");

    // Error in nested function
    let result = eval_string(&mut ctx, r#"
        function inner() { throw new Error("deep"); }
        function outer() { inner(); }
        try { outer(); } catch(e) { e.message }
    "#);
    assert_eq!(result, "deep");

    // Promise rejection (static check)
    assert!(eval_bool(&mut ctx, "typeof Promise.reject === 'function'"));

    // =============================================
    // === Error in callbacks / higher-order ===
    // =============================================

    // Error in Array.map callback
    let result = eval_string(&mut ctx, r#"
        try {
            [1,2,3].map(function(x) { throw new Error("map_err"); });
        } catch(e) { e.message }
    "#);
    assert_eq!(result, "map_err");

    // Error in setTimeout-like eval
    let result = eval_string(&mut ctx, r#"
        var caught = false;
        try { JSON.parse("invalid{json"); } catch(e) { caught = true; }
        caught ? "caught" : "not_caught"
    "#);
    assert_eq!(result, "caught");

    // =============================================
    // === Edge cases ===
    // =============================================

    // Throw non-Error
    let result = eval_string(&mut ctx, r#"
        try { throw 42; } catch(e) { String(e) }
    "#);
    assert_eq!(result, "42");

    // Throw string
    let result = eval_string(&mut ctx, r#"
        try { throw "string_error"; } catch(e) { e }
    "#);
    assert_eq!(result, "string_error");

    // Throw object
    let result = eval_string(&mut ctx, r#"
        try { throw {code: 500, msg: "fail"}; } catch(e) { e.code }
    "#);
    assert_eq!(result, "500");

    // Throw undefined
    let result = eval_string(&mut ctx, r#"
        try { throw undefined; } catch(e) { String(e) }
    "#);
    assert_eq!(result, "undefined");

    // Throw null
    let result = eval_string(&mut ctx, r#"
        try { throw null; } catch(e) { String(e) }
    "#);
    assert_eq!(result, "null");

    // Error.toString()
    let result = eval_string(&mut ctx, r#"
        new Error("test").toString()
    "#);
    assert!(result.contains("Error") && result.contains("test"), "Error toString should contain type and message, got: {}", result);

    // Multiple catch re-throw
    let result = eval_string(&mut ctx, r#"
        var r = "";
        try {
            try { throw new Error("a"); } catch(e) { r += e.message; throw new Error("b"); }
        } catch(e) { r += "_" + e.message; }
        r
    "#);
    assert_eq!(result, "a_b");

    std::mem::forget(ctx);
}
