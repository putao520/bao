// Bun.* namespace + process global + servers + test runner
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ffi::CString;
use ::std::fs;
use ::std::io::Read;
use ::std::path;
use base64::Engine;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, NullValue, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1,
};
use mozjs::conversions::jsstr_to_string;

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

        // Bun.env → copy of process.env (same data source)
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
                JS_DefineProperty3(cx, bun_obj.handle(), c"env".as_ptr(), env_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // Bun.argv → process.argv (same data source)
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
                JS_DefineProperty3(cx, bun_obj.handle(), c"argv".as_ptr(), argv_arr.handle(), JSPROP_ENUMERATE as u32);
            }
        }

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

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"serve".as_ptr(),
            ::std::option::Option::Some(bun_serve),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"spawn".as_ptr(),
            ::std::option::Option::Some(bun_spawn),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"cwd".as_ptr(),
            ::std::option::Option::Some(bun_cwd),
            0,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"gc".as_ptr(),
            ::std::option::Option::Some(bun_gc),
            0,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"sleep".as_ptr(),
            ::std::option::Option::Some(bun_sleep),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"which".as_ptr(),
            ::std::option::Option::Some(bun_which),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"inspect".as_ptr(),
            ::std::option::Option::Some(bun_inspect),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"resolve".as_ptr(),
            ::std::option::Option::Some(bun_resolve),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"build".as_ptr(),
            ::std::option::Option::Some(bun_build),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"test".as_ptr(),
            ::std::option::Option::Some(bun_test),
            2,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"testRun".as_ptr(),
            ::std::option::Option::Some(test_run),
            0,
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

        // process.argv
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

        // process.env
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

        // process.version
        {
            let ver_str = JS_NewStringCopyZ(cx.raw_cx(), b"v18.0.0\0".as_ptr() as *const ::std::os::raw::c_char);
            if !ver_str.is_null() {
                rooted!(&in(cx) let v = StringValue(&*ver_str));
                JS_DefineProperty(cx.raw_cx(), proc_obj.handle().into(), c"version".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.versions
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

        // process.stdout
        {
            rooted!(&in(cx) let stdout_obj = JS_NewPlainObject(cx));
            if !stdout_obj.get().is_null() {
                let fd_val = Int32Value(1);
                rooted!(&in(cx) let fd = fd_val);
                JS_DefineProperty(cx.raw_cx(), stdout_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
                let is_tty = libc::isatty(1) == 1;
                let tty_val = BooleanValue(is_tty);
                rooted!(&in(cx) let tv = tty_val);
                JS_DefineProperty(cx.raw_cx(), stdout_obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
                JS_DefineFunction(cx, stdout_obj.handle(), c"write".as_ptr(), ::std::option::Option::Some(process_stdout_write), 1, JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, proc_obj.handle(), c"stdout".as_ptr(), stdout_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }
        // process.stderr
        {
            rooted!(&in(cx) let stderr_obj = JS_NewPlainObject(cx));
            if !stderr_obj.get().is_null() {
                let fd_val = Int32Value(2);
                rooted!(&in(cx) let fd = fd_val);
                JS_DefineProperty(cx.raw_cx(), stderr_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
                let is_tty = libc::isatty(2) == 1;
                let tty_val = BooleanValue(is_tty);
                rooted!(&in(cx) let tv = tty_val);
                JS_DefineProperty(cx.raw_cx(), stderr_obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
                JS_DefineFunction(cx, stderr_obj.handle(), c"write".as_ptr(), ::std::option::Option::Some(process_stderr_write), 1, JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, proc_obj.handle(), c"stderr".as_ptr(), stderr_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.stdin
        {
            rooted!(&in(cx) let stdin_obj = JS_NewPlainObject(cx));
            if !stdin_obj.get().is_null() {
                let fd_val = Int32Value(0);
                rooted!(&in(cx) let fd = fd_val);
                JS_DefineProperty(cx.raw_cx(), stdin_obj.handle().into(), c"fd".as_ptr(), fd.handle().into(), JSPROP_ENUMERATE as u32);
                let is_tty = libc::isatty(0) == 1;
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
                JS_DefineProperty3(cx, proc_obj.handle(), c"stdin".as_ptr(), stdin_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.on()
        JS_DefineFunction(cx, proc_obj.handle(), c"on".as_ptr(), ::std::option::Option::Some(process_on), 2, JSPROP_ENUMERATE as u32);

        // process.nextTick()
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

        // process.hrtime()
        JS_DefineFunction(cx, proc_obj.handle(), c"hrtime".as_ptr(), ::std::option::Option::Some(process_hrtime), 0, JSPROP_ENUMERATE as u32);

        // process.uptime()
        JS_DefineFunction(cx, proc_obj.handle(), c"uptime".as_ptr(), ::std::option::Option::Some(process_uptime), 0, JSPROP_ENUMERATE as u32);

        // process.chdir()
        JS_DefineFunction(cx, proc_obj.handle(), c"chdir".as_ptr(), ::std::option::Option::Some(process_chdir), 1, JSPROP_ENUMERATE as u32);

        // process.memoryUsage()
        JS_DefineFunction(cx, proc_obj.handle(), c"memoryUsage".as_ptr(), ::std::option::Option::Some(process_memory_usage), 0, JSPROP_ENUMERATE as u32);

        // process.kill()
        JS_DefineFunction(cx, proc_obj.handle(), c"kill".as_ptr(), ::std::option::Option::Some(process_kill), 2, JSPROP_ENUMERATE as u32);

        // process.umask()
        JS_DefineFunction(cx, proc_obj.handle(), c"umask".as_ptr(), ::std::option::Option::Some(process_umask), 0, JSPROP_ENUMERATE as u32);

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
                JS_DefineProperty3(cx, proc_obj.handle(), c"config".as_ptr(), config_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        // process.argv0
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

        // process.execPath
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

        // process EventEmitter — on/once/addListener delegate to process_on
        // emit/off/removeListener/removeAllListeners use process_noop (accept and ignore)
        JS_DefineFunction(cx, proc_obj.handle(), c"on".as_ptr(), Some(process_on), 2, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, proc_obj.handle(), c"once".as_ptr(), Some(process_on), 2, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, proc_obj.handle(), c"addListener".as_ptr(), Some(process_on), 2, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, proc_obj.handle(), c"emit".as_ptr(), Some(process_noop), 1, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, proc_obj.handle(), c"off".as_ptr(), Some(process_noop), 2, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, proc_obj.handle(), c"removeListener".as_ptr(), Some(process_noop), 2, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, proc_obj.handle(), c"removeAllListeners".as_ptr(), Some(process_noop), 0, JSPROP_ENUMERATE as u32);
    }
}

thread_local! {
    static SPAWNED_PROCS: RefCell<Vec<*mut ::std::process::Child>> = RefCell::new(Vec::new());
}

struct TestCase {
    name: String,
    callback: *mut JSObject,
}

thread_local! {
    static TEST_REGISTRY: RefCell<Vec<TestCase>> = RefCell::new(Vec::new());
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

            let pid_val = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

            let exited_val = BooleanValue(false);
            rooted!(&in(cx_ref) let ev = exited_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

            let exit_code_val = Int32Value(-1);
            rooted!(&in(cx_ref) let ecv = exit_code_val);
            JS_DefineProperty(cx, subproc_obj.handle().into(), c"exitCode".as_ptr(), ecv.handle().into(), JSPROP_ENUMERATE as u32);

            let ptr_bits = child_ptr as u64;
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
unsafe extern "C" fn stdin_read(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut buf = [0u8; 4096];
    match ::std::io::stdin().lock().read(&mut buf) {
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
    static STDIN_LISTENERS: RefCell<Vec<*mut JSObject>> = RefCell::new(Vec::new());
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
    let args = CallArgs::from_vp(vp, argc);

    let mut port: u16 = 3000;
    let mut hostname = "0.0.0.0".to_string();
    let mut _fetch_handler: Option<*mut JSObject> = None;

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
                _fetch_handler = Some(fetch_val.to_object());
            }
        }
    }

    let addr = format!("{}:{}", hostname, port);
    let server = match tiny_http::Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Bun.serve() failed to bind: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let actual_port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
    eprint!("Bun.serve() listening on {}:{}\n", hostname, actual_port);

    let server_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if server_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let srv_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &server_obj };

    let port_jsval = Int32Value(actual_port as i32);
    let port_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &port_jsval };
    JS_DefineProperty(cx, srv_h, c"port".as_ptr(), port_h, JSPROP_ENUMERATE as u32);

    if let Ok(c_hn) = ::std::ffi::CString::new(hostname.as_str()) {
        let hn_str = JS_NewStringCopyZ(cx, c_hn.as_ptr());
        if !hn_str.is_null() {
            let hn_v = StringValue(&*hn_str);
            let hn_vh = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hn_v };
            JS_DefineProperty(cx, srv_h, c"hostname".as_ptr(), hn_vh, JSPROP_ENUMERATE as u32);
        }
    }

    static SRV_STOP: ::std::sync::atomic::AtomicBool = ::std::sync::atomic::AtomicBool::new(false);
    let stop_flag = &SRV_STOP;
    stop_flag.store(false, ::std::sync::atomic::Ordering::Relaxed);

    let bg_stop = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
    let bg_stop_clone = bg_stop.clone();
    let bg_port = actual_port;
    let bg_hostname = hostname.clone();

    ::std::thread::spawn(move || {
        loop {
            if bg_stop_clone.load(::std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            match server.recv_timeout(::std::time::Duration::from_millis(100)) {
                Ok(Some(req)) => {
                    let method = req.method().to_string().to_uppercase();
                    let url_path = req.url().to_string();

                    // Check for WebSocket upgrade
                    let is_ws_upgrade = req.headers().iter().any(|h| {
                        h.field.equiv("Upgrade") && h.value.as_str() == "websocket"
                    });

                    if is_ws_upgrade {
                        // Accept WebSocket upgrade using tungstenite
                        let ws_key = req.headers().iter().find_map(|h| {
                            if h.field.equiv("Sec-WebSocket-Key") {
                                Some(h.value.as_str().to_string())
                            } else { None }
                        }).unwrap_or_default();

                        let accept_key = {
                            use sha1::{Digest, Sha1};
                            let mut hasher = Sha1::new();
                            hasher.update(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", ws_key));
                            let result = hasher.finalize();
                            base64::engine::general_purpose::STANDARD.encode(result)
                        };

                        // Build and send upgrade response
                        let mut response = tiny_http::Response::new_empty(tiny_http::StatusCode(101));
                        response.add_header(tiny_http::Header::from_bytes(&b"Upgrade"[..], &b"websocket"[..]).expect("static header bytes"));
                        response.add_header(tiny_http::Header::from_bytes(&b"Connection"[..], &b"Upgrade"[..]).expect("static header bytes"));
                        response.add_header(tiny_http::Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept_key.as_bytes()).expect("valid accept key"));
                        let _ = req.respond(response);
                    } else {
                        // Regular HTTP response
                        let body = format!("{{\"method\":\"{}\",\"url\":\"{}\"}}", method, url_path);
                        let response = tiny_http::Response::from_string(body);
                        let _ = req.respond(response);
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
        eprint!("Bun.serve() stopped on {}:{}\n", bg_hostname, bg_port);
    });

    thread_local! {
        static SRV_STOP_HANDLES: RefCell<Vec<::std::sync::Arc<::std::sync::atomic::AtomicBool>>> = RefCell::new(Vec::new());
    }
    let stop_idx = SRV_STOP_HANDLES.with(|h| {
        let mut handles = h.borrow_mut();
        handles.push(bg_stop);
        handles.len() - 1
    });
    let stop_idx_val = Int32Value(stop_idx as i32);
    let stop_idx_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &stop_idx_val };
    JS_DefineProperty(cx, srv_h, c"_stopIdx".as_ptr(), stop_idx_h, 0);

    unsafe extern "C" fn server_stop(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let this_obj = args.thisv().to_object();
        let this_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };
        let mut idx_val = Int32Value(-1);
        JS_GetProperty(cx, this_h, c"_stopIdx".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut idx_val });
        let idx = idx_val.to_int32() as usize;
        SRV_STOP_HANDLES.with(|h| {
            let handles = h.borrow();
            if idx < handles.len() {
                handles[idx].store(true, ::std::sync::atomic::Ordering::Relaxed);
            }
        });
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

    args.rval().set(mozjs::jsval::ObjectValue(server_obj));
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
                let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
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
    let name = crate::js_to_rust_string(cx, name_val);

    let path_var = ::std::env::var("PATH").unwrap_or_default();
    let separator = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(separator) {
        let candidate = ::std::path::Path::new(dir).join(&name);
        if candidate.exists() {
            let result = candidate.to_string_lossy().into_owned();
            let Ok(c_result) = ::std::ffi::CString::new(result.as_str()) else {
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
                let Ok(c_result) = ::std::ffi::CString::new(result.as_str()) else {
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
        let rust_str = crate::js_to_rust_string(cx, val);
        format!("'{}'", rust_str)
    } else if val.is_object() {
        "[object]".to_string()
    } else {
        "undefined".to_string()
    };
    let Ok(c_s) = ::std::ffi::CString::new(s.as_str()) else {
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

            let ep_name = ::std::ffi::CString::new("entrypoints").expect("static ASCII");
            let mut has_ep: bool = false;
            JS_HasProperty(cx, cfg_h, ep_name.as_ptr(), &mut has_ep);
            if has_ep {
                let mut ep_val = UndefinedValue();
                let ep_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ep_val };
                JS_GetProperty(cx, cfg_h, ep_name.as_ptr(), ep_rv);
                if ep_val.is_object() {
                    let ep_obj = ep_val.to_object();
                    let mut len_val = UndefinedValue();
                    let len_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val };
                    let len_name = ::std::ffi::CString::new("length").expect("static ASCII");
                    let ep_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ep_obj };
                    JS_GetProperty(cx, ep_obj_h, len_name.as_ptr(), len_rv);
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

            let od_name = ::std::ffi::CString::new("outdir").expect("static ASCII");
            let mut has_od: bool = false;
            JS_HasProperty(cx, cfg_h, od_name.as_ptr(), &mut has_od);
            if has_od {
                let mut od_val = UndefinedValue();
                let od_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut od_val };
                JS_GetProperty(cx, cfg_h, od_name.as_ptr(), od_rv);
                if od_val.is_string() {
                    outdir = jsstr_to_string(cx, NonNull::new_unchecked(od_val.to_string()));
                }
            }

            let nm_name = ::std::ffi::CString::new("naming").expect("static ASCII");
            let mut has_nm: bool = false;
            JS_HasProperty(cx, cfg_h, nm_name.as_ptr(), &mut has_nm);
            if has_nm {
                let mut nm_val = UndefinedValue();
                let nm_rv = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut nm_val };
                JS_GetProperty(cx, cfg_h, nm_name.as_ptr(), nm_rv);
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
        let content = match fs::read_to_string(epath) {
            Ok(c) => c,
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

        let Ok(c_path) = ::std::ffi::CString::new(entry.as_str()) else { continue };
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
        let Ok(c_out) = ::std::ffi::CString::new(out_path.as_str()) else { continue };
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
        let Ok(c_kind) = ::std::ffi::CString::new(kind_str) else { continue };
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
            let Ok(c_err) = ::std::ffi::CString::new(error_msg.as_str()) else {
                args.rval().set(ObjectValue(result_obj));
                return true;
            };
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
            eprint!("\n\u{2713} {}\n", tc.name);
            passed += 1;
        } else {
            JS_ClearPendingException(cx);
            eprint!("\n\u{2717} {}\n", tc.name);
            failures.push(tc.name.clone());
            failed += 1;
        }
    }

    let total = passed + failed;
    eprint!("\n{} test(s) ran, {} passed, {} failed\n", total, passed, failed);

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
            let Ok(c_name) = ::std::ffi::CString::new(fname.as_str()) else { continue };
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
    let _path_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    let s = crate::js_to_rust_string(cx, path_val);
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
    let fpath = crate::js_to_rust_string(cx, path_val);
    let content = crate::js_to_rust_string(cx, content_val);
    match ::std::fs::write(fpath.as_str(), content.as_bytes()) {
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
    let fpath = crate::js_to_rust_string(cx, path_val);
    match ::std::fs::read_to_string(fpath.as_str()) {
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
unsafe extern "C" fn bun_cwd(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    process_cwd(cx, argc, vp)
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
        let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
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
        let s = crate::js_to_rust_string(cx, val);
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
        let s = crate::js_to_rust_string(cx, val);
        ::std::io::Write::write_all(&mut ::std::io::stderr(), s.as_bytes()).ok();
        ::std::io::Write::flush(&mut ::std::io::stderr()).ok();
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
        JS_ReportErrorUTF8(cx, b"process.nextTick() requires a callback\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let cb_val = *args.get(0).ptr;
    if !cb_val.is_object() {
        JS_ReportErrorUTF8(cx, b"process.nextTick() callback must be a function\0".as_ptr() as *const ::std::os::raw::c_char);
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
    let qmt_name = ::std::ffi::CString::new("queueMicrotask").unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
    JS_GetProperty(cx, global_h, qmt_name.as_ptr(), qmt_rv);

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
        let cb_name = ::std::ffi::CString::new("__nextTickCb").unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
        let cb_h_val = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val_obj };
        JS_SetProperty(cx, global_h, cb_name.as_ptr(), cb_h_val);

        let eval_src = "queueMicrotask(__nextTickCb); delete globalThis.__nextTickCb;";
        let _c_src = ::std::ffi::CString::new(eval_src).unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
        let c_filename = ::std::ffi::CString::new("<nextTick>").unwrap_or_else(|_| ::std::ffi::CString::new("<eval>").unwrap());
        let opts = mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(eval_src);
            let mut eval_rval = UndefinedValue();
            let eval_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut eval_rval };
            mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, eval_rval_h);
            libc::free(opts as *mut _);
        }
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
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_ns = (now.as_secs() as i64) * 1_000_000_000i64 + (now.subsec_nanos() as i64);
    let src = format!("BigInt(\"{}\")", total_ns);
    let mut rval = UndefinedValue();
    let opts = mozjs::glue::NewCompileOptions(cx, b"hrtime_bigint\0".as_ptr() as *const ::std::os::raw::c_char, 1);
    if !opts.is_null() {
        let mut eval_src = mozjs::rust::transform_str_to_source_text(&src);
        mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut eval_src, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
        libc::free(opts as *mut _);
    }
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
    let rss = ::std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &::std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .unwrap_or(0.0)
        * 1024.0;
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
        pid_val.to_int32() as i32
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
    let _ = libc::kill(pid, sig_num);
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
    let old = unsafe { libc::umask(0o022) };
    unsafe { libc::umask(old) };
    args.rval().set(Int32Value(old as i32));
    true
}

thread_local! {
    static PROCESS_START: RefCell<Option<::std::time::Instant>> = RefCell::new(None);
}

pub fn init_process_start() {
    PROCESS_START.with(|s| *s.borrow_mut() = Some(::std::time::Instant::now()));
}
