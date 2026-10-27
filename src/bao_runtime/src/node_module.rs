// @trace REQ-ENG-005
use ::std::path::Path;
use ::std::ptr::NonNull;
use bun_core::ZBox;

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
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"createRequire".as_ptr(),
            Some(module_create_require),
            1,
            0,
        );

        // @trace REQ-ENG-005 [module:createRequire] — node v24 oracle (2026-10-25):
        // `Module._cache === require.cache === createRequire(f).cache` — one
        // shared module cache per context. Mount the ONE cache object
        // require.rs maintains (record_module_in_cache populates it); the
        // previous fresh plain object was a dead face that never reflected
        // loaded modules.
        rooted!(&in(cx) let cache_obj = crate::require::shared_require_cache(cx.raw_cx()));
        if !cache_obj.get().is_null() {
            JS_DefineProperty3(
                cx.raw_cx(),
                mod_obj.handle().into(),
                c"_cache".as_ptr(),
                cache_obj.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // _extensions — node v24 oracle key set: ['.js', '.json', '.node'].
        // `createRequire(f).extensions` is THIS SAME object (oracle identity:
        // `createRequire(f).extensions === Module._extensions`).
        rooted!(&in(cx) let ext_obj = w2::JS_NewPlainObject(cx));
        if !ext_obj.get().is_null() {
            JS_DefineProperty3(
                cx.raw_cx(),
                ext_obj.handle().into(),
                c".js".as_ptr(),
                ext_obj.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineProperty3(
                cx.raw_cx(),
                ext_obj.handle().into(),
                c".json".as_ptr(),
                ext_obj.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            // @trace REQ-ENG-004 — `.node` carries the REAL loader body (E30):
            // node contract `(module, filename) => { process.dlopen(module,
            // filename); }` — oracle v24.19.0: anonymous function, arity 2.
            // The previous face mounted the extensions object itself as the
            // value (E20 key-only registration: key present, body absent —
            // the dead face is retired now that the load chain exists). The
            // SAME native body backs process.dlopen (one operation, two
            // faces — see process_dlopen_native).
            let node_loader_fn = JS_NewFunction(
                cx.raw_cx(),
                Some(process_dlopen_native),
                2,
                0,
                c"".as_ptr(),
            );
            if !node_loader_fn.is_null() {
                rooted!(&in(cx) let nl_obj = JS_GetFunctionObject(node_loader_fn));
                JS_DefineProperty3(
                    cx.raw_cx(),
                    ext_obj.handle().into(),
                    c".node".as_ptr(),
                    nl_obj.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            JS_DefineProperty3(
                cx.raw_cx(),
                mod_obj.handle().into(),
                c"_extensions".as_ptr(),
                ext_obj.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // @trace REQ-ENG-004 — process.dlopen (E30): the native-addon entry
        // Module._extensions['.node'] delegates to. Mounted on the
        // already-installed process global (install order:
        // bun_api::install_process_global runs before node_module::install
        // in globals.rs::install_node_apis). Oracle v24.19.0: enumerable own
        // function property, name "dlopen", declared arity 0. Absent process
        // global = loud-by-absence (no fake face mounted).
        let global = JS::CurrentGlobalOrNull(cx.raw_cx());
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let mut proc_val = UndefinedValue();
            JS_GetProperty(
                cx.raw_cx(),
                global_root.handle().into(),
                c"process".as_ptr(),
                MutableHandle::<JSVal> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut proc_val,
                },
            );
            if proc_val.is_object() {
                let proc_obj = proc_val.to_object();
                rooted!(&in(cx) let proc_root = proc_obj);
                w2::JS_DefineFunction(
                    cx,
                    proc_root.handle(),
                    c"dlopen".as_ptr(),
                    Some(process_dlopen_native),
                    0,
                    0,
                );
            }
        }

        // _resolveFilename
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"_resolveFilename".as_ptr(),
            Some(module_resolve_filename),
            2,
            0,
        );

        // _nodeModulePaths
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"_nodeModulePaths".as_ptr(),
            Some(module_node_module_paths),
            1,
            0,
        );

        // builtinModules array
        let builtins = [
            "assert",
            "buffer",
            "child_process",
            "crypto",
            "dns",
            "events",
            "fs",
            "http",
            "https",
            "net",
            "os",
            "path",
            "querystring",
            "readline",
            "stream",
            "string_decoder",
            "tls",
            "tty",
            "url",
            "util",
            "vm",
            "zlib",
            "perf_hooks",
            "process",
            "timers",
        ];
        rooted!(&in(cx) let arr = w2::NewArrayObject1(cx, builtins.len()));
        if !arr.get().is_null() {
            for (i, name) in builtins.iter().enumerate() {
                let c_name = ZBox::from_bytes(name.as_bytes());
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
            JS_DefineProperty3(
                cx.raw_cx(),
                mod_obj.handle().into(),
                c"builtinModules".as_ptr(),
                arr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // globalPaths
        rooted!(&in(cx) let gp = w2::NewArrayObject1(cx, 0));
        if !gp.get().is_null() {
            JS_DefineProperty3(
                cx.raw_cx(),
                mod_obj.handle().into(),
                c"globalPaths".as_ptr(),
                gp.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // _pathCache
        rooted!(&in(cx) let pc = w2::JS_NewPlainObject(cx));
        if !pc.get().is_null() {
            JS_DefineProperty3(
                cx.raw_cx(),
                mod_obj.handle().into(),
                c"_pathCache".as_ptr(),
                pc.handle().into(),
                0,
            );
        }

        // wrapSafe — returns the module source wrapper
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"wrapSafe".as_ptr(),
            Some(module_wrap_safe),
            2,
            0,
        );

        // syncBuiltinLoader
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"SyncModuleLoader".as_ptr(),
            Some(module_sync_loader),
            0,
            0,
        );
    }

    cache_builtin(cx, "module", mod_obj.get());
}

// @trace REQ-ENG-004 — fresh CJS module record for loader dispatch (E30):
// node `new Module(id)` observable shape — id / filename / exports / loaded.
// `require` is attached by the Module-constructor face only (module_ctor);
// the .node loader contract reads module.exports, not module.require.
// Returns null on allocation failure — callers fail loud.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn new_module_record(cx: *mut JSContext, id: &str, filename: &str) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        return ::std::ptr::null_mut();
    }

    let id_str = JS_NewStringCopyN(cx, id.as_ptr() as *const ::std::os::raw::c_char, id.len());
    if !id_str.is_null() {
        rooted!(&in(cx_ref) let iv = mozjs::jsval::StringValue(&*id_str));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"id".as_ptr(),
            iv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let fn_str = JS_NewStringCopyN(
        cx,
        filename.as_ptr() as *const ::std::os::raw::c_char,
        filename.len(),
    );
    if !fn_str.is_null() {
        rooted!(&in(cx_ref) let fv = mozjs::jsval::StringValue(&*fn_str));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"filename".as_ptr(),
            fv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // loaded = false
    rooted!(&in(cx_ref) let lv = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"loaded".as_ptr(),
        lv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // exports = {}
    rooted!(&in(cx_ref) let exports_obj = w2::JS_NewPlainObject(cx_ref));
    if !exports_obj.get().is_null() {
        JS_DefineProperty3(
            cx,
            obj.handle().into(),
            c"exports".as_ptr(),
            exports_obj.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    obj.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_ctor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // id / filename / loaded / exports — the shared module-record builder
    // (E30: the same shape the .node loader dispatch constructs).
    let id = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        ".".to_string()
    };
    let filename = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        id.clone()
    };
    let record = new_module_record(cx, &id, &filename);
    if record.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let obj = record);

    // require (uses the global require)
    let global = JS::CurrentGlobalOrNull(cx);
    if !global.is_null() {
        rooted!(&in(cx_ref) let global_root = global);
        let mut req_val = UndefinedValue();
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            c"require".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut req_val,
            },
        );
        if req_val.is_object() {
            rooted!(&in(cx_ref) let rv = req_val);
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"require".as_ptr(),
                rv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

// @trace REQ-ENG-005 [module:createRequire] — node:module createRequire,
// full node v24 semantics (oracle 2026-10-25):
//   * input: a file URL object (`protocol === "file:"`, path from
//     `pathname`), a `file://` URL string, or an absolute path string.
//     Everything else — relative paths, non-file URLs, non-strings — throws
//     TypeError ERR_INVALID_ARG_VALUE (the oracle REJECTS
//     `createRequire('./x')`; "relative resolves against cwd" is not node
//     v24 behavior).
//   * returns a FRESH require function per call (oracle: `r === r2` is
//     false) whose resolution base is `dirname(filename)`: relative
//     specifiers and node_modules chains resolve from the created require's
//     own directory via the shared require_fn machinery
//     (require::call_require_with_base), so builtin caching, extension
//     probing and the CJS wrapper pipeline behave identically to the global
//     require.
//   * own enumerable keys: resolve (base-aware), main (undefined — oracle),
//     extensions (=== Module._extensions — oracle object identity), cache
//     (=== the shared require.cache — oracle object identity).
// URL-string form: the scheme prefix is stripped WITHOUT percent-decoding —
// mirrors module_loader's host_populate_import_meta convention (bao's
// import.meta.url is an unencoded raw fs path, so stripping is the exact
// inverse of how that URL was produced).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_create_require(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    const BAD_FILENAME: &str = "The argument 'filename' must be a file URL object, file URL string, or absolute path string";

    // --- filename → absolute fs path (node v24 oracle input contract) ---
    if argc == 0 {
        return throw_invalid_arg_value(cx, &format!("{}. Received undefined", BAD_FILENAME));
    }
    let arg0 = *args.get(0).ptr;
    let filename: String = if arg0.is_object() {
        rooted!(&in(cx_ref) let obj_root = arg0.to_object());
        let mut proto_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"protocol".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut proto_val,
            },
        );
        let proto = if proto_val.is_string() {
            crate::js_to_rust_string(cx, proto_val)
        } else {
            String::new()
        };
        if proto != "file:" {
            return throw_invalid_arg_value(
                cx,
                &format!("{}. Received non-file URL", BAD_FILENAME),
            );
        }
        let mut path_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"pathname".as_ptr(),
            MutableHandle::<JSVal> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut path_val,
            },
        );
        if !path_val.is_string() {
            return throw_invalid_arg_value(
                cx,
                &format!("{}. Received URL without a pathname", BAD_FILENAME),
            );
        }
        crate::js_to_rust_string(cx, path_val)
    } else if arg0.is_string() {
        let s = crate::js_to_rust_string(cx, arg0);
        if let Some(p) = s.strip_prefix("file://") {
            // `file://<host>/path` with a non-empty host: node v24 accepts
            // only the empty/localhost host; a strip result that is not an
            // absolute path means a host component was present — reject
            // loudly instead of resolving from a bogus base.
            if Path::new(p).is_absolute() {
                p.to_string()
            } else {
                return throw_invalid_arg_value(
                    cx,
                    &format!("{}. Received file URL with unsupported host", BAD_FILENAME),
                );
            }
        } else if Path::new(&s).is_absolute() {
            s
        } else {
            return throw_invalid_arg_value(
                cx,
                &format!("{}. Received relative path '{}'", BAD_FILENAME, s),
            );
        }
    } else {
        return throw_invalid_arg_value(cx, &format!("{}. Received non-path value", BAD_FILENAME));
    };

    // Resolution base = dirname(filename): node `createRequire('/x/y.js')`
    // resolves './z' and bare packages from '/x'.
    let base_dir = Path::new(&filename)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "/".to_string());

    // --- fresh require function carrying its base in a hidden slot ---
    let require_fn_ptr = JS_NewFunction(cx, Some(created_require_fn), 1, 0, c"require".as_ptr());
    if require_fn_ptr.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fn_obj = JS_GetFunctionObject(require_fn_ptr);
    rooted!(&in(cx_ref) let fn_root = fn_obj);

    let c_base = ZBox::from_bytes(base_dir.as_bytes());
    let base_js = JS_NewStringCopyZ(cx, c_base.as_ptr());
    if base_js.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let base_root = StringValue(&*base_js));
    // Hidden, non-enumerable slot (flags 0): the oracle enumerates exactly
    // [resolve, main, extensions, cache] on a created require.
    JS_DefineProperty(
        cx,
        fn_root.handle().into(),
        c"@baoRequireBase".as_ptr(),
        base_root.handle().into(),
        0,
    );

    // resolve — base-aware (oracle: `createRequire('/tmp/x.js').resolve('./y')`
    // resolves from /tmp; same extension probing as the global resolve).
    let resolve_fn = JS_NewFunction(
        cx,
        Some(created_require_resolve_fn),
        2,
        0,
        c"resolve".as_ptr(),
    );
    if !resolve_fn.is_null() {
        let resolve_rooted = JS_GetFunctionObject(resolve_fn);
        rooted!(&in(cx_ref) let resolve_root = resolve_rooted);
        // The resolve property face carries its OWN hidden base slot (same
        // base as the parent require). Do NOT rely on CallArgs::callee()
        // here: on a function-valued property face, the stack-frame callee is
        // not the property-carrying require object, so args.callee() would
        // miss the parent's hidden slot (the fail-closed
        // "internal base slot missing" loud error on every call). An own slot
        // on the resolve function itself is deterministic and self-contained.
        rooted!(&in(cx_ref) let base_for_resolve = StringValue(&*base_js));
        JS_DefineProperty(
            cx,
            resolve_root.handle().into(),
            c"@baoRequireBase".as_ptr(),
            base_for_resolve.handle().into(),
            0,
        );
        // @trace REQ-ENG-048 [bug:eval-cjs-body-unrooted-args] — read the
        // live root, not the raw `resolve_rooted` pointer: the
        // JS_DefineProperty above (hidden @baoRequireBase slot) is a GC
        // point and can evacuate the resolve function object; the raw
        // pointer would seed a from-space Value as the public `resolve`
        // property (same unrooted-args class as eval_cjs_body's seed fix).
        rooted!(&in(cx_ref) let resolve_prop = ObjectValue(*resolve_root));
        JS_DefineProperty(
            cx,
            fn_root.handle().into(),
            c"resolve".as_ptr(),
            resolve_prop.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // main — node oracle: undefined on created requires (no main module in
    // this context). Enumerable own property per the oracle Object.keys.
    rooted!(&in(cx_ref) let main_root = UndefinedValue());
    JS_DefineProperty(
        cx,
        fn_root.handle().into(),
        c"main".as_ptr(),
        main_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // extensions — SAME object as Module._extensions (oracle object identity:
    // `createRequire(f).extensions === Module._extensions`). Absent only if
    // the module builtin was never installed — loud-by-absence, no fake face.
    let ext_obj = read_module_extensions(cx);
    if !ext_obj.is_null() {
        rooted!(&in(cx_ref) let ext_root = ObjectValue(ext_obj));
        JS_DefineProperty(
            cx,
            fn_root.handle().into(),
            c"extensions".as_ptr(),
            ext_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // cache — SAME object as require.cache / Module._cache (oracle object
    // identity; this is the one cache record_module_in_cache populates).
    let cache_obj = crate::require::shared_require_cache(cx);
    if !cache_obj.is_null() {
        rooted!(&in(cx_ref) let cache_root = ObjectValue(cache_obj));
        JS_DefineProperty(
            cx,
            fn_root.handle().into(),
            c"cache".as_ptr(),
            cache_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval().set(ObjectValue(fn_root.get()));
    true
}

// @trace REQ-ENG-005 [module:createRequire] — the `extensions` object shared
// between Module._extensions and created requires (node oracle: same object).
// Returns null when the module builtin is not installed — the caller then
// skips the property instead of mounting a dead stand-in.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_module_extensions(cx: *mut JSContext) -> *mut JSObject {
    let mod_obj = match crate::require::get_builtin(cx, "module") {
        Some(o) if !o.is_null() => o,
        _ => return ::std::ptr::null_mut(),
    };
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let mod_root = mod_obj);
    let mut ext_val = UndefinedValue();
    JS_GetProperty(
        cx,
        mod_root.handle().into(),
        c"_extensions".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ext_val,
        },
    );
    if ext_val.is_object() {
        ext_val.to_object()
    } else {
        ::std::ptr::null_mut()
    }
}

// @trace REQ-ENG-005 [module:createRequire] — read the hidden base-directory
// slot a created require carries on its function object (written by
// module_create_require as the non-enumerable `@baoRequireBase` property).
// None ⇒ the callee was not produced by module_create_require; the dispatch
// wrappers fail closed (loud error) rather than silently resolving from the
// wrong base.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn created_require_base(cx: *mut JSContext, callee: *mut JSObject) -> Option<String> {
    if callee.is_null() {
        return None;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let callee_root = callee);
    let mut base_val = UndefinedValue();
    JS_GetProperty(
        cx,
        callee_root.handle().into(),
        c"@baoRequireBase".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut base_val,
        },
    );
    if !base_val.is_string() {
        return None;
    }
    Some(crate::js_to_rust_string(cx, base_val))
}

// @trace REQ-ENG-005 [module:createRequire] — the created require body:
// re-dispatches into the SHARED require implementation with the created
// require's own base directory (require::call_require_with_base), so every
// loader feature is byte-identical to the global require except the
// resolution base.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn created_require_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let Some(base) = created_require_base(cx, args.callee()) else {
        JS_ReportErrorUTF8(
            cx,
            c"require: internal base slot missing (not created via module.createRequire)".as_ptr(),
        );
        return false;
    };
    crate::require::call_require_with_base(cx, Path::new(&base), argc, vp)
}

