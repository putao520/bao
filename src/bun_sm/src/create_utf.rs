//! UTF string creation.
//!
//! Creates JS string values from UTF-8 byte slices.

use ::std::ffi::CString;

use mozjs::jsapi::*;
use mozjs::jsval::StringValue;

use crate::global_object::JSGlobalObject;
use crate::js_value::JSValue;

/// Create a JS string from UTF-8 bytes.
///
/// # Safety
/// The global's JSContext must be valid.
pub fn create_utf(global: &JSGlobalObject, s: &[u8]) -> JSValue {
    let raw_cx = global.raw();
    match ::std::str::from_utf8(s) {
        Ok(valid_str) => {
            let c_str = CString::new(valid_str).unwrap_or_default();
            unsafe {
                let js_str = JS_NewStringCopyZ(raw_cx, c_str.as_ptr());
                if js_str.is_null() {
                    JSValue::UNDEFINED
                } else {
                    JSValue::from_raw(raw_cx, StringValue(&*js_str))
                }
            }
        }
        Err(_) => JSValue::UNDEFINED,
    }
}
