// @trace REQ-ENG-006 [api:node:inspector]
//
// Node.js `inspector` module — CDP bridge surface.
// Provides inspector.open(), close(), url(), waitForDebugger(), and console accessor.
// The inspector state is tracked via atomics; actual CDP integration is handled
// by bao_cdp when the browser/CDP server is active.

use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Whether the inspector is currently "open".
static INSPECTOR_OPEN: AtomicBool = AtomicBool::new(false);

/// The port the inspector is listening on (0 = not set).
static INSPECTOR_PORT: AtomicU16 = AtomicU16::new(0);

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let inspector_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if inspector_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            inspector_obj.handle(),
            c"open".as_ptr(),
            Some(inspector_open),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            inspector_obj.handle(),
            c"close".as_ptr(),
            Some(inspector_close),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            inspector_obj.handle(),
            c"url".as_ptr(),
            Some(inspector_url),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            inspector_obj.handle(),
            c"waitForDebugger".as_ptr(),
            Some(inspector_wait_for_debugger),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            inspector_obj.handle(),
            c"console".as_ptr(),
            Some(inspector_console),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    cache_builtin(cx, "inspector", inspector_obj.get());
}

// ── Native callbacks ──

/// inspector.open([port[, host[, wait]]])
/// Stores the port and marks the inspector as open.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn inspector_open(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let port = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_int32() {
            val.to_int32() as u16
        } else if val.is_double() {
            val.to_double() as u16
        } else {
            9229u16 // default inspector port
        }
    } else {
        9229u16
    };

    INSPECTOR_PORT.store(port, Ordering::SeqCst);
    INSPECTOR_OPEN.store(true, Ordering::SeqCst);

    args.rval().set(UndefinedValue());
    true
}

/// inspector.close()
/// Marks the inspector as closed.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn inspector_close(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    INSPECTOR_OPEN.store(false, Ordering::SeqCst);

    args.rval().set(UndefinedValue());
    true
}

/// inspector.url()
/// Returns the WebSocket URL if the inspector is open, otherwise undefined.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn inspector_url(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    if INSPECTOR_OPEN.load(Ordering::SeqCst) {
        let port = INSPECTOR_PORT.load(Ordering::SeqCst);
        let url_str = format!("ws://127.0.0.1:{}/ws", port);
        let c_url = ::std::ffi::CString::new(url_str).unwrap_or_default();
        let js_str = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
        } else {
            args.rval().set(UndefinedValue());
        }
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

/// inspector.waitForDebugger()
/// Returns a Promise that resolves immediately (no real debugger wait in Bao).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn inspector_wait_for_debugger(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    // Create a resolved Promise. We use JS::NewPromiseObject if available,
    // otherwise we create one via the global Promise constructor.
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Get the Promise constructor from the global
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);

    let mut promise_ctor_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"Promise".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut promise_ctor_val,
        },
    );

    if promise_ctor_val.is_object() {
        rooted!(&in(wrapped_cx) let ctor_obj = promise_ctor_val.to_object());

        // Create a resolve function: (resolve) => resolve()
        let resolve_fn = JS_NewFunction(
            cx,
            Some(inspector_resolve_fn),
            1,
            0,
            c"inspectorResolve".as_ptr(),
        );
        if !resolve_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(resolve_fn);
            rooted!(&in(wrapped_cx) let fn_val = ObjectValue(fn_obj));

            let elems = [fn_val.get()];
            let call_args = HandleValueArray {
                length_: elems.len(),
                elements_: elems.as_ptr(),
            };

            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                ctor_obj.handle().into(),
                fn_val.handle().into(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );

            args.rval().set(rval);
            return true;
        }
    }

    // Fallback: return undefined if Promise is not available
    args.rval().set(UndefinedValue());
    true
}

/// The executor function passed to new Promise() — immediately resolves.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn inspector_resolve_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let resolve = *args.get(0).ptr;
        if resolve.is_object() {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let resolve_obj = resolve.to_object());
            rooted!(&in(wrapped_cx) let resolve_fn_val = ObjectValue(resolve_obj.get()));

            let empty_args = HandleValueArray::empty();
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                resolve_obj.handle().into(),
                resolve_fn_val.handle().into(),
                &empty_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// inspector.console
/// Returns the builtin:console object from the require cache, or undefined.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn inspector_console(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    // Try to get the console object from gc_store
    if let Some(console_obj) = crate::gc_store::gc_store_get(cx, "builtin:console") {
        if !console_obj.is_null() {
            args.rval().set(ObjectValue(console_obj));
            return true;
        }
    }

    args.rval().set(UndefinedValue());
    true
}
