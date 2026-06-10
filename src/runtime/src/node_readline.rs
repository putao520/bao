// @trace REQ-ENG-007
use ::std::ptr::NonNull;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, ObjectValue};
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

        let promises_src = r#"
          (function(mod) {
            mod.promises = {
              createInterface: function(options) {
                return new Promise(function(resolve) {
                  resolve(mod.createInterface(options || {}));
                });
              }
            };
            return mod;
          })
        "#;

        {
            let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx.raw_cx(), c"node:readline".as_ptr(), 1) as *mut _) else { return; };
            let opts = _opts_guard.as_ptr();
                let mut src_text = mozjs::rust::transform_str_to_source_text(promises_src);
                let mut rval = UndefinedValue();
                let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts as *const _, &mut src_text, MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
                });
                if ok && rval.is_object() {
                    let fn_obj = rval.to_object();
                    let fn_val = ObjectValue(fn_obj);
                    let fn_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
                    let mod_val = ObjectValue(rl_mod.get());
                    let call_args = HandleValueArray { length_: 1, elements_: &mod_val as *const JSVal };
                    let global = CurrentGlobalOrNull(cx.raw_cx());
                    if !global.is_null() {
                        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                        let mut res = UndefinedValue();
                        JS_CallFunctionValue(cx.raw_cx(), global_h, fn_h, &call_args, MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData, ptr: &mut res,
                        });
                    }
                }
        }
    }

    cache_builtin(cx, "readline", rl_mod.get());
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

    let iface_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &iface.get() };

    let mut input_val = UndefinedValue();
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts };
        JS_GetProperty(cx, opts_h, c"input".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut input_val });
    }
    let input_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &input_val };
    JS_DefineProperty(cx, iface_h, c"input".as_ptr(), input_h, JSPROP_ENUMERATE as u32);

    let closed_val = mozjs::jsval::BooleanValue(false);
    let closed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closed_val };
    JS_DefineProperty(cx, iface_h, c"closed".as_ptr(), closed_h, JSPROP_ENUMERATE as u32);

    let paused_val = mozjs::jsval::BooleanValue(false);
    let paused_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &paused_val };
    JS_DefineProperty(cx, iface_h, c"paused".as_ptr(), paused_h, JSPROP_ENUMERATE as u32);

    for name in &["on", "close", "pause", "resume", "write", "prompt", "setPrompt", "question"] {
        let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
        JS_DefineFunction(cx, iface_h, c_name.as_ptr(), Some(rl_noop), 0, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(ObjectValue(iface.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
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
