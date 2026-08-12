// @trace REQ-ENG-002 [module:bun_sm]
//! `JSValue` — SpiderMonkey-backed value type replacing `bun_jsc::JSValue`.
//!
//! In JSC, `JSValue` is an ABI-encoded 64-bit immediate value.
//! In SpiderMonkey, the equivalent is `mozjs::jsval::JSVal`, but we expose
//! `crate::value::JsValue` as the high-level safe enum for ergonomics.
//!
//! This module provides both:
//! - `JSValue` (newtype over `crate::value::JsValue`) — safe, ergonomic
//! - `RawJSValue` (type alias for `mozjs::jsval::JSVal`) — FFI-compatible

use crate::value::JsValue;
use mozjs::jsapi::JSObject;
use mozjs::jsval::JSVal;
use mozjs::rooted;
/// SpiderMonkey-backed JSValue, compatible with the `bun_jsc::JSValue` API surface.
///
/// This is a newtype over `crate::value::JsValue`, providing JSC-compatible
/// methods like `is_undefined()`, `as_number()`, `to_boolean()`, etc.
///
/// # GC Safety
///
/// When this contains an `Object` variant, the raw `*mut JSObject` pointer
/// is NOT rooted. See `crate::value::JsValue::Object` for full caveats.
/// Use `GcStore` for persistent object storage.
#[derive(Debug, Clone)]
pub struct JSValue(pub(crate) JsValue);

// ── Constants (JSC API compatibility) ──────────────────────────────────────

impl JSValue {
    pub const UNDEFINED: JSValue = JSValue(JsValue::Undefined);
    pub const NULL: JSValue = JSValue(JsValue::Null);
    pub const TRUE: JSValue = JSValue(JsValue::Bool(true));
    pub const FALSE: JSValue = JSValue(JsValue::Bool(false));
    pub const ZERO: JSValue = JSValue(JsValue::Number(0.0));
    pub const ONE: JSValue = JSValue(JsValue::Number(1.0));
    pub const NAN: JSValue = JSValue(JsValue::Number(f64::NAN));
    pub const INFINITY: JSValue = JSValue(JsValue::Number(f64::INFINITY));
    pub const NEG_INFINITY: JSValue = JSValue(JsValue::Number(f64::NEG_INFINITY));
}

// ── Type checks ────────────────────────────────────────────────────────────

impl JSValue {
    #[inline]
    pub fn is_undefined(&self) -> bool {
        self.0.is_undefined()
    }

    #[inline]
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    #[inline]
    pub fn is_boolean(&self) -> bool {
        matches!(self.0, JsValue::Bool(_))
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        self.0.is_number()
    }

    #[inline]
    pub fn is_string(&self) -> bool {
        self.0.is_string()
    }

    #[inline]
    pub fn is_object(&self) -> bool {
        self.0.is_object()
    }

    #[inline]
    pub fn is_cell(&self) -> bool {
        self.is_object()
    }

    pub fn is_true(&self) -> bool {
        matches!(self.0, JsValue::Bool(true))
    }

    pub fn is_false(&self) -> bool {
        matches!(self.0, JsValue::Bool(false))
    }

    /// Returns true if this value is falsy per JS semantics.
    pub fn is_falsy(&self) -> bool {
        !self.to_boolean()
    }

    /// Returns true if this value is truthy.
    pub fn is_truthy(&self) -> bool {
        self.to_boolean()
    }

    /// Check if this is an integer number (no fractional part).
    pub fn is_int32(&self) -> bool {
        match self.0 {
            JsValue::Number(n) => n == (n as i32) as f64 && n.abs() < i32::MAX as f64,
            _ => false,
        }
    }

    /// Check if the number is NaN.
    pub fn is_nan(&self) -> bool {
        match self.0 {
            JsValue::Number(n) => n.is_nan(),
            _ => false,
        }
    }
}

// ── Accessors ──────────────────────────────────────────────────────────────

impl JSValue {
    #[inline]
    pub fn as_boolean(&self) -> Option<bool> {
        self.0.as_bool()
    }

    #[inline]
    pub fn as_number(&self) -> Option<f64> {
        self.0.as_number()
    }

    #[inline]
    pub fn as_string(&self) -> Option<&str> {
        self.0.as_string()
    }

    #[inline]
    pub fn as_object(&self) -> Option<*mut JSObject> {
        self.0.as_object()
    }

    #[inline]
    pub fn as_cell(&self) -> Option<*mut JSObject> {
        self.as_object()
    }

    /// Convert to boolean per JS semantics.
    pub fn to_boolean(&self) -> bool {
        match &self.0 {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::String(s) => !s.is_empty(),
            JsValue::Object(_) => true,
        }
    }

