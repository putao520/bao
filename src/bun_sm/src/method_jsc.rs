// @trace REQ-ENG-001
//! Method JSC — helper utilities for JS method registration.
//!
//! Actual JS method registration is done in bao_runtime.
//! This module provides the helper function for defining methods on JS objects.

use mozjs::jsapi::{JSContext, JSObject, JSNative, JS_DefineFunction, JSPROP_ENUMERATE};

pub struct MethodJsc;

impl MethodJsc {
    /// Registration is handled by bao_runtime modules.
    /// This is a placeholder for API compatibility.
    pub fn install(_cx: *mut JSContext, _global: *mut JSObject) {}
}

/// Register a single JS function on a JS object.
pub unsafe fn define_method(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: &str,
    native: JSNative,
    nargs: u32,
) {
    let c_name = ::std::ffi::CString::new(name).unwrap_or_default();
    let obj_h = mozjs::jsapi::Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };
    JS_DefineFunction(cx, obj_h, c_name.as_ptr(), native, nargs, JSPROP_ENUMERATE as u32);
}

/// Register multiple methods on a JS object.
pub unsafe fn define_methods(
    cx: *mut JSContext,
    obj: *mut JSObject,
    methods: &[(&str, JSNative, u32)],
) {
    for (name, native, nargs) in methods {
        define_method(cx, obj, name, *native, *nargs);
    }
}
