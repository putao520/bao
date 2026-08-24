// @trace REQ-ENG-007
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let timers_mod = unsafe { w2::JS_NewPlainObject(cx) });
    if timers_mod.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            timers_mod.handle(),
            c"setTimeout".as_ptr(),
            Some(timers_set_timeout),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            timers_mod.handle(),
            c"clearTimeout".as_ptr(),
            Some(timers_clear_timeout),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            timers_mod.handle(),
            c"setInterval".as_ptr(),
            Some(timers_set_interval),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            timers_mod.handle(),
            c"clearInterval".as_ptr(),
            Some(timers_clear_interval),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            timers_mod.handle(),
            c"setImmediate".as_ptr(),
            Some(timers_set_immediate),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            timers_mod.handle(),
            c"clearImmediate".as_ptr(),
            Some(timers_clear_immediate),
            1,
            0,
        );

        rooted!(&in(cx) let promises_obj = w2::JS_NewPlainObject(cx));
        if !promises_obj.get().is_null() {
            w2::JS_DefineFunction(
                cx,
                promises_obj.handle(),
                c"setTimeout".as_ptr(),
                Some(timers_promises_set_timeout),
                1,
                0,
            );
            w2::JS_DefineFunction(
                cx,
                promises_obj.handle(),
                c"setImmediate".as_ptr(),
                Some(timers_promises_set_immediate),
                0,
                0,
            );
            w2::JS_DefineFunction(
                cx,
                promises_obj.handle(),
                c"setInterval".as_ptr(),
                Some(timers_promises_set_interval),
                1,
                0,
            );

            rooted!(&in(cx) let scheduler_obj = w2::JS_NewPlainObject(cx));
            if !scheduler_obj.get().is_null() {
                w2::JS_DefineFunction(
                    cx,
                    scheduler_obj.handle(),
                    c"wait".as_ptr(),
                    Some(timers_promises_set_timeout),
                    1,
                    0,
                );
                w2::JS_DefineFunction(
                    cx,
                    scheduler_obj.handle(),
                    c"yield".as_ptr(),
                    Some(timers_promises_set_immediate),
                    0,
                    0,
                );
                rooted!(&in(cx) let sched_val = ObjectValue(scheduler_obj.get()));
                JS_DefineProperty(
                    cx.raw_cx(),
                    promises_obj.handle().into(),
                    c"scheduler".as_ptr(),
                    sched_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }

            rooted!(&in(cx) let prom_val = ObjectValue(promises_obj.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                timers_mod.handle().into(),
                c"promises".as_ptr(),
                prom_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            cache_builtin(cx, "timers/promises", promises_obj.get());

            stamp_promisify_customs(cx, promises_obj.get());
        }
    }

    cache_builtin(cx, "timers", timers_mod.get());
}

/// domain-check a1f2e22140 (own-idiom fix, wiring v3): value-stamp the
/// `nodejs.util.promisify.custom` symbol on THIS realm's global timer
/// functions so util.promisify(setTimeout) IS timers/promises.setTimeout —
/// identity, not a wrapper (Node wires the same custom in lib/timers.js).
///
/// Stamp points (idempotent value property — safe to run on every realm):
///   1. `install` above (install_node_apis phase, right after the promises
///      object is cached) — covers the realm that runs the node segment;
///   2. `timers::install_timer_globals` (install_web_apis phase, per-realm)
///      — pulls the cached promises singleton via `get_builtin`, covering
///      LATER realms whose web segment runs after an earlier realm already
///      populated the module cache (cache miss = first realm before the
///      node segment: skip silently, point 1 carries it).
/// The consumer chain: native setTimeout ← this stamp; the async_hooks
/// timer wrapper (which REPLACES the global functions at its install) ←
/// forwards the custom symbol through a live getter onto the wrapped
/// original (node_async_hooks `_wrapTimerConstructor`), so the stamp stays
/// visible through the replacement that previously sank it.
///
/// v3 hardening (vs the bare-identifier v2):
///   1. Stamp TARGET is explicit: the factory reads `g.setTimeout` BY NAME
///      off the global handed in as an argument — no bare-identifier lexical
///      lookup that could resolve against a different global or hit the
///      timers_mod-local same-name function (a different JSFunction object;
///      identity would never hold).
///   2. Symbol family: `Symbol.for(...)` constructed inside the factory JS —
///      the exact same registered-symbol family the util.promisify probe
///      reads (dns×7 green is this contract working end-to-end).
///   3. Value property (plain assignment), no getter.
/// Args are array-backed (install_module_global precedent) — a former
/// single-element `&rooted.get() as *const Value` pointed at a destroyed
/// rvalue temporary (UB: the factory could receive garbage for its
/// parameter and throw, silently un-stamping). Every failure path prints
/// one stderr line — a silent no-op is exactly how the earlier wirings died
/// undetected.
pub fn stamp_promisify_customs(cx: &mut mozjs::context::JSContext, promises_obj: *mut JSObject) {
    const STAMP_SRC: &str = r#"(function(g, p) {
  var custom = Symbol.for('nodejs.util.promisify.custom');
  g.setTimeout[custom] = p.setTimeout;
  g.setInterval[custom] = p.setInterval;
  g.setImmediate[custom] = p.setImmediate;
})"#;
    unsafe {
        let mut stamp_js = mozjs::rust::transform_str_to_source_text(STAMP_SRC);
        let mut factory_val = UndefinedValue();
        let factory_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut factory_val,
        };
        let opts =
            mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<timers_promisify_custom>".as_ptr(), 1);
        if opts.is_null() {
            eprintln!("[node_timers_module] promisify-custom wiring skipped: no compile options");
            return;
        }
        let evaluated = JS::Evaluate2(cx.raw_cx(), opts, &mut stamp_js, factory_h);
        libc::free(opts as *mut _);
        if !evaluated || !factory_val.is_object() {
            eprintln!("[node_timers_module] promisify-custom wiring factory evaluation failed");
            return;
        }
        let global = CurrentGlobalOrNull(cx.raw_cx());
        if global.is_null() {
            eprintln!("[node_timers_module] promisify-custom wiring skipped: null global");
            return;
        }
        rooted!(&in(cx) let global_root = global);
        rooted!(&in(cx) let factory_obj = factory_val.to_object());
        rooted!(&in(cx) let factory_call_val = ObjectValue(factory_obj.get()));
        // Array-backed args: the slice outlives the call.
        let elems = [ObjectValue(global_root.get()), ObjectValue(promises_obj)];
        let args = HandleValueArray {
            length_: 2,
            elements_: elems.as_ptr(),
        };
        let mut call_rval = UndefinedValue();
        let called = JS_CallFunctionValue(
            cx.raw_cx(),
            global_root.handle().into(),
            factory_call_val.handle().into(),
            &args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut call_rval,
            },
        );
        if !called {
            // Drain and surface the pending exception, then clear it —
            // install must not abort, but the failure must stay observable.
            let mut exn = UndefinedValue();
            JS_GetPendingException(
                cx.raw_cx(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut exn,
                },
            );
            JS_ClearPendingException(cx.raw_cx());
            // Surface the drained exception's message — "must stay
            // observable" means identifiable, not just counted (this log is
            // the perfect-correlation signal for the cdp_ws load-race
            // diagnosis, 2026-08-24).
            let exn_msg = if exn.is_object() {
                let mut msg_val = UndefinedValue();
                let wr = &mut *cx;
                rooted!(&in(wr) let exn_obj = exn.to_object());
                let got = JS_GetProperty(
                    cx.raw_cx(),
                    exn_obj.handle().into(),
                    c"message".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut msg_val,
                    },
                );
                if got && msg_val.is_string() {
                    crate::js_to_rust_string(cx.raw_cx(), msg_val)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            eprintln!(
                "[node_timers_module] promisify-custom wiring call failed: {}",
                if exn_msg.is_empty() { "<no message>" } else { &exn_msg }
            );
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_set_timeout(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let callback_root = (*args.get(0).ptr).to_object());
    let delay = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32().max(0) as u64
        } else if v.is_double() {
            v.to_double().max(0.0) as u64
        } else {
            0
        }
    } else {
        0
    };

    let id = crate::timers::schedule_raw(cx, callback_root.get(), delay, false, &[]);
    args.rval().set(Int32Value(id as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_clear_timeout(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
unsafe extern "C" fn timers_set_interval(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let callback_root = (*args.get(0).ptr).to_object());
    let delay = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32().max(1) as u64
        } else if v.is_double() {
            v.to_double().max(1.0) as u64
        } else {
            1
        }
    } else {
        1
    };

    let id = crate::timers::schedule_raw(cx, callback_root.get(), delay, true, &[]);
    args.rval().set(Int32Value(id as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_clear_interval(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    timers_clear_timeout(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_set_immediate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let callback_root = (*args.get(0).ptr).to_object());
    let id = crate::timers::schedule_raw(cx, callback_root.get(), 0, false, &[]);
    args.rval().set(Int32Value(id as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn timers_clear_immediate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
        if v.is_int32() {
            v.to_int32().max(0) as u64
        } else if v.is_double() {
            v.to_double().max(0.0) as u64
        } else {
            0
        }
    } else {
        0
    };

    let resolve_src = format!(
        "new Promise(function(resolve) {{ setTimeout(resolve, {}) }})",
        delay
    );
    let mut rval = UndefinedValue();
    let opts = mozjs::glue::NewCompileOptions(cx, c"timers_promises".as_ptr(), 1);
    if !opts.is_null() {
        let mut src = mozjs::rust::transform_str_to_source_text(&resolve_src);
        JS::Evaluate2(
            cx,
            opts,
            &mut src,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
        libc::free(opts as *mut _);
    }
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
    let opts = mozjs::glue::NewCompileOptions(cx, c"timers_promises".as_ptr(), 1);
    if !opts.is_null() {
        let mut src = mozjs::rust::transform_str_to_source_text(
            "new Promise(function(resolve) { setImmediate(resolve) })",
        );
        JS::Evaluate2(
            cx,
            opts,
            &mut src,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
        libc::free(opts as *mut _);
    }
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
        if v.is_int32() {
            v.to_int32().max(1) as u64
        } else if v.is_double() {
            v.to_double().max(1.0) as u64
        } else {
            1
        }
    } else {
        1
    };

    let resolve_src = format!(
        "new Promise(function(resolve) {{ setInterval(resolve, {}) }})",
        delay
    );
    let mut rval = UndefinedValue();
    let opts = mozjs::glue::NewCompileOptions(cx, c"timers_promises".as_ptr(), 1);
    if !opts.is_null() {
        let mut src = mozjs::rust::transform_str_to_source_text(&resolve_src);
        JS::Evaluate2(
            cx,
            opts,
            &mut src,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
        libc::free(opts as *mut _);
    }
    args.rval().set(rval);
    true
}
