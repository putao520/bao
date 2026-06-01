// @trace TEST-ENG-003 [req:REQ-ENG-003] [level:unit]
use bao_engine::error::JsError;
use bao_engine::value::JsValue;
use std::ptr;

// ---------------------------------------------------------------------------
// JsValue construction
// ---------------------------------------------------------------------------

#[test]
fn jsvalue_undefined_construction() {
    let v = JsValue::Undefined;
    assert!(v.is_undefined());
}

#[test]
fn jsvalue_null_construction() {
    let v = JsValue::Null;
    assert!(v.is_null());
}

#[test]
fn jsvalue_bool_construction() {
    let t = JsValue::Bool(true);
    let f = JsValue::Bool(false);
    assert_eq!(t.as_bool(), Some(true));
    assert_eq!(f.as_bool(), Some(false));
}

#[test]
fn jsvalue_number_construction() {
    let zero = JsValue::Number(0.0);
    let pi = JsValue::Number(std::f64::consts::PI);
    let neg = JsValue::Number(-42.5);
    assert!(zero.is_number());
    assert!(pi.is_number());
    assert!(neg.is_number());
    assert_eq!(zero.as_number(), Some(0.0));
    assert_eq!(pi.as_number(), Some(std::f64::consts::PI));
    assert_eq!(neg.as_number(), Some(-42.5));
}

#[test]
fn jsvalue_string_construction() {
    let empty = JsValue::String(String::new());
    let hello = JsValue::String("hello".into());
    assert!(empty.is_string());
    assert!(hello.is_string());
    assert_eq!(empty.as_string(), Some(""));
    assert_eq!(hello.as_string(), Some("hello"));
}

#[test]
fn jsvalue_object_construction() {
    let null_obj = JsValue::Object(ptr::null_mut());
    assert!(null_obj.is_object());
    assert_eq!(null_obj.as_object(), Some(ptr::null_mut()));
}

// ---------------------------------------------------------------------------
// JsValue is_* predicates — exhaustive cross-checks
// ---------------------------------------------------------------------------

#[test]
fn is_undefined_only_true_for_undefined() {
    assert!(JsValue::Undefined.is_undefined());
    assert!(!JsValue::Null.is_undefined());
    assert!(!JsValue::Bool(false).is_undefined());
    assert!(!JsValue::Number(0.0).is_undefined());
    assert!(!JsValue::String("".into()).is_undefined());
    assert!(!JsValue::Object(ptr::null_mut()).is_undefined());
}

#[test]
fn is_null_only_true_for_null() {
    assert!(!JsValue::Undefined.is_null());
    assert!(JsValue::Null.is_null());
    assert!(!JsValue::Bool(false).is_null());
    assert!(!JsValue::Number(0.0).is_null());
    assert!(!JsValue::String("".into()).is_null());
    assert!(!JsValue::Object(ptr::null_mut()).is_null());
}

#[test]
fn is_number_only_true_for_number() {
    assert!(!JsValue::Undefined.is_number());
    assert!(!JsValue::Null.is_number());
    assert!(!JsValue::Bool(false).is_number());
    assert!(JsValue::Number(0.0).is_number());
    assert!(!JsValue::String("".into()).is_number());
    assert!(!JsValue::Object(ptr::null_mut()).is_number());
}

#[test]
fn is_string_only_true_for_string() {
    assert!(!JsValue::Undefined.is_string());
    assert!(!JsValue::Null.is_string());
    assert!(!JsValue::Bool(false).is_string());
    assert!(!JsValue::Number(0.0).is_string());
    assert!(JsValue::String("".into()).is_string());
    assert!(!JsValue::Object(ptr::null_mut()).is_string());
}

#[test]
fn is_object_only_true_for_object() {
    assert!(!JsValue::Undefined.is_object());
    assert!(!JsValue::Null.is_object());
    assert!(!JsValue::Bool(false).is_object());
    assert!(!JsValue::Number(0.0).is_object());
    assert!(!JsValue::String("".into()).is_object());
    assert!(JsValue::Object(ptr::null_mut()).is_object());
}

// ---------------------------------------------------------------------------
// JsValue as_* extractors — None on type mismatch
// ---------------------------------------------------------------------------

#[test]
fn as_bool_returns_none_for_wrong_types() {
    assert_eq!(JsValue::Undefined.as_bool(), None);
    assert_eq!(JsValue::Null.as_bool(), None);
    assert_eq!(JsValue::Number(1.0).as_bool(), None);
    assert_eq!(JsValue::String("true".into()).as_bool(), None);
    assert_eq!(JsValue::Object(ptr::null_mut()).as_bool(), None);
}

#[test]
fn as_number_returns_none_for_wrong_types() {
    assert_eq!(JsValue::Undefined.as_number(), None);
    assert_eq!(JsValue::Null.as_number(), None);
    assert_eq!(JsValue::Bool(false).as_number(), None);
    assert_eq!(JsValue::String("42".into()).as_number(), None);
    assert_eq!(JsValue::Object(ptr::null_mut()).as_number(), None);
}

