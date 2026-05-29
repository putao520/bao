use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::JSContext;
use mozjs::jsval::JSVal;

#[derive(Debug, Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(::std::string::String),
    Object(*mut mozjs::jsapi::JSObject),
}

impl JsValue {
    pub fn is_undefined(&self) -> bool {
        matches!(self, JsValue::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, JsValue::Null)
    }

    pub fn is_number(&self) -> bool {
        matches!(self, JsValue::Number(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, JsValue::String(_))
    }

    pub fn to_display_string(&self) -> ::std::string::String {
        match self {
            JsValue::Undefined => "undefined".into(),
            JsValue::Null => "null".into(),
            JsValue::Bool(b) => b.to_string(),
            JsValue::Number(n) => format_number(*n),
            JsValue::String(s) => s.clone(),
            JsValue::Object(_) => "[object Object]".into(),
        }
    }
}

fn format_number(n: f64) -> ::std::string::String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n == (n as i64) as f64 && n.abs() < 2e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn jsval_to_jsvalue(cx: *mut JSContext, val: JSVal) -> JsValue {
    if val.is_undefined() {
        JsValue::Undefined
    } else if val.is_null() {
        JsValue::Null
    } else if val.is_boolean() {
        JsValue::Bool(val.to_boolean())
    } else if val.is_int32() {
        JsValue::Number(val.to_int32() as f64)
    } else if val.is_double() {
        JsValue::Number(val.to_double())
    } else if val.is_string() {
        let raw_handle = mozjs::rust::HandleValue::from_marked_location(&val);
        let s = mozjs::rust::ToString(cx, raw_handle);
        if !s.is_null() {
            let rust_str = jsstr_to_string(cx, NonNull::new(s).unwrap());
            JsValue::String(rust_str)
        } else {
            JsValue::String(::std::string::String::new())
        }
    } else if val.is_object() {
        JsValue::Object(val.to_object())
    } else {
        JsValue::Undefined
    }
}