    /// Convert to f64. Returns NaN for non-numbers.
    pub fn to_number(&self) -> f64 {
        match &self.0 {
            JsValue::Number(n) => *n,
            JsValue::Bool(true) => 1.0,
            JsValue::Bool(false) => 0.0,
            JsValue::Undefined => f64::NAN,
            JsValue::Null => 0.0,
            JsValue::String(s) => s.parse::<f64>().unwrap_or(f64::NAN),
            JsValue::Object(_) => f64::NAN,
        }
    }

    /// Convert to i32. Returns 0 for non-integers.
    pub fn to_int32(&self) -> i32 {
        self.to_number() as i32
    }

    /// Get a display string for this value.
    pub fn to_display_string(&self) -> String {
        self.0.to_display_string()
    }
}

// ── Constructors ───────────────────────────────────────────────────────────

impl JSValue {
    pub fn from_boolean(b: bool) -> Self {
        JSValue(JsValue::Bool(b))
    }

    pub fn from_number(n: f64) -> Self {
        JSValue(JsValue::Number(n))
    }

    pub fn from_string(s: String) -> Self {
        JSValue(JsValue::String(s))
    }

    pub fn from_object(obj: *mut JSObject) -> Self {
        JSValue(JsValue::Object(obj))
    }

    /// Create from a raw SM JSVal.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. `val` must be a valid JSVal.
    pub unsafe fn from_raw(cx: *mut mozjs::jsapi::JSContext, val: JSVal) -> Self {
        JSValue(unsafe { crate::value::jsval_to_jsvalue(cx, val) })
    }

    /// Create from a bao_engine JsValue.
    pub fn from_inner(v: JsValue) -> Self {
        JSValue(v)
    }

    /// Extract the inner bao_engine JsValue.
    pub fn into_inner(self) -> JsValue {
        self.0
    }

    /// Get a reference to the inner bao_engine JsValue.
    pub fn as_inner(&self) -> &JsValue {
        &self.0
    }
}

// ── SpiderMonkey-backed conversions ────────────────────────────────────────

impl JSValue {
    /// Convert to a Rust String using SpiderMonkey's ToString.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn to_string(
        &self,
        cx: *mut mozjs::jsapi::JSContext,
    ) -> ::std::result::Result<String, crate::error::JsError> {
        let js_val = self.0.to_jsval(cx);
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let val_root = js_val);
        let js_str = unsafe { mozjs::rust::ToString(cx, val_root.handle().into()) };
        if js_str.is_null() {
            return Err(crate::error::JsError {
                message: "ToString failed".into(),
                filename: String::new(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        let nn = ::std::ptr::NonNull::new(js_str).unwrap();
        Ok(unsafe { mozjs::conversions::jsstr_to_string(cx, nn) })
    }

    /// Convert to a JSObject pointer using SpiderMonkey's ToObject.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn to_object(
        &self,
        cx: *mut mozjs::jsapi::JSContext,
    ) -> ::std::result::Result<*mut JSObject, crate::error::JsError> {
        match self.as_object() {
            Some(obj) => Ok(obj),
            None => {
                // BCE-012: root js_val — ToObjectSlow can trigger GC
                let wrapped_cx =
                    mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                let js_val = self.0.to_jsval(cx);
                rooted!(&in(wrapped_cx) let js_val_root = js_val);
                let obj =
                    unsafe { mozjs::jsapi::ToObjectSlow(cx, js_val_root.handle().into(), false) };
                if obj.is_null() {
                    Err(crate::error::JsError {
                        message: "ToObject failed".into(),
                        filename: String::new(),
                        line: 0,
                        column: 0,
                        stack: None,
                    })
                } else {
                    Ok(obj)
                }
            }
        }
    }

    /// Convert to f64 using SpiderMonkey's ToNumber.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn to_number_sm(
        &self,
        cx: *mut mozjs::jsapi::JSContext,
    ) -> ::std::result::Result<f64, crate::error::JsError> {
        let js_val = self.0.to_jsval(cx);
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let val_root = js_val);
        unsafe { mozjs::rust::ToNumber(cx, val_root.handle().into()) }.map_err(|()| {
            crate::error::JsError {
                message: "ToNumber failed".into(),
                filename: String::new(),
                line: 0,
                column: 0,
                stack: None,
            }
        })
    }

    /// Check if this value is a JS function.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn is_function(&self) -> bool {
        match self.as_object() {
            Some(obj) if !obj.is_null() => unsafe { mozjs::jsapi::JS_ObjectIsFunction(obj) },
            _ => false,
        }
    }

    /// Check if this value is a JS Array.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn is_array(&self, cx: *mut mozjs::jsapi::JSContext) -> bool {
        match self.as_object() {
            Some(obj) if !obj.is_null() => {
                let mut is_array = false;
                // BCE-20260619-012: root obj before passing as Handle to JS API.
                let cx_ref = &mut mozjs::context::JSContext::from_ptr(
                    ::std::ptr::NonNull::new_unchecked(cx),
                );
                rooted!(&in(cx_ref) let obj_root = obj);
                mozjs::jsapi::IsArrayObject1(cx, obj_root.handle().into(), &mut is_array);
                is_array
            }
            _ => false,
        }
    }

    /// Check if this value is a JS Error object.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn is_error(&self, _cx: *mut mozjs::jsapi::JSContext) -> bool {
        match self.as_object() {
            Some(obj) if !obj.is_null() => {
                let clasp = mozjs::rust::get_object_class(obj);
                if clasp.is_null() {
                    return false;
                }
                let name_ptr = (*clasp).name;
                if name_ptr.is_null() {
                    return false;
                }
                let name = ::std::ffi::CStr::from_ptr(name_ptr);
                let name_str = name.to_string_lossy();
                name_str.ends_with("Error")
            }
            _ => false,
        }
    }

    /// If this value is a function, return the JSObject pointer.
    pub fn as_function(&self) -> Option<*mut JSObject> {
        if self.is_function() {
            self.as_object()
        } else {
            None
        }
    }

    /// Get a named property from this value (as an object).
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_property(&self, cx: *mut mozjs::jsapi::JSContext, name: &str) -> JSValue {
        match self.to_object(cx) {
            Ok(obj) => crate::value::get_property(cx, obj, name).into(),
            Err(_) => JSValue::UNDEFINED,
        }
    }
}

