use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, NullValue, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1,
};
use mozjs::conversions::jsstr_to_string;
use digest::Digest;

pub fn install_bun_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let bun_obj = JS_NewPlainObject(cx));
        if bun_obj.get().is_null() {
            return;
        }

        let version_str = JS_NewStringCopyZ(
            cx.raw_cx(),
            b"0.1.0\0".as_ptr() as *const ::std::os::raw::c_char,
        );
        if !version_str.is_null() {
            rooted!(&in(cx) let ver_val = StringValue(&*version_str));
            JS_DefineProperty(
                cx.raw_cx(),
                bun_obj.handle().into(),
                c"version".as_ptr(),
                ver_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"env".as_ptr(),
            ::std::option::Option::Some(bun_env),
            0,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"file".as_ptr(),
            ::std::option::Option::Some(bun_file),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"write".as_ptr(),
            ::std::option::Option::Some(bun_write),
            2,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"readFile".as_ptr(),
            ::std::option::Option::Some(bun_read_file),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.serve() — HTTP server stub (placeholder, needs event loop integration)
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"serve".as_ptr(),
            ::std::option::Option::Some(bun_serve),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.spawn() — spawn a subprocess
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"spawn".as_ptr(),
            ::std::option::Option::Some(bun_spawn),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.gc() — garbage collection
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"gc".as_ptr(),
            ::std::option::Option::Some(bun_gc),
            0,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.sleep() — synchronous sleep
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"sleep".as_ptr(),
            ::std::option::Option::Some(bun_sleep),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.which() — find executable in PATH
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"which".as_ptr(),
            ::std::option::Option::Some(bun_which),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.inspect() — format value for display
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"inspect".as_ptr(),
            ::std::option::Option::Some(bun_inspect),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Bun.resolve() — resolve module specifier to absolute path
        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"resolve".as_ptr(),
            ::std::option::Option::Some(bun_resolve),
            1,
            JSPROP_ENUMERATE as u32,
        );

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

pub fn install_process_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let proc_obj = JS_NewPlainObject(cx));
        if proc_obj.get().is_null() {
            return;
        }

        // process.arch
        let arch_cstr = ::std::ffi::CString::new(::std::env::consts::ARCH).unwrap_or_default();
        let arch_str = JS_NewStringCopyZ(cx.raw_cx(), arch_cstr.as_ptr());
        if !arch_str.is_null() {
            rooted!(&in(cx) let arch_val = StringValue(&*arch_str));
            JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"arch".as_ptr(), arch_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // process.platform
        let plat_cstr = ::std::ffi::CString::new(::std::env::consts::OS).unwrap_or_default();
        let platform_str = JS_NewStringCopyZ(cx.raw_cx(), plat_cstr.as_ptr());
        if !platform_str.is_null() {
            rooted!(&in(cx) let plat_val = StringValue(&*platform_str));
            JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"platform".as_ptr(), plat_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // process.cwd()
        JS_DefineFunction(cx, proc_obj.handle(), c"cwd".as_ptr(), ::std::option::Option::Some(process_cwd), 0, JSPROP_ENUMERATE as u32);

        // process.exit()
        JS_DefineFunction(cx, proc_obj.handle(), c"exit".as_ptr(), ::std::option::Option::Some(process_exit), 1, JSPROP_ENUMERATE as u32);

        // process.argv — real command line args
        {
            let args: Vec<::std::string::String> = ::std::env::args().collect();
            rooted!(&in(cx) let argv_arr = NewArrayObject1(cx, args.len()));
            if !argv_arr.get().is_null() {
                for (i, arg) in args.iter().enumerate() {
                    let Ok(c_arg) = ::std::ffi::CString::new(arg.as_str()) else { continue };
                    let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_arg.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(cx) let v = StringValue(&*js_str));
                        JS_DefineElement(cx.raw_cx(), argv_arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
                JS_DefineProperty3(cx, proc_obj.handle(), c"argv".as_ptr(), argv_arr.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.env — environment variables object
        {
            rooted!(&in(cx) let env_obj = JS_NewPlainObject(cx));
            if !env_obj.get().is_null() {
                for (key, value) in ::std::env::vars() {
                    let Ok(c_key) = ::std::ffi::CString::new(key) else { continue };
                    let Ok(c_val) = ::std::ffi::CString::new(value) else { continue };
                    let val_str = JS_NewStringCopyZ(cx.raw_cx(), c_val.as_ptr());
                    if !val_str.is_null() {
                        rooted!(&in(cx) let v = StringValue(&*val_str));
                        JS_DefineProperty(cx.raw_cx(), env_obj.handle().into(), c_key.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
                JS_DefineProperty3(cx, proc_obj.handle(), c"env".as_ptr(), env_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.version — Node.js compatible version
        {
            let ver_str = JS_NewStringCopyZ(cx.raw_cx(), b"v18.0.0\0".as_ptr() as *const ::std::os::raw::c_char);
            if !ver_str.is_null() {
                rooted!(&in(cx) let v = StringValue(&*ver_str));
                JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"version".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.versions — version info object
        {
            rooted!(&in(cx) let ver_obj = JS_NewPlainObject(cx));
            if !ver_obj.get().is_null() {
                let node_ver = JS_NewStringCopyZ(cx.raw_cx(), b"18.0.0\0".as_ptr() as *const ::std::os::raw::c_char);
                if !node_ver.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*node_ver));
                    JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"node".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                }
                let bao_ver = JS_NewStringCopyZ(cx.raw_cx(), b"0.1.0\0".as_ptr() as *const ::std::os::raw::c_char);
                if !bao_ver.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*bao_ver));
                    JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"bao".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                }
                let sm_ver = JS_NewStringCopyZ(cx.raw_cx(), b"115.0\0".as_ptr() as *const ::std::os::raw::c_char);
                if !sm_ver.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*sm_ver));
                    JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"spidermonkey".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                }
                let rust_ver = JS_NewStringCopyZ(cx.raw_cx(), b"1.80.0\0".as_ptr() as *const ::std::os::raw::c_char);
                if !rust_ver.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*rust_ver));
                    JS_DefineProperty(cx.raw_cx(), ver_obj.handle().into(), c"rust".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                }
                JS_DefineProperty3(cx, proc_obj.handle(), c"versions".as_ptr(), ver_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.stdout / process.stderr — stream-like objects with write()
        {
            rooted!(&in(cx) let stdout_obj = JS_NewPlainObject(cx));
            if !stdout_obj.get().is_null() {
                let fd_val = Int32Value(1);
                rooted!(&in(cx) let fd = fd_val);
                JS_DefineProperty(cx.raw_cx(), stdout_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
                JS_DefineFunction(cx, stdout_obj.handle(), c"write".as_ptr(), ::std::option::Option::Some(process_stdout_write), 1, JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, proc_obj.handle(), c"stdout".as_ptr(), stdout_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }
        {
            rooted!(&in(cx) let stderr_obj = JS_NewPlainObject(cx));
            if !stderr_obj.get().is_null() {
                let fd_val = Int32Value(2);
                rooted!(&in(cx) let fd = fd_val);
                JS_DefineProperty(cx.raw_cx(), stderr_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
                JS_DefineFunction(cx, stderr_obj.handle(), c"write".as_ptr(), ::std::option::Option::Some(process_stderr_write), 1, JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, proc_obj.handle(), c"stderr".as_ptr(), stderr_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.stdin — readable stream stub
        {
            rooted!(&in(cx) let stdin_obj = JS_NewPlainObject(cx));
            if !stdin_obj.get().is_null() {
                let fd_val = Int32Value(0);
                rooted!(&in(cx) let fd = fd_val);
                JS_DefineProperty(cx.raw_cx(), stdin_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, proc_obj.handle(), c"stdin".as_ptr(), stdin_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.on() — event handler registration
        JS_DefineFunction(cx, proc_obj.handle(), c"on".as_ptr(), ::std::option::Option::Some(process_on), 2, JSPROP_ENUMERATE as u32);

        // process.nextTick() — microtask scheduling
        JS_DefineFunction(cx, proc_obj.handle(), c"nextTick".as_ptr(), ::std::option::Option::Some(process_next_tick), 1, JSPROP_ENUMERATE as u32);

        // process.pid / process.ppid
        {
            let pid_val = Int32Value(::std::process::id() as i32);
            rooted!(&in(cx) let pid = pid_val);
            JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"pid".as_ptr(), pid.handle().into(), JSPROP_ENUMERATE as u32);
        }
        {
            let ppid = libc::getppid();
            let ppid_val = Int32Value(ppid as i32);
            rooted!(&in(cx) let p = ppid_val);
            JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"ppid".as_ptr(), p.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // process.title
        {
            let title_str = JS_NewStringCopyZ(cx.raw_cx(), b"bao\0".as_ptr() as *const ::std::os::raw::c_char);
            if !title_str.is_null() {
                rooted!(&in(cx) let v = StringValue(&*title_str));
                JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"title".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.hrtime() — high-resolution time
        JS_DefineFunction(cx, proc_obj.handle(), c"hrtime".as_ptr(), ::std::option::Option::Some(process_hrtime), 0, JSPROP_ENUMERATE as u32);

        // process.uptime() — process uptime in seconds
        JS_DefineFunction(cx, proc_obj.handle(), c"uptime".as_ptr(), ::std::option::Option::Some(process_uptime), 0, JSPROP_ENUMERATE as u32);

        // process.chdir() — change working directory
        JS_DefineFunction(cx, proc_obj.handle(), c"chdir".as_ptr(), ::std::option::Option::Some(process_chdir), 1, JSPROP_ENUMERATE as u32);

        // process.argv0 — first argument (binary path)
        {
            let args: Vec<::std::string::String> = ::std::env::args().collect();
            if !args.is_empty() {
                if let Ok(c_arg) = ::std::ffi::CString::new(args[0].as_str()) {
                    let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_arg.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(cx) let v = StringValue(&*js_str));
                        JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"argv0".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
            }
        }

        // process.execPath — path to the bao binary
        {
            let exec_path = ::std::env::current_exe().unwrap_or_default();
            if let Ok(c_path) = ::std::ffi::CString::new(exec_path.to_string_lossy().into_owned()) {
                let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_path.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx) let v = StringValue(&*js_str));
                    JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"execPath".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
        }

        JS_DefineProperty3(cx, global, c"process".as_ptr(), proc_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

thread_local! {
    static SPAWNED_PROCS: RefCell<Vec<*mut ::std::process::Child>> = RefCell::new(Vec::new());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_spawn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"Bun.spawn() requires an options object\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let opts_val = *args.get(0).ptr;
    if !opts_val.is_object() {
        JS_ReportErrorUTF8(cx, b"Bun.spawn() requires an options object\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let opts_obj = opts_val.to_object();
    let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };

    // cmd (string or array of strings)
    let cmd = get_string_prop(cx, opts_h, c"cmd".as_ptr()).unwrap_or_else(|| "echo".to_string());
    let cmd_args = get_string_array_prop(cx, opts_h, c"args".as_ptr());
    let cwd = get_string_prop(cx, opts_h, c"cwd".as_ptr());
    let env_obj = get_env_prop(cx, opts_h);

    let stdin_mode = get_stdio_mode(cx, opts_h, c"stdin".as_ptr());
    let stdout_mode = get_stdio_mode(cx, opts_h, c"stdout".as_ptr());
    let stderr_mode = get_stdio_mode(cx, opts_h, c"stderr".as_ptr());

    let mut command = ::std::process::Command::new(&cmd);
    for arg in &cmd_args {
        command.arg(arg);
    }
    if let Some(ref dir) = cwd {
        command.current_dir(dir);
    }
    if let Some(env) = env_obj {
        command.env_clear();
        for (k, v) in env {
            command.env(k, v);
        }
    }
    command.stdin(stdin_mode);
    command.stdout(stdout_mode);
    command.stderr(stderr_mode);

    match command.spawn() {
        Ok(child) => {
            let pid = child.id();
            let boxed_child = Box::new(child);
            let child_ptr = Box::into_raw(boxed_child);
            SPAWNED_PROCS.with(|p| p.borrow_mut().push(child_ptr));

            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let subproc_obj = JS_NewPlainObject(cx_ref));
            if subproc_obj.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }

            // pid
            let pid_val = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

            // exited
            let exited_val = BooleanValue(false);
            rooted!(&in(cx_ref) let ev = exited_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

            // exitCode
            let exit_code_val = Int32Value(-1);
            rooted!(&in(cx_ref) let ecv = exit_code_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"exitCode".as_ptr(), ecv.handle().into(), JSPROP_ENUMERATE as u32);

            // Store child pointer as two i32 halves (high/low) to avoid PrivateValue issues
            let ptr_bits = child_ptr as u64;
            let ptr_hi = (ptr_bits >> 32) as i32;
            let ptr_lo = (ptr_bits & 0xFFFFFFFF) as i32;
            rooted!(&in(cx_ref) let hi = Int32Value(ptr_hi));
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"_ptrHi".as_ptr(), hi.handle().into(), 0);
            rooted!(&in(cx_ref) let lo = Int32Value(ptr_lo));
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"_ptrLo".as_ptr(), lo.handle().into(), 0);

            // stdout reader
            let stdout_reader_fn = JS_NewFunction(cx, Some(subproc_stdout_read), 0, 0, c"stdout".as_ptr());
            if !stdout_reader_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(stdout_reader_fn);
                let fn_val = ObjectValue(fn_obj);
                rooted!(&in(cx_ref) let fv = fn_val);
                JS_DefineProperty(cx, subproc_obj.handle().into(), c"_readStdout".as_ptr(), fv.handle().into(), 0);
            }

            // stderr reader
            let stderr_reader_fn = JS_NewFunction(cx, Some(subproc_stderr_read), 0, 0, c"stderr".as_ptr());
            if !stderr_reader_fn.is_null() {
                let fn_obj = JS_GetFunctionObject(stderr_reader_fn);
                let fn_val = ObjectValue(fn_obj);
                rooted!(&in(cx_ref) let fv = fn_val);
                JS_DefineProperty(cx, subproc_obj.handle().into(), c"_readStderr".as_ptr(), fv.handle().into(), 0);
            }

            // wait()
            let mut wrapped_cx2 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            JS_DefineFunction(&mut wrapped_cx2, subproc_obj.handle(), c"wait".as_ptr(), ::std::option::Option::Some(subproc_wait), 0, JSPROP_ENUMERATE as u32);

            // kill()
            JS_DefineFunction(&mut wrapped_cx2, subproc_obj.handle(), c"kill".as_ptr(), ::std::option::Option::Some(subproc_kill), 0, JSPROP_ENUMERATE as u32);

            // killed
            let killed_val = BooleanValue(false);
            rooted!(&in(cx_ref) let kv = killed_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"killed".as_ptr(), kv.handle().into(), JSPROP_ENUMERATE as u32);

            args.rval().set(ObjectValue(subproc_obj.get()));
            true
        }
        Err(e) => {
            let msg = format!("Bun.spawn() failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            false
        }
    }
}

unsafe fn get_child_ptr_from_this(cx: *mut JSContext, args: &CallArgs) -> Option<*mut ::std::process::Child> { unsafe {
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
        let ptr = ((hi << 32) | lo) as *mut ::std::process::Child;
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
    let child_ptr = match get_child_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(Int32Value(-1));
            return true;
        }
    };

    let child = &mut *child_ptr;
    match child.wait() {
        Ok(status) => {
            let exit_code = status.code().unwrap_or(-1);

            // Update exited and exitCode on this object
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
        }
        Err(e) => {
            let msg = format!("wait() failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn subproc_kill(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let child_ptr = match get_child_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(BooleanValue(false));
            return true;
        }
    };

    let child = &mut *child_ptr;
    let result = child.kill().is_ok();

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
    let child_ptr = match get_child_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(NullValue());
            return true;
        }
    };

    let child = &mut *child_ptr;
    if let Some(ref mut stdout) = child.stdout {
        let mut buf = Vec::new();
        use ::std::io::Read;
        stdout.read_to_end(&mut buf).ok();
        let s = String::from_utf8_lossy(&buf).into_owned();
        let Ok(c_s) = ::std::ffi::CString::new(s.as_str()) else {
            args.rval().set(NullValue());
            return true;
        };
        let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
        args.rval().set(if js_str.is_null() { NullValue() } else { StringValue(&*js_str) });
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
    let child_ptr = match get_child_ptr_from_this(cx, &args) {
        Some(p) => p,
        None => {
            args.rval().set(NullValue());
            return true;
        }
    };

    let child = &mut *child_ptr;
    if let Some(ref mut stderr) = child.stderr {
        let mut buf = Vec::new();
        use ::std::io::Read;
        stderr.read_to_end(&mut buf).ok();
        let s = String::from_utf8_lossy(&buf).into_owned();
        let Ok(c_s) = ::std::ffi::CString::new(s.as_str()) else {
            args.rval().set(NullValue());
            return true;
        };
        let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
        args.rval().set(if js_str.is_null() { NullValue() } else { StringValue(&*js_str) });
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
        Some(jsstr_to_string(cx, NonNull::new(val.to_string()).unwrap()))
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
            result.push(jsstr_to_string(cx, NonNull::new(elem.to_string()).unwrap()));
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
        // Fallback: iterate common env vars or skip
        return None;
    }
    None
}}

unsafe fn get_stdio_mode(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> ::std::process::Stdio { unsafe {
    let mode_str = get_string_prop(cx, obj_h, name);
    match mode_str.as_deref() {
        Some("pipe") => ::std::process::Stdio::piped(),
        Some("inherit") => ::std::process::Stdio::inherit(),
        Some("null") | Some("ignore") => ::std::process::Stdio::null(),
        _ => ::std::process::Stdio::piped(),
    }
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_serve(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let mut port: u16 = 3000;
    let mut hostname = "0.0.0.0".to_string();
    let mut fetch_handler: Option<*mut JSObject> = None;

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
                hostname = jsstr_to_string(cx, NonNull::new(hn_val.to_string()).unwrap());
            }

            let mut fetch_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"fetch".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fetch_val });
            if fetch_val.is_object() && JS_ObjectIsFunction(fetch_val.to_object()) {
                fetch_handler = Some(fetch_val.to_object());
            }
        }
    }

    let addr = format!("{}:{}", hostname, port);
    let server = match tiny_http::Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Bun.serve() failed to bind: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let actual_port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);

    // Log server started
    eprint!("Bun.serve() listening on {}:{}\n", hostname, actual_port);

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    // Synchronous request loop — handles requests on the JS thread
    loop {
        let mut request = match server.recv() {
            Ok(r) => r,
            Err(_) => break,
        };

        let method = request.method().to_string();
        let url = request.url().to_string();

        if let Some(handler) = fetch_handler {
            // Build Request object for JS handler
            let req_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if req_obj.is_null() {
                let resp = tiny_http::Response::from_string("Internal Server Error").with_status_code(500);
                request.respond(resp).ok();
                continue;
            }
            let req_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &req_obj };

            // method
            if let Ok(c_m) = CString::new(method.as_str()) {
                let ms = JS_NewStringCopyZ(cx, c_m.as_ptr());
                if !ms.is_null() {
                    let mv = StringValue(&*ms);
                    let mv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mv };
                    JS_DefineProperty(cx, req_h, c"method".as_ptr(), mv_h, JSPROP_ENUMERATE as u32);
                }
            }

            // url
            if let Ok(c_u) = CString::new(url.as_str()) {
                let us = JS_NewStringCopyZ(cx, c_u.as_ptr());
                if !us.is_null() {
                    let uv = StringValue(&*us);
                    let uv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &uv };
                    JS_DefineProperty(cx, req_h, c"url".as_ptr(), uv_h, JSPROP_ENUMERATE as u32);
                }
            }

            // headers
            let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if !headers_obj.is_null() {
                let hdrs_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };
                for h in request.headers() {
                    let Ok(c_k) = CString::new(h.field.as_str().as_str()) else { continue };
                    let Ok(c_v) = CString::new(h.value.as_str()) else { continue };
                    let vs = JS_NewStringCopyZ(cx, c_v.as_ptr());
                    if !vs.is_null() {
                        let vv = StringValue(&*vs);
                        let vv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &vv };
                        JS_DefineProperty(cx, hdrs_h, c_k.as_ptr(), vv_h, JSPROP_ENUMERATE as u32);
                    }
                }
                let hdrs_val = ObjectValue(headers_obj);
                let hdrs_val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_val };
                JS_DefineProperty(cx, req_h, c"headers".as_ptr(), hdrs_val_h, JSPROP_ENUMERATE as u32);
            }

            // body
            let mut body_bytes = Vec::new();
            if let Some(len) = request.body_length() {
                body_bytes.resize(len.min(1_048_576), 0);
                let _ = ::std::io::Read::read(&mut request.as_reader(), &mut body_bytes);
            }
            if let Ok(c_b) = CString::new(String::from_utf8_lossy(&body_bytes).into_owned()) {
                let bs = JS_NewStringCopyZ(cx, c_b.as_ptr());
                if !bs.is_null() {
                    let bv = StringValue(&*bs);
                    let bv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bv };
                    JS_DefineProperty(cx, req_h, c"body".as_ptr(), bv_h, JSPROP_ENUMERATE as u32);
                }
            }

            // Call fetch handler: fetch_handler(request) -> response
            let req_val = ObjectValue(req_obj);
            rooted!(&in(cx_ref) let rv = req_val);
            let call_args = HandleValueArray { length_: 1, elements_: &rv.get() };
            let handler_val = ObjectValue(handler);
            let handler_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &handler_val };
            let mut rval = UndefinedValue();
            let ok = JS_CallFunctionValue(cx, global_h, handler_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });

            if !ok || !rval.is_object() {
                JS_ClearPendingException(cx);
                let resp = tiny_http::Response::from_string("Internal Server Error").with_status_code(500);
                request.respond(resp).ok();
                continue;
            }

            let resp_obj = rval.to_object();
            let resp_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

            let mut status_val = UndefinedValue();
            JS_GetProperty(cx, resp_h, c"status".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut status_val });
            let status_code = if status_val.is_int32() { status_val.to_int32() as u16 } else { 200 };

            let mut body_val = UndefinedValue();
            JS_GetProperty(cx, resp_h, c"_bodyText".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val });
            let body_str = if body_val.is_string() {
                jsstr_to_string(cx, NonNull::new(body_val.to_string()).unwrap())
            } else {
                String::new()
            };

            let mut response = tiny_http::Response::from_string(body_str).with_status_code(status_code as u32);

            // Copy response headers
            let mut resp_headers_val = UndefinedValue();
            JS_GetProperty(cx, resp_h, c"headers".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut resp_headers_val });
            if resp_headers_val.is_object() {
                let rh_obj = resp_headers_val.to_object();
                let rh_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &rh_obj };
                let mut ids_arr = UndefinedValue();
                JS_GetProperty(cx, rh_h, c"__proto__".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ids_arr });
            }

            response = response.with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]).unwrap()
            );
            request.respond(response).ok();
        } else {
            // No fetch handler — return 200 with basic info
            let body = format!("{{\"method\":\"{}\",\"url\":\"{}\"}}", method, url);
            let resp = tiny_http::Response::from_string(body)
                .with_status_code(200)
                .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
            request.respond(resp).ok();
        }
    }

    args.rval().set(UndefinedValue());
    true
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
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;
    let ms = if val.is_int32() {
        val.to_int32() as u64
    } else if val.is_double() {
        val.to_double() as u64
    } else {
        0
    };
    ::std::thread::sleep(::std::time::Duration::from_millis(ms));
    args.rval().set(UndefinedValue());
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
        JS_ReportErrorUTF8(cx, b"Bun.resolve requires a specifier\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let spec_val = *args.get(0).ptr;
    if !spec_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Bun.resolve requires a string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let specifier = mozjs::conversions::jsstr_to_string(cx, NonNull::new_unchecked(spec_val.to_string()));

    let from = if argc > 1 && (*args.get(1).ptr).is_string() {
        let from_str = mozjs::conversions::jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()));
        Some(::std::path::PathBuf::from(from_str))
    } else {
        ::std::env::current_dir().ok()
    };

    let spec_path = ::std::path::Path::new(&specifier);
    let resolved = if spec_path.is_absolute() {
        spec_path.to_path_buf()
    } else if specifier.starts_with("./") || specifier.starts_with("../") {
        let base = from.as_deref().unwrap_or(::std::path::Path::new("."));
        base.join(&specifier)
    } else {
        match crate::require::resolve_node_modules(&specifier, from.as_deref()) {
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
                let msg = format!("Cannot resolve '{}'", specifier);
                let c_msg = CString::new(msg).unwrap_or_default();
                JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
                return false;
            }
        }
    };

    let canonical = resolved.canonicalize().unwrap_or(resolved);
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
    let name = jsstr_to_string(cx, NonNull::new(name_val.to_string()).unwrap());

    let path_var = ::std::env::var("PATH").unwrap_or_default();
    let separator = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(separator) {
        let candidate = ::std::path::Path::new(dir).join(&name);
        if candidate.exists() {
            let result = candidate.to_string_lossy().into_owned();
            let Ok(c_result) = CString::new(result.as_str()) else {
                args.rval().set(NullValue());
                return true;
            };
            let js_str = JS_NewStringCopyZ(cx, c_result.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(NullValue());
            }
            return true;
        }
        #[cfg(target_family = "unix")]
        {
            let candidate = ::std::path::Path::new(dir).join(format!("{}", name));
            if candidate.exists() {
                let result = candidate.to_string_lossy().into_owned();
                let Ok(c_result) = CString::new(result.as_str()) else {
                    args.rval().set(NullValue());
                    return true;
                };
                let js_str = JS_NewStringCopyZ(cx, c_result.as_ptr());
                if !js_str.is_null() {
                    args.rval().set(StringValue(&*js_str));
                } else {
                    args.rval().set(NullValue());
                }
                return true;
            }
        }
    }
    args.rval().set(NullValue());
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
        let js_str = JS_NewStringCopyZ(cx, b"undefined\0".as_ptr() as *const ::std::os::raw::c_char);
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
        let rust_str = jsstr_to_string(cx, NonNull::new(val.to_string()).unwrap());
        format!("'{}'", rust_str)
    } else if val.is_object() {
        "[object]".to_string()
    } else {
        "undefined".to_string()
    };
    let Ok(c_s) = CString::new(s.as_str()) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_env(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let env_obj = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx) };
    if env_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (key, value) in ::std::env::vars() {
        let Ok(c_key) = ::std::ffi::CString::new(key) else { continue };
        let Ok(c_val) = ::std::ffi::CString::new(value) else { continue };
        let val_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
        if !val_str.is_null() {
            let val = StringValue(&*val_str);
            let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &env_obj };
            JS_DefineProperty(cx, obj_handle, c_key.as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(env_obj));
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
    let path_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    let _ = path_str;
    let s = mozjs::conversions::jsstr_to_string(
        cx,
        ::std::ptr::NonNull::new(path_val.to_string()).unwrap(),
    );
    let file_obj = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx) };
    if file_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let c_path = match ::std::ffi::CString::new(s.as_str()) {
        Ok(p) => p,
        Err(_) => { args.rval().set(UndefinedValue()); return true; }
    };
    let path_js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
    if !path_js_str.is_null() {
        let val = StringValue(&*path_js_str);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle, c"path".as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
    }
    if let Ok(meta) = ::std::fs::metadata(&s) {
        let size_val = Int32Value(meta.len() as i32);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &size_val };
        let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle, c"size".as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
        let exists_val = mozjs::jsval::BooleanValue(true);
        let val_handle2 = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exists_val };
        let obj_handle2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle2, c"exists".as_ptr(), val_handle2, JSPROP_ENUMERATE as u32);
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
        JS_ReportErrorUTF8(cx, b"Bun.write requires 2 arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path_val = *args.get(0).ptr;
    let content_val = *args.get(1).ptr;
    if !path_val.is_string() || !content_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Bun.write requires string arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(path_val.to_string()).unwrap());
    let content = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(content_val.to_string()).unwrap());
    match ::std::fs::write(path.as_str(), content.as_bytes()) {
        Ok(()) => {
            let written = Int32Value(content.len() as i32);
            args.rval().set(written);
            true
        }
        Err(e) => {
            let msg = format!("Bun.write failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
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
        JS_ReportErrorUTF8(cx, b"Bun.readFile requires a path argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path_val = *args.get(0).ptr;
    if !path_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Bun.readFile requires a string path\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(path_val.to_string()).unwrap());
    match ::std::fs::read_to_string(path.as_str()) {
        Ok(content) => {
            let Ok(c_content) = ::std::ffi::CString::new(content) else {
                args.rval().set(UndefinedValue());
                return true;
            };
            let js_str = JS_NewStringCopyZ(cx, c_content.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
        Err(e) => {
            let msg = format!("Bun.readFile failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
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
    match ::std::env::current_dir() {
        Ok(dir) => {
            let s = dir.to_string_lossy().into_owned();
            let Ok(c_s) = ::std::ffi::CString::new(s) else {
                args.rval().set(UndefinedValue());
                return true;
            };
            let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
        }
        Err(_) => { args.rval().set(UndefinedValue()); }
    }
    true
}

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
    ::std::process::exit(code);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_chdir(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"process.chdir requires a directory path\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let dir_val = *args.get(0).ptr;
    if !dir_val.is_string() {
        JS_ReportErrorUTF8(cx, b"process.chdir requires a string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let dir = jsstr_to_string(cx, NonNull::new_unchecked(dir_val.to_string()));
    if let Err(e) = ::std::env::set_current_dir(&dir) {
        let msg = format!("process.chdir failed: {}", e);
        let c_msg = CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
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
        let s = jsstr_to_string(cx, ::std::ptr::NonNull::new(val.to_string()).unwrap());
        ::std::io::Write::write_all(&mut ::std::io::stdout(), s.as_bytes()).ok();
        ::std::io::Write::flush(&mut ::std::io::stdout()).ok();
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
        let s = jsstr_to_string(cx, ::std::ptr::NonNull::new(val.to_string()).unwrap());
        ::std::io::Write::write_all(&mut ::std::io::stderr(), s.as_bytes()).ok();
        ::std::io::Write::flush(&mut ::std::io::stderr()).ok();
    }
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

thread_local! {
    static EXIT_HANDLERS: RefCell<Vec<*mut JSObject>> = RefCell::new(Vec::new());
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

    let event = jsstr_to_string(cx, ::std::ptr::NonNull::new(event_val.to_string()).unwrap());
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
        JS_ReportErrorUTF8(cx, b"process.nextTick() requires a callback\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let cb_val = *args.get(0).ptr;
    if !cb_val.is_object() {
        JS_ReportErrorUTF8(cx, b"process.nextTick() callback must be a function\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let cb_val_obj = mozjs::jsval::ObjectValue(cb_val.to_object());
    let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val_obj };
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let global_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let empty_args = HandleValueArray::empty();
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    JS_CallFunctionValue(cx, global_handle, cb_h, &empty_args, rval_handle);

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
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let sec = now.as_secs() as i32;
    let nsec = now.subsec_nanos() as i32;

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

    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_uptime(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let uptime_secs = match PROCESS_START.with(|s| s.borrow().clone()) {
        Some(start) => {
            let now = ::std::time::Instant::now();
            now.duration_since(start).as_secs_f64()
        }
        None => 0.0,
    };
    args.rval().set(mozjs::jsval::DoubleValue(uptime_secs));
    true
}

thread_local! {
    static PROCESS_START: RefCell<Option<::std::time::Instant>> = RefCell::new(None);
}

pub fn init_process_start() {
    PROCESS_START.with(|s| *s.borrow_mut() = Some(::std::time::Instant::now()));
}

pub fn install_buffer_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let buf_obj = JS_NewPlainObject(cx));
        if buf_obj.get().is_null() {
            return;
        }

        JS_DefineFunction(
            cx, buf_obj.handle(), c"from".as_ptr(),
            ::std::option::Option::Some(buffer_from), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"alloc".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"isBuffer".as_ptr(),
            ::std::option::Option::Some(buffer_is_buffer), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"concat".as_ptr(),
            ::std::option::Option::Some(buffer_concat), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"allocUnsafe".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(cx, global, c"Buffer".as_ptr(), buf_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_from(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }

    let input = *args.get(0).ptr;
    if input.is_string() {
        let js_str = ::std::ptr::NonNull::new(input.to_string()).unwrap();
        let s = mozjs::conversions::jsstr_to_string(cx, js_str);
        let bytes = s.as_bytes();
        create_buffer_from_bytes(cx, &args, bytes)
    } else if input.is_object() {
        let obj = input.to_object();
        let obj_handle = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &obj,
        };
        let mut length_val = UndefinedValue();
        let length_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut length_val,
        };
        JS_GetProperty(cx, obj_handle, c"length".as_ptr(), length_handle);
        let len = if length_val.is_int32() { length_val.to_int32() as usize } else { 0 };

        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            let mut elem = UndefinedValue();
            let elem_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            };
            JS_GetElement(cx, obj_handle, i as u32, elem_handle);
            bytes.push(if elem.is_int32() { elem.to_int32() as u8 } else { 0 });
        }
        create_buffer_from_bytes(cx, &args, &bytes)
    } else {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_buffer_from_bytes(
    cx: *mut JSContext,
    args: &CallArgs,
    bytes: &[u8],
) -> bool {
    let buf_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_obj };

    let length_val = Int32Value(bytes.len() as i32);
    let length_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &length_val };
    JS_DefineProperty(cx, obj_handle, c"length".as_ptr(), length_handle, JSPROP_ENUMERATE as u32);

    let marker_val = Int32Value(1);
    let marker_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &marker_val };
    JS_DefineProperty(cx, obj_handle, c"_isBuffer".as_ptr(), marker_handle, 0);

    for (i, &byte) in bytes.iter().enumerate() {
        let val = Int32Value(byte as i32);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineElement(cx, obj_handle, i as u32, val_handle, JSPROP_ENUMERATE as u32);
    }

    let to_string_fn = JS_NewFunction(cx, Some(buffer_to_string), 0, 0, c"toString".as_ptr());
    if !to_string_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(to_string_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"toString".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    let slice_fn = JS_NewFunction(cx, Some(buffer_slice), 2, 0, c"slice".as_ptr());
    if !slice_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(slice_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"slice".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    let copy_fn = JS_NewFunction(cx, Some(buffer_copy), 1, 0, c"copy".as_ptr());
    if !copy_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(copy_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"copy".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_to_string(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut length_val = UndefinedValue();
    let length_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut length_val };
    JS_GetProperty(cx, obj_handle, c"length".as_ptr(), length_handle);

    let len = if length_val.is_int32() { length_val.to_int32() as usize } else { 0 };
    let mut bytes = Vec::with_capacity(len);
    for i in 0..len {
        let mut elem = UndefinedValue();
        let elem_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem };
        JS_GetElement(cx, obj_handle, i as u32, elem_handle);
        bytes.push(if elem.is_int32() { elem.to_int32() as u8 } else { 0 });
    }

    let s = String::from_utf8_lossy(&bytes).into_owned();
    let Ok(c_s) = ::std::ffi::CString::new(s) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_alloc(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let size = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32().max(0) as usize } else { 0 }
    } else { 0 };

    create_buffer_from_bytes(cx, &args, &vec![0u8; size])
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_buffer(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let v = *args.get(0).ptr;
    if !v.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = v.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut marker = UndefinedValue();
    let marker_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut marker };
    JS_GetProperty(_cx, obj_handle, c"_isBuffer".as_ptr(), marker_handle);
    args.rval().set(mozjs::jsval::BooleanValue(marker.is_int32() && marker.to_int32() == 1));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_concat(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        create_buffer_from_bytes(cx, &args, &[])
    } else {
        let list_val = *args.get(0).ptr;
        if !list_val.is_object() {
            create_buffer_from_bytes(cx, &args, &[])
        } else {
            let list_obj = list_val.to_object();
            let list_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &list_obj };
            let mut len_val = UndefinedValue();
            JS_GetProperty(cx, list_h, c"length".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
            });
            let list_len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

            let mut all_bytes = Vec::new();
            for i in 0..list_len {
                let mut elem = UndefinedValue();
                JS_GetElement(cx, list_h, i as u32, MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut elem,
                });
                if elem.is_object() {
                    let buf_obj = elem.to_object();
                    let buf_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_obj };
                    let mut blen = UndefinedValue();
                    JS_GetProperty(cx, buf_h, c"length".as_ptr(), MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData, ptr: &mut blen,
                    });
                    let b_len = if blen.is_int32() { blen.to_int32() as usize } else { 0 };
                    for j in 0..b_len {
                        let mut byte_val = UndefinedValue();
                        JS_GetElement(cx, buf_h, j as u32, MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
                        });
                        all_bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
                    }
                }
            }
            create_buffer_from_bytes(cx, &args, &all_bytes)
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_slice(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

    let start = if argc > 0 && (*args.get(0).ptr).is_int32() {
        let s = (*args.get(0).ptr).to_int32();
        if s < 0 { (len as i32 + s).max(0) as usize } else { s.min(len as i32) as usize }
    } else { 0 };

    let end = if argc > 1 && (*args.get(1).ptr).is_int32() {
        let e = (*args.get(1).ptr).to_int32();
        if e < 0 { (len as i32 + e).max(0) as usize } else { e.min(len as i32) as usize }
    } else { len };

    let mut bytes = Vec::new();
    for i in start..end.min(len) {
        let mut byte_val = UndefinedValue();
        JS_GetElement(cx, obj_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
        });
        bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
    }
    create_buffer_from_bytes(cx, &args, &bytes)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_copy(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() || argc == 0 {
        args.rval().set(Int32Value(0));
        return true;
    }

    let src_obj = this.to_object();
    let src_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &src_obj };
    let mut src_len_val = UndefinedValue();
    JS_GetProperty(cx, src_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut src_len_val,
    });
    let src_len = if src_len_val.is_int32() { src_len_val.to_int32() as usize } else { 0 };

    let target_val = *args.get(0).ptr;
    if !target_val.is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let tgt_obj = target_val.to_object();
    let tgt_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &tgt_obj };

    let tgt_start = if argc > 1 && (*args.get(1).ptr).is_int32() {
        (*args.get(1).ptr).to_int32().max(0) as usize
    } else { 0 };

    let mut copied = 0usize;
    for i in tgt_start..src_len {
        let mut byte_val = UndefinedValue();
        JS_GetElement(cx, src_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
        });
        let b = if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 };
        let b_val = Int32Value(b as i32);
        let b_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &b_val };
        JS_SetElement(cx, tgt_h, i as u32, b_handle);
        copied += 1;
    }
    args.rval().set(Int32Value(copied as i32));
    true
}

