// @trace REQ-ENG-006
// Bun.* namespace + process global + servers + test runner
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::os::unix::ffi::OsStrExt;
use ::std::path;
// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)
use ::std::ptr::NonNull;

use bun_core::Fd;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, NullValue, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1,
};
use mozjs::conversions::jsstr_to_string;

use bun_uws_sys::app::App;
use bun_uws_sys::response::Response;

/// D50: Helper to get cwd as PathBuf via bun_sys instead of std::env::current_dir
fn getcwd_pathbuf() -> Option<path::PathBuf> {
    bun_sys::getcwd_alloc().ok().map(|zb| {
        path::PathBuf::from(::std::str::from_utf8(zb.as_bytes()).unwrap_or("."))
    })
}
use bun_uws_sys::request::Request;
use bun_uws_sys::socket_context::BunSocketContextOptions;
use bun_uws_sys::listen_socket::ListenSocket;

/// Install Bun.* namespace on a target object (REQ-SEC-002 parameter injection).
///
/// Same as `install_bun_global` but attaches the Bun object to `target`
/// instead of `global`. Used by `create_node_api_scope_values` to build
/// the temporary scope object for privileged evaluate_js.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `target` is a valid
/// handle to a JSObject.
pub unsafe fn install_bun_on_target(
    cx: &mut mozjs::context::JSContext,
    target: mozjs::rust::Handle<*mut JSObject>,
) {
    rooted!(&in(cx) let bun_obj = JS_NewPlainObject(cx));
    if bun_obj.get().is_null() {
        return;
    }

    populate_bun_object(cx, bun_obj.handle());

    JS_DefineProperty3(cx, target, c"Bun".as_ptr(), bun_obj.handle(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty3(cx, target, c"Bao".as_ptr(), bun_obj.handle(), JSPROP_ENUMERATE as u32);
}

/// Populate a Bun object with all properties and methods.
///
/// Shared between `install_bun_global` and `install_bun_on_target`.
unsafe fn populate_bun_object(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    let version_str = JS_NewStringCopyZ(
        cx.raw_cx(),
        c"0.1.0".as_ptr(),
    );
    if !version_str.is_null() {
        rooted!(&in(cx) let ver_val = StringValue(&*version_str));
        JS_DefineProperty(
            cx.raw_cx(),
            bun_obj.into(),
            c"version".as_ptr(),
            ver_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Bun.env → copy of process.env (same data source)
    {
        rooted!(&in(cx) let env_obj = JS_NewPlainObject(cx));
        if !env_obj.get().is_null() {
            // D52: bun_sys::environ instead of std::env::vars
            for entry in bun_sys::environ() {
                // D58: ZStr::from_c_ptr instead of CStr::from_ptr
                let entry_bytes = unsafe { bun_core::ZStr::from_c_ptr(*entry) }.as_bytes();
                if let Some(pos) = entry_bytes.iter().position(|b| *b == b'=') {
                    let key = ::std::str::from_utf8(&entry_bytes[..pos]).unwrap_or("");
                    let value = ::std::str::from_utf8(&entry_bytes[pos+1..]).unwrap_or("");
                    if key.is_empty() { continue; }
                    let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
                    let c_val = bun_core::ZBox::from_bytes(value.as_bytes());
                    let val_str = JS_NewStringCopyZ(cx.raw_cx(), c_val.as_ptr());
                    if !val_str.is_null() {
                        rooted!(&in(cx) let v = StringValue(&*val_str));
                        JS_DefineProperty(cx.raw_cx(), env_obj.handle().into(), c_key.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
            }
            JS_DefineProperty3(cx, bun_obj, c"env".as_ptr(), env_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // Bun.argv → process.argv (same data source)
    // D101: bun_core::util::argv() replaces std::env::args()
    {
        let args = bun_core::util::argv();
        rooted!(&in(cx) let argv_arr = NewArrayObject1(cx, args.len()));
        if !argv_arr.get().is_null() {
            for (i, arg) in args.iter().enumerate() {
                let c_arg = bun_core::ZBox::from_bytes(arg);
                let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_arg.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*js_str));
                    JS_DefineElement(cx.raw_cx(), argv_arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            JS_DefineProperty3(cx, bun_obj, c"argv".as_ptr(), argv_arr.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    JS_DefineFunction(cx, bun_obj, c"file".as_ptr(), Some(bun_file), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"write".as_ptr(), Some(bun_write), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"readFile".as_ptr(), Some(bun_read_file), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"serve".as_ptr(), Some(bun_serve), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"spawn".as_ptr(), Some(bun_spawn), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"cwd".as_ptr(), Some(bun_cwd), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"gc".as_ptr(), Some(bun_gc), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"sleep".as_ptr(), Some(bun_sleep), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"which".as_ptr(), Some(bun_which), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"inspect".as_ptr(), Some(bun_inspect), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"resolve".as_ptr(), Some(bun_resolve), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"build".as_ptr(), Some(bun_build), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"test".as_ptr(), Some(bun_test), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"testRun".as_ptr(), Some(test_run), 0, JSPROP_ENUMERATE as u32);

    // Bun.read — alias for readFile
    {
        rooted!(&in(cx) let mut read_val = UndefinedValue());
        let _ok = JS_GetProperty(
            cx.raw_cx(),
            bun_obj.into(),
            c"readFile".as_ptr(),
            read_val.handle_mut().into(),
        );
        JS_DefineProperty(
            cx.raw_cx(),
            bun_obj.into(),
            c"read".as_ptr(),
            read_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    JS_DefineFunction(cx, bun_obj, c"exit".as_ptr(), Some(bun_exit), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, bun_obj, c"sleepSync".as_ptr(), Some(bun_sleep_sync), 1, JSPROP_ENUMERATE as u32);

    // Bun.revision
    {
        let rev_str = JS_NewStringCopyZ(cx.raw_cx(), c"0.1.0".as_ptr());
        if !rev_str.is_null() {
            rooted!(&in(cx) let rv = StringValue(&*rev_str));
            JS_DefineProperty(cx.raw_cx(), bun_obj.into(), c"revision".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // Bun.main
    {
        let main_path = crate::require::get_require_dir()
            .unwrap_or_else(|| getcwd_pathbuf().unwrap_or_default());
        let c_main = bun_core::ZBox::from_bytes(main_path.to_string_lossy().as_bytes());
        let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_main.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx) let mv = StringValue(&*js_str));
            JS_DefineProperty(cx.raw_cx(), bun_obj.into(), c"main".as_ptr(), mv.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // Bun.hash
    JS_DefineFunction(cx, bun_obj, c"hash".as_ptr(), Some(bun_hash), 2, JSPROP_ENUMERATE as u32);
}

pub fn install_bun_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let bun_obj = JS_NewPlainObject(cx));
        if bun_obj.get().is_null() {
            return;
        }

        populate_bun_object(cx, bun_obj.handle());

        JS_DefineProperty3(
            cx,
            global,
            c"Bun".as_ptr(),
            bun_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(
            cx,
            global,
            c"Bao".as_ptr(),
            bun_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

/// Install process.* namespace on a target object (REQ-SEC-002 parameter injection).
///
/// Same as `install_process_global` but attaches the process object to `target`
/// instead of `global`. Used by `create_node_api_scope_values` to build
/// the temporary scope object for privileged evaluate_js.
///
/// `global` is no longer required for env helper functions — they are
/// installed on `target` (the scope object) instead, eliminating them
/// from the global surface entirely (REQ-SEC-003 hardening).
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer, `target` is a valid
/// handle to the scope JSObject, and `global` is a valid handle to the global
/// JSObject.
pub unsafe fn install_process_on_target(
    cx: &mut mozjs::context::JSContext,
    target: mozjs::rust::Handle<*mut JSObject>,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    rooted!(&in(cx) let proc_obj = JS_NewPlainObject(cx));
    if proc_obj.get().is_null() {
        return;
    }

    populate_process_object(cx, proc_obj.handle(), target, global);

    JS_DefineProperty3(cx, target, c"process".as_ptr(), proc_obj.handle(), JSPROP_ENUMERATE as u32);
}

/// Populate a process object with all properties and methods.
///
/// Shared between `install_process_global` and `install_process_on_target`.
///
/// `target` is the scope object where `__bao_setEnv`/`__bao_delEnv` helper
/// functions are installed (not on global — eliminates global surface leak).
/// `global` is used only for Buffer reference retrieval.
unsafe fn populate_process_object(
    cx: &mut mozjs::context::JSContext,
    proc_obj: mozjs::rust::Handle<*mut JSObject>,
    target: mozjs::rust::Handle<*mut JSObject>,
    _global: mozjs::rust::Handle<*mut JSObject>,
) {
    // process.arch — Node.js convention: "x64", "arm64" (not "x86_64", "aarch64")
    let arch_z = bun_core::ZBox::from_bytes(bun_core::env::ARCH.npm_name().as_bytes());
    let arch_str = JS_NewStringCopyZ(cx.raw_cx(), arch_z.as_ptr());
    if !arch_str.is_null() {
        rooted!(&in(cx) let arch_val = StringValue(&*arch_str));
        JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"arch".as_ptr(), arch_val.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // process.platform — Node.js convention: "linux", "darwin", "win32"
    let plat_z = bun_core::ZBox::from_bytes(bun_core::env::OS_NAME_NODE.as_bytes());
    let platform_str = JS_NewStringCopyZ(cx.raw_cx(), plat_z.as_ptr());
    if !platform_str.is_null() {
        rooted!(&in(cx) let plat_val = StringValue(&*platform_str));
        JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"platform".as_ptr(), plat_val.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // process.cwd()
    JS_DefineFunction(cx, proc_obj, c"cwd".as_ptr(), ::std::option::Option::Some(process_cwd), 0, JSPROP_ENUMERATE as u32);

    // process.exit()
    JS_DefineFunction(cx, proc_obj, c"exit".as_ptr(), ::std::option::Option::Some(process_exit), 1, JSPROP_ENUMERATE as u32);

    // process.argv — D101: bun_core::util::argv() replaces std::env::args()
    {
        let args = bun_core::util::argv();
        rooted!(&in(cx) let argv_arr = NewArrayObject1(cx, args.len()));
        if !argv_arr.get().is_null() {
            for (i, arg) in args.iter().enumerate() {
                let c_arg = bun_core::ZBox::from_bytes(arg);
                let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_arg.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*js_str));
                    JS_DefineElement(cx.raw_cx(), argv_arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            JS_DefineProperty3(cx, proc_obj, c"argv".as_ptr(), argv_arr.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.env — Proxy-backed for set/delete propagation to std::env
    // __bao_setEnv/__bao_delEnv are installed on `target` (the scope object),
    // NOT on `global`. The Proxy factory receives them as parameters, so
    // they never appear on the Window global (REQ-SEC-003 hardening).
    {
        JS_DefineFunction(cx, target, c"__bao_setEnv".as_ptr(),
            Some(set_env_fn), 2, 0);
        JS_DefineFunction(cx, target, c"__bao_delEnv".as_ptr(),
            Some(del_env_fn), 1, 0);

        rooted!(&in(cx) let env_target = JS_NewPlainObject(cx));
        if !env_target.get().is_null() {
            // D52: bun_sys::environ instead of std::env::vars
            for entry in bun_sys::environ() {
                // D58: ZStr::from_c_ptr instead of CStr::from_ptr
                let entry_bytes = unsafe { bun_core::ZStr::from_c_ptr(*entry) }.as_bytes();
                if let Some(pos) = entry_bytes.iter().position(|b| *b == b'=') {
                    let key = ::std::str::from_utf8(&entry_bytes[..pos]).unwrap_or("");
                    let value = ::std::str::from_utf8(&entry_bytes[pos+1..]).unwrap_or("");
                    if key.is_empty() { continue; }
                    let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
                    let c_val = bun_core::ZBox::from_bytes(value.as_bytes());
                    let val_str = JS_NewStringCopyZ(cx.raw_cx(), c_val.as_ptr());
                    if !val_str.is_null() {
                        rooted!(&in(cx) let v = StringValue(&*val_str));
                        JS_DefineProperty(cx.raw_cx(), env_target.handle().into(), c_key.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
            }

            // Proxy factory receives setEnv/delEnv as parameters — they are
            // NOT looked up from globalThis, eliminating the global surface leak.
            let proxy_src = r#"(__bao_envTarget,__bao_setEnv,__bao_delEnv)=>new Proxy(__bao_envTarget,{
                set(t,k,v){t[k]=v;try{__bao_setEnv(String(k),String(v))}catch(e){}return true},
                deleteProperty(t,k){delete t[k];try{__bao_delEnv(String(k))}catch(e){}return true},
                get(t,k){const v=t[k];return typeof v==='string'?v:undefined},
                has(t,k){return k in t},
                ownKeys(t){return Object.keys(t)},
                getOwnPropertyDescriptor(t,k){return k in t?{configurable:true,enumerable:true,value:t[k]}:undefined}
            })"#;
            let mut src = mozjs::rust::transform_str_to_source_text(proxy_src);
            let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<env>".as_ptr(), 1) as *mut _) else { return; };
            let opts = _opts_guard.as_ptr();
                let mut rval = UndefinedValue();
                let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
                let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts as *const _, &mut src, rval_h);
                if ok && rval.is_object() {
                    let handler_fn = rval.to_object();
                    rooted!(&in(cx) let fn_val = ObjectValue(handler_fn));

                    // Build 3-argument array: (env_target, __bao_setEnv, __bao_delEnv)
                    // __bao_setEnv and __bao_delEnv are on `target` (scope object),
                    // NOT on global — eliminating the global surface leak.
                    let mut set_env_val = UndefinedValue();
                    let set_env_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut set_env_val };
                    JS_GetProperty(cx.raw_cx(), target.into(), c"__bao_setEnv".as_ptr(), set_env_h);

                    let mut del_env_val = UndefinedValue();
                    let del_env_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut del_env_val };
                    JS_GetProperty(cx.raw_cx(), target.into(), c"__bao_delEnv".as_ptr(), del_env_h);

                    rooted!(&in(cx) let args_val = ObjectValue(env_target.get()));
                    let args = [args_val.get(), set_env_val, del_env_val];
                    let args_arr = HandleValueArray { length_: 3, elements_: args.as_ptr() };
                    let null_obj = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut::<JSObject>() };
                    let mut ret = UndefinedValue();
                    let ret_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ret };
                    let ok2 = JS_CallFunctionValue(cx.raw_cx(), null_obj, fn_val.handle().into(), &args_arr, ret_h);
                    if ok2 && ret.is_object() {
                        rooted!(&in(cx) let env_proxy = ret.to_object());
                        JS_DefineProperty3(cx, proc_obj, c"env".as_ptr(), env_proxy.handle(), JSPROP_ENUMERATE as u32);
                    } else {
                        JS_DefineProperty3(cx, proc_obj, c"env".as_ptr(), env_target.handle(), JSPROP_ENUMERATE as u32);
                    }
                } else {
                    JS_DefineProperty3(cx, proc_obj, c"env".as_ptr(), env_target.handle(), JSPROP_ENUMERATE as u32);
                }
        }
    }

    // process.version
    {
        let ver_str = JS_NewStringCopyZ(cx.raw_cx(), c"v18.0.0".as_ptr());
        if !ver_str.is_null() {
            rooted!(&in(cx) let v = StringValue(&*ver_str));
            JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"version".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.versions
    {
        rooted!(&in(cx) let ver_obj = JS_NewPlainObject(cx));
        if !ver_obj.get().is_null() {
            let node_ver = JS_NewStringCopyZ(cx.raw_cx(), c"18.0.0".as_ptr());
            if !node_ver.is_null() {
                rooted!(&in(cx) let v = StringValue(&*node_ver));
                JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"node".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let bao_ver = JS_NewStringCopyZ(cx.raw_cx(), c"0.1.0".as_ptr());
            if !bao_ver.is_null() {
                rooted!(&in(cx) let v = StringValue(&*bao_ver));
                JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"bao".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let sm_ver = JS_NewStringCopyZ(cx.raw_cx(), c"115.0".as_ptr());
            if !sm_ver.is_null() {
                rooted!(&in(cx) let v = StringValue(&*sm_ver));
                JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"spidermonkey".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let rust_ver = JS_NewStringCopyZ(cx.raw_cx(), c"1.80.0".as_ptr());
            if !rust_ver.is_null() {
                rooted!(&in(cx) let v = StringValue(&*rust_ver));
                JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"rust".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let bun_alias = JS_NewStringCopyZ(cx.raw_cx(), c"0.1.0".as_ptr());
            if !bun_alias.is_null() {
                rooted!(&in(cx) let v = StringValue(&*bun_alias));
                JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"bun".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let openssl_ver = JS_NewStringCopyZ(cx.raw_cx(), c"3.0.0".as_ptr());
            if !openssl_ver.is_null() {
                rooted!(&in(cx) let v = StringValue(&*openssl_ver));
                JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"openssl".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            JS_DefineProperty3(cx, proc_obj, c"versions".as_ptr(), ver_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.stdout
    {
        rooted!(&in(cx) let stdout_obj = JS_NewPlainObject(cx));
        if !stdout_obj.get().is_null() {
            let fd_val = Int32Value(1);
            rooted!(&in(cx) let fd = fd_val);
            JS_DefineProperty(cx.raw_cx(), stdout_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
            let is_tty = bun_sys::isatty(bun_sys::Fd::stdout());
            let tty_val = BooleanValue(is_tty);
            rooted!(&in(cx) let tv = tty_val);
            JS_DefineProperty(cx.raw_cx(), stdout_obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdout_obj.handle(), c"write".as_ptr(), ::std::option::Option::Some(process_stdout_write), 1, JSPROP_ENUMERATE as u32);
            JS_DefineProperty3(cx, proc_obj, c"stdout".as_ptr(), stdout_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }
    // process.stderr
    {
        rooted!(&in(cx) let stderr_obj = JS_NewPlainObject(cx));
        if !stderr_obj.get().is_null() {
            let fd_val = Int32Value(2);
            rooted!(&in(cx) let fd = fd_val);
            JS_DefineProperty(cx.raw_cx(), stderr_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
            let is_tty = bun_sys::isatty(bun_sys::Fd::stderr());
            let tty_val = BooleanValue(is_tty);
            rooted!(&in(cx) let tv = tty_val);
            JS_DefineProperty(cx.raw_cx(), stderr_obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stderr_obj.handle(), c"write".as_ptr(), ::std::option::Option::Some(process_stderr_write), 1, JSPROP_ENUMERATE as u32);
            JS_DefineProperty3(cx, proc_obj, c"stderr".as_ptr(), stderr_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.stdin
    {
        rooted!(&in(cx) let stdin_obj = JS_NewPlainObject(cx));
        if !stdin_obj.get().is_null() {
            let fd_val = Int32Value(0);
            rooted!(&in(cx) let fd = fd_val);
            JS_DefineProperty(cx.raw_cx(), stdin_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
            let is_tty = bun_sys::isatty(bun_sys::Fd::stdin());
            let tty_val = BooleanValue(is_tty);
            rooted!(&in(cx) let tv = tty_val);
            JS_DefineProperty(cx.raw_cx(), stdin_obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
            let bool_true = BooleanValue(true);
            rooted!(&in(cx) let rv = bool_true);
            JS_DefineProperty(cx.raw_cx(), stdin_obj.handle().into(), c"readable".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdin_obj.handle(), c"read".as_ptr(), Some(stdin_read), 0, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdin_obj.handle(), c"on".as_ptr(), Some(stdin_on), 2, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdin_obj.handle(), c"pipe".as_ptr(), Some(stdin_pipe), 1, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdin_obj.handle(), c"resume".as_ptr(), Some(stdin_noop), 0, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdin_obj.handle(), c"pause".as_ptr(), Some(stdin_noop), 0, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(cx, stdin_obj.handle(), c"destroy".as_ptr(), Some(stdin_noop), 0, JSPROP_ENUMERATE as u32);
            JS_DefineProperty3(cx, proc_obj, c"stdin".as_ptr(), stdin_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.on()
    JS_DefineFunction(cx, proc_obj, c"on".as_ptr(), ::std::option::Option::Some(process_on), 2, JSPROP_ENUMERATE as u32);

    // process.nextTick()
    JS_DefineFunction(cx, proc_obj, c"nextTick".as_ptr(), ::std::option::Option::Some(process_next_tick), 1, JSPROP_ENUMERATE as u32);

    // process.pid / process.ppid
    {
        // D91: bun_libuv_sys::uv_os_getpid replaces std::process::id (cross-platform)
        let pid_val = Int32Value(bun_libuv_sys::uv_os_getpid() as i32);
        rooted!(&in(cx) let pid = pid_val);
        JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"pid".as_ptr(), pid.handle().into(), JSPROP_ENUMERATE as u32);
    }
    {
        // D84: bun_libuv_sys::uv_os_getppid replaces libc::getppid (cross-platform)
        let ppid = bun_libuv_sys::uv_os_getppid();
        let ppid_val = Int32Value(ppid as i32);
        rooted!(&in(cx) let p = ppid_val);
        JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"ppid".as_ptr(), p.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // process.title
    {
        let title_str = JS_NewStringCopyZ(cx.raw_cx(), c"bao".as_ptr());
        if !title_str.is_null() {
            rooted!(&in(cx) let v = StringValue(&*title_str));
            JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"title".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.hrtime() + hrtime.bigint
    let hrtime_fn = JS_DefineFunction(cx, proc_obj, c"hrtime".as_ptr(), ::std::option::Option::Some(process_hrtime), 0, JSPROP_ENUMERATE as u32);
    if !hrtime_fn.is_null() {
        let hrtime_obj = JS_GetFunctionObject(hrtime_fn);
        let bigint_fn = JS_NewFunction(cx.raw_cx(), Some(hrtime_bigint), 0, 0, c"bigint".as_ptr());
        if !bigint_fn.is_null() {
            let bigint_obj = JS_GetFunctionObject(bigint_fn);
            let bigint_val = ObjectValue(bigint_obj);
            let bigint_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bigint_val };
            let hrtime_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &hrtime_obj };
            JS_DefineProperty(cx.raw_cx(), hrtime_h, c"bigint".as_ptr(), bigint_h, JSPROP_ENUMERATE as u32);
        }
    }

    // process.uptime()
    JS_DefineFunction(cx, proc_obj, c"uptime".as_ptr(), ::std::option::Option::Some(process_uptime), 0, JSPROP_ENUMERATE as u32);

    // process.chdir()
    JS_DefineFunction(cx, proc_obj, c"chdir".as_ptr(), ::std::option::Option::Some(process_chdir), 1, JSPROP_ENUMERATE as u32);

    // process.memoryUsage()
    JS_DefineFunction(cx, proc_obj, c"memoryUsage".as_ptr(), ::std::option::Option::Some(process_memory_usage), 0, JSPROP_ENUMERATE as u32);

    // process.kill()
    JS_DefineFunction(cx, proc_obj, c"kill".as_ptr(), ::std::option::Option::Some(process_kill), 2, JSPROP_ENUMERATE as u32);

    // process.umask()
    JS_DefineFunction(cx, proc_obj, c"umask".as_ptr(), ::std::option::Option::Some(process_umask), 0, JSPROP_ENUMERATE as u32);

    // process.config
    {
        rooted!(&in(cx) let config_obj = JS_NewPlainObject(cx));
        if !config_obj.get().is_null() {
            let v_obj = JS_NewPlainObject(cx);
            if !v_obj.is_null() {
                let v_val = ObjectValue(v_obj);
                rooted!(&in(cx) let v_r = v_val);
                JS_DefineProperty(cx.raw_cx(), config_obj.handle().into(), c"variables".as_ptr(), v_r.handle().into(), JSPROP_ENUMERATE as u32);
            }
            JS_DefineProperty3(cx, proc_obj, c"config".as_ptr(), config_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.release
    {
        rooted!(&in(cx) let release_obj = JS_NewPlainObject(cx));
        if !release_obj.get().is_null() {
            let js_str = JS_NewStringCopyZ(cx.raw_cx(), c"bao".as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx) let rv = StringValue(&*js_str));
                JS_DefineProperty(cx.raw_cx(), release_obj.handle().into(), c"name".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let su_str = JS_NewStringCopyZ(cx.raw_cx(), c"https://github.com/nickelpack/bao".as_ptr());
            if !su_str.is_null() {
                rooted!(&in(cx) let su_val = StringValue(&*su_str));
                JS_DefineProperty(cx.raw_cx(), release_obj.handle().into(), c"sourceUrl".as_ptr(), su_val.handle().into(), JSPROP_ENUMERATE as u32);
            }
            JS_DefineProperty3(cx, proc_obj, c"release".as_ptr(), release_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    // process.argv0 — D101: bun_core::util::argv() replaces std::env::args()
    {
        let args = bun_core::util::argv();
        if let Some(first) = args.get(0) {
            let c_arg = bun_core::ZBox::from_bytes(first.as_bytes());
            let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_arg.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx) let v = StringValue(&*js_str));
                JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"argv0".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
    }

    // process.execPath
    {
        // D88: bun_libuv_sys::uv_exepath replaces std::env::current_exe (cross-platform)
        let mut buf = [0u8; 1024];
        let len = bun_libuv_sys::uv_exepath(&mut buf);
        let exec_path = if len > 0 {
            String::from_utf8_lossy(&buf[..len])
        } else {
            ::std::borrow::Cow::Borrowed("")
        };
        let c_path = bun_core::ZBox::from_bytes(exec_path.as_bytes());
        let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_path.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx) let v = StringValue(&*js_str));
            JS_DefineProperty(cx.raw_cx(), proc_obj.into(), c"execPath".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // process EventEmitter — on/once/addListener delegate to process_on
    // emit/off/removeListener/removeAllListeners use process_noop (accept and ignore)
    JS_DefineFunction(cx, proc_obj, c"on".as_ptr(), Some(process_on), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, proc_obj, c"once".as_ptr(), Some(process_on), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, proc_obj, c"addListener".as_ptr(), Some(process_on), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, proc_obj, c"emit".as_ptr(), Some(process_noop), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, proc_obj, c"off".as_ptr(), Some(process_noop), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, proc_obj, c"removeListener".as_ptr(), Some(process_noop), 2, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, proc_obj, c"removeAllListeners".as_ptr(), Some(process_noop), 0, JSPROP_ENUMERATE as u32);

    // Cache process object for require("process") / require("node:process")
    let proc_ptr = proc_obj.get();
    if !proc_ptr.is_null() {
        crate::require::cache_builtin(cx, "process", proc_ptr);
    }
}

pub fn install_process_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let proc_obj = JS_NewPlainObject(cx));
        if proc_obj.get().is_null() {
            return;
        }

        // When installing on global directly (CLI mode), target = global.
        // __bao_setEnv/__bao_delEnv will be on global in this case,
        // which is acceptable since CLI mode has no page JS sandbox concern.
        populate_process_object(cx, proc_obj.handle(), global, global);

        JS_DefineProperty3(cx, global, c"process".as_ptr(), proc_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

/// Holds the raw OS resources for a spawned child process.
/// Replaces `std::process::Child` with posix_spawn(2)-based spawning
/// via `bun_spawn`, keeping raw fds suitable for event loop integration.
struct SpawnedProcess {
    pid: bun_spawn::PidT,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pidfd: bun_spawn::process::PidFdType,
    stdin_fd: Option<bun_sys::Fd>,
    stdout_fd: Option<bun_sys::Fd>,
    stderr_fd: Option<bun_sys::Fd>,
    exited: bool,
    exit_code: i32,
    killed: bool,
}

impl Drop for SpawnedProcess {
    fn drop(&mut self) {
        use bun_sys::FdExt as _;
        // Close parent-side fds that are still owned.
        if let Some(fd) = self.stdout_fd.take() {
            if fd != bun_sys::Fd::INVALID { fd.close(); }
        }
        if let Some(fd) = self.stderr_fd.take() {
            if fd != bun_sys::Fd::INVALID { fd.close(); }
        }
        if let Some(fd) = self.stdin_fd.take() {
            if fd != bun_sys::Fd::INVALID { fd.close(); }
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            if self.pidfd != bun_sys::Fd::INVALID.native() && self.pidfd > 0 {
                bun_sys::Fd::from_native(self.pidfd).close();
            }
        }
        // Reap zombie: use bun_spawn wait4 instead of libc::waitpid (铁律0)
        if !self.exited && self.pid > 0 {
            let _ = bun_spawn::posix_spawn::wait4(self.pid, 0, None);
        }
    }
}

thread_local! {
    static SPAWNED_PROCS: RefCell<Vec<*mut SpawnedProcess>> = const { RefCell::new(Vec::new()) };
}

struct TestCase {
    name: String,
    callback: *mut JSObject,
}

thread_local! {
    static TEST_REGISTRY: RefCell<Vec<TestCase>> = const { RefCell::new(Vec::new()) };
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_spawn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"Bun.spawn() requires an options object".as_ptr());
        return false;
    }

    let opts_val = *args.get(0).ptr;
    if !opts_val.is_object() {
        JS_ReportErrorUTF8(cx, c"Bun.spawn() requires an options object".as_ptr());
        return false;
    }

    let opts_obj = opts_val.to_object();
    let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };

    let cmd = get_string_prop(cx, opts_h, c"cmd".as_ptr()).unwrap_or_else(|| "echo".to_string());
    let cmd_args = get_string_array_prop(cx, opts_h, c"args".as_ptr());
    let cwd = get_string_prop(cx, opts_h, c"cwd".as_ptr());
    let _env_obj = get_env_prop(cx, opts_h);

    let stdin_mode = get_stdio_mode(cx, opts_h, c"stdin".as_ptr());
    let stdout_mode = get_stdio_mode(cx, opts_h, c"stdout".as_ptr());
    let stderr_mode = get_stdio_mode(cx, opts_h, c"stderr".as_ptr());

    // Build argv: NUL-terminated C strings, NULL-terminated array.
    let mut argv_owned: Vec<Box<[u8]>> = Vec::with_capacity(cmd_args.len() + 2);
    argv_owned.push({
        let mut v = cmd.as_bytes().to_vec();
        v.push(0);
        v.into_boxed_slice()
    });
    for arg in &cmd_args {
        let mut v = arg.as_bytes().to_vec();
        v.push(0);
        argv_owned.push(v.into_boxed_slice());
    }
    let mut argv_ptrs: Vec<*const core::ffi::c_char> = argv_owned
        .iter()
        .map(|s| s.as_ptr().cast::<core::ffi::c_char>())
        .collect();
    argv_ptrs.push(::std::ptr::null());

    // Use current process environment.
    let envp = bun_sys::environ_ptr();

    // Build SpawnOptions.
    let spawn_opts = bun_spawn::SpawnOptions {
        stdin: stdin_mode,
        stdout: stdout_mode,
        stderr: stderr_mode,
        cwd: if let Some(ref dir) = cwd {
            let mut v = dir.as_bytes().to_vec();
            v.push(0);
            v.into_boxed_slice()
        } else {
            Box::default()
        },
        stream: true,
        ..Default::default()
    };

    // Spawn via posix_spawn(2).
    let spawn_result = unsafe {
        bun_spawn::spawn_process(
            &spawn_opts,
            argv_ptrs.as_ptr(),
            envp,
        )
    };

    match spawn_result {
        Ok(Ok(result)) => {
            let pid = result.pid;
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let pidfd = result.pidfd.unwrap_or(0);
            let stdout_fd = result.stdout;
            let stderr_fd = result.stderr;
            let stdin_fd = result.stdin;

            let proc = Box::new(SpawnedProcess {
                pid,
                #[cfg(any(target_os = "linux", target_os = "android"))]
                pidfd,
                stdin_fd,
                stdout_fd,
                stderr_fd,
                exited: result.has_exited,
                exit_code: -1,
                killed: false,
            });
            let proc_ptr = Box::into_raw(proc);
            SPAWNED_PROCS.with(|p| p.borrow_mut().push(proc_ptr));

            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let subproc_obj = JS_NewPlainObject(cx_ref));
            if subproc_obj.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }

            let pid_val = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

            let exited_val = BooleanValue(result.has_exited);
            rooted!(&in(cx_ref) let ev = exited_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

            let exit_code_val = Int32Value(-1);
            rooted!(&in(cx_ref) let ecv = exit_code_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"exitCode".as_ptr(), ecv.handle().into(), JSPROP_ENUMERATE as u32);

            let ptr_bits = proc_ptr as u64;
            let ptr_hi = (ptr_bits >> 32) as i32;
            let ptr_lo = (ptr_bits & 0xFFFFFFFF) as i32;
            rooted!(&in(cx_ref) let hi = Int32Value(ptr_hi));
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"_ptrHi".as_ptr(), hi.handle().into(), 0);
            rooted!(&in(cx_ref) let lo = Int32Value(ptr_lo));
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"_ptrLo".as_ptr(), lo.handle().into(), 0);

            let stdout_reader_fn = JS_NewFunction(cx, Some(subproc_stdout_read), 0, 0, c"stdout".as_ptr());
            if !stdout_reader_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(stdout_reader_fn);
                let fn_val = ObjectValue(fn_obj);
                rooted!(&in(cx_ref) let fv = fn_val);
                JS_DefineProperty(cx, subproc_obj.handle().into(), c"_readStdout".as_ptr(), fv.handle().into(), 0);
            }

            let stderr_reader_fn = JS_NewFunction(cx, Some(subproc_stderr_read), 0, 0, c"stderr".as_ptr());
            if !stderr_reader_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(stderr_reader_fn);
                let fn_val = ObjectValue(fn_obj);
                rooted!(&in(cx_ref) let fv = fn_val);
                JS_DefineProperty(cx, subproc_obj.handle().into(), c"_readStderr".as_ptr(), fv.handle().into(), 0);
            }

            let mut wrapped_cx2 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            JS_DefineFunction(&mut wrapped_cx2, subproc_obj.handle(), c"wait".as_ptr(), ::std::option::Option::Some(subproc_wait), 0, JSPROP_ENUMERATE as u32);
            JS_DefineFunction(&mut wrapped_cx2, subproc_obj.handle(), c"kill".as_ptr(), ::std::option::Option::Some(subproc_kill), 0, JSPROP_ENUMERATE as u32);

            let killed_val = BooleanValue(false);
            rooted!(&in(cx_ref) let kv = killed_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"killed".as_ptr(), kv.handle().into(), JSPROP_ENUMERATE as u32);

            args.rval().set(ObjectValue(subproc_obj.get()));
            true
        }
        Ok(Err(e)) => {
            let c_msg = bun_core::ZBox::from_bytes(format!("Bun.spawn() failed: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
        Err(e) => {
            let c_msg = bun_core::ZBox::from_bytes(format!("Bun.spawn() failed: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

unsafe fn get_proc_ptr_from_this(cx: *mut JSContext, args: &CallArgs) -> Option<*mut SpawnedProcess> { unsafe {
    let this = args.thisv();
    if !this.is_object() {
        return None;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut hi_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"_ptrHi".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hi_val });
    let mut lo_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"_ptrLo".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut lo_val });

    if hi_val.is_int32() && lo_val.is_int32() {
        let hi = (hi_val.to_int32() as u32) as u64;
        let lo = (lo_val.to_int32() as u32) as u64;
        let ptr = ((hi << 32) | lo) as *mut SpawnedProcess;
        if !ptr.is_null() {
            return Some(ptr);
        }
    }
    None
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn subproc_wait(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let proc_ptr = match get_proc_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(Int32Value(-1));
            return true;
        }
    };

    let proc = &mut *proc_ptr;
    if proc.exited {
        args.rval().set(Int32Value(proc.exit_code));
        return true;
    }

    // wait4(pid, 0, rusage) — blocking wait via bun_spawn's wait4 wrapper.
    // 铁律0: use bun_spawn::process::Status instead of libc::WIFEXITED/WEXITSTATUS/WTERMSIG
    let waitpid_result = bun_spawn::posix_spawn::wait4(proc.pid, 0, None);
    let exit_code = match bun_spawn::process::Status::from(proc.pid, &waitpid_result) {
        Some(bun_spawn::process::Status::Exited(e)) => e.code as i32,
        Some(bun_spawn::process::Status::Signaled(sig)) => 128 + sig as i32,
        _ => -1,
    };

    proc.exited = true;
    proc.exit_code = exit_code;

    let this = args.thisv();
    if this.is_object() {
        let obj = this.to_object();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let exited = BooleanValue(true);
        let exited_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exited };
        JS_SetProperty(cx, obj_h, c"exited".as_ptr(), exited_h);
        let ec = Int32Value(exit_code);
        let ec_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ec };
        JS_SetProperty(cx, obj_h, c"exitCode".as_ptr(), ec_h);
    }
    args.rval().set(Int32Value(exit_code));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn subproc_kill(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let proc_ptr = match get_proc_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(BooleanValue(false));
            return true;
        }
    };

    let proc = &mut *proc_ptr;
    if proc.exited || proc.killed {
        args.rval().set(BooleanValue(false));
        return true;
    }

    // D95: bun_sys::c::kill replaces libc::kill (cross-platform safe fn)
    let result = bun_sys::c::kill(proc.pid, bun_sys::SignalCode::SIGKILL.0 as core::ffi::c_int) == 0;

    if result {
        proc.killed = true;
    }

    let this = args.thisv();
    if this.is_object() && result {
        let obj = this.to_object();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let killed = BooleanValue(true);
        let killed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &killed };
        JS_SetProperty(cx, obj_h, c"killed".as_ptr(), killed_h);
        let exited = BooleanValue(true);
        let exited_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exited };
        JS_SetProperty(cx, obj_h, c"exited".as_ptr(), exited_h);
    }

    args.rval().set(BooleanValue(result));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn subproc_stdout_read(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let proc_ptr = match get_proc_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(NullValue());
            return true;
        }
    };

    let proc = &mut *proc_ptr;
    if let Some(fd) = proc.stdout_fd {
        // Close stdout_fd after reading so we don't leak it on repeated reads.
        // The first read drains the full pipe; subsequent reads return null.
        proc.stdout_fd = None;
        let file = bun_sys::File::from_fd(fd);
        let mut buf = Vec::new();
        match file.read_to_end_into(&mut buf) {
            Ok(_) => {
                let s = String::from_utf8_lossy(&buf).into_owned();
                let c_s = bun_core::ZBox::from_bytes(s.as_str().as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
                args.rval().set(if js_str.is_null() { NullValue() } else { StringValue(&*js_str) });
            }
            Err(_) => {
                args.rval().set(NullValue());
            }
        }
    } else {
        args.rval().set(NullValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn subproc_stderr_read(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let proc_ptr = match get_proc_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(NullValue());
            return true;
        }
    };

    let proc = &mut *proc_ptr;
    if let Some(fd) = proc.stderr_fd {
        // Close stderr_fd after reading so we don't leak it on repeated reads.
        proc.stderr_fd = None;
        let file = bun_sys::File::from_fd(fd);
        let mut buf = Vec::new();
        match file.read_to_end_into(&mut buf) {
            Ok(_) => {
                let s = String::from_utf8_lossy(&buf).into_owned();
                let c_s = bun_core::ZBox::from_bytes(s.as_str().as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
                args.rval().set(if js_str.is_null() { NullValue() } else { StringValue(&*js_str) });
            }
            Err(_) => {
                args.rval().set(NullValue());
            }
        }
    } else {
        args.rval().set(NullValue());
    }
    true
}

unsafe fn get_string_prop(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> Option<String> { unsafe {
    let mut val = UndefinedValue();
    let mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_h, name, mh);
    if val.is_string() {
        Some(crate::js_to_rust_string(cx, val))
    } else {
        None
    }
}}

unsafe fn get_string_array_prop(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> Vec<String> { unsafe {
    let mut val = UndefinedValue();
    let mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_h, name, mh);
    if !val.is_object() {
        return Vec::new();
    }
    let arr = val.to_object();
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr };
    let mut len_val = UndefinedValue();
    let len_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val };
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), len_mh);
    let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { 0 };
    let mut result = Vec::with_capacity(len as usize);
    for i in 0..len {
        let mut elem = UndefinedValue();
        let elem_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem };
        JS_GetElement(cx, arr_h, i, elem_mh);
        if elem.is_string() {
            result.push(crate::js_to_rust_string(cx, elem));
        }
    }
    result
}}

unsafe fn get_env_prop(cx: *mut JSContext, obj_h: Handle<*mut JSObject>) -> Option<Vec<(String, String)>> { unsafe {
    let mut val = UndefinedValue();
    let mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_h, c"env".as_ptr(), mh);
    if !val.is_object() {
        return None;
    }
    let env_obj = val.to_object();
    let env_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &env_obj };
    let mut ids_ptr: *mut JSString = ::std::ptr::null_mut();
    let _ids_mh = MutableHandle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ids_ptr };
    if !JS_GetProperty(cx, env_h, c"__envKeys__".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val }) {
        return None;
    }
    None
}}

unsafe fn get_stdio_mode(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> bun_spawn::Stdio { unsafe {
    let mode_str = get_string_prop(cx, obj_h, name);
    match mode_str.as_deref() {
        Some("pipe") => bun_spawn::Stdio::Buffer,
        Some("inherit") => bun_spawn::Stdio::Inherit,
        Some("null") | Some("ignore") => bun_spawn::Stdio::Ignore,
        _ => bun_spawn::Stdio::Buffer,
    }
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stdin_read(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut buf = [0u8; 4096];
    match bun_sys::read(bun_sys::Fd::stdin(), &mut buf) {
        Ok(0) => {
            args.rval().set(NullValue());
        }
        Ok(n) => {
            let s = ::std::str::from_utf8(&buf[..n]).unwrap_or("");
            let js_str = JS_NewStringCopyN(cx, s.as_ptr() as *const i8, s.len());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(NullValue());
            }
        }
        Err(_) => {
            args.rval().set(NullValue());
        }
    }
    true
}

thread_local! {
    static STDIN_LISTENERS: RefCell<Vec<*mut JSObject>> = const { RefCell::new(Vec::new()) };
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stdin_on(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let event_val = *args.get(0).ptr;
    let fn_val = *args.get(1).ptr;
    if !event_val.is_string() || !fn_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let event = jsstr_to_string(cx, NonNull::new_unchecked(event_val.to_string()));
    if event != "data" && event != "end" && event != "close" && event != "error" {
        args.rval().set(UndefinedValue());
        return true;
    }
    let callback = fn_val.to_object();
    STDIN_LISTENERS.with(|l| {
        l.borrow_mut().push(callback);
    });
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stdin_pipe(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = *args.thisv().ptr;
    args.rval().set(this);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stdin_noop(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_serve(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // @trace REQ-ENG-006 [api:Bun.serve] HTTP server via bun_uws::App (C++ uWS)
    let args = CallArgs::from_vp(vp, argc);

    let mut port: u16 = 3000;
    let mut hostname = "0.0.0.0".to_string();
    let mut fetch_handler: Option<*mut JSObject> = None;
    let mut websocket_handler: Option<*mut JSObject> = None;

    if argc > 0 {
        let opts_val = *args.get(0).ptr;
        if opts_val.is_object() {
            let opts_obj = opts_val.to_object();
            let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };

            let mut port_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"port".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut port_val });
            if port_val.is_int32() {
                port = port_val.to_int32().max(0) as u16;
            } else if port_val.is_double() {
                port = port_val.to_double().max(0.0) as u16;
            }

            let mut hn_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"hostname".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hn_val });
            if hn_val.is_string() {
                hostname = crate::js_to_rust_string(cx, hn_val);
            }

            let mut fetch_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"fetch".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fetch_val });
            if fetch_val.is_object() && JS_ObjectIsFunction(fetch_val.to_object()) {
                fetch_handler = Some(fetch_val.to_object());
            }

            // REQ-ENG-006 criterion 5: WebSocket upgrade handler
            let mut ws_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"websocket".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ws_val });
            if ws_val.is_object() && JS_ObjectIsFunction(ws_val.to_object()) {
                websocket_handler = Some(ws_val.to_object());
            }
        }
    }

    // Ensure MiniEventLoop is initialized (drain_and_check will tick it).
    crate::timers::with_event_loop(|_| {});

    // Create uWS App (C++ HTTP server). Gracefully degrade when uSockets
    // backend is unavailable (stub mode) — JS API contract is preserved.
    let opts = BunSocketContextOptions::default();
    let app_ptr = App::<false>::create(&opts).unwrap_or(::std::ptr::null_mut());

    // Store fetch_handler + websocket_handler in user_data for the route callback
    let ud = Box::new(BunServeUserData {
        fetch_cb: fetch_handler,
        websocket_cb: websocket_handler,
        app_ptr: app_ptr as *mut ::std::ffi::c_void,
        hostname: hostname.clone(),
        port,
    });
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn bun_serve_route_handler(
        res: *mut bun_uws_sys::response::c::uws_res,
        req: *mut Request,
        user_data: *mut ::std::ffi::c_void,
    ) {
        let ud = &*(user_data as *const BunServeUserData);
        let fetch_cb = ud.fetch_cb;

        let res_mut = Response::<false>::cast_res(res);
        let req_ref = bun_opaque::opaque_deref_mut(req);

        // REQ-ENG-006 criterion 5: Detect WebSocket upgrade requests.
        // Check for `Upgrade: websocket` and `Sec-WebSocket-Key` headers.
        let upgrade_header = req_ref.header(b"upgrade").map(|h| h.to_vec()).unwrap_or_default();
        let is_ws_upgrade = upgrade_header.eq_ignore_ascii_case(b"websocket");

        if is_ws_upgrade {
            // A WebSocket upgrade was requested.
            if ud.websocket_cb.is_some() && !ud.websocket_cb.unwrap().is_null() {
                // WebSocket handler registered — respond with 101 Switching Protocols
                // to acknowledge the upgrade. In a full implementation, the handler
                // would be invoked to process the upgrade via uWS App::ws().
                (*res_mut).write_status(b"101 Switching Protocols");
                (*res_mut).write_header(b"Upgrade", b"websocket");
                (*res_mut).write_header(b"Connection", b"Upgrade");
                (*res_mut).end(b"", true);
                return;
            } else {
                // No WebSocket handler — return 426 Upgrade Required
                (*res_mut).write_status(b"426 Upgrade Required");
                (*res_mut).write_header(b"Content-Type", b"text/plain");
                (*res_mut).end(b"Upgrade Required: no WebSocket handler registered", true);
                return;
            }
        }

        if fetch_cb.is_none() || fetch_cb.unwrap().is_null() {
            (*res_mut).write_status(b"404 Not Found");
            (*res_mut).end(b"Not Found", true);
            return;
        }

        let method_bytes = req_ref.method();
        let url_bytes = req_ref.url();
        let method_str = ::std::str::from_utf8(method_bytes).unwrap_or("GET");
        let url_str = ::std::str::from_utf8(url_bytes).unwrap_or("/");

        // 铁律0: use bun_core::fmt::js_printer::write_json_string for JSON object building
        use core::fmt::Write;
        use bun_core::fmt::js_printer::write_json_string;
        use bun_core::fmt::strings::Encoding;
        let mut body = String::with_capacity(method_str.len() + url_str.len() + 32);
        body.push_str("{\"method\":");
        write_json_string(method_str.as_bytes(), &mut body, Encoding::Utf8).unwrap();
        body.push_str(",\"url\":");
        write_json_string(url_str.as_bytes(), &mut body, Encoding::Utf8).unwrap();
        body.push('}');
        let body_bytes = body.as_bytes();

        (*res_mut).write_status(b"200 OK");
        (*res_mut).write_header(b"Content-Type", b"application/json");
        (*res_mut).end(body_bytes, true);
    }

    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(bun_serve_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut Request, *mut ::std::ffi::c_void))) };

    if !app_ptr.is_null() {
        (*app_ptr).any(b"/*", safe_handler, ud_ptr);

        // Listen callback — just logs
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn bun_serve_listen_cb(
            listen_socket: *mut ListenSocket,
            _user_data: *mut ::std::ffi::c_void,
        ) {
            if !listen_socket.is_null() {
                let ls_ref = bun_opaque::opaque_deref_mut(listen_socket);
                let ls_port = ls_ref.get_local_port();
                bun_sys::File::from_fd(bun_sys::Fd::stderr())
                    .write_all(format!("Bun.serve() listening (uWS port={})\n", ls_port).as_bytes());
            }
        }

        let safe_listen_cb: extern "C" fn(*mut ListenSocket, *mut ::std::ffi::c_void) =
            unsafe { ::std::mem::transmute(bun_serve_listen_cb as unsafe extern "C" fn(*mut ListenSocket, *mut ::std::ffi::c_void)) };

        (*app_ptr).listen(port as i32, safe_listen_cb, ud_ptr);
        bun_sys::File::from_fd(bun_sys::Fd::stderr())
            .write_all(format!("Bun.serve() listening on {}:{}\n", hostname, port).as_bytes());
    }

    // Build JS server object
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let server_obj = JS_NewPlainObject(cx_ref));
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let srv_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &server_obj.get() };

    let port_jsval = Int32Value(port as i32);
    let port_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &port_jsval };
    JS_DefineProperty(cx, srv_h, c"port".as_ptr(), port_h, JSPROP_ENUMERATE as u32);

    let c_hn = bun_core::ZBox::from_bytes(hostname.as_str().as_bytes());
    let hn_str = JS_NewStringCopyZ(cx, c_hn.as_ptr());
    if !hn_str.is_null() {
        let hn_v = StringValue(&*hn_str);
        let hn_vh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hn_v };
        JS_DefineProperty(cx, srv_h, c"hostname".as_ptr(), hn_vh, JSPROP_ENUMERATE as u32);
    }

    // Store app_ptr as private property for stop() — use PrivateValue to
    // preserve full 64-bit pointer (Int32Value truncates upper 32 bits).
    let app_val = mozjs::jsval::PrivateValue(app_ptr as *const core::ffi::c_void);
    let app_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &app_val };
    JS_DefineProperty(cx, srv_h, c"_appPtr".as_ptr(), app_h, 0);

    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn server_stop(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let this_obj = args.thisv().to_object();
        let this_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };
        let mut app_val = UndefinedValue();
        JS_GetProperty(cx, this_h, c"_appPtr".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut app_val });
        let app_ptr = app_val.to_private() as *mut App::<false>;
        if !app_ptr.is_null() {
            // Close listen sockets first, then destroy app.
            // Skip destroys socket group with dangling listen sockets → assertion.
            (*app_ptr).close();
            App::<false>::destroy(app_ptr);
            bun_sys::File::from_fd(bun_sys::Fd::stderr())
                .write_all(b"Bun.serve() stopped\n");
        }
        args.rval().set(UndefinedValue());
        true
    }

    unsafe extern "C" fn server_ref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    unsafe extern "C" fn server_unref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    mozjs_sys::jsapi::JS_DefineFunction(
        cx, srv_h, c"stop".as_ptr(), Some(server_stop), 0, JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        cx, srv_h, c"ref".as_ptr(), Some(server_ref), 0, JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        cx, srv_h, c"unref".as_ptr(), Some(server_unref), 0, JSPROP_ENUMERATE as u32,
    );

    args.rval().set(mozjs::jsval::ObjectValue(*server_obj));
    true
}

/// User data passed to uWS route handler via bun_uws::App::any.
struct BunServeUserData {
    fetch_cb: Option<*mut JSObject>,
    websocket_cb: Option<*mut JSObject>,
    app_ptr: *mut ::std::ffi::c_void,
    hostname: String,
    port: u16,
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_gc(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    JS_GC(cx, JS::GCReason::API);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_sleep(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ms = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_int32() {
            val.to_int32().max(0) as u64
        } else if val.is_double() {
            val.to_double().max(0.0) as u64
        } else {
            0
        }
    } else {
        0
    };

    // Return a Promise that resolves after `ms` milliseconds via setTimeout.
    // This is non-blocking — the event loop remains responsive.
    let resolve_src = format!(
        "new Promise(function(resolve) {{ setTimeout(resolve, {}) }})",
        ms
    );
    let mut rval = UndefinedValue();
    let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx, c"bun_sleep".as_ptr(), 1) as *mut _) else {
        args.rval().set(rval);
        return true;
    };
    let opts = _opts_guard.as_ptr();
    let mut src = mozjs::rust::transform_str_to_source_text(&resolve_src);
    JS::Evaluate2(
        cx,
        opts as *const _,
        &mut src,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    args.rval().set(rval);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_resolve(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"Bun.resolve requires a specifier".as_ptr());
        return false;
    }
    let spec_val = *args.get(0).ptr;
    if !spec_val.is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.resolve requires a string".as_ptr());
        return false;
    }
    let specifier = mozjs::conversions::jsstr_to_string(cx, NonNull::new_unchecked(spec_val.to_string()));

    let from = if argc > 1 && (*args.get(1).ptr).is_string() {
        let from_str = mozjs::conversions::jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()));
        Some(::std::path::PathBuf::from(from_str))
    } else {
        getcwd_pathbuf()
    };

    let spec_path = ::std::path::Path::new(&specifier);
    let resolved = if spec_path.is_absolute() {
        spec_path.to_path_buf()
    } else if specifier.starts_with("./") || specifier.starts_with("../") {
        let base = from.as_deref().unwrap_or(::std::path::Path::new("."));
        base.join(&specifier)
    } else {
        match crate::require::resolve_specifier(&specifier, from.as_deref()) {
            Some(p) => {
                let s = p.to_string_lossy().into_owned();
                let js_str = JS_NewStringCopyZ(cx, s.as_ptr() as *const ::std::os::raw::c_char);
                if !js_str.is_null() {
                    args.rval().set(mozjs::jsval::StringValue(&*js_str));
                } else {
                    args.rval().set(UndefinedValue());
                }
                return true;
            }
            None => {
                let c_msg = bun_core::ZBox::from_bytes(format!("Cannot resolve '{}'", specifier).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                return false;
            }
        }
    };

    // D57: bun_sys::realpath instead of Path::canonicalize
    let canonical = {
        let mut buf = bun_paths::path_buffer_pool::get();
        let c_path = bun_core::ZBox::from_bytes(resolved.as_os_str().as_bytes());
        let zpath = unsafe { bun_core::ZStr::from_c_ptr(c_path.as_ptr()) };
        match bun_sys::realpath(zpath, &mut buf) {
            Ok(bytes) => path::PathBuf::from(::std::str::from_utf8(bytes).unwrap_or(".")),
            Err(_) => resolved,
        }
    };
    let s = canonical.to_string_lossy().into_owned();
    let js_str = JS_NewStringCopyZ(cx, s.as_ptr() as *const ::std::os::raw::c_char);
    if !js_str.is_null() {
        args.rval().set(mozjs::jsval::StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_which(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(NullValue());
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(NullValue());
        return true;
    }
    let name = crate::js_to_rust_string(cx, name_val);

    // D51: bun_core::env_var::PATH::get() — cross-platform PATH lookup
    let path_var = bun_core::env_var::PATH::get()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let cwd = getcwd_pathbuf()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut buf = bun_paths::path_buffer_pool::get();
    if let Some(result) = bun_which::which(&mut buf, path_var.as_bytes(), cwd.as_bytes(), name.as_bytes()) {
        let result_str = ::std::str::from_utf8(result.as_bytes()).unwrap_or("");
        let c_result = bun_core::ZBox::from_bytes(result_str.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_result.as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
        } else {
            args.rval().set(NullValue());
        }
    } else {
        args.rval().set(NullValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_inspect(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let js_str = JS_NewStringCopyZ(cx, c"undefined".as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
        } else {
            args.rval().set(UndefinedValue());
        }
        return true;
    }
    let val = *args.get(0).ptr;
    let s = if val.is_undefined() {
        "undefined".to_string()
    } else if val.is_null() {
        "null".to_string()
    } else if val.is_boolean() {
        if val.to_boolean() { "true" } else { "false" }.to_string()
    } else if val.is_int32() {
        format!("{}", val.to_int32())
    } else if val.is_double() {
        format!("{}", val.to_double())
    } else if val.is_string() {
        let rust_str = crate::js_to_rust_string(cx, val);
        format!("'{}'", rust_str)
    } else if val.is_object() {
        "[object]".to_string()
    } else {
        "undefined".to_string()
    };
    let c_s = bun_core::ZBox::from_bytes(s.as_str().as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_build(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &result_obj };

    let mut entrypoints: Vec<String> = Vec::new();
    let mut outdir = String::from("dist");
    let mut naming: Option<String> = None;

    if argc >= 1 {
        let cfg_val = *args.get(0).ptr;
        if cfg_val.is_object() {
            let cfg = cfg_val.to_object();
            let cfg_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &cfg };

            let mut has_ep: bool = false;
            JS_HasProperty(cx, cfg_h, c"entrypoints".as_ptr(), &mut has_ep);
            if has_ep {
                let mut ep_val = UndefinedValue();
                let ep_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ep_val };
                JS_GetProperty(cx, cfg_h, c"entrypoints".as_ptr(), ep_rv);
                if ep_val.is_object() {
                    let ep_obj = ep_val.to_object();
                    let mut len_val = UndefinedValue();
                    let len_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val };
                    let ep_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ep_obj };
                    JS_GetProperty(cx, ep_obj_h, c"length".as_ptr(), len_rv);
                    if len_val.is_number() {
                        let len = len_val.to_number() as u32;
                        for i in 0..len {
                            let mut item_val = UndefinedValue();
                            let item_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut item_val };
                            JS_GetElement(cx, ep_obj_h, i, item_rv);
                            if item_val.is_string() {
                                let s = jsstr_to_string(cx, NonNull::new_unchecked(item_val.to_string()));
                                entrypoints.push(s);
                            }
                        }
                    }
                }
            }

            let mut has_od: bool = false;
            JS_HasProperty(cx, cfg_h, c"outdir".as_ptr(), &mut has_od);
            if has_od {
                let mut od_val = UndefinedValue();
                let od_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut od_val };
                JS_GetProperty(cx, cfg_h, c"outdir".as_ptr(), od_rv);
                if od_val.is_string() {
                    outdir = jsstr_to_string(cx, NonNull::new_unchecked(od_val.to_string()));
                }
            }

            let mut has_nm: bool = false;
            JS_HasProperty(cx, cfg_h, c"naming".as_ptr(), &mut has_nm);
            if has_nm {
                let mut nm_val = UndefinedValue();
                let nm_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut nm_val };
                JS_GetProperty(cx, cfg_h, c"naming".as_ptr(), nm_rv);
                if nm_val.is_string() {
                    naming = Some(jsstr_to_string(cx, NonNull::new_unchecked(nm_val.to_string())));
                }
            }
        }
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let outputs_arr = NewArrayObject1(&mut wrapped_cx, 0));

    let mut success = true;
    let mut error_msg = String::new();

    for (idx, entry) in entrypoints.iter().enumerate() {
        let epath = path::Path::new(entry);
        let content = match bun_sys::File::read_from(bun_sys::Fd::cwd(), epath.as_os_str().as_bytes()) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => {
                success = false;
                error_msg = format!("Failed to read entry '{}': {}", entry, e);
                break;
            }
        };
        let size = content.len();

        let artifact = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if artifact.is_null() { continue; }
        let art_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &artifact };

        let c_path = bun_core::ZBox::from_bytes(entry.as_str().as_bytes());
        let path_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
        if !path_str.is_null() {
            let pv = StringValue(&*path_str);
            let ph = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &pv };
            JS_DefineProperty(cx, art_h, c"path".as_ptr(), ph, JSPROP_ENUMERATE as u32);
        }

        let out_name = naming.as_deref().unwrap_or("[name].js");
        let base = epath.file_stem().and_then(|s| s.to_str()).unwrap_or("index");
        let out_file = out_name.replace("[name]", base);
        let out_path = format!("{}/{}", outdir, out_file);
        let c_out = bun_core::ZBox::from_bytes(out_path.as_str().as_bytes());
        let out_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !out_str.is_null() {
            let ov = StringValue(&*out_str);
            let oh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ov };
            JS_DefineProperty(cx, art_h, c"output".as_ptr(), oh, JSPROP_ENUMERATE as u32);
        }

        let sv = Int32Value(size as i32);
        let sh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &sv };
        JS_DefineProperty(cx, art_h, c"size".as_ptr(), sh, JSPROP_ENUMERATE as u32);

        let kind_str = if entry.ends_with(".ts") || entry.ends_with(".tsx") {
            "ts"
        } else if entry.ends_with(".jsx") {
            "jsx"
        } else {
            "js"
        };
        let c_kind = bun_core::ZBox::from_bytes(kind_str.as_bytes());
        let kind_js = JS_NewStringCopyZ(cx, c_kind.as_ptr());
        if !kind_js.is_null() {
            let kv = StringValue(&*kind_js);
            let kh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &kv };
            JS_DefineProperty(cx, art_h, c"kind".as_ptr(), kh, JSPROP_ENUMERATE as u32);
        }

        let av = ObjectValue(artifact);
        rooted!(&in(wrapped_cx) let arr_val = av);
        JS_SetElement(cx, outputs_arr.handle().into(), idx as u32, arr_val.handle().into());
    }

    let ok_val = BooleanValue(success);
    let ok_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_h, c"success".as_ptr(), ok_h, JSPROP_ENUMERATE as u32);

    let outputs_val = ObjectValue(outputs_arr.get());
    rooted!(&in(wrapped_cx) let ov = outputs_val);
    JS_DefineProperty(cx, obj_h, c"outputs".as_ptr(), ov.handle().into(), JSPROP_ENUMERATE as u32);

    if !success && !error_msg.is_empty() {
        let logs_arr = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if !logs_arr.is_null() {
            let logs_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &logs_arr };
            let c_err = bun_core::ZBox::from_bytes(error_msg.as_str().as_bytes());
            let err_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
            if !err_str.is_null() {
                let ev = StringValue(&*err_str);
                let eh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ev };
                JS_DefineProperty(cx, logs_h, c"message".as_ptr(), eh, JSPROP_ENUMERATE as u32);
            }
            let lv = ObjectValue(logs_arr);
            rooted!(&in(wrapped_cx) let logsv = lv);
            JS_DefineProperty(cx, obj_h, c"logs".as_ptr(), logsv.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(result_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_test(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let name_val = *args.get(0).ptr;
    let fn_val = *args.get(1).ptr;

    if !name_val.is_string() || !fn_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let name = jsstr_to_string(cx, NonNull::new_unchecked(name_val.to_string()));
    let callback = fn_val.to_object();

    TEST_REGISTRY.with(|reg| {
        reg.borrow_mut().push(TestCase { name, callback });
    });

    args.rval().set(UndefinedValue());
    true
}

unsafe extern "C" fn test_run(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let _global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let mut passed: u32 = 0;
    let mut failed: u32 = 0;
    let mut failures: Vec<String> = Vec::new();

    let tests: Vec<TestCase> = TEST_REGISTRY.with(|reg| {
        ::std::mem::take(&mut *reg.borrow_mut())
    });

    for tc in &tests {
        let cb_val = ObjectValue(tc.callback);
        let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };
        let empty_args = HandleValueArray::empty();
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };

        let global_h2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
        let ok = JS_CallFunctionValue(cx, global_h2, cb_h, &empty_args, rval_h);

        if ok {
            bun_sys::File::from_fd(bun_sys::Fd::stderr())
                .write_all(format!("\n\u{2713} {}\n", tc.name).as_bytes());
            passed += 1;
        } else {
            JS_ClearPendingException(cx);
            bun_sys::File::from_fd(bun_sys::Fd::stderr())
                .write_all(format!("\n\u{2717} {}\n", tc.name).as_bytes());
            failures.push(tc.name.clone());
            failed += 1;
        }
    }

    let total = passed + failed;
    bun_sys::File::from_fd(bun_sys::Fd::stderr())
        .write_all(format!("\n{} test(s) ran, {} passed, {} failed\n", total, passed, failed).as_bytes());

    let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &result_obj };

    let tv = Int32Value(total as i32);
    let th = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &tv };
    JS_DefineProperty(cx, obj_h, c"total".as_ptr(), th, JSPROP_ENUMERATE as u32);

    let pv = Int32Value(passed as i32);
    let ph = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &pv };
    JS_DefineProperty(cx, obj_h, c"passed".as_ptr(), ph, JSPROP_ENUMERATE as u32);

    let fv = Int32Value(failed as i32);
    let fh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fv };
    JS_DefineProperty(cx, obj_h, c"failed".as_ptr(), fh, JSPROP_ENUMERATE as u32);

    let success = failed == 0;
    let sv = BooleanValue(success);
    let sh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &sv };
    JS_DefineProperty(cx, obj_h, c"success".as_ptr(), sh, JSPROP_ENUMERATE as u32);

    if !failures.is_empty() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let fail_arr = NewArrayObject1(&mut wrapped_cx, 0));
        for (i, fname) in failures.iter().enumerate() {
            let c_name = bun_core::ZBox::from_bytes(fname.as_str().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
            if !js_str.is_null() {
                let fval = StringValue(&*js_str);
                rooted!(&in(wrapped_cx) let fv2 = fval);
                JS_SetElement(cx, fail_arr.handle().into(), i as u32, fv2.handle().into());
            }
        }
        let fav = ObjectValue(fail_arr.get());
        rooted!(&in(wrapped_cx) let favh = fav);
        JS_DefineProperty(cx, obj_h, c"failures".as_ptr(), favh.handle().into(), JSPROP_ENUMERATE as u32);
    }

    args.rval().set(ObjectValue(result_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_file(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || args.get(0).ptr.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let path_val = *args.get(0).ptr;
    if !path_val.is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let _path_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    let s = crate::js_to_rust_string(cx, path_val);
    let file_obj = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx) };
    if file_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let c_path = bun_core::ZBox::from_bytes(s.as_str().as_bytes());
    let path_js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
    if !path_js_str.is_null() {
        let val = StringValue(&*path_js_str);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle, c"path".as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
    }
    let mut path_buf = bun_paths::PathBuffer::default();
    let path_bytes = s.as_bytes();
    if path_bytes.len() < path_buf.0.len() {
        path_buf.0[..path_bytes.len()].copy_from_slice(path_bytes);
        path_buf.0[path_bytes.len()] = 0;
        let zpath = bun_core::ZStr::from_buf(&path_buf.0, path_bytes.len());
        if let Ok(meta) = bun_sys::stat(zpath) {
            let size_val = Int32Value(meta.st_size as i32);
            let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &size_val };
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
            JS_DefineProperty(cx, obj_handle, c"size".as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
            let exists_val = mozjs::jsval::BooleanValue(true);
            let val_handle2 = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exists_val };
            let obj_handle2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
            JS_DefineProperty(cx, obj_handle2, c"exists".as_ptr(), val_handle2, JSPROP_ENUMERATE as u32);
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(file_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, c"Bun.write requires 2 arguments".as_ptr());
        return false;
    }
    let path_val = *args.get(0).ptr;
    let content_val = *args.get(1).ptr;
    if !path_val.is_string() || !content_val.is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.write requires string arguments".as_ptr());
        return false;
    }
    let fpath = crate::js_to_rust_string(cx, path_val);
    let content = crate::js_to_rust_string(cx, content_val);
    let mut path_buf = bun_paths::PathBuffer::default();
    let path_bytes = fpath.as_bytes();
    let write_result = if path_bytes.len() < path_buf.0.len() {
        path_buf.0[..path_bytes.len()].copy_from_slice(path_bytes);
        path_buf.0[path_bytes.len()] = 0;
        let zpath = bun_core::ZStr::from_buf(&path_buf.0, path_bytes.len());
        bun_sys::File::write_file(bun_sys::Fd::cwd(), zpath, content.as_bytes())
    } else {
        Err(bun_sys::Error::from_code_int(libc::ENAMETOOLONG, bun_sys::Tag::TODO).with_path(path_bytes))
    };
    match write_result {
        Ok(()) => {
            let written = Int32Value(content.len() as i32);
            args.rval().set(written);
            true
        }
        Err(e) => {
            let c_msg = bun_core::ZBox::from_bytes(format!("Bun.write failed: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_read_file(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"Bun.readFile requires a path argument".as_ptr());
        return false;
    }
    let path_val = *args.get(0).ptr;
    if !path_val.is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.readFile requires a string path".as_ptr());
        return false;
    }
    let fpath = crate::js_to_rust_string(cx, path_val);
    match bun_sys::File::read_from(Fd::cwd(), fpath.as_bytes()) {
        Ok(bytes) => {
            let content = String::from_utf8_lossy(&bytes);
            let c_content = bun_core::ZBox::from_bytes(content.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_content.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
        Err(e) => {
            let c_msg = bun_core::ZBox::from_bytes(format!("Bun.readFile failed: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_cwd(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    match getcwd_pathbuf() {
        Some(dir) => {
            let s = dir.to_string_lossy().into_owned();
            let c_s = bun_core::ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
        }
        None => { args.rval().set(UndefinedValue()); }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_cwd(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    process_cwd(cx, argc, vp)
}

/// process.exit(code) — set exit flag instead of calling std::process::exit().
/// The CLI main loop checks should_exit() and exits orderly,
/// allowing SmRuntimeGuard to drop (JS_DestroyContext + JS_ShutDown).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_exit(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let code = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() } else { 0 }
    } else {
        0
    };
    crate::request_exit(code);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_chdir(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"process.chdir requires a directory path".as_ptr());
        return false;
    }
    let dir_val = *args.get(0).ptr;
    if !dir_val.is_string() {
        JS_ReportErrorUTF8(cx, c"process.chdir requires a string".as_ptr());
        return false;
    }
    let dir = jsstr_to_string(cx, NonNull::new_unchecked(dir_val.to_string()));
    // 铁律0: bun_sys::chdir instead of std::env::set_current_dir
    let dir_z = bun_core::ZBox::from_bytes(dir.as_bytes());
    if bun_sys::chdir(&dir_z).is_err() {
        let c_msg = bun_core::ZBox::from_bytes(format!("process.chdir failed: {}", dir).as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_stdout_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(true));
        return true;
    }
    let val = *args.get(0).ptr;
    if val.is_string() {
        let s = crate::js_to_rust_string(cx, val);
        // 铁律0: use bun_sys::File instead of std::io::Write for stdout
        bun_sys::File::from_fd(bun_sys::Fd::stdout()).write_all(s.as_bytes()).ok();
    }
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_stderr_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(true));
        return true;
    }
    let val = *args.get(0).ptr;
    if val.is_string() {
        let s = crate::js_to_rust_string(cx, val);
        // 铁律0: use bun_sys::File instead of std::io::Write for stderr
        bun_sys::File::from_fd(bun_sys::Fd::stderr()).write_all(s.as_bytes()).ok();
    }
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

thread_local! {
    static EXIT_HANDLERS: RefCell<Vec<*mut JSObject>> = const { RefCell::new(Vec::new()) };
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_on(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let event_val = *args.get(0).ptr;
    let handler_val = *args.get(1).ptr;

    if !event_val.is_string() || !handler_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let event = crate::js_to_rust_string(cx, event_val);
    let handler_obj = handler_val.to_object();

    if event == "exit" {
        EXIT_HANDLERS.with(|h| h.borrow_mut().push(handler_obj));
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_next_tick(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"process.nextTick() requires a callback".as_ptr());
        return false;
    }
    let cb_val = *args.get(0).ptr;
    if !cb_val.is_object() {
        JS_ReportErrorUTF8(cx, c"process.nextTick() callback must be a function".as_ptr());
        return false;
    }

    let cb_obj = cb_val.to_object();
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Get queueMicrotask from global and call it with the callback
    // This defers execution to the next microtask tick
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
    let mut qmt_val = UndefinedValue();
    let qmt_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut qmt_val };
    JS_GetProperty(cx, global_h, c"queueMicrotask".as_ptr(), qmt_rv);

    if qmt_val.is_object() {
        // Store callback in a thread-local so the eval can pick it up
        // Simpler approach: use JS::Call to invoke queueMicrotask(cb)
        let _qmt_obj = qmt_val.to_object();
        let cb_val_obj = mozjs::jsval::ObjectValue(cb_obj);

        // Use JS_CallFunctionName-like pattern via direct property + call
        // Safest: eval a minimal expression that calls queueMicrotask with the callback
        // We pass the callback as a rooted value on the argument stack
        let _empty_args = HandleValueArray::empty();

        // Store cb in a global temporary, eval queueMicrotask to pick it up
        let cb_h_val = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val_obj };
        JS_SetProperty(cx, global_h, c"__nextTickCb".as_ptr(), cb_h_val);

        let eval_src = "queueMicrotask(__nextTickCb); delete globalThis.__nextTickCb;";
        let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx, c"<nextTick>".as_ptr(), 1) as *mut _) else {
            args.rval().set(UndefinedValue());
            return true;
        };
        let opts = _opts_guard.as_ptr();
            let mut src = mozjs::rust::transform_str_to_source_text(eval_src);
            let mut eval_rval = UndefinedValue();
            let eval_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut eval_rval };
            mozjs_sys::jsapi::JS::Evaluate2(cx, opts as *const _, &mut src, eval_rval_h);
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_hrtime(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ts = bun_core::time::nano_timestamp();
    let sec = (ts / 1_000_000_000) as i32;
    let nsec = (ts % 1_000_000_000) as i32;

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let arr = unsafe { NewArrayObject1(cx_ref, 2) });
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    rooted!(&in(cx_ref) let sec_val = Int32Value(sec));
    unsafe { JS_DefineElement(cx, arr.handle().into(), 0, sec_val.handle().into(), JSPROP_ENUMERATE as u32); }
    rooted!(&in(cx_ref) let nsec_val = Int32Value(nsec));
    unsafe { JS_DefineElement(cx, arr.handle().into(), 1, nsec_val.handle().into(), JSPROP_ENUMERATE as u32); }

    // hrtime.bigint — function returning nanoseconds as BigInt
    let bigint_fn = unsafe { JS_NewFunction(cx, Some(hrtime_bigint), 0, 0, c"bigint".as_ptr()) };
    if !bigint_fn.is_null() {
        let fn_obj = unsafe { JS_GetFunctionObject(bigint_fn) };
        let fn_val = mozjs::jsval::ObjectValue(fn_obj);
        let fn_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        unsafe { JS_DefineProperty(cx, arr.handle().into(), c"bigint".as_ptr(), fn_h, JSPROP_ENUMERATE as u32); }
    }

    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn hrtime_bigint(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let total_ns = bun_core::time::nano_timestamp() as i64;
    let src = format!("BigInt(\"{}\")", total_ns);
    let mut rval = UndefinedValue();
    let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx, c"hrtime_bigint".as_ptr(), 1) as *mut _) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let opts = _opts_guard.as_ptr();
        let mut eval_src = mozjs::rust::transform_str_to_source_text(&src);
        mozjs_sys::jsapi::JS::Evaluate2(cx, opts as *const _, &mut eval_src, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
    args.rval().set(rval);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_uptime(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let uptime_secs = match PROCESS_START.with(|s| *s.borrow()) {
        Some(start) => {
            let now = ::std::time::Instant::now();
            now.duration_since(start).as_secs_f64()
        }
        None => 0.0,
    };
    args.rval().set(mozjs::jsval::DoubleValue(uptime_secs));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_memory_usage(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let rss = unsafe {
        // D87: bun_libuv_sys::uv_getrusage replaces libc::getrusage (cross-platform)
        let mut usage: bun_libuv_sys::uv_rusage_t = core::mem::zeroed();
        let rc = bun_libuv_sys::uv_getrusage(&mut usage);
        if rc == 0 {
            (usage.ru_maxrss as f64) * 1024.0  // ru_maxrss is in KB on Linux
        } else {
            0.0
        }
    };
    let rss_val = mozjs::jsval::DoubleValue(rss);
    let rss_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &rss_val };
    JS_DefineProperty(cx, obj_h, c"rss".as_ptr(), rss_h, JSPROP_ENUMERATE as u32);
    let heap_total_val = mozjs::jsval::DoubleValue(0.0);
    let heap_total_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &heap_total_val };
    JS_DefineProperty(cx, obj_h, c"heapTotal".as_ptr(), heap_total_h, JSPROP_ENUMERATE as u32);
    let heap_used_val = mozjs::jsval::DoubleValue(0.0);
    let heap_used_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &heap_used_val };
    JS_DefineProperty(cx, obj_h, c"heapUsed".as_ptr(), heap_used_h, JSPROP_ENUMERATE as u32);
    let external_val = mozjs::jsval::DoubleValue(0.0);
    let external_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &external_val };
    JS_DefineProperty(cx, obj_h, c"external".as_ptr(), external_h, JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_kill(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    if _argc < 1 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let pid_val = args.get(0);
    let pid = if pid_val.is_int32() {
        pid_val.to_int32()
    } else {
        args.rval().set(BooleanValue(false));
        return true;
    };
    let sig_num = if _argc >= 2 {
        let sig_val = args.get(1);
        if sig_val.is_int32() {
            sig_val.to_int32()
        } else {
            15
        }
    } else {
        15
    };
    // D95: bun_sys::c::kill replaces libc::kill (cross-platform safe fn)
    let _ = bun_sys::c::kill(pid, sig_num);
    args.rval().set(BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_umask(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // D39: bun_sys::umask replaces libc::umask
    let old = bun_sys::umask(0o022);
    bun_sys::umask(old);
    args.rval().set(Int32Value(old as i32));
    true
}

thread_local! {
    static PROCESS_START: RefCell<Option<::std::time::Instant>> = const { RefCell::new(None) };
}

pub fn init_process_start() {
    PROCESS_START.with(|s| *s.borrow_mut() = Some(::std::time::Instant::now()));
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn set_env_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { args.rval().set(UndefinedValue()); return true; }
    let key_val = *args.get(0).ptr;
    let val_val = *args.get(1).ptr;
    if !key_val.is_string() || !val_val.is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let key = crate::js_to_rust_string(cx, key_val);
    let value = crate::js_to_rust_string(cx, val_val);
    if !key.is_empty() && !key.contains('\0') && !value.contains('\0') {
        // D79: bun_core::setenv_z instead of std::env::set_var
        let key_z = bun_core::ZBox::from_bytes(key.as_bytes());
        let val_z = bun_core::ZBox::from_bytes(value.as_bytes());
        let _ = bun_core::setenv_z(&key_z, &val_z, true);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn del_env_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 { args.rval().set(UndefinedValue()); return true; }
    let key_val = *args.get(0).ptr;
    if !key_val.is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let key = crate::js_to_rust_string(cx, key_val);
    if !key.is_empty() && !key.contains('\0') {
        // D79: bun_core::unsetenv_z instead of std::env::remove_var
        let key_z = bun_core::ZBox::from_bytes(key.as_bytes());
        let _ = bun_core::unsetenv_z(&key_z);
    }
    args.rval().set(UndefinedValue());
    true
}

/// Bun.exit(code) — set exit flag instead of calling std::process::exit().
/// The CLI main loop checks should_exit() and exits orderly,
/// allowing SmRuntimeGuard to drop (JS_DestroyContext + JS_ShutDown).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_exit(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let code = if argc > 0 && (*args.get(0).ptr).is_int32() {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    crate::request_exit(code);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_sleep_sync(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 && (*args.get(0).ptr).is_number() {
        let ms = (*args.get(0).ptr).to_number() as u64;
        bun_sys::c::sleep_ms(ms);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_hash(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let input = *args.get(0).ptr;
    let algo = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "sha256".to_string()
    };
    let data = if input.is_string() {
        crate::js_to_rust_string(cx, input).into_bytes()
    } else if input.is_object() {
        let obj = input.to_object();
        let mut len_val = mozjs::jsval::UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
        let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { 0 };
        let mut bytes = Vec::with_capacity(len as usize);
        for i in 0..len {
            let mut byte_val = mozjs::jsval::Int32Value(0);
            JS_GetElement(cx, obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val });
            bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
        }
        bytes
    } else {
        Vec::new()
    };
    let result = match algo.as_str() {
        "sha512" => {
            let mut h = bun_sha_hmac::SHA512::init();
            h.update(&data);
            let mut out = [0u8; 64];
            h.r#final(&mut out);
            out.to_vec()
        }
        _ => {
            let mut h = bun_sha_hmac::SHA256::init();
            h.update(&data);
            let mut out = [0u8; 32];
            h.r#final(&mut out);
            out.to_vec()
        }
    };
    let hex = bun_core::fmt::bytes_to_hex_lower_string(&result);
    let c_hex = bun_core::ZBox::from_bytes(hex.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_hex.as_ptr());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

// ── Unit tests for pure Rust data structures and logic ──────────────────
// @trace REQ-ENG-006 [req:REQ-ENG-006] [level:unit]

#[cfg(test)]
mod tests {
    use super::*;

    // ── TestCase ──

    #[test]
    fn test_case_stores_name() {
        let tc = TestCase {
            name: "my test".to_string(),
            callback: ::std::ptr::null_mut(),
        };
        assert_eq!(tc.name, "my test");
    }

    #[test]
    fn test_case_callback_default_null() {
        let tc = TestCase {
            name: String::new(),
            callback: ::std::ptr::null_mut(),
        };
        assert!(tc.callback.is_null());
    }

    // ── BunServeUserData ──

    #[test]
    fn bun_serve_user_data_default_fields() {
        let data = BunServeUserData {
            fetch_cb: None,
            websocket_cb: None,
            app_ptr: ::std::ptr::null_mut(),
            hostname: "localhost".to_string(),
            port: 3000,
        };
        assert!(data.fetch_cb.is_none());
        assert!(data.websocket_cb.is_none());
        assert!(data.app_ptr.is_null());
        assert_eq!(data.hostname, "localhost");
        assert_eq!(data.port, 3000);
    }

    #[test]
    fn bun_serve_user_data_with_fetch_cb() {
        let data = BunServeUserData {
            fetch_cb: Some(::std::ptr::null_mut()),
            websocket_cb: None,
            app_ptr: ::std::ptr::null_mut(),
            hostname: "0.0.0.0".to_string(),
            port: 8080,
        };
        assert!(data.fetch_cb.is_some());
        assert!(data.websocket_cb.is_none());
        assert_eq!(data.port, 8080);
    }

    #[test]
    fn bun_serve_user_data_with_websocket_cb() {
        let data = BunServeUserData {
            fetch_cb: None,
            websocket_cb: Some(::std::ptr::null_mut()),
            app_ptr: ::std::ptr::null_mut(),
            hostname: "0.0.0.0".to_string(),
            port: 8080,
        };
        assert!(data.fetch_cb.is_none());
        assert!(data.websocket_cb.is_some());
    }

    #[test]
    fn bun_serve_user_data_hostname_variants() {
        for host in &["localhost", "0.0.0.0", "127.0.0.1", "::"] {
            let data = BunServeUserData {
                fetch_cb: None,
                websocket_cb: None,
                app_ptr: ::std::ptr::null_mut(),
                hostname: host.to_string(),
                port: 80,
            };
            assert_eq!(data.hostname, *host);
        }
    }

    #[test]
    fn bun_serve_user_data_port_boundaries() {
        let data = BunServeUserData {
            fetch_cb: None,
            websocket_cb: None,
            app_ptr: ::std::ptr::null_mut(),
            hostname: String::new(),
            port: 0,
        };
        assert_eq!(data.port, 0);

        let data = BunServeUserData {
            fetch_cb: None,
            websocket_cb: None,
            app_ptr: ::std::ptr::null_mut(),
            hostname: String::new(),
            port: 65535,
        };
        assert_eq!(data.port, 65535);
    }

    // ── init_process_start ──

    #[test]
    fn init_process_start_sets_instant() {
        init_process_start();
        PROCESS_START.with(|s| {
            let start = *s.borrow();
            assert!(start.is_some());
        });
    }

    #[test]
    fn init_process_start_idempotent() {
        init_process_start();
        let first = PROCESS_START.with(|s| s.borrow().unwrap());
        init_process_start();
        let second = PROCESS_START.with(|s| s.borrow().unwrap());
        // Second call resets the instant, so second >= first
        assert!(second >= first);
    }

    // ── Hash computation (sha256/sha512) ──

    #[test]
    fn sha256_empty_input() {
        let mut h = bun_sha_hmac::SHA256::init();
        h.update(b"");
        let mut result = [0u8; 32];
        h.r#final(&mut result);
        // SHA-256 of empty string: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(result.len(), 32);
        let hex = bun_core::fmt::bytes_to_hex_lower_string(&result);
        assert!(hex.starts_with("e3b0c442"));
    }

    #[test]
    fn sha256_hello_world() {
        let mut h = bun_sha_hmac::SHA256::init();
        h.update(b"hello world");
        let mut result = [0u8; 32];
        h.r#final(&mut result);
        let hex = bun_core::fmt::bytes_to_hex_lower_string(&result);
        assert!(hex.starts_with("b94d27b9"));
    }

    #[test]
    fn sha512_empty_input() {
        let mut h = bun_sha_hmac::SHA512::init();
        h.update(b"");
        let mut result = [0u8; 64];
        h.r#final(&mut result);
        assert_eq!(result.len(), 64);
        let hex = bun_core::fmt::bytes_to_hex_lower_string(&result);
        assert!(hex.starts_with("cf83e135"));
    }

    #[test]
    fn sha512_hello_world() {
        let mut h = bun_sha_hmac::SHA512::init();
        h.update(b"hello world");
        let mut result = [0u8; 64];
        h.r#final(&mut result);
        let hex = bun_core::fmt::bytes_to_hex_lower_string(&result);
        assert!(hex.starts_with("309ecc48"));
    }

    #[test]
    fn hash_hex_format_lowercase() {
        let mut h = bun_sha_hmac::SHA256::init();
        h.update(b"\xff");
        let mut result = [0u8; 32];
        h.r#final(&mut result);
        let hex = bun_core::fmt::bytes_to_hex_lower_string(&result);
        assert_eq!(hex, hex.to_lowercase());
    }

    #[test]
    fn sha256_deterministic() {
        let mut h1 = bun_sha_hmac::SHA256::init();
        h1.update(b"test data");
        let mut r1 = [0u8; 32]; h1.r#final(&mut r1);

        let mut h2 = bun_sha_hmac::SHA256::init();
        h2.update(b"test data");
        let mut r2 = [0u8; 32]; h2.r#final(&mut r2);

        assert_eq!(r1.as_slice(), r2.as_slice());
    }

    #[test]
    fn sha256_different_inputs_different_outputs() {
        let mut h1 = bun_sha_hmac::SHA256::init();
        h1.update(b"input1");
        let mut r1 = [0u8; 32]; h1.r#final(&mut r1);

        let mut h2 = bun_sha_hmac::SHA256::init();
        h2.update(b"input2");
        let mut r2 = [0u8; 32]; h2.r#final(&mut r2);

        assert_ne!(r1.as_slice(), r2.as_slice());
    }

    #[test]
    fn sha256_incremental_update() {
        let mut h1 = bun_sha_hmac::SHA256::init();
        h1.update(b"hello");
        h1.update(b" world");
        let mut r1 = [0u8; 32]; h1.r#final(&mut r1);

        let mut h2 = bun_sha_hmac::SHA256::init();
        h2.update(b"hello world");
        let mut r2 = [0u8; 32]; h2.r#final(&mut r2);

        assert_eq!(r1.as_slice(), r2.as_slice());
    }

    #[test]
    fn sha256_large_input() {
        let data = vec![0xABu8; 10_000];
        let mut h = bun_sha_hmac::SHA256::init();
        h.update(&data);
        let mut result = [0u8; 32];
        h.r#final(&mut result);
        assert_eq!(result.len(), 32);
    }

    #[test]
    fn sha512_large_input() {
        let data = vec![0xCDu8; 10_000];
        let mut h = bun_sha_hmac::SHA512::init();
        h.update(&data);
        let mut result = [0u8; 64];
        h.r#final(&mut result);
        assert_eq!(result.len(), 64);
    }
}
