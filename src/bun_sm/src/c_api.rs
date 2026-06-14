//! C API compatibility layer.

use mozjs::jsapi::{JSContext, JSObject};
use mozjs::jsval::JSVal;

pub type JSNative = unsafe extern "C" fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool;

pub mod c {
    pub type JSNative = super::JSNative;
}