#[test]
fn as_string_returns_none_for_wrong_types() {
    assert_eq!(JsValue::Undefined.as_string(), None);
    assert_eq!(JsValue::Null.as_string(), None);
    assert_eq!(JsValue::Bool(true).as_string(), None);
    assert_eq!(JsValue::Number(42.0).as_string(), None);
    assert_eq!(JsValue::Object(ptr::null_mut()).as_string(), None);
}

#[test]
fn as_object_returns_none_for_wrong_types() {
    assert_eq!(JsValue::Undefined.as_object(), None);
    assert_eq!(JsValue::Null.as_object(), None);
    assert_eq!(JsValue::Bool(true).as_object(), None);
    assert_eq!(JsValue::Number(42.0).as_object(), None);
    assert_eq!(JsValue::String("{}".into()).as_object(), None);
}

// ---------------------------------------------------------------------------
// JsValue::to_display_string — all variants
// ---------------------------------------------------------------------------

#[test]
fn to_display_string_undefined() {
    assert_eq!(JsValue::Undefined.to_display_string(), "undefined");
}

#[test]
fn to_display_string_null() {
    assert_eq!(JsValue::Null.to_display_string(), "null");
}

#[test]
fn to_display_string_bool() {
    assert_eq!(JsValue::Bool(true).to_display_string(), "true");
    assert_eq!(JsValue::Bool(false).to_display_string(), "false");
}

#[test]
fn to_display_string_integer_numbers() {
    assert_eq!(JsValue::Number(0.0).to_display_string(), "0");
    assert_eq!(JsValue::Number(42.0).to_display_string(), "42");
    assert_eq!(JsValue::Number(-7.0).to_display_string(), "-7");
    assert_eq!(JsValue::Number(1_000_000_000.0).to_display_string(), "1000000000");
}

#[test]
fn to_display_string_float_numbers() {
    assert_eq!(JsValue::Number(3.14).to_display_string(), "3.14");
    assert_eq!(JsValue::Number(0.001).to_display_string(), "0.001");
}

#[test]
fn to_display_string_special_floats() {
    assert_eq!(JsValue::Number(f64::NAN).to_display_string(), "NaN");
    assert_eq!(JsValue::Number(f64::INFINITY).to_display_string(), "Infinity");
    assert_eq!(JsValue::Number(f64::NEG_INFINITY).to_display_string(), "-Infinity");
}

#[test]
fn to_display_string_string_value() {
    assert_eq!(JsValue::String("hello world".into()).to_display_string(), "hello world");
    assert_eq!(JsValue::String(String::new()).to_display_string(), "");
}

#[test]
fn to_display_string_object() {
    assert_eq!(JsValue::Object(ptr::null_mut()).to_display_string(), "[object Object]");
}

// ---------------------------------------------------------------------------
// JsValue Debug formatting
// ---------------------------------------------------------------------------

#[test]
fn jsvalue_debug_format_undefined() {
    let debug = format!("{:?}", JsValue::Undefined);
    assert_eq!(debug, "Undefined");
}

#[test]
fn jsvalue_debug_format_null() {
    let debug = format!("{:?}", JsValue::Null);
    assert_eq!(debug, "Null");
}

#[test]
fn jsvalue_debug_format_bool() {
    assert_eq!(format!("{:?}", JsValue::Bool(true)), "Bool(true)");
    assert_eq!(format!("{:?}", JsValue::Bool(false)), "Bool(false)");
}

#[test]
fn jsvalue_debug_format_number() {
    let debug = format!("{:?}", JsValue::Number(3.14));
    assert!(debug.starts_with("Number("));
    assert!(debug.contains("3.14"));
}

#[test]
fn jsvalue_debug_format_string() {
    assert_eq!(format!("{:?}", JsValue::String("abc".into())), "String(\"abc\")");
}

#[test]
fn jsvalue_debug_format_object() {
    let debug = format!("{:?}", JsValue::Object(ptr::null_mut()));
    assert!(debug.starts_with("Object("));
}

// ---------------------------------------------------------------------------
// JsValue Clone
// ---------------------------------------------------------------------------

#[test]
fn jsvalue_clone_undefined() {
    let original = JsValue::Undefined;
    let cloned = original.clone();
    assert!(cloned.is_undefined());
}

#[test]
fn jsvalue_clone_null() {
    let original = JsValue::Null;
    let cloned = original.clone();
    assert!(cloned.is_null());
}

#[test]
fn jsvalue_clone_bool() {
    let original = JsValue::Bool(true);
    let cloned = original.clone();
    assert_eq!(cloned.as_bool(), Some(true));
}

#[test]
fn jsvalue_clone_number() {
    let original = JsValue::Number(2.718);
    let cloned = original.clone();
    assert_eq!(cloned.as_number(), Some(2.718));
}

#[test]
fn jsvalue_clone_string() {
    let original = JsValue::String("test string".into());
    let cloned = original.clone();
    assert_eq!(cloned.as_string(), Some("test string"));
}

