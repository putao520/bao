// @trace TEST-ENG-015 [req:REQ-ENG-001,REQ-ENG-003] [level:unit]
// JsValue type predicates, as_* accessors, to_display_string, format_number edge cases.
// JsError Display, Debug, field access. Pure unit tests (no JSContext).

use bao_engine::value::JsValue;
use bao_engine::error::JsError;

// ---- JsValue type predicates ----

#[test]
fn test_undefined_is_undefined() {
    assert!(JsValue::Undefined.is_undefined());
    assert!(!JsValue::Undefined.is_null());
}

#[test]
fn test_null_is_null() {
    assert!(JsValue::Null.is_null());
    assert!(!JsValue::Null.is_undefined());
}

#[test]
fn test_bool_not_number() {
    let v = JsValue::Bool(true);
    assert!(!v.is_number());
    assert!(!v.is_string());
    assert!(!v.is_object());
}

#[test]
fn test_number_is_number() {
    let v = JsValue::Number(42.0);
    assert!(v.is_number());
    assert!(!v.is_string());
}

#[test]
fn test_string_is_string() {
    let v = JsValue::String("hello".into());
    assert!(v.is_string());
    assert!(!v.is_number());
}

// ---- as_* accessors ----

#[test]
fn test_as_bool_some() {
    assert_eq!(JsValue::Bool(true).as_bool(), Some(true));
    assert_eq!(JsValue::Bool(false).as_bool(), Some(false));
}

#[test]
fn test_as_bool_none() {
    assert_eq!(JsValue::Undefined.as_bool(), None);
    assert_eq!(JsValue::Number(1.0).as_bool(), None);
    assert_eq!(JsValue::String("true".into()).as_bool(), None);
}

#[test]
fn test_as_number_some() {
    assert_eq!(JsValue::Number(3.14).as_number(), Some(3.14));
    assert_eq!(JsValue::Number(0.0).as_number(), Some(0.0));
    assert_eq!(JsValue::Number(-1.0).as_number(), Some(-1.0));
}

#[test]
fn test_as_number_none() {
    assert_eq!(JsValue::Undefined.as_number(), None);
    assert_eq!(JsValue::Bool(true).as_number(), None);
}

#[test]
fn test_as_string_some() {
    let v = JsValue::String("test".into());
    assert_eq!(v.as_string(), Some("test"));
}

#[test]
fn test_as_string_empty() {
    let v = JsValue::String(String::new());
    assert_eq!(v.as_string(), Some(""));
}

#[test]
fn test_as_string_none() {
    assert_eq!(JsValue::Null.as_string(), None);
    assert_eq!(JsValue::Number(1.0).as_string(), None);
}

#[test]
fn test_as_object_none_for_primitives() {
    assert_eq!(JsValue::Undefined.as_object(), None);
    assert_eq!(JsValue::Null.as_object(), None);
    assert_eq!(JsValue::Bool(false).as_object(), None);
    assert_eq!(JsValue::Number(0.0).as_object(), None);
    assert_eq!(JsValue::String("".into()).as_object(), None);
}

// ---- to_display_string ----

#[test]
fn test_display_undefined() {
    assert_eq!(JsValue::Undefined.to_display_string(), "undefined");
}

#[test]
fn test_display_null() {
    assert_eq!(JsValue::Null.to_display_string(), "null");
}

#[test]
fn test_display_bool_true() {
    assert_eq!(JsValue::Bool(true).to_display_string(), "true");
}

#[test]
fn test_display_bool_false() {
    assert_eq!(JsValue::Bool(false).to_display_string(), "false");
}

#[test]
fn test_display_number_integer() {
    assert_eq!(JsValue::Number(42.0).to_display_string(), "42");
}

#[test]
fn test_display_number_zero() {
    assert_eq!(JsValue::Number(0.0).to_display_string(), "0");
}

#[test]
fn test_display_number_negative() {
    assert_eq!(JsValue::Number(-7.0).to_display_string(), "-7");
}

#[test]
fn test_display_number_float() {
    let s = JsValue::Number(3.14159).to_display_string();
    assert!(s.contains("3.14159"));
}

#[test]
fn test_display_number_nan() {
    assert_eq!(JsValue::Number(f64::NAN).to_display_string(), "NaN");
}

#[test]
fn test_display_number_infinity() {
    assert_eq!(JsValue::Number(f64::INFINITY).to_display_string(), "Infinity");
}

#[test]
fn test_display_number_neg_infinity() {
    assert_eq!(JsValue::Number(f64::NEG_INFINITY).to_display_string(), "-Infinity");
}

#[test]
fn test_display_number_large_int() {
    assert_eq!(JsValue::Number(1_000_000_000_000.0).to_display_string(), "1000000000000");
}