pub fn install_crypto_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let crypto_obj = JS_NewPlainObject(cx));
        if crypto_obj.get().is_null() {
            return;
        }

        // crypto.randomUUID()
        JS_DefineFunction(cx, crypto_obj.handle(), c"randomUUID".as_ptr(), Some(crypto_random_uuid), 0, JSPROP_ENUMERATE as u32);

        // crypto.getRandomValues()
        JS_DefineFunction(cx, crypto_obj.handle(), c"getRandomValues".as_ptr(), Some(crypto_get_random_values), 1, JSPROP_ENUMERATE as u32);

        // crypto.subtle — Web Crypto API subset
        {
            rooted!(&in(cx) let subtle_obj = JS_NewPlainObject(cx));
            if !subtle_obj.get().is_null() {
                JS_DefineFunction(cx, subtle_obj.handle(), c"digest".as_ptr(), Some(crypto_subtle_digest), 2, JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, crypto_obj.handle(), c"subtle".as_ptr(), subtle_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        JS_DefineProperty3(cx, global, c"crypto".as_ptr(), crypto_obj.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_uuid(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let uuid = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        rand::random::<u32>(),
        rand::random::<u16>(),
        (rand::random::<u16>() & 0x0fff) | 0x4000,
        (rand::random::<u16>() & 0x3fff) | 0x8000,
        rand::random::<u64>() & 0xffffffffffff);
    let Ok(c_uuid) = CString::new(uuid) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let js_str = JS_NewStringCopyZ(_cx, c_uuid.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_random_values(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_val = *args.get(0).ptr;
    if !arr_val.is_object() {
        args.rval().set(arr_val);
        return true;
    }
    let arr = arr_val.to_object();
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr };
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32().max(0) as usize } else { 0 };

    let mut buf = vec![0u8; len];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut buf);
    for (i, &byte) in buf.iter().enumerate() {
        let v = Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_SetElement(cx, arr_h, i as u32, v_h);
    }
    args.rval().set(arr_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_subtle_digest(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, b"crypto.subtle.digest requires algorithm and data\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let algo_val = *args.get(0).ptr;
    let algo = if algo_val.is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked(algo_val.to_string())).to_lowercase()
    } else {
        "sha-256".to_string()
    };

    let data_val = *args.get(1).ptr;
    let bytes = if data_val.is_object() {
        let obj = data_val.to_object();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let mut len_val = UndefinedValue();
        JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
        });
        let len = if len_val.is_int32() { len_val.to_int32().max(0) as usize } else { 0 };
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(cx, obj_h, i as u32, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut elem,
            });
            v.push(if elem.is_int32() { elem.to_int32() as u8 } else { 0 });
        }
        v
    } else if data_val.is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked(data_val.to_string())).into_bytes()
    } else {
        Vec::new()
    };

    let hash = match algo.as_str() {
        "sha-1" | "sha1" => sha1::Sha1::digest(&bytes).to_vec(),
        "sha-256" | "sha256" => sha2::Sha256::digest(&bytes).to_vec(),
        "sha-384" | "sha384" => sha2::Sha384::digest(&bytes).to_vec(),
        "sha-512" | "sha512" => sha2::Sha512::digest(&bytes).to_vec(),
        _ => {
            let msg = format!("Unsupported algorithm: {}", algo);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let arr_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr_obj };
    let lv = Int32Value(hash.len() as i32);
    let lv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &lv };
    JS_DefineProperty(cx, arr_h, c"length".as_ptr(), lv_h, JSPROP_ENUMERATE as u32);
    for (i, &byte) in hash.iter().enumerate() {
        let v = Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_DefineElement(cx, arr_h, i as u32, v_h, JSPROP_ENUMERATE as u32);
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr_obj));
    true
}