#[test]
fn jsvalue_clone_object() {
    let addr = 0xDEAD_usize;
    let original = JsValue::Object(addr as *mut _);
    let cloned = original.clone();
    assert_eq!(cloned.as_object(), original.as_object());
}

#[test]
fn jsvalue_clone_independent_from_original() {
    let original = JsValue::String("original".into());
    let cloned = original.clone();
    // Cloned string is independent — modifying clone doesn't affect original
    if let Some(s) = cloned.as_string() {
        assert_eq!(s, "original");
    }
    assert_eq!(original.as_string(), Some("original"));
}

// ---------------------------------------------------------------------------
// JsError construction
// ---------------------------------------------------------------------------

#[test]
fn jserror_construction_all_fields() {
    let err = JsError {
        message: "type error".into(),
        filename: "main.js".into(),
        line: 42,
        column: 7,
        stack: Some("at foo (main.js:42:7)\nat bar (main.js:10:3)".into()),
    };
    assert_eq!(err.message, "type error");
    assert_eq!(err.filename, "main.js");
    assert_eq!(err.line, 42);
    assert_eq!(err.column, 7);
    assert!(err.stack.is_some());
    assert!(err.stack.as_ref().unwrap().contains("at foo"));
}

#[test]
fn jserror_construction_minimal() {
    let err = JsError {
        message: "err".into(),
        filename: "a.js".into(),
        line: 0,
        column: 0,
        stack: None,
    };
    assert_eq!(err.message, "err");
    assert_eq!(err.filename, "a.js");
    assert_eq!(err.line, 0);
    assert_eq!(err.column, 0);
    assert!(err.stack.is_none());
}

#[test]
fn jserror_construction_empty_strings() {
    let err = JsError {
        message: String::new(),
        filename: String::new(),
        line: 1,
        column: 1,
        stack: Some(String::new()),
    };
    assert!(err.message.is_empty());
    assert!(err.filename.is_empty());
    assert!(err.stack.as_ref().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// JsError Display formatting
// ---------------------------------------------------------------------------

#[test]
fn jserror_display_without_stack() {
    let err = JsError {
        message: "something went wrong".into(),
        filename: "test.js".into(),
        line: 10,
        column: 5,
        stack: None,
    };
    assert_eq!(format!("{err}"), "test.js:10:5: something went wrong");
}

#[test]
fn jserror_display_with_stack() {
    let err = JsError {
        message: "oops".into(),
        filename: "app.js".into(),
        line: 1,
        column: 1,
        stack: Some("  at foo (app.js:1:1)\n  at bar (app.js:2:2)".into()),
    };
    let displayed = format!("{err}");
    assert!(displayed.starts_with("app.js:1:1: oops\n"));
    assert!(displayed.contains("at foo"));
    assert!(displayed.contains("at bar"));
}

#[test]
fn jserror_display_zero_position() {
    let err = JsError {
        message: "x".into(),
        filename: "y".into(),
        line: 0,
        column: 0,
        stack: None,
    };
    assert_eq!(format!("{err}"), "y:0:0: x");
}

#[test]
fn jserror_display_large_line_column() {
    let err = JsError {
        message: "big".into(),
        filename: "bundle.js".into(),
        line: u32::MAX,
        column: u32::MAX,
        stack: None,
    };
    assert_eq!(
        format!("{err}"),
        format!("bundle.js:{}:{}: big", u32::MAX, u32::MAX)
    );
}

// ---------------------------------------------------------------------------
// JsError Debug formatting
// ---------------------------------------------------------------------------

#[test]
fn jserror_debug_includes_all_fields() {
    let err = JsError {
        message: "err".into(),
        filename: "a.js".into(),
        line: 3,
        column: 7,
        stack: Some("trace".into()),
    };
    let debug = format!("{err:?}");
    assert!(debug.contains("err"), "debug should contain message");
    assert!(debug.contains("a.js"), "debug should contain filename");
    assert!(debug.contains("trace"), "debug should contain stack");
}

#[test]
fn jserror_debug_without_stack() {
    let err = JsError {
        message: "msg".into(),
        filename: "f.js".into(),
        line: 1,
        column: 1,
        stack: None,
    };
    let debug = format!("{err:?}");
    assert!(debug.contains("msg"));
    assert!(debug.contains("f.js"));
    assert!(debug.contains("None"));
}

// ---------------------------------------------------------------------------
// JsError std::error::Error trait
// ---------------------------------------------------------------------------

#[test]
fn jserror_implements_std_error() {
    let err = JsError {
        message: "test".into(),
        filename: "f.js".into(),
        line: 0,
        column: 0,
        stack: None,
    };
    let _: &dyn std::error::Error = &err;
}

#[test]
fn jserror_can_be_used_in_result() {
    fn fallible() -> Result<(), JsError> {
        Err(JsError {
            message: "fail".into(),
            filename: "test.js".into(),
            line: 1,
            column: 1,
            stack: None,
        })
    }
    let result = fallible();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.message, "fail");
}