// @trace REQ-ENG-005 [module:createRequire] — base-aware require.resolve for
// created requires (require::call_require_resolve_with_base).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn created_require_resolve_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let Some(base) = created_require_base(cx, args.callee()) else {
        JS_ReportErrorUTF8(
            cx,
            c"require.resolve: internal base slot missing (not created via module.createRequire)"
                .as_ptr(),
        );
        return false;
    };
    crate::require::call_require_resolve_with_base(cx, Path::new(&base), argc, vp)
}

// @trace REQ-ENG-005 [module:createRequire] — TypeError with `.code =
// ERR_INVALID_ARG_VALUE` for createRequire input validation (node oracle
// error face). House pattern: globals.rs throw_error_with_code.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_invalid_arg_value(cx: *mut JSContext, msg: &str) -> bool {
    let c_msg = ::std::ffi::CString::new(msg)
        .unwrap_or_else(|_| ::std::ffi::CString::new("invalid argument").unwrap());
    {
        let mut cx_s = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        mozjs::error::throw_type_error(&mut cx_s, c_msg.as_ref());
    }
    if JS_IsExceptionPending(cx) {
        rooted!(in(cx) let mut exn = UndefinedValue());
        JS_GetPendingException(cx, exn.handle_mut().into());
        let exn_val = exn.get();
        if exn_val.is_object() {
            let cx_ref_err = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref_err) let exn_root = exn_val.to_object());
            let code_str =
                JS_NewStringCopyZ(cx, ZBox::from_bytes(b"ERR_INVALID_ARG_VALUE").as_ptr());
            if !code_str.is_null() {
                let code_val = StringValue(&*code_str);
                rooted!(&in(cx_ref_err) let code_r = code_val);
                JS_DefineProperty(
                    cx,
                    exn_root.handle().into(),
                    c"code".as_ptr(),
                    code_r.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                JS_SetPendingException(
                    cx,
                    exn.handle().into(),
                    ExceptionStackBehavior::DoNotCapture,
                );
            }
        }
    }
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn module_resolve_filename(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 && (*args.get(0).ptr).is_string() {
        // Just return the specifier as-is (simplified)
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    args.rval().set(UndefinedValue());
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
unsafe extern "C" fn module_wrap_safe(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Return the source as-is wrapped in a function
    if argc > 0 && (*args.get(0).ptr).is_string() {
        args.rval().set(*args.get(0).ptr);
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

unsafe extern "C" fn module_sync_loader(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// ── .node native addon load chain (E30) ─────────────────────────────────

// @trace REQ-ENG-004 — node v24.19.0 oracle (2026-10-25, real binary, gcc
// fixtures): non-ELF file → plain `Error` code ERR_DLOPEN_FAILED whose
// message is the raw dlerror text; a shared object without the
// self-registration symbol → ERR_DLOPEN_FAILED "Module did not
// self-register: '<path>'."; a NAPI addon loads via dlopen +
// dlsym('napi_register_module_v1') + call(env, exports), the return value
// becoming module.exports.
//
// Bao layering (honest terminal state, fail-closed): dlopen and the
// registration-symbol probe are REAL here — crate::bun_ffi::FfiLibrary, the
// same loader face bun:ffi uses (reuse ladder: no hand-rolled dlopen). The
// registration CALL is the NEXT LAYER: it needs a napi_env (a NAPI-on-SM
// implementation; the tree has zero napi faces). A real NAPI addon
// dereferences env immediately, so a fabricated env pointer would SIGSEGV —
// the chain terminates in a loud, precisely attributed Error instead of a
// fake load.
//
// Ok(exports): registration succeeded and module.exports is populated (next
// layer lands here). Err(()): a JS exception is pending on the context.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn process_dlopen_impl(
    cx: *mut JSContext,
    module_obj: *mut JSObject,
    filename: &str,
) -> ::std::result::Result<JSVal, ()> {
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let module_root = module_obj);

    // node contract: module.exports must exist — a record without it is a
    // caller bug (loud, not silent).
    let mut exports_val = UndefinedValue();
    JS_GetProperty(
        cx,
        module_root.handle().into(),
        c"exports".as_ptr(),
        MutableHandle::<JSVal> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut exports_val,
        },
    );
    if !exports_val.is_object() {
        throw_dlopen_failed(cx, "process.dlopen: the module record has no exports object");
        return Err(());
    }

    // Real dlopen. FfiLibrary::drop dlcloses, so every failure path below
    // releases the handle. Oracle: the dlerror text IS the ERR_DLOPEN_FAILED
    // message verbatim.
    let lib = match crate::bun_ffi::FfiLibrary::dlopen(filename) {
        Ok(l) => l,
        Err(msg) => {
            throw_dlopen_failed(cx, &msg);
            return Err(());
        }
    };

    // Registration-symbol probe — node dlsyms `napi_register_module_v1` for
    // NAPI addons; `node_register_module_v1` is the legacy one-shot face.
    let napi_sym = lib.symbol("napi_register_module_v1").ok();
    let legacy_sym = if napi_sym.is_some() {
        None
    } else {
        lib.symbol("node_register_module_v1").ok()
    };
    drop(lib);

    if napi_sym.is_none() && legacy_sym.is_none() {
        // Oracle-exact shape for a shared object without self-registration.
        throw_dlopen_failed(cx, &format!("Module did not self-register: '{}'.", filename));
        return Err(());
    }

    // Registration symbol present — the call protocol is the NEXT LAYER
    // (napi_env provisioning / NAPI-on-SM). Loud and precisely attributed:
    // this is the one .node class node loads and bao does not (registered
    // divergence, not a silent failure).
    let sym_name = if napi_sym.is_some() {
        "napi_register_module_v1"
    } else {
        "node_register_module_v1"
    };
    throw_dlopen_failed(
        cx,
        &format!(
            "native addon registration is not implemented in this layer: '{}' exposes {} but there is no NAPI environment to run it (next layer: NAPI-on-SM; see process_dlopen_impl)",
            filename, sym_name
        ),
    );
    Err(())
}

// @trace REQ-ENG-004 — the shared process.dlopen native body, registered
// under BOTH faces (node's two faces are the same operation):
//   * global `process.dlopen` — oracle: name "dlopen", declared arity 0;
//   * `Module._extensions['.node']` — oracle: anonymous, arity 2.
// Returns undefined; the loaded module communicates through module.exports.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_dlopen_native(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Guarded reads: the process.dlopen face declares arity 0, so actual
    // argc can be below the two parameters the contract needs.
    let module_arg = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let filename_arg = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    if !module_arg.is_object() || !filename_arg.is_string() {
        return throw_dlopen_failed(
            cx,
            "The \"module\" argument must be an object and the \"filename\" argument must be a string",
        );
    }
    let module_obj = module_arg.to_object();
    let filename = crate::js_to_rust_string(cx, filename_arg);
    match process_dlopen_impl(cx, module_obj, &filename) {
        Ok(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        // Exception pending on the context — propagate unchanged.
        Err(()) => false,
    }
}

// @trace REQ-ENG-004 — ERR_DLOPEN_FAILED error face: node throws a plain
// `Error` (oracle constructor check 2026-10-25) with
// .code = "ERR_DLOPEN_FAILED". House pattern: node_module.rs
// throw_invalid_arg_value (pending-exception code attach).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_dlopen_failed(cx: *mut JSContext, msg: &str) -> bool {
    let c_msg = ::std::ffi::CString::new(msg)
        .unwrap_or_else(|_| ::std::ffi::CString::new("dlopen failed").unwrap());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    if JS_IsExceptionPending(cx) {
        rooted!(in(cx) let mut exn = UndefinedValue());
        JS_GetPendingException(cx, exn.handle_mut().into());
        let exn_val = exn.get();
        if exn_val.is_object() {
            let cx_ref_err = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref_err) let exn_root = exn_val.to_object());
            let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(b"ERR_DLOPEN_FAILED").as_ptr());
            if !code_str.is_null() {
                let code_val = StringValue(&*code_str);
                rooted!(&in(cx_ref_err) let code_r = code_val);
                JS_DefineProperty(
                    cx,
                    exn_root.handle().into(),
                    c"code".as_ptr(),
                    code_r.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                JS_SetPendingException(
                    cx,
                    exn.handle().into(),
                    ExceptionStackBehavior::DoNotCapture,
                );
            }
        }
    }
    false
}