pub fn install_websocket_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ws_fun = JS_NewFunction(cx.raw_cx(), Some(websocket_constructor), 1, JSFUN_CONSTRUCTOR as u32, c"WebSocket".as_ptr());
        if !ws_fun.is_null() {
            let ctor_obj = JS_GetFunctionObject(ws_fun);
            if !ctor_obj.is_null() {
                let val = mozjs::jsval::ObjectValue(ctor_obj);
                rooted!(&in(cx) let v = val);
                JS_DefineProperty(cx.raw_cx(), global.into(), c"WebSocket".as_ptr(), v.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);

                // WebSocket.CONNECTING = 0, OPEN = 1, CLOSING = 2, CLOSED = 3
                let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor_obj };
                for (name, value) in &[("CONNECTING", 0i32), ("OPEN", 1), ("CLOSING", 2), ("CLOSED", 3)] {
                    let c_name = CString::new(*name).unwrap_or_default();
                    let v = Int32Value(*value);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(cx.raw_cx(), ctor_h, c_name.as_ptr(), v_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
                }
            }
        }
    }
}

thread_local! {
    static WS_CONNECTIONS: RefCell<Vec<*mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<::std::net::TcpStream>>>> = RefCell::new(Vec::new());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn websocket_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"WebSocket requires a URL argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, b"WebSocket URL must be a string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let url = jsstr_to_string(cx, NonNull::new_unchecked(url_val.to_string()));

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let ws_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if ws_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Set url property
    if let Ok(c_url) = CString::new(url.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj.get() };
            JS_DefineProperty(cx, obj_h, c"url".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
        }
    }

    // readyState = CONNECTING initially
    let state_val = Int32Value(0);
    let state_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &state_val };
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj.get() };
    JS_DefineProperty(cx, obj_h, c"readyState".as_ptr(), state_h, JSPROP_ENUMERATE as u32);
    let ba_val = Int32Value(0);
    let ba_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ba_val };
    JS_DefineProperty(cx, obj_h, c"bufferedAmount".as_ptr(), ba_h, JSPROP_ENUMERATE as u32);

    // onopen, onmessage, onerror, onclose — placeholder properties
    for name in &["onopen", "onmessage", "onerror", "onclose"] {
        let c_name = CString::new(*name).unwrap_or_default();
        let ud = UndefinedValue();
        let ud_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ud };
        JS_DefineProperty(cx, obj_h, c_name.as_ptr(), ud_h, JSPROP_ENUMERATE as u32);
    }

    // Attempt connection
    match tungstenite::client::connect(url.as_str()) {
        Ok((mut socket, _response)) => {
            // Update readyState to OPEN
            JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &Int32Value(1),
            });

            WS_CONNECTIONS.with(|c| c.borrow_mut().push(Box::into_raw(Box::new(socket))));
            let _ = obj_h;
        }
        Err(e) => {
            let msg = format!("WebSocket connection failed: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }

    args.rval().set(mozjs::jsval::ObjectValue(ws_obj.get()));
    true
}

pub fn install_performance(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let perf_obj = JS_NewPlainObject(cx));
        if perf_obj.get().is_null() {
            return;
        }
        JS_DefineFunction(cx, perf_obj.handle(), c"now".as_ptr(), Some(performance_now), 0, JSPROP_ENUMERATE as u32);
        JS_DefineProperty3(cx, global, c"performance".as_ptr(), perf_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn performance_now(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ms = now.as_secs_f64() * 1000.0;
    args.rval().set(mozjs::jsval::DoubleValue(ms));
    true
}

pub unsafe fn install_all(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    install_bun_global(cx, global);
    install_process_global(cx, global);
    install_buffer_global(cx, global);
    install_fetch_global(cx, global);
    install_response_constructor(cx, global);
    install_headers_constructor(cx, global);
    crate::require::install_require(cx, global);
    crate::timers::install_timer_globals(cx, global);
    install_performance(cx, global);
    install_websocket_constructor(cx, global);
    install_crypto_global(cx, global);
    crate::node_events::install(cx);
    crate::node_path::install(cx);
    crate::node_fs::install(cx);
    crate::node_crypto::install(cx);
    crate::node_http::install(cx);
    crate::node_os::install(cx);
    crate::node_url::install(cx, global);
    crate::node_util::install_util(cx);
    crate::node_util::install_assert(cx);
    crate::node_child_process::install(cx);
    crate::node_stream::install(cx);
    crate::node_net::install(cx);
    install_web_encodings(cx, global);
    install_queue_microtask(cx, global);
}

pub fn install_fetch_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"fetch".as_ptr(),
            ::std::option::Option::Some(fetch_fn), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fetch_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"fetch requires a URL argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, b"fetch requires a string URL\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let url = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(url_val.to_string()).unwrap());

    let method = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut m_val = UndefinedValue();
            let m_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut m_val };
            JS_GetProperty(cx, obj_handle, c"method".as_ptr(), m_handle);
            if m_val.is_string() {
                mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(m_val.to_string()).unwrap()).to_uppercase()
            } else {
                "GET".to_string()
            }
        } else {
            "GET".to_string()
        }
    } else {
        "GET".to_string()
    };

    let body = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut b_val = UndefinedValue();
            let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut b_val };
            JS_GetProperty(cx, obj_handle, c"body".as_ptr(), b_handle);
            if b_val.is_string() {
                Some(mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(b_val.to_string()).unwrap()))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let response = match do_fetch(&url, &method, body.as_deref()) {
        Ok(resp) => resp,
        Err(e) => {
            let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
            if !promise.is_null() {
                let msg = format!("fetch failed: {}", e);
                let Ok(c_msg) = ::std::ffi::CString::new(msg) else {
                    args.rval().set(mozjs::jsval::ObjectValue(promise));
                    return true;
                };
                let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                if !err_obj.is_null() {
                    let err_msg = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                    if !err_msg.is_null() {
                        let msg_val = StringValue(&*err_msg);
                        let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
                        let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
                        JS_SetProperty(cx, err_h, c"message".as_ptr(), msg_h);
                    }
                }
                let err_val = mozjs::jsval::ObjectValue(err_obj);
                let err_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
                let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
                mozjs_sys::jsapi::JS::RejectPromise(cx, promise_h, err_handle);
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise));
            return true;
        }
    };

    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(response.status_code as i32);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(response.status_code >= 200 && response.status_code < 300);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    if let Ok(c_url) = ::std::ffi::CString::new(response.url.as_str()) {
        let url_js = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !url_js.is_null() {
            let url_val = StringValue(&*url_js);
            let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
            JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
        }
    }

    if let Ok(c_st) = ::std::ffi::CString::new(response.status_text.as_str()) {
        let st_js = JS_NewStringCopyZ(cx, c_st.as_ptr());
        if !st_js.is_null() {
            let st_val = StringValue(&*st_js);
            let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
            JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
        }
    }

    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !headers_obj.is_null() {
        let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };
        for (key, value) in &response.headers {
            let Ok(c_key) = ::std::ffi::CString::new(key.as_str()) else { continue };
            let Ok(c_val) = ::std::ffi::CString::new(value.as_str()) else { continue };
            let val_js = JS_NewStringCopyZ(cx, c_val.as_ptr());
            if !val_js.is_null() {
                let hv = StringValue(&*val_js);
                let hv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hv };
                JS_DefineProperty(cx, h_handle, c_key.as_ptr(), hv_handle, JSPROP_ENUMERATE as u32);
            }
        }
        let hdrs_val = mozjs::jsval::ObjectValue(headers_obj);
        let hdrs_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), hdrs_handle, JSPROP_ENUMERATE as u32);
    }

    let Ok(c_body) = ::std::ffi::CString::new(response.body.clone()) else {
        args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
        return true;
    };
    let body_str = JS_NewStringCopyZ(cx, c_body.as_ptr());
    if !body_str.is_null() {
        let body_val = StringValue(&*body_str);
        let bt_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &body_val };
        JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bt_handle, 0);
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    let resp_val = mozjs::jsval::ObjectValue(resp_obj);
    let resp_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_val };
    let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, resp_handle);

    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