// ── JsValue → JSValue conversion ──────────────────────────────────────────

impl From<JsValue> for JSValue {
    fn from(v: JsValue) -> Self {
        JSValue(v)
    }
}

// ── Equality ───────────────────────────────────────────────────────────────

impl PartialEq for JSValue {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (JsValue::Undefined, JsValue::Undefined) => true,
            (JsValue::Null, JsValue::Null) => true,
            (JsValue::Bool(a), JsValue::Bool(b)) => a == b,
            (JsValue::Number(a), JsValue::Number(b)) => {
                if a.is_nan() && b.is_nan() {
                    false // NaN !== NaN per JS spec
                } else {
                    a == b
                }
            }
            (JsValue::String(a), JsValue::String(b)) => a == b,
            (JsValue::Object(a), JsValue::Object(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for JSValue {}

/// Raw FFI-compatible JSVal type alias.
pub type RawJSValue = JSVal;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undefined_is_falsy() {
        assert!(JSValue::UNDEFINED.is_falsy());
        assert!(!JSValue::UNDEFINED.is_truthy());
    }

    #[test]
    fn null_is_falsy() {
        assert!(JSValue::NULL.is_falsy());
    }

    #[test]
    fn true_is_truthy() {
        assert!(JSValue::TRUE.is_truthy());
    }

    #[test]
    fn false_is_falsy() {
        assert!(JSValue::FALSE.is_falsy());
    }

    #[test]
    fn zero_is_falsy() {
        assert!(JSValue::ZERO.is_falsy());
    }

    #[test]
    fn one_is_truthy() {
        assert!(JSValue::ONE.is_truthy());
    }

    #[test]
    fn nan_is_falsy() {
        assert!(JSValue::NAN.is_falsy());
    }

    #[test]
    fn empty_string_is_falsy() {
        assert!(JSValue::from_string(String::new()).is_falsy());
    }

    #[test]
    fn nonempty_string_is_truthy() {
        assert!(JSValue::from_string("hello".into()).is_truthy());
    }

    #[test]
    fn nan_not_equal_nan() {
        assert_ne!(JSValue::NAN, JSValue::NAN);
    }

    #[test]
    fn to_number_conversions() {
        assert_eq!(JSValue::TRUE.to_number(), 1.0);
        assert_eq!(JSValue::FALSE.to_number(), 0.0);
        assert!(JSValue::UNDEFINED.to_number().is_nan());
        assert_eq!(JSValue::NULL.to_number(), 0.0);
    }

    #[test]
    fn is_int32_true() {
        assert!(JSValue::from_number(42.0).is_int32());
        assert!(JSValue::from_number(0.0).is_int32());
    }

    #[test]
    fn is_int32_false() {
        assert!(!JSValue::from_number(3.14).is_int32());
        assert!(!JSValue::from_number(f64::NAN).is_int32());
    }

    #[test]
    fn from_inner_roundtrip() {
        let inner = JsValue::Number(99.0);
        let js_val = JSValue::from_inner(inner);
        assert!(js_val.is_number());
        assert_eq!(js_val.as_number(), Some(99.0));
        let back = js_val.into_inner();
        // JsValue doesn't impl PartialEq, so verify via pattern match.
        assert!(matches!(back, JsValue::Number(n) if n == 99.0));
    }
}
