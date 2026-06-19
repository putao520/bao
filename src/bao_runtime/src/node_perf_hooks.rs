// @trace REQ-ENG-007
use bun_core::ZBox;
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
        w2::JS_DefineFunction(cx, perf_mod.handle(), c"now".as_ptr(), Some(perf_now), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, perf_mod.handle(), c"mark".as_ptr(), Some(perf_mark), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, perf_mod.handle(), c"measure".as_ptr(), Some(perf_measure), 2, JSPROP_ENUMERATE as u32);

        rooted!(&in(cx) let performance_obj = w2::JS_NewPlainObject(cx));
        if !performance_obj.get().is_null() {
            w2::JS_DefineFunction(cx, performance_obj.handle(), c"now".as_ptr(), Some(perf_now), 0, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, performance_obj.handle(), c"mark".as_ptr(), Some(perf_mark), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, performance_obj.handle(), c"measure".as_ptr(), Some(perf_measure), 2, JSPROP_ENUMERATE as u32);
            rooted!(&in(cx) let perf_val = ObjectValue(performance_obj.get()));
            JS_DefineProperty(cx.raw_cx(), perf_mod.handle().into(), c"performance".as_ptr(), perf_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        let _ = ZBox::from_bytes(b"
          (function(mod) {
            mod.nodeTiming = { name: 'node', startTime: 0 };
            mod.eventLoopUtilization = function() { return { idle: 0, active: 0, utilization: 0 }; };
            mod.timerify = function(fn) { return fn; };
            return mod;
          })
        ");
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
        rooted!(&in(wrapped_cx) let ms_v = DoubleValue(ms));
        JS_DefineProperty(cx, obj.handle().into(), c"startTime".as_ptr(), ms_v.handle().into(), JSPROP_ENUMERATE as u32);
        if !name_val.is_undefined() {
            rooted!(&in(wrapped_cx) let nv = name_val);
            JS_DefineProperty(cx, obj.handle().into(), c"name".as_ptr(), nv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        rooted!(&in(wrapped_cx) let et_v = Int32Value(0));
        JS_DefineProperty(cx, obj.handle().into(), c"entryType".as_ptr(), et_v.handle().into(), JSPROP_ENUMERATE as u32);
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
        rooted!(&in(wrapped_cx) let ms_v = DoubleValue(ms));
        JS_DefineProperty(cx, obj.handle().into(), c"startTime".as_ptr(), ms_v.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(wrapped_cx) let dur_v = DoubleValue(0.0));
        JS_DefineProperty(cx, obj.handle().into(), c"duration".as_ptr(), dur_v.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(wrapped_cx) let et_v = Int32Value(1));
        JS_DefineProperty(cx, obj.handle().into(), c"entryType".as_ptr(), et_v.handle().into(), JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}
