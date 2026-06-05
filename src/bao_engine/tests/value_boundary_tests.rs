// @trace TEST-ENG-001-VAL [req:REQ-ENG-001] [level:unit]
// bao_engine value type boundary tests — single test function to avoid mozjs single-init.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_val(ctx: &mut JsContext, source: &str) -> JsValue {
    ctx.eval(source, "<test>").unwrap_or(JsValue::Undefined)
}

unsafe fn install_test_globals(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
) {
    bao_engine::host_fn::install_console(cx, global);
}

#[test]
fn test_value_type_boundaries_all() {
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(install_test_globals);

    // --- Undefined ---
    let val = eval_val(&mut ctx, "undefined");
    assert!(val.is_undefined(), "undefined should be undefined");
    assert!(!val.is_null(), "undefined should not be null");
    assert!(!val.is_number(), "undefined should not be number");

    // --- Null ---
    let val = eval_val(&mut ctx, "null");
    assert!(val.is_null(), "null should be null");
    assert!(!val.is_undefined(), "null should not be undefined");

    // --- Boolean true ---
    let val = eval_val(&mut ctx, "true");
    assert_eq!(val.as_bool(), Some(true), "true should be bool(true)");

    // --- Boolean false ---
    let val = eval_val(&mut ctx, "false");
    assert_eq!(val.as_bool(), Some(false), "false should be bool(false)");

    // --- Integer ---
    let val = eval_val(&mut ctx, "42");
    assert!(val.is_number(), "42 should be number");
    assert_eq!(val.as_number(), Some(42.0), "42 should be 42.0");

    // --- Float ---
    let val = eval_val(&mut ctx, "3.14");
    assert!(val.is_number(), "3.14 should be number");
    assert_eq!(val.as_number(), Some(3.14), "3.14 should be 3.14");

    // --- Negative ---
    let val = eval_val(&mut ctx, "-100");
    assert!(val.is_number(), "-100 should be number");
    assert_eq!(val.as_number(), Some(-100.0), "-100 should be -100.0");

    // --- Zero ---
    let val = eval_val(&mut ctx, "0");
    assert!(val.is_number(), "0 should be number");
    assert_eq!(val.as_number(), Some(0.0), "0 should be 0.0");

    // --- Empty string ---
    let val = eval_val(&mut ctx, "''");
    assert!(val.is_string(), "'' should be string");
    assert_eq!(val.as_string(), Some(""), "'' should be empty string");

    // --- Non-empty string ---
    let val = eval_val(&mut ctx, "'hello'");
    assert!(val.is_string(), "'hello' should be string");
    assert_eq!(val.as_string(), Some("hello"), "'hello' should be hello");

    // --- Object ---
    let val = eval_val(&mut ctx, "({a: 1})");
    assert!(val.is_object(), "object literal should be object");
    assert!(val.as_object().is_some(), "object literal should have object ref");

    // --- Array is object ---
    let val = eval_val(&mut ctx, "[1, 2, 3]");
    assert!(val.is_object(), "[1,2,3] should be object");

    // --- Cross-type: as_number on string returns None ---
    let val = eval_val(&mut ctx, "'not a number'");
    assert!(val.is_string(), "'not a number' should be string");
    assert!(val.as_number().is_none(), "as_number on string should be None");

    // --- Cross-type: as_string on number returns None ---
    let val = eval_val(&mut ctx, "42");
    assert!(val.is_number(), "42 should be number");
    assert!(val.as_string().is_none(), "as_string on number should be None");

    // --- to_display_string ---
    let val = eval_val(&mut ctx, "'test'");
    let display = val.to_display_string();
    assert!(display.contains("test") || display.contains("String"), "display should contain test info");

    // --- eval error returns undefined via eval_val ---
    let val = eval_val(&mut ctx, "throw new Error('test')");
    assert!(val.is_undefined(), "throw should result in undefined via eval_val");

    // --- eval error returns Err ---
    let result = ctx.eval("throw new Error('test')", "<test>");
    assert!(result.is_err(), "throw should return Err from eval");

    // --- Syntax error ---
    let result = ctx.eval("function(", "<test>");
    assert!(result.is_err(), "syntax error should return Err");

    // --- MAX_SAFE_INTEGER ---
    let val = eval_val(&mut ctx, "Number.MAX_SAFE_INTEGER");
    assert!(val.is_number(), "MAX_SAFE_INTEGER should be number");
    assert_eq!(val.as_number(), Some(9007199254740991.0), "MAX_SAFE_INTEGER");

    // --- NaN ---
    let val = eval_val(&mut ctx, "NaN");
    assert!(val.is_number(), "NaN should be number");
    let num = val.as_number().unwrap();
    assert!(num.is_nan(), "NaN should be NaN");

    // --- Infinity ---
    let val = eval_val(&mut ctx, "Infinity");
    assert!(val.is_number(), "Infinity should be number");
    let num = val.as_number().unwrap();
    assert!(num.is_infinite() && num.is_sign_positive(), "Infinity should be +inf");

    // --- String concatenation ---
    let val = eval_val(&mut ctx, "'hello' + ' ' + 'world'");
    assert_eq!(val.as_string(), Some("hello world"), "string concat");

    std::mem::forget(ctx);
}
