//! SM-backed string conversion traits.

use ::std::borrow::Cow;
use ::std::ffi::CString;

use mozjs::jsapi::*;
use mozjs::jsval::StringValue;

use crate::js_value::JSValue;

// ─── StringJsc trait ────────────────────────────────────────────────────────

/// Trait for converting Rust strings to JS strings (JSC StringJsc compat).
pub trait StringJsc: Sized {
    /// Create a JSString from this Rust string.
    fn to_js_string(&self, cx: *mut JSContext) -> Option<*mut JSString>;
}

impl StringJsc for &str {
    fn to_js_string(&self, cx: *mut JSContext) -> Option<*mut JSString> {
        let c_str = CString::new(*self).ok()?;
        unsafe {
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { None } else { Some(js_str) }
        }
    }
}

impl StringJsc for String {
    fn to_js_string(&self, cx: *mut JSContext) -> Option<*mut JSString> {
        self.as_str().to_js_string(cx)
    }
}

impl StringJsc for Cow<'_, str> {
    fn to_js_string(&self, cx: *mut JSContext) -> Option<*mut JSString> {
        self.as_ref().to_js_string(cx)
    }
}

// ─── ZigStringJsc trait ─────────────────────────────────────────────────────

/// Trait for Zig-style string conversion (JSC ZigStringJsc compat).
/// Returns a JSValue directly.
pub trait ZigStringJsc: Sized {
    /// Convert to a JSValue (string or undefined on failure).
    fn to_js_value_str(&self, cx: *mut JSContext) -> JSValue;
}

impl ZigStringJsc for &str {
    fn to_js_value_str(&self, cx: *mut JSContext) -> JSValue {
        self.to_js_string(cx)
            .map(|s| unsafe { JSValue::from_raw(cx, StringValue(&*s)) })
            .unwrap_or(JSValue::UNDEFINED)
    }
}

impl ZigStringJsc for String {
    fn to_js_value_str(&self, cx: *mut JSContext) -> JSValue {
        self.as_str().to_js_value_str(cx)
    }
}

impl ZigStringJsc for Cow<'_, str> {
    fn to_js_value_str(&self, cx: *mut JSContext) -> JSValue {
        self.as_ref().to_js_value_str(cx)
    }
}

impl ZigStringJsc for &[u8] {
    fn to_js_value_str(&self, cx: *mut JSContext) -> JSValue {
        match ::std::str::from_utf8(*self) {
            Ok(s) => s.to_js_value_str(cx),
            Err(_) => JSValue::UNDEFINED,
        }
    }
}

/// Helper function: convert a Rust string to a JSValue containing a JSString.
pub fn string_to_js_value(s: &str, cx: *mut JSContext) -> JSValue {
    s.to_js_value_str(cx)
}
