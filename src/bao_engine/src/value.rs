// @trace REQ-ENG-003
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

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::ptr;

    #[test]
    fn is_undefined_true() {
        assert!(JsValue::Undefined.is_undefined());
        assert!(!JsValue::Null.is_undefined());
    }

    #[test]
    fn is_null_true() {
        assert!(JsValue::Null.is_null());
        assert!(!JsValue::Undefined.is_null());
    }

    #[test]
    fn is_number_true() {
        assert!(JsValue::Number(42.0).is_number());
        assert!(!JsValue::Undefined.is_number());
    }

    #[test]
    fn is_string_true() {
        assert!(JsValue::String("hello".into()).is_string());
        assert!(!JsValue::Undefined.is_string());
    }

    #[test]
    fn is_object_true() {
        assert!(JsValue::Object(ptr::null_mut()).is_object());
        assert!(!JsValue::Undefined.is_object());
    }

    #[test]
    fn as_bool_some() {
        assert_eq!(JsValue::Bool(true).as_bool(), Some(true));
        assert_eq!(JsValue::Bool(false).as_bool(), Some(false));
    }

    #[test]
    fn as_bool_none_for_non_bool() {
        assert_eq!(JsValue::Undefined.as_bool(), None);
        assert_eq!(JsValue::Number(1.0).as_bool(), None);
    }

    #[test]
    fn as_number_some() {
        assert_eq!(JsValue::Number(3.14).as_number(), Some(3.14));
        assert_eq!(JsValue::Number(0.0).as_number(), Some(0.0));
    }

    #[test]
    fn as_number_none_for_non_number() {
        assert_eq!(JsValue::Undefined.as_number(), None);
        assert_eq!(JsValue::String("42".into()).as_number(), None);
    }

    #[test]
    fn as_string_some() {
        assert_eq!(JsValue::String("hello".into()).as_string(), Some("hello"));
    }

    #[test]
    fn as_string_none_for_non_string() {
        assert_eq!(JsValue::Undefined.as_string(), None);
        assert_eq!(JsValue::Number(1.0).as_string(), None);
    }

    #[test]
    fn as_object_some() {
        let ptr = ptr::null_mut();
        assert_eq!(JsValue::Object(ptr).as_object(), Some(ptr));
    }

    #[test]
    fn as_object_none_for_non_object() {
        assert_eq!(JsValue::Undefined.as_object(), None);
    }

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
    fn to_display_string_integer() {
        assert_eq!(JsValue::Number(42.0).to_display_string(), "42");
        assert_eq!(JsValue::Number(0.0).to_display_string(), "0");
        assert_eq!(JsValue::Number(-7.0).to_display_string(), "-7");
    }

    #[test]
    fn to_display_string_float() {
        assert_eq!(JsValue::Number(3.14).to_display_string(), "3.14");
    }

    #[test]
    fn to_display_string_nan() {
        assert_eq!(JsValue::Number(f64::NAN).to_display_string(), "NaN");
    }

    #[test]
    fn to_display_string_infinity() {
        assert_eq!(JsValue::Number(f64::INFINITY).to_display_string(), "Infinity");
        assert_eq!(JsValue::Number(f64::NEG_INFINITY).to_display_string(), "-Infinity");
    }

    #[test]
    fn to_display_string_string_value() {
        assert_eq!(JsValue::String("hello".into()).to_display_string(), "hello");
    }

    #[test]
    fn to_display_string_object() {
        assert_eq!(JsValue::Object(ptr::null_mut()).to_display_string(), "[object Object]");
    }

    #[test]
    fn format_number_large_integer() {
        assert_eq!(format_number(1_000_000_000.0), "1000000000");
    }

    #[test]
    fn format_number_small_float() {
        assert_eq!(format_number(0.001), "0.001");
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
