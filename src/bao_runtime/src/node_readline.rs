// @trace REQ-ENG-007
use bun_core::ZBox;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let rl_mod = unsafe { w2::JS_NewPlainObject(cx) });
    if rl_mod.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, rl_mod.handle(), c"createInterface".as_ptr(), Some(rl_create_interface), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, rl_mod.handle(), c"clearLine".as_ptr(), Some(rl_clear_line), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, rl_mod.handle(), c"clearScreenDown".as_ptr(), Some(rl_clear_screen), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, rl_mod.handle(), c"cursorTo".as_ptr(), Some(rl_cursor_to), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, rl_mod.handle(), c"moveCursor".as_ptr(), Some(rl_move_cursor), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, rl_mod.handle(), c"emitKeypressEvents".as_ptr(), Some(rl_emit_keypress), 1, JSPROP_ENUMERATE as u32);

        // Create the readline.promises namespace with a Promise-based
        // createInterface and an Interface class that supports .question()
        // returning a Promise (matching Bun's readline.promises shape).
        rooted!(&in(cx) let promises_obj = w2::JS_NewPlainObject(cx));
        if !promises_obj.get().is_null() {
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"createInterface".as_ptr(), Some(rl_promises_create_interface), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"Interface".as_ptr(), Some(rl_promises_interface_ctor), 1, JSPROP_ENUMERATE as u32);

            rooted!(&in(cx) let prom_val = ObjectValue(promises_obj.get()));
            JS_DefineProperty(cx.raw_cx(), rl_mod.handle().into(), c"promises".as_ptr(), prom_val.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    cache_builtin(cx, "readline", rl_mod.get());

    // Cache the promises sub-object as a standalone module so
    // require("readline/promises") works via node_subpath_aliases.
    let rl_cached = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, "builtin:readline");
    if let Some(rl_obj) = rl_cached {
        if !rl_obj.is_null() {
            unsafe {
                let raw_cx = cx.raw_cx();
                rooted!(&in(cx) let rl_root = rl_obj);
                let mut prom_val = UndefinedValue();
                JS_GetProperty(
                    raw_cx,
                    rl_root.handle().into(),
                    c"promises".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut prom_val,
                    },
                );
                if prom_val.is_object() {
                    cache_builtin(cx, "readline/promises", prom_val.to_object());
                }
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_create_interface(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let iface = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if iface.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut input_val = UndefinedValue();
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        rooted!(&in(wrapped_cx) let opts_root = opts);
        JS_GetProperty(cx, opts_root.handle().into(), c"input".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut input_val });
    }
    rooted!(&in(wrapped_cx) let input_val_root = input_val);
    JS_DefineProperty(cx, iface.handle().into(), c"input".as_ptr(), input_val_root.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(wrapped_cx) let closed_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, iface.handle().into(), c"closed".as_ptr(), closed_val.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(wrapped_cx) let paused_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, iface.handle().into(), c"paused".as_ptr(), paused_val.handle().into(), JSPROP_ENUMERATE as u32);

    // on — delegate to EventEmitter
    JS_DefineFunction(cx, iface.handle().into(), c"on".as_ptr(), Some(crate::node_events::ee_on), 2, JSPROP_ENUMERATE as u32);
    // close — mark as closed
    JS_DefineFunction(cx, iface.handle().into(), c"close".as_ptr(), Some(rl_close), 0, JSPROP_ENUMERATE as u32);
    // pause/resume — toggle paused flag
    JS_DefineFunction(cx, iface.handle().into(), c"pause".as_ptr(), Some(rl_pause), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, iface.handle().into(), c"resume".as_ptr(), Some(rl_resume), 0, JSPROP_ENUMERATE as u32);
    // write/prompt/setPrompt/question — return this for chaining
    for name in &["write", "prompt", "setPrompt", "question"] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        JS_DefineFunction(cx, iface.handle().into(), c_name.as_ptr(), Some(rl_chain), 0, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(ObjectValue(iface.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = this.to_object());
    rooted!(&in(wrapped_cx) let closed_v = BooleanValue(true));
    JS_DefineProperty(cx, this_obj.handle().into(), c"closed".as_ptr(), closed_v.handle().into(), JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_pause(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = this.to_object());
    rooted!(&in(wrapped_cx) let paused_v = BooleanValue(true));
    JS_DefineProperty(cx, this_obj.handle().into(), c"paused".as_ptr(), paused_v.handle().into(), JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_resume(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = this.to_object());
    rooted!(&in(wrapped_cx) let paused_v = BooleanValue(false));
    JS_DefineProperty(cx, this_obj.handle().into(), c"paused".as_ptr(), paused_v.handle().into(), JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_chain(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if this.is_object() {
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(_cx));
        rooted!(&in(wrapped_cx) let this_obj = this.to_object());
        args.rval().set(ObjectValue(this_obj.get()));
    } else { args.rval().set(UndefinedValue()); }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_clear_line(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_clear_screen(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_cursor_to(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_move_cursor(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_emit_keypress(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

/// readline.promises.createInterface — returns a Promise that resolves
/// to an Interface instance (wrapping the sync createInterface).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_promises_create_interface(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    // Create an Interface object the same way rl_create_interface does
    rooted!(&in(wrapped_cx) let iface = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if iface.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut input_val = UndefinedValue();
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        rooted!(&in(wrapped_cx) let opts_root = opts);
        JS_GetProperty(cx, opts_root.handle().into(), c"input".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut input_val });
    }
    rooted!(&in(wrapped_cx) let input_val_root = input_val);
    JS_DefineProperty(cx, iface.handle().into(), c"input".as_ptr(), input_val_root.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(wrapped_cx) let closed_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, iface.handle().into(), c"closed".as_ptr(), closed_val.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(wrapped_cx) let paused_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, iface.handle().into(), c"paused".as_ptr(), paused_val.handle().into(), JSPROP_ENUMERATE as u32);

    JS_DefineFunction(cx, iface.handle().into(), c"on".as_ptr(), Some(crate::node_events::ee_on), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, iface.handle().into(), c"close".as_ptr(), Some(rl_close), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, iface.handle().into(), c"pause".as_ptr(), Some(rl_pause), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, iface.handle().into(), c"resume".as_ptr(), Some(rl_resume), 0, JSPROP_ENUMERATE as u32);

    // question() returns a Promise (readline/promises spec)
    JS_DefineFunction(cx, iface.handle().into(), c"question".as_ptr(), Some(rl_promises_question), 1, JSPROP_ENUMERATE as u32);

    for name in &["write", "prompt", "setPrompt"] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        JS_DefineFunction(cx, iface.handle().into(), c_name.as_ptr(), Some(rl_chain), 0, JSPROP_ENUMERATE as u32);
    }

    // Wrap in a resolved Promise
    let iface_val = ObjectValue(iface.get());
    let eval_src = "(function(val) { return Promise.resolve(val); })";
    let mut rval = UndefinedValue();
    let opts = mozjs::glue::NewCompileOptions(cx, c"<rl_promises>".as_ptr(), 1);
    if !opts.is_null() {
        let mut src = mozjs::rust::transform_str_to_source_text(eval_src);
        let ok = mozjs_sys::jsapi::JS::Evaluate2(
            cx,
            opts,
            &mut src,
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval },
        );
        libc::free(opts as *mut _);
        if ok && rval.is_object() {
            let fn_obj = rval.to_object();
            rooted!(&in(wrapped_cx) let fn_root = fn_obj);
            let iface_val_rooted = iface_val;
            rooted!(&in(wrapped_cx) let arg_val = iface_val_rooted);
            let elems = [arg_val.get()];
            let call_args = HandleValueArray {
                length_: 1,
                elements_: elems.as_ptr(),
            };
            let mut call_rval = UndefinedValue();
            rooted!(&in(wrapped_cx) let fn_val = ObjectValue(fn_root.get()));
            JS_CallFunctionValue(
                cx,
                fn_root.handle().into(),
                fn_val.handle().into(),
                &call_args,
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut call_rval },
            );
            args.rval().set(call_rval);
            return true;
        }
    }

    // Fallback: just return the interface object directly
    args.rval().set(iface_val);
    true
}

/// readline.promises.Interface — constructor for the promises Interface class.
/// Same as rl_promises_create_interface but callable as `new Interface(options)`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_promises_interface_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // Delegate to rl_promises_create_interface which creates an Interface
    // with question() returning Promise.
    rl_promises_create_interface(cx, argc, vp)
}

/// readline/promises Interface .question() — returns a Promise that resolves
/// with the answer string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_promises_question(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // Resolve with an empty string — we don't have a real TTY to read from.
    // This matches the stub nature of the readline module.
    let src = "Promise.resolve('')";
    let mut rval = UndefinedValue();
    let opts = mozjs::glue::NewCompileOptions(cx, c"<rl_question>".as_ptr(), 1);
    if !opts.is_null() {
        let mut s = mozjs::rust::transform_str_to_source_text(src);
        let ok = mozjs_sys::jsapi::JS::Evaluate2(
            cx,
            opts,
            &mut s,
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval },
        );
        libc::free(opts as *mut _);
        if ok {
            args.rval().set(rval);
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}
