//! JS object types and property operation helpers.
//!
//! Provides wrappers for common JS object property operations.

use ::std::ffi::CString;
use ::std::marker::PhantomData;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;

use crate::js_value::JSValue;

/// Opaque JS object type (JSC API compatibility).
///
/// In JSC, `JSObject` is a concrete type. In SM, `*mut mozjs::jsapi::JSObject`
/// is the actual type used. This ZST exists purely for JSC API compatibility
/// at the type-name level; actual object references use raw pointers to
/// `mozjs::jsapi::JSObject`.
pub struct JSObject { _private: () }

/// Opaque JS function type (JSC API compatibility).
pub struct JSFunction { _private: () }

/// Opaque JS string type (JSC API compatibility).
pub struct JSString { _private: () }

/// Opaque JS array iterator type (JSC API compatibility).
pub struct JSArrayIterator { _private: () }

/// Opaque JS array type (JSC API compatibility).
pub struct JSArray { _private: () }

/// Opaque JS BigInt type (JSC API compatibility).
pub struct JSBigInt { _private: () }

// ─── Property operation helpers ────────────────────────────────────────────

/// Get a property from a JS object by name.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn get_property(cx: *mut JSContext, obj: *mut mozjs::jsapi::JSObject, name: &str) -> JSValue {
    let c_name = CString::new(name).unwrap_or_default();
    // BCE-20260619-012: root obj before passing as Handle to JS API.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: PhantomData,
            ptr: &mut val,
        },
    );
    JSValue::from_raw(cx, val)
}

/// Set a property on a JS object by name.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn set_property(cx: *mut JSContext, obj: *mut mozjs::jsapi::JSObject, name: &str, value: JSVal) {
    let c_name = CString::new(name).unwrap_or_default();
    // BCE-20260619-012: root obj and value (may be Object/String JSVal) before passing as Handle.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    rooted!(&in(cx_ref) let value_root = value);
    JS_SetProperty(cx, obj_root.handle().into(), c_name.as_ptr(), value_root.handle().into());
}

/// Define a property on a JS object with specific attributes.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn define_property(
    cx: *mut JSContext,
    obj: *mut mozjs::jsapi::JSObject,
    name: &str,
    value: JSVal,
    attrs: u32,
) -> bool {
    let c_name = CString::new(name).unwrap_or_default();
    // BCE-20260619-012: root obj and value (may be Object/String JSVal) before passing as Handle.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    rooted!(&in(cx_ref) let value_root = value);
    JS_DefineProperty(cx, obj_root.handle().into(), c_name.as_ptr(), value_root.handle().into(), attrs)
}

/// Check if a JS object has a property.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn has_property(cx: *mut JSContext, obj: *mut mozjs::jsapi::JSObject, name: &str) -> bool {
    let c_name = CString::new(name).unwrap_or_default();
    // BCE-20260619-012: root obj before passing as Handle to JS API.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut found = false;
    JS_HasProperty(cx, obj_root.handle().into(), c_name.as_ptr(), &mut found);
    found
}

/// Delete a property from a JS object.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn delete_property(cx: *mut JSContext, obj: *mut mozjs::jsapi::JSObject, name: &str) -> bool {
    let c_name = CString::new(name).unwrap_or_default();
    // BCE-20260619-012: root obj before passing as Handle to JS API.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    JS_DeleteProperty1(cx, obj_root.handle().into(), c_name.as_ptr())
}
