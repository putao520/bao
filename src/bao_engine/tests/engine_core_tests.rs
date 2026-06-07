// @trace TEST-ENG-001-CORE [req:REQ-ENG-001] [level:unit]
// Unit tests for bao_engine core: context, value conversion, error handling
// All assertions in a single test to avoid mozjs per-thread single-init issue.

use bao_engine::context::JsContext;
use bao_engine::error::JsError;
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
fn test_bao_engine_core_all() {
    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");
    ctx.set_global_setup(install_test_globals);

    // --- Context creation (already done above) ---
    assert!(true, "JsContext created successfully");

    // --- Simple expression ---
    let result = eval_number(&mut ctx, "1 + 2");
    assert_eq!(result, 3.0, "1 + 2 should equal 3");

    // --- String literal ---
    let result = eval_string(&mut ctx, "'hello' + ' ' + 'world'");
    assert_eq!(result, "hello world");

    // --- Boolean ---
    match ctx.eval("true", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b),
        other => panic!("expected bool(true), got: {:?}", other),
    }
    match ctx.eval("false", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(!b),
        other => panic!("expected bool(false), got: {:?}", other),
    }
    match ctx.eval("1 === 1", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b),
        other => panic!("expected bool(true) for 1===1, got: {:?}", other),
    }
    match ctx.eval("1 === 2", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(!b),
        other => panic!("expected bool(false) for 1===2, got: {:?}", other),
    }

    // --- Undefined ---
    match ctx.eval("undefined", "<test>") {
        Ok(JsValue::Undefined) => {},
        other => panic!("expected undefined, got: {:?}", other),
    }

    // --- Null ---
    match ctx.eval("null", "<test>") {
        Ok(JsValue::Null) => {},
        other => panic!("expected null, got: {:?}", other),
    }

    // --- Object ---
    match ctx.eval("({a: 1})", "<test>") {
        Ok(JsValue::Object(_)) => {},
        other => panic!("expected object, got: {:?}", other),
    }

    // --- Error thrown ---
    let result = ctx.eval("throw new Error('test error')", "<test>");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("test error"), "error message should contain 'test error', got: {}", err.message);

    // --- Syntax error ---
    let result = ctx.eval("function(", "<test>");
    assert!(result.is_err(), "syntax error should return Err");

    // --- Arithmetic ---
    assert_eq!(eval_number(&mut ctx, "10 / 3"), 10.0 / 3.0);
    assert_eq!(eval_number(&mut ctx, "2 ** 10"), 1024.0);
    assert_eq!(eval_number(&mut ctx, "Math.PI > 3.14 && Math.PI < 3.15 ? 1 : 0"), 1.0);

    // --- JSON ---
    let result = eval_string(&mut ctx, "JSON.stringify({x: 1, y: [2, 3]})");
    assert!(result.contains("\"x\":1"), "JSON: {}", result);

    let result = eval_number(&mut ctx, "JSON.parse('{\"a\":42}').a");
    assert_eq!(result, 42.0);

    // --- Array methods ---
    assert_eq!(eval_string(&mut ctx, "[1,2,3].map(x => x * 2).join(',')"), "2,4,6");
    assert_eq!(eval_number(&mut ctx, "[1,2,3,4,5].filter(x => x > 3).length"), 2.0);
    assert_eq!(eval_number(&mut ctx, "[10,20,30].reduce((a,b) => a+b, 0)"), 60.0);

    // --- String methods ---
    assert_eq!(eval_string(&mut ctx, "'hello'.toUpperCase()"), "HELLO");
    assert_eq!(eval_string(&mut ctx, "'HELLO'.toLowerCase()"), "hello");
    assert_eq!(eval_string(&mut ctx, "'abc'.split('').reverse().join('')"), "cba");

    // --- Console ---
    match ctx.eval("typeof console === 'object'", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b, "console should be object"),
        other => panic!("expected bool for console check, got: {:?}", other),
    }
    match ctx.eval("typeof console.log === 'function'", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b),
        other => panic!("expected bool for console.log, got: {:?}", other),
    }

    // --- Promise ---
    match ctx.eval("typeof Promise === 'function'", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b),
        other => panic!("expected bool for Promise, got: {:?}", other),
    }

    // --- RegExp ---
    match ctx.eval("/hello/.test('hello world')", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b),
        other => panic!("expected bool for regex test, got: {:?}", other),
    }
    assert_eq!(eval_string(&mut ctx, "'abc123def'.replace(/\\d+/, 'NUM')"), "abcNUMdef");

    // --- Date ---
    match ctx.eval("new Date().getTime() > 0", "<test>") {
        Ok(JsValue::Bool(b)) => assert!(b),
        other => panic!("expected bool for date, got: {:?}", other),
    }

    // --- Map/Set ---
    let result = eval_string(&mut ctx, r#"
        var m = new Map();
        m.set('key', 'val');
        var s = new Set([1,2,3]);
        m.get('key') + ',' + s.size
    "#);
    assert_eq!(result, "val,3");

    // --- JsError struct ---
    let err = JsError {
        message: "test".into(),
        filename: "file.js".into(),
        line: 10,
        column: 5,
        stack: Some("at file.js:10:5".into()),
    };
    assert_eq!(err.message, "test");
    assert_eq!(err.filename, "file.js");
    assert_eq!(err.line, 10);
    assert_eq!(err.column, 5);
    assert!(err.stack.is_some());

    bao_engine::context::JsContext::shutdown_thread_sm();
}
