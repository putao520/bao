// @trace REQ-ENG-007
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, ObjectValue, Int32Value};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let timers_mod = unsafe { w2::JS_NewPlainObject(cx) });
    if timers_mod.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, timers_mod.handle(), c"setTimeout".as_ptr(), Some(timers_set_timeout), 2, 0);
        w2::JS_DefineFunction(cx, timers_mod.handle(), c"clearTimeout".as_ptr(), Some(timers_clear_timeout), 1, 0);
        w2::JS_DefineFunction(cx, timers_mod.handle(), c"setInterval".as_ptr(), Some(timers_set_interval), 2, 0);
        w2::JS_DefineFunction(cx, timers_mod.handle(), c"clearInterval".as_ptr(), Some(timers_clear_interval), 1, 0);
        w2::JS_DefineFunction(cx, timers_mod.handle(), c"setImmediate".as_ptr(), Some(timers_set_immediate), 1, 0);
        w2::JS_DefineFunction(cx, timers_mod.handle(), c"clearImmediate".as_ptr(), Some(timers_clear_immediate), 1, 0);

        rooted!(&in(cx) let promises_obj = w2::JS_NewPlainObject(cx));
        if !promises_obj.get().is_null() {
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"setTimeout".as_ptr(), Some(timers_promises_set_timeout), 1, 0);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"setImmediate".as_ptr(), Some(timers_promises_set_immediate), 0, 0);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"setInterval".as_ptr(), Some(timers_promises_set_interval), 1, 0);

            rooted!(&in(cx) let scheduler_obj = w2::JS_NewPlainObject(cx));
            if !scheduler_obj.get().is_null() {
                w2::JS_DefineFunction(cx, scheduler_obj.handle(), c"wait".as_ptr(), Some(timers_promises_set_timeout), 1, 0);
                w2::JS_DefineFunction(cx, scheduler_obj.handle(), c"yield".as_ptr(), Some(timers_promises_set_immediate), 0, 0);
                let sched_val = ObjectValue(scheduler_obj.get());
                let sched_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &sched_val };
                JS_DefineProperty(cx.raw_cx(), promises_obj.handle().into(), c"scheduler".as_ptr(), sched_h, JSPROP_ENUMERATE as u32);
            }

            let prom_val = ObjectValue(promises_obj.get());
            let prom_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &prom_val };
            JS_DefineProperty(cx.raw_cx(), timers_mod.handle().into(), c"promises".as_ptr(), prom_h, JSPROP_ENUMERATE as u32);

            cache_builtin(cx, "timers/promises", promises_obj.get());
        }
    }

    cache_builtin(cx, "timers", timers_mod.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_set_timeout(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let callback = (*args.get(0).ptr).to_object();
    let delay = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32().max(0) as u64 } else if v.is_double() { v.to_double().max(0.0) as u64 } else { 0 }
    } else { 0 };

    let id = crate::timers::schedule_raw(callback, delay, false, &[]);
    args.rval().set(Int32Value(id as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_clear_timeout(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            crate::timers::cancel_raw(v.to_int32() as u32);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_set_interval(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let callback = (*args.get(0).ptr).to_object();
    let delay = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32().max(1) as u64 } else if v.is_double() { v.to_double().max(1.0) as u64 } else { 1 }
    } else { 1 };

    let id = crate::timers::schedule_raw(callback, delay, true, &[]);
    args.rval().set(Int32Value(id as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_clear_interval(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    timers_clear_timeout(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_set_immediate(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let callback = (*args.get(0).ptr).to_object();
    let id = crate::timers::schedule_raw(callback, 0, false, &[]);
    args.rval().set(Int32Value(id as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_clear_immediate(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    timers_clear_timeout(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_promises_set_timeout(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let delay = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32().max(0) as u64 } else if v.is_double() { v.to_double().max(0.0) as u64 } else { 0 }
    } else { 0 };

    let resolve_src = format!(
        "new Promise(function(resolve) {{ setTimeout(resolve, {}) }})",
        delay
    );
    let mut rval = UndefinedValue();
    let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx, c"timers_promises".as_ptr(), 1) as *mut _) else {
        args.rval().set(rval);
        return true;
    };
    let opts = _opts_guard.as_ptr();
    let mut src = mozjs::rust::transform_str_to_source_text(&resolve_src);
    JS::Evaluate2(cx, opts as *const _, &mut src, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
    args.rval().set(rval);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_promises_set_immediate(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut rval = UndefinedValue();
    let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx, c"timers_promises".as_ptr(), 1) as *mut _) else {
        args.rval().set(rval);
        return true;
    };
    let opts = _opts_guard.as_ptr();
    let mut src = mozjs::rust::transform_str_to_source_text("new Promise(function(resolve) { setImmediate(resolve) })");
    JS::Evaluate2(cx, opts as *const _, &mut src, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
    args.rval().set(rval);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_promises_set_interval(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let delay = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32().max(1) as u64 } else if v.is_double() { v.to_double().max(1.0) as u64 } else { 1 }
    } else { 1 };

    let resolve_src = format!(
        "new Promise(function(resolve) {{ setInterval(resolve, {}) }})",
        delay
    );
    let mut rval = UndefinedValue();
    let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx, c"timers_promises".as_ptr(), 1) as *mut _) else {
        args.rval().set(rval);
        return true;
    };
    let opts = _opts_guard.as_ptr();
    let mut src = mozjs::rust::transform_str_to_source_text(&resolve_src);
    JS::Evaluate2(cx, opts as *const _, &mut src, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
    args.rval().set(rval);
    true
}