struct FetchResponse {
    status_code: u16,
    body: String,
    headers: Vec<(String, String)>,
    url: String,
    status_text: String,
}

fn do_fetch(url: &str, method: &str, body: Option<&str>) -> ::std::result::Result<FetchResponse, String> {
    let req = match method {
        "POST" => minreq::post(url),
        "PUT" => minreq::put(url),
        "DELETE" => minreq::delete(url),
        "PATCH" => minreq::patch(url),
        "HEAD" => minreq::head(url),
        _ => minreq::get(url),
    };

    let req = if let Some(b) = body {
        req.with_body(b)
    } else {
        req
    };

    let resp = req.send().map_err(|e| format!("{}", e))?;
    let headers: Vec<(String, String)> = resp.headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    ::std::result::Result::Ok(FetchResponse {
        status_code: resp.status_code as u16,
        body: resp.as_str().unwrap_or("").to_string(),
        headers,
        url: url.to_string(),
        status_text: String::new(),
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);
    args.rval().set(body_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_json(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, b"response.json(): invalid this\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);

    if !body_val.is_string() {
        JS_ReportErrorUTF8(cx, b"response.json(): body is not a string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let js_str = body_val.to_string();
    let str_handle = Handle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &js_str };
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_handle, rval_handle);

    if !ok {
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, b"response.json(): invalid JSON\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    args.rval().set(rval);
    true
}

pub fn install_response_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(response_constructor), 2, JSFUN_CONSTRUCTOR as u32, c"Response".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Response".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(200);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(true);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    let url_js_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    if !url_js_str.is_null() {
        let url_val = StringValue(&*url_js_str);
        let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
        JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
    }

    let st_js_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    if !st_js_str.is_null() {
        let st_val = StringValue(&*st_js_str);
        let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
        JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
    }

    let empty_headers = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !empty_headers.is_null() {
        let h_val = mozjs::jsval::ObjectValue(empty_headers);
        let h_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &h_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), h_handle, JSPROP_ENUMERATE as u32);
    }

    if argc > 0 {
        let body_val = *args.get(0).ptr;
        if body_val.is_string() {
            let body_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(body_val.to_string()).unwrap());
            if let Ok(c_body) = ::std::ffi::CString::new(body_str.as_str()) {
                let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
                if !body_js.is_null() {
                    let bv = StringValue(&*body_js);
                    let bv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bv };
                    JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bv_handle, 0);
                }
            }
        }
    }

    if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let opts_obj = opts.to_object();
            let opts_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };
            let mut st_val = UndefinedValue();
            let st_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut st_val };
            JS_GetProperty(cx, opts_handle, c"status".as_ptr(), st_mh);
            if st_val.is_int32() {
                let st_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
                JS_SetProperty(cx, obj_handle, c"status".as_ptr(), st_h);
                let ok = mozjs::jsval::BooleanValue(st_val.to_int32() >= 200 && st_val.to_int32() < 300);
                let ok_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok };
                JS_SetProperty(cx, obj_handle, c"ok".as_ptr(), ok_h);
            }
        }
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
    true
}

