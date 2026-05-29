// REQ-ENG-004: SM Value ↔ Rust type conversion
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::JSVal;

#[derive(Debug, Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(::std::string::String),
    Object(*mut JSObject),
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

    pub fn is_object(&self) -> bool {
        matches!(self, JsValue::Object(_))
    }

    pub fn as_bool(&self) -> ::std::option::Option<bool> {
        match self {
            JsValue::Bool(b) => ::std::option::Option::Some(*b),
            _ => ::std::option::Option::None,
        }
    }

    pub fn as_number(&self) -> ::std::option::Option<f64> {
        match self {
            JsValue::Number(n) => ::std::option::Option::Some(*n),
            _ => ::std::option::Option::None,
        }
    }

    pub fn as_string(&self) -> ::std::option::Option<&str> {
        match self {
            JsValue::String(s) => ::std::option::Option::Some(s.as_str()),
            _ => ::std::option::Option::None,
        }
    }

    pub fn as_object(&self) -> ::std::option::Option<*mut JSObject> {
        match self {
            JsValue::Object(obj) => ::std::option::Option::Some(*obj),
            _ => ::std::option::Option::None,
        }
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

    /// # Safety
    /// `cx` must be a valid JSContext pointer. The returned JSVal borrows from
    /// SpiderMonkey GC — caller must ensure the value is rooted before any GC.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn to_jsval(&self, cx: *mut JSContext) -> JSVal {
        match self {
            JsValue::Undefined => mozjs::jsval::UndefinedValue(),
            JsValue::Null => mozjs::jsval::NullValue(),
            JsValue::Bool(b) => mozjs::jsval::BooleanValue(*b),
            JsValue::Number(n) => {
                if *n == (*n as i32) as f64 && n.abs() < i32::MAX as f64 {
                    mozjs::jsval::Int32Value(*n as i32)
                } else {
                    mozjs::jsval::DoubleValue(*n)
                }
            }
            JsValue::String(s) => {
                let c_str = ::std::ffi::CString::new(s.as_str())
                    .unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
                let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
                if js_str.is_null() {
                    mozjs::jsval::UndefinedValue()
                } else {
                    mozjs::jsval::StringValue(&*js_str)
                }
            }
            JsValue::Object(obj) => mozjs::jsval::ObjectValue(*obj),
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

/// Convert a SpiderMonkey JSVal to a safe JsValue enum.
///
/// # Safety
/// `cx` must be a valid JSContext pointer. `val` must be a valid JSVal.
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
            let rust_str = jsstr_to_string(cx, NonNull::new(s).expect("null-checked JSString"));
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

/// Get a JS property from an object as a JsValue.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject pointer.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn get_property(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: &str,
) -> JsValue {
    let c_name = ::std::ffi::CString::new(name)
        .unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
    let mut val = mozjs::jsval::UndefinedValue();
    let handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    };
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };
    if JS_GetProperty(cx, obj_handle, c_name.as_ptr(), handle) {
        jsval_to_jsvalue(cx, val)
    } else {
        JsValue::Undefined
    }
}

/// Set a JS property on an object from a JsValue.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject pointer.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn set_property(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: &str,
    value: &JsValue,
) -> bool {
    let c_name = ::std::ffi::CString::new(name)
        .unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
    let js_val = value.to_jsval(cx);
    let val_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &js_val,
    };
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };
    JS_SetProperty(cx, obj_handle, c_name.as_ptr(), val_handle)
}
