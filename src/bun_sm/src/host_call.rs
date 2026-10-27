// @trace REQ-ENG-001
//! Host call utilities — cross-module JS function invocation.
//!
//! Wraps `JS_CallFunctionValue` for host→JS and JS→host value passing.
//! The identity-pass functions (to_js_host_call/from_js_host_call) remain
//! for simple value passthrough; the full call variants use SM API.

use crate::js_error::JsResult;
use crate::js_value::JSValue;
use mozjs::rooted;

pub fn to_js_host_call(value: &JSValue) -> JSValue {
    value.clone()
}

pub fn from_js_host_call(value: &JSValue) -> JsResult<JSValue> {
    Ok(value.clone())
}

pub fn to_js_host_fn_result(result: JsResult<JSValue>) -> JSValue {
    result.unwrap_or(JSValue::UNDEFINED)
}

/// Call a named method on a JS object, passing a single JSValue argument.
///
/// Returns the method's return value, or UNDEFINED if the call fails.
pub unsafe fn call_method_on_object(
    cx: *mut mozjs::jsapi::JSContext,
    obj: *mut mozjs::jsapi::JSObject,
    method_name: &str,
    args: &[mozjs::jsapi::Value],
) -> mozjs::jsapi::Value {
    if cx.is_null() || obj.is_null() {
        return mozjs::jsval::UndefinedValue();
    }

    // BCE-012: root obj/global/method_val — JS_GetProperty and JS_CallFunctionValue can trigger GC
    let wrapped_cx =
        unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx)) };

    let c_method = ::std::ffi::CString::new(method_name).unwrap_or_default();
    let mut method_val = mozjs::jsval::UndefinedValue();
    rooted!(&in(wrapped_cx) let obj_root = obj);
    unsafe {
        mozjs::jsapi::JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c_method.as_ptr(),
            mozjs::jsapi::MutableHandle::<mozjs::jsapi::Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut method_val,
            },
        );
    }

    if !method_val.is_object() {
        return mozjs::jsval::UndefinedValue();
    }

    let global = unsafe { mozjs::jsapi::CurrentGlobalOrNull(cx) };
    if global.is_null() {
        return mozjs::jsval::UndefinedValue();
    }

    rooted!(&in(wrapped_cx) let cb_val_root = method_val);
    rooted!(&in(wrapped_cx) let global_root = global);

    // (Bao, BCE) rooted arg carrier (E45 unrooted-args class, 2026-10-26
    // sweep): the old bare `HandleValueArray { elements_: args.as_ptr() }`
    // pointed at raw JSVal copies invisible to the tracer — JS_CallFunctionValue
    // below is a GC point, and a moving GC updated the caller's slots but not
    // these copies, feeding stale from-space pointers into the call. The
    // rooted Vec keeps the slots tracer-visible for the whole call window
    // (node_events.rs:969 verified template, Vec form; an empty Vec yields
    // length_=0 + dangling ptr, semantically identical to empty()).
    rooted!(&in(wrapped_cx) let args_rooted = args.to_vec());
    let call_args = mozjs::jsapi::JS::HandleValueArray::from(&args_rooted);

    let mut rval = mozjs::jsval::UndefinedValue();
    let rval_h = mozjs::jsapi::MutableHandle::<mozjs::jsapi::Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    unsafe {
        mozjs::jsapi::JS_CallFunctionValue(
            cx,
            global_root.handle().into(),
            cb_val_root.handle().into(),
            &call_args,
            rval_h,
        );
    }
    unsafe {
        mozjs::jsapi::JS_ClearPendingException(cx);
    }
    rval
}