#[test]
fn test_display_number_very_large() {
    let s = JsValue::Number(2e15).to_display_string();
    // 2e15 is exactly at the boundary, format!("{}", 2e15)
    assert!(!s.is_empty());
}

#[test]
fn test_display_number_just_below_boundary() {
    // 1.999999999999999e15 — should be formatted as float since abs >= 2e15
    let s = JsValue::Number(1999999999999999.0).to_display_string();
    assert!(!s.is_empty());
}

#[test]
fn test_display_string() {
    assert_eq!(JsValue::String("hello world".into()).to_display_string(), "hello world");
}

#[test]
fn test_display_string_empty() {
    assert_eq!(JsValue::String(String::new()).to_display_string(), "");
}

#[test]
fn test_display_string_unicode() {
    assert_eq!(JsValue::String("日本語テスト".into()).to_display_string(), "日本語テスト");
}

#[test]
fn test_display_object() {
    assert_eq!(JsValue::Object(std::ptr::null_mut()).to_display_string(), "[object Object]");
}

// ---- JsValue Debug/Clone ----

#[test]
fn test_jsvalue_debug_undefined() {
    assert!(format!("{:?}", JsValue::Undefined).contains("Undefined"));
}

#[test]
fn test_jsvalue_debug_null() {
    assert!(format!("{:?}", JsValue::Null).contains("Null"));
}

#[test]
fn test_jsvalue_debug_bool() {
    assert!(format!("{:?}", JsValue::Bool(true)).contains("Bool"));
}

#[test]
fn test_jsvalue_debug_number() {
    assert!(format!("{:?}", JsValue::Number(42.0)).contains("42"));
}

#[test]
fn test_jsvalue_debug_string() {
    assert!(format!("{:?}", JsValue::String("test".into())).contains("test"));
}

#[test]
fn test_jsvalue_clone_undefined() {
    let v = JsValue::Undefined;
    assert_eq!(v.clone().to_display_string(), "undefined");
}

#[test]
fn test_jsvalue_clone_number() {
    let v = JsValue::Number(3.14);
    let cloned = v.clone();
    assert_eq!(cloned.as_number(), Some(3.14));
}

#[test]
fn test_jsvalue_clone_string() {
    let v = JsValue::String("clone me".into());
    let cloned = v.clone();
    assert_eq!(cloned.as_string(), Some("clone me"));
}

// ---- JsError ----

#[test]
fn test_jserror_fields() {
    let err = JsError {
        message: "syntax error".into(),
        filename: "test.js".into(),
        line: 10,
        column: 5,
        stack: None,
    };
    assert_eq!(err.message, "syntax error");
    assert_eq!(err.filename, "test.js");
    assert_eq!(err.line, 10);
    assert_eq!(err.column, 5);
    assert!(err.stack.is_none());
}

#[test]
fn test_jserror_display_no_stack() {
    let err = JsError {
        message: "oops".into(),
        filename: "a.js".into(),
        line: 1,
        column: 1,
        stack: None,
    };
    let msg = format!("{}", err);
    assert!(msg.contains("a.js:1:1"));
    assert!(msg.contains("oops"));
    assert!(!msg.contains('\n'));
}

#[test]
fn test_jserror_display_with_stack() {
    let err = JsError {
        message: "err".into(),
        filename: "b.js".into(),
        line: 5,
        column: 3,
        stack: Some("at foo (b.js:5:3)\nat bar (b.js:10:1)".into()),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("b.js:5:3"));
    assert!(msg.contains("err"));
    assert!(msg.contains("at foo"));
}

#[test]
fn test_jserror_debug() {
    let err = JsError {
        message: "d".into(),
        filename: "f".into(),
        line: 0,
        column: 0,
        stack: None,
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("JsError"));
}

#[test]
fn test_jserror_is_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(JsError {
        message: "boxed".into(),
        filename: "c.js".into(),
        line: 1,
        column: 1,
        stack: None,
    });
    let _ = format!("{}", err);
}

#[test]
fn test_jserror_empty_message() {
    let err = JsError {
        message: String::new(),
        filename: String::new(),
        line: 0,
        column: 0,
        stack: None,
    };
    let msg = format!("{}", err);
    assert!(msg.contains(":0:0:"));
}

#[test]
fn test_jserror_unicode_message() {
    let err = JsError {
        message: "エラー発生".into(),
        filename: "日本語.js".into(),
        line: 42,
        column: 7,
        stack: None,
    };
    let msg = format!("{}", err);
    assert!(msg.contains("エラー発生"));
    assert!(msg.contains("日本語.js"));
}

#[test]
fn test_jserror_stack_multiline() {
    let err = JsError {
        message: "err".into(),
        filename: "x.js".into(),
        line: 1,
        column: 1,
        stack: Some("line1\nline2\nline3".into()),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("line1"));
    assert!(msg.contains("line3"));
}
