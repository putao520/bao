// @trace REQ-ENG-007
use ::std::ffi::CString;
use ::std::ptr::NonNull;
use ::std::time::Instant;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, DoubleValue, ObjectValue, Int32Value};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

thread_local! {
    static PERFORMANCE_ORIGIN: Instant = Instant::now();
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let perf_mod = unsafe { w2::JS_NewPlainObject(cx) });
    if perf_mod.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, perf_mod.handle(), c"now".as_ptr(), Some(perf_now), 0, 0);
        w2::JS_DefineFunction(cx, perf_mod.handle(), c"mark".as_ptr(), Some(perf_mark), 1, 0);
        w2::JS_DefineFunction(cx, perf_mod.handle(), c"measure".as_ptr(), Some(perf_measure), 2, 0);

        rooted!(&in(cx) let performance_obj = w2::JS_NewPlainObject(cx));
        if !performance_obj.get().is_null() {
            w2::JS_DefineFunction(cx, performance_obj.handle(), c"now".as_ptr(), Some(perf_now), 0, 0);
            w2::JS_DefineFunction(cx, performance_obj.handle(), c"mark".as_ptr(), Some(perf_mark), 1, 0);
            w2::JS_DefineFunction(cx, performance_obj.handle(), c"measure".as_ptr(), Some(perf_measure), 2, 0);
            let perf_val = ObjectValue(performance_obj.get());
            let perf_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &perf_val };
            JS_DefineProperty(cx.raw_cx(), perf_mod.handle().into(), c"performance".as_ptr(), perf_h, JSPROP_ENUMERATE as u32);
        }

        let _ = CString::new(r#"
          (function(mod) {
            mod.nodeTiming = { name: 'node', startTime: 0 };
            mod.eventLoopUtilization = function() { return { idle: 0, active: 0, utilization: 0 }; };
            mod.timerify = function(fn) { return fn; };
            return mod;
          })
        "#).unwrap_or_default();
    }

    cache_builtin(cx, "perf_hooks", perf_mod.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn perf_now(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let ms = PERFORMANCE_ORIGIN.with(|origin| {
        origin.elapsed().as_secs_f64() * 1000.0
    });
    args.rval().set(DoubleValue(ms));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn perf_mark(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    let ms = PERFORMANCE_ORIGIN.with(|origin| {
        origin.elapsed().as_secs_f64() * 1000.0
    });
    let name_val = if _argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    if !obj.get().is_null() {
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj.get() };
        let ms_v = DoubleValue(ms);
        let ms_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ms_v };
        JS_DefineProperty(cx, obj_h, c"startTime".as_ptr(), ms_h, JSPROP_ENUMERATE as u32);
        if !name_val.is_undefined() {
            JS_DefineProperty(cx, obj_h, c"name".as_ptr(), Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &name_val }, JSPROP_ENUMERATE as u32);
        }
        let et_v = Int32Value(0);
        let et_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &et_v };
        JS_DefineProperty(cx, obj_h, c"entryType".as_ptr(), et_h, JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn perf_measure(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    let ms = PERFORMANCE_ORIGIN.with(|origin| {
        origin.elapsed().as_secs_f64() * 1000.0
    });
    if !obj.get().is_null() {
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj.get() };
        let ms_v = DoubleValue(ms);
        let ms_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ms_v };
        JS_DefineProperty(cx, obj_h, c"startTime".as_ptr(), ms_h, JSPROP_ENUMERATE as u32);
        let dur_v = DoubleValue(0.0);
        let dur_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &dur_v };
        JS_DefineProperty(cx, obj_h, c"duration".as_ptr(), dur_h, JSPROP_ENUMERATE as u32);
        let et_v = Int32Value(1);
        let et_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &et_v };
        JS_DefineProperty(cx, obj_h, c"entryType".as_ptr(), et_h, JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}
