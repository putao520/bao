// @trace REQ-ENG-005
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use mozjs_sys::jsapi::JS_DefineProperty3;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        // Module constructor
        let module_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(module_ctor),
            1,
            JSFUN_CONSTRUCTOR,
            c"Module".as_ptr(),
        );
        if !module_fn.is_null() {
            let module_ctor_obj = JS_GetFunctionObject(module_fn);
            rooted!(&in(cx) let mv = ObjectValue(module_ctor_obj));
            JS_DefineProperty(
                cx.raw_cx(),
                mod_obj.handle().into(),
                c"Module".as_ptr(),
                mv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // createRequire
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"createRequire".as_ptr(), Some(module_create_require), 1, 0);

        // _cache — shared module cache object
        rooted!(&in(cx) let cache_obj = w2::JS_NewPlainObject(cx));
        if !cache_obj.get().is_null() {
            JS_DefineProperty3(cx.raw_cx(), mod_obj.handle().into(), c"_cache".as_ptr(), cache_obj.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // _extensions
        rooted!(&in(cx) let ext_obj = w2::JS_NewPlainObject(cx));
        if !ext_obj.get().is_null() {
            JS_DefineProperty3(cx.raw_cx(), ext_obj.handle().into(), c".js".as_ptr(), ext_obj.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineProperty3(cx.raw_cx(), ext_obj.handle().into(), c".json".as_ptr(), ext_obj.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineProperty3(cx.raw_cx(), mod_obj.handle().into(), c"_extensions".as_ptr(), ext_obj.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // _resolveFilename
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"_resolveFilename".as_ptr(), Some(module_resolve_filename), 2, 0);

        // _nodeModulePaths
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"_nodeModulePaths".as_ptr(), Some(module_node_module_paths), 1, 0);

        // builtinModules array
        let builtins = [
            "assert", "buffer", "child_process", "crypto", "dns", "events",
            "fs", "http", "https", "module", "net", "os", "path", "querystring",
            "readline", "stream", "string_decoder", "tls", "tty", "url",
            "util", "vm", "zlib", "perf_hooks", "process", "timers",
        ];
        rooted!(&in(cx) let arr = w2::NewArrayObject1(cx, builtins.len()));
        if !arr.get().is_null() {
            for (i, name) in builtins.iter().enumerate() {
                let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
                let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_name.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx) let v = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineElement(
                        cx.raw_cx(),
                        arr.handle().into(),
                        i as u32,
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            JS_DefineProperty3(cx.raw_cx(), mod_obj.handle().into(), c"builtinModules".as_ptr(), arr.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // globalPaths
        rooted!(&in(cx) let gp = w2::NewArrayObject1(cx, 0));
        if !gp.get().is_null() {
            JS_DefineProperty3(cx.raw_cx(), mod_obj.handle().into(), c"globalPaths".as_ptr(), gp.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // _pathCache
        rooted!(&in(cx) let pc = w2::JS_NewPlainObject(cx));
        if !pc.get().is_null() {
            JS_DefineProperty3(cx.raw_cx(), mod_obj.handle().into(), c"_pathCache".as_ptr(), pc.handle().into(), 0);
        }

        // wrapSafe — returns the module source wrapper
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"wrapSafe".as_ptr(), Some(module_wrap_safe), 2, 0);

        // syncBuiltinLoader
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"SyncModuleLoader".as_ptr(), Some(module_sync_loader), 0, 0);
    }

    cache_builtin(cx, "module", mod_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // id
    let id = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        ".".to_string()
    };
    let id_str = JS_NewStringCopyN(cx, id.as_ptr() as *const ::std::os::raw::c_char, id.len());
    if !id_str.is_null() {
        rooted!(&in(cx_ref) let iv = mozjs::jsval::StringValue(&*id_str));
        JS_DefineProperty(cx, obj.handle().into(), c"id".as_ptr(), iv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // filename
    let filename = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        id.clone()
    };
    let fn_str = JS_NewStringCopyN(cx, filename.as_ptr() as *const ::std::os::raw::c_char, filename.len());
    if !fn_str.is_null() {
        rooted!(&in(cx_ref) let fv = mozjs::jsval::StringValue(&*fn_str));
        JS_DefineProperty(cx, obj.handle().into(), c"filename".as_ptr(), fv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // loaded = false
    rooted!(&in(cx_ref) let lv = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, obj.handle().into(), c"loaded".as_ptr(), lv.handle().into(), JSPROP_ENUMERATE as u32);

    // exports = {}
    rooted!(&in(cx_ref) let exports_obj = w2::JS_NewPlainObject(cx_ref));
    if !exports_obj.get().is_null() {
        JS_DefineProperty3(cx, obj.handle().into(), c"exports".as_ptr(), exports_obj.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // require (uses the global require)
    let global = JS::CurrentGlobalOrNull(cx);
    if !global.is_null() {
        let mut req_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
            c"require".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut req_val },
        );
        if req_val.is_object() {
            rooted!(&in(cx_ref) let rv = req_val);
            JS_DefineProperty(cx, obj.handle().into(), c"require".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_create_require(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // createRequire returns the global require function
    let global = JS::CurrentGlobalOrNull(cx);
    if !global.is_null() {
        let mut req_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
            c"require".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut req_val },
        );
        if req_val.is_object() {
            args.rval().set(req_val);
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_resolve_filename(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        let msg = c"Module._resolveFilename requires a specifier".as_ptr();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg);
        return false;
    }

    let specifier = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let parent_str = if argc > 1 && (*args.get(1).ptr).is_string() {
        Some(crate::js_to_rust_string(cx, *args.get(1).ptr))
    } else {
        None
    };
    let base_dir = parent_str.as_ref().map(|p| ::std::path::Path::new(p));

    match bao_engine::module_loader::try_external_resolve(&specifier, base_dir) {
        Some(resolved) => {
            let path_str = resolved.to_string_lossy().into_owned();
            let c_path = bun_core::ZBox::from_bytes(path_str.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(StringValue(&*js_str));
            }
        }
        None => {
            let msg = {
                let prefix = b"Cannot find module '";
                let suffix = b"'";
                let mut bytes = Vec::with_capacity(prefix.len() + specifier.len() + suffix.len());
                bytes.extend_from_slice(prefix);
                bytes.extend_from_slice(specifier.as_bytes());
                bytes.extend_from_slice(suffix);
                bun_core::ZBox::from_bytes(bytes)
            };
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_node_module_paths(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, 0));
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_wrap_safe(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Return the source as-is wrapped in a function
    if argc > 0 && (*args.get(0).ptr).is_string() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

unsafe extern "C" fn module_sync_loader(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}