pub fn install_headers_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(headers_constructor), 1, JSFUN_CONSTRUCTOR as u32, c"Headers".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Headers".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if headers_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };

    let get_fn = JS_NewFunction(cx, Some(headers_get), 1, 0, c"get".as_ptr());
    if !get_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(get_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"get".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let set_fn = JS_NewFunction(cx, Some(headers_set), 2, 0, c"set".as_ptr());
    if !set_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(set_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"set".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let has_fn = JS_NewFunction(cx, Some(headers_has), 1, 0, c"has".as_ptr());
    if !has_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(has_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"has".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(headers_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_get(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(name_js).unwrap());
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    };
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    if val.is_undefined() || val.is_null() {
        args.rval().set(mozjs::jsval::NullValue());
    } else {
        args.rval().set(val);
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_set(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, b"Headers.set requires name and value\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let name_val = *args.get(0).ptr;
    let value_val = *args.get(1).ptr;
    if !name_val.is_string() || !value_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Headers.set requires string arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let name_js = name_val.to_string();
    let name_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(name_js).unwrap());
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &value_val };
    JS_SetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_has(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(name_js).unwrap());
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    };
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(mozjs::jsval::BooleanValue(!val.is_undefined() && !val.is_null()));
    true
}

fn install_web_encodings(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let te_fun = JS_NewFunction(cx.raw_cx(), Some(text_encoder_constructor), 0, JSFUN_CONSTRUCTOR as u32, c"TextEncoder".as_ptr());
        if !te_fun.is_null() {
            let te_obj = JS_GetFunctionObject(te_fun);
            if !te_obj.is_null() {
                rooted!(&in(cx) let te_obj_r = te_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(cx, proto.handle(), c"encode".as_ptr(), Some(text_encoder_encode), 1, JSPROP_ENUMERATE as u32);
                    JS_DefineFunction(cx, proto.handle(), c"encodeInto".as_ptr(), Some(text_encoder_encode_into), 2, JSPROP_ENUMERATE as u32);
                    JS_DefineProperty3(cx, te_obj_r.handle(), c"prototype".as_ptr(), proto.handle(), JSPROP_PERMANENT as u32);
                }
                JS_DefineProperty3(cx, global, c"TextEncoder".as_ptr(), te_obj_r.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }

        let td_fun = JS_NewFunction(cx.raw_cx(), Some(text_decoder_constructor), 1, JSFUN_CONSTRUCTOR as u32, c"TextDecoder".as_ptr());
        if !td_fun.is_null() {
            let td_obj = JS_GetFunctionObject(td_fun);
            if !td_obj.is_null() {
                rooted!(&in(cx) let td_obj_r = td_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(cx, proto.handle(), c"decode".as_ptr(), Some(text_decoder_decode), 1, JSPROP_ENUMERATE as u32);
                    JS_DefineProperty3(cx, td_obj_r.handle(), c"prototype".as_ptr(), proto.handle(), JSPROP_PERMANENT as u32);
                }
                JS_DefineProperty3(cx, global, c"TextDecoder".as_ptr(), td_obj_r.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

fn install_queue_microtask(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(cx, global, c"queueMicrotask".as_ptr(), Some(queue_microtask_fn), 1, JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding_str = JS_NewStringCopyZ(cx, b"utf-8\0".as_ptr() as *const ::std::os::raw::c_char);
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c"encoding".as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"encode".as_ptr(), Some(text_encoder_encode), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"encodeInto".as_ptr(), Some(text_encoder_encode_into), 2, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            jsstr_to_string(cx, NonNull::new(v.to_string()).unwrap())
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let bytes = input.as_bytes();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr = NewArrayObject1(&mut wrapped_cx, bytes.len()));

    for (i, &byte) in bytes.iter().enumerate() {
        let val = Int32Value(byte as i32);
        rooted!(&in(wrapped_cx) let v = val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let global = CurrentGlobalOrNull(cx);
    if !global.is_null() {
        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
        let mut buf_ctor = UndefinedValue();
        JS_GetProperty(cx, global_h, c"Uint8Array".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut buf_ctor });
        if buf_ctor.is_object() {
            let arr_val = ObjectValue(arr.get());
            rooted!(&in(wrapped_cx) let av = arr_val);
            let call_args = HandleValueArray { length_: 1, elements_: &av.get() };
            let ctor_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_ctor };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_h, ctor_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            if rval.is_object() {
                args.rval().set(rval);
                return true;
            }
        }
    }

    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode_into(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { jsstr_to_string(cx, NonNull::new(v.to_string()).unwrap()) } else { "utf-8".to_string() }
    } else {
        "utf-8".to_string()
    };
    let encoding_lower = encoding.to_lowercase();
    let encoding_str = JS_NewStringCopyZ(cx, CString::new(encoding_lower).unwrap_or_default().as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c"encoding".as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }
    let fatal_val = BooleanValue(false);
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let fatal_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fatal_val };
    JS_DefineProperty(cx, obj_h, c"fatal".as_ptr(), fatal_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    let bom_val = BooleanValue(false);
    let bom_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bom_val };
    JS_DefineProperty(cx, obj_h, c"ignoreBOM".as_ptr(), bom_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"decode".as_ptr(), Some(text_decoder_decode), 1, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_decode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let empty = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
        args.rval().set(if empty.is_null() { UndefinedValue() } else { StringValue(&*empty) });
        return true;
    }

    let input = *args.get(0).ptr;

    let bytes = if input.is_object() {
        let obj = input.to_object();
        let mut len_val = UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
        let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { 0 };
        let mut result = Vec::new();
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(cx, obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
            if elem.is_int32() {
                result.push(elem.to_int32() as u8);
            }
        }
        result
    } else {
        Vec::new()
    };

    let decoded = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            JS_ReportErrorUTF8(cx, b"The encoded data was not valid\0".as_ptr() as *const ::std::os::raw::c_char);
            return false;
        }
    };

    let utf16: Vec<u16> = decoded.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn queue_microtask_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return true;
    }
    let callback = (*args.get(0).ptr).to_object();
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return true;
    }
    let cb_val = ObjectValue(callback);
    let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
    let empty_args = HandleValueArray::empty();
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    JS_CallFunctionValue(cx, global_h, cb_h, &empty_args, rval_h);
    args.rval().set(UndefinedValue());
    true
}