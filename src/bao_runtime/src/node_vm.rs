use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let vm_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if vm_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"runInThisContext".as_ptr(), Some(vm_run_in_this_context), 2, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"runInNewContext".as_ptr(), Some(vm_run_in_new_context), 3, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"createContext".as_ptr(), Some(vm_create_context), 1, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"isContext".as_ptr(), Some(vm_is_context), 1, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"compileFunction".as_ptr(), Some(vm_compile_function), 2, 0);

        // Script constructor
        let script_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(vm_script_ctor),
            2,
            JSFUN_CONSTRUCTOR as u32,
            c"Script".as_ptr(),
        );
        if !script_fn.is_null() {
            let script_obj = JS_GetFunctionObject(script_fn);
            rooted!(&in(cx) let sv = ObjectValue(script_obj));
            JS_DefineProperty(
                cx.raw_cx(),
                vm_obj.handle().into(),
                c"Script".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // Script.prototype.runInThisContext / runInNewContext
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(cx, proto.handle(), c"runInThisContext".as_ptr(), Some(vm_script_run_in_this_context), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"runInNewContext".as_ptr(), Some(vm_script_run_in_new_context), 2, 0);

                let proto_val = ObjectValue(proto.get());
                rooted!(&in(cx) let pv = proto_val);
                rooted!(&in(cx) let script_h = script_obj);
                JS_SetPrototype(cx.raw_cx(), script_h.handle().into(), proto.handle().into());
            }
        }
    }

    cache_builtin(cx, "vm", vm_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, b"Script requires a code string argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);

    // Get filename from options
    let filename = if argc > 1 && (*args.get(1).ptr).is_object() {
        let opts = (*args.get(1).ptr).to_object();
        let mut fn_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts },
            c"filename".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
        );
        if fn_val.is_string() {
            crate::js_to_rust_string(cx, fn_val)
        } else {
            "vm.js".to_string()
        }
    } else if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "vm.js".to_string()
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // Methods on the Script instance
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"runInThisContext".as_ptr(), Some(vm_script_run_in_this_context), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"runInNewContext".as_ptr(), Some(vm_script_run_in_new_context), 2, 0);

    // Store code and filename as hidden properties
    let code_str = JS_NewStringCopyN(cx, code.as_ptr() as *const ::std::os::raw::c_char, code.len());
    if !code_str.is_null() {
        rooted!(&in(cx_ref) let cv = mozjs::jsval::StringValue(&*code_str));
        JS_DefineProperty(cx, obj.handle().into(), c"__code".as_ptr(), cv.handle().into(), 0);
    }
    let fn_str = JS_NewStringCopyN(cx, filename.as_ptr() as *const ::std::os::raw::c_char, filename.len());
    if !fn_str.is_null() {
        rooted!(&in(cx_ref) let fv = mozjs::jsval::StringValue(&*fn_str));
        JS_DefineProperty(cx, obj.handle().into(), c"__filename".as_ptr(), fv.handle().into(), 0);
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

fn eval_code(cx: *mut JSContext, code: &str, filename: &str) -> Option<*mut JSObject> {
    unsafe {
        let c_filename = CString::new(filename.as_bytes()).unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr() as *const _, 1);
        if opts.is_null() {
            return None;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(code);
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h);
        libc::free(opts as *mut _);

        if ok {
            // Return the global object (in vm, the result is the last expression value)
            // Wrap the result in an object if it isn't one
            if rval.is_object() {
                Some(rval.to_object())
            } else {
                let global = JS::CurrentGlobalOrNull(cx);
                Some(global)
            }
        } else {
            None
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_run_in_this_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, b"runInThisContext requires a code string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let filename = if argc > 1 && (*args.get(1).ptr).is_object() {
        let opts = (*args.get(1).ptr).to_object();
        let mut fn_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts },
            c"filename".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
        );
        if fn_val.is_string() { crate::js_to_rust_string(cx, fn_val) } else { "vm.js".to_string() }
    } else if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "vm.js".to_string()
    };

    match eval_code(cx, &code, &filename) {
        Some(_) => {
            // Evaluation succeeded — the side effects (variable definitions, etc.) are
            // visible in the current scope since we evaluate in the same global.
            args.rval().set(UndefinedValue());
            true
        }
        None => false,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_run_in_new_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, b"runInNewContext requires a code string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let filename = "vm.js".to_string();

    // Wrap in IIFE to isolate scope — variables defined inside won't leak to outer scope
    let sandbox_code = format!("(function() {{ {} }})()", code);

    match eval_code(cx, &sandbox_code, &filename) {
        Some(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        None => false,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_create_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let ctx_obj = w2::JS_NewPlainObject(cx_ref));
    if ctx_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Mark as context
    rooted!(&in(cx_ref) let marker = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(cx, ctx_obj.handle().into(), c"__isVMContext".as_ptr(), marker.handle().into(), 0);

    // Copy properties from input object
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let input = (*args.get(0).ptr).to_object();
        let js_code = r#"
            (function(dst, src) {
                if (src && typeof src === 'object') {
                    var keys = Object.keys(src);
                    for (var i = 0; i < keys.length; i++) {
                        dst[keys[i]] = src[keys[i]];
                    }
                }
                return dst;
            })
        "#;
        let c_filename = CString::new("<vm>").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr() as *const _, 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(js_code);
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
            let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h);
            libc::free(opts as *mut _);
            if ok && rval.is_object() {
                let fn_obj = rval.to_object();
                let dst_val = ObjectValue(ctx_obj.get());
                let src_val = ObjectValue(input);
                let mut ret = UndefinedValue();
                let ret_h = MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ret };
                let elems = [dst_val, src_val];
                let args_arr = HandleValueArray { length_: 2, elements_: elems.as_ptr() };
                let null_obj = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut::<JSObject>() };
                let fn_val = mozjs::jsval::ObjectValue(fn_obj);
                let fn_h = Handle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
                JS_CallFunctionValue(cx, null_obj, fn_h, &args_arr, ret_h);
            }
        }
    }

    args.rval().set(ObjectValue(ctx_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_is_context(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let obj = (*args.get(0).ptr).to_object();
        let mut val = UndefinedValue();
        JS_GetProperty(
            _cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
            c"__isVMContext".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val },
        );
        args.rval().set(mozjs::jsval::BooleanValue(val.is_boolean() && val.to_boolean()));
    } else {
        args.rval().set(mozjs::jsval::BooleanValue(false));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_compile_function(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, b"compileFunction requires a code string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let fn_name = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "anonymous".to_string()
    };

    // Wrap in a function expression
    let wrapped = format!("(function {}() {{ {} }})", fn_name, code);

    let c_filename = CString::new("vm.js").unwrap_or_default();
    let opts = mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr() as *const _, 1);
    if opts.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    let mut src = mozjs::rust::transform_str_to_source_text(&wrapped);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h);
    libc::free(opts as *mut _);

    if ok && rval.is_object() {
        args.rval().set(rval);
        true
    } else {
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_run_in_this_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();

    // Read __code and __filename from this
    let mut code_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__code".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut code_val },
    );
    let code = crate::js_to_rust_string(cx, code_val);

    let mut fn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__filename".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
    );
    let filename = crate::js_to_rust_string(cx, fn_val);

    match eval_code(cx, &code, &filename) {
        Some(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        None => false,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_run_in_new_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();

    let mut code_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__code".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut code_val },
    );
    let code = crate::js_to_rust_string(cx, code_val);

    let mut fn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__filename".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
    );
    let filename = crate::js_to_rust_string(cx, fn_val);

    match eval_code(cx, &code, &filename) {
        Some(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        None => false,
    }
}
