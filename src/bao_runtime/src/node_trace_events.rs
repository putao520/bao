// @trace REQ-ENG-006 [api:node:trace_events]
//
// Node.js trace_events module stub. Bao does not implement Chrome trace event
// categories. createTracing() returns a disabled Tracing object;
// getEnabledCategories() returns empty string.

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use ::std::ptr::NonNull;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() { return; }

    unsafe {
        let raw_cx = cx.raw_cx();

        // createTracing(opts) — returns a Tracing object with enabled=false, categories=""
        let create_fn = JS_NewFunction(raw_cx, Some(trace_create_tracing), 1, 0, c"createTracing".as_ptr());
        if !create_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(create_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"createTracing".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // getEnabledCategories() — returns ""
        let get_fn = JS_NewFunction(raw_cx, Some(trace_get_enabled_categories), 0, 0, c"getEnabledCategories".as_ptr());
        if !get_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(get_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"getEnabledCategories".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
    }

    cache_builtin(cx, "trace_events", obj.get());
}

/// createTracing(opts) — returns { enabled: false, categories: "" }
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn trace_create_tracing(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let tracing_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if tracing_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // enabled = false
    rooted!(&in(cx_ref) let en = BooleanValue(false));
    let _ = JS_DefineProperty(cx, tracing_obj.handle().into(), c"enabled".as_ptr(), en.handle().into(), JSPROP_ENUMERATE as u32);

    // categories = ""
    let cat_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !cat_str.is_null() {
        rooted!(&in(cx_ref) let cat_val = mozjs::jsval::StringValue(&*cat_str));
        let _ = JS_DefineProperty(cx, tracing_obj.handle().into(), c"categories".as_ptr(), cat_val.handle().into(), JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(tracing_obj.get()));
    true
}

/// getEnabledCategories() — returns ""
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn trace_get_enabled_categories(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, _argc);
    let s = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !s.is_null() {
        args.rval().set(mozjs::jsval::StringValue(&*s));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}
