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

        let _ = ZBox::from_bytes(b"
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
        ");
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(_cx));
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
