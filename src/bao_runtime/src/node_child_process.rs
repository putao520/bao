use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use mozjs::conversions::jsstr_to_string;

use crate::require::cache_builtin;

thread_local! {
    static CHILD_PROCS: RefCell<Vec<*mut ::std::process::Child>> = RefCell::new(Vec::new());
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"spawn".as_ptr(), Some(cp_spawn), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"exec".as_ptr(), Some(cp_exec), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"execSync".as_ptr(), Some(cp_exec_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"fork".as_ptr(), Some(cp_fork), 1, JSPROP_ENUMERATE as u32);

        w2::JS_DefineProperty3(cx, mod_obj.handle(), c"ChildProcess".as_ptr(), mod_obj.handle(), JSPROP_ENUMERATE as u32);
        cache_builtin(cx, "child_process", mod_obj.get());
    }
}

unsafe fn js_str_prop(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> Option<String> { unsafe {
    let mut val = UndefinedValue();
    JS_GetProperty(cx, obj_h, name, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
    if val.is_string() {
        Some(crate::js_to_rust_string(cx, val))
    } else {
        None
    }
}}

unsafe fn js_str_array_prop(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> Vec<String> { unsafe {
    let mut val = UndefinedValue();
    JS_GetProperty(cx, obj_h, name, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
    if !val.is_object() {
        return Vec::new();
    }
    let arr = val.to_object();
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr };
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
    let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { 0 };
    let mut result = Vec::with_capacity(len as usize);
    for i in 0..len {
        let mut elem = UndefinedValue();
        JS_GetElement(cx, arr_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
        if elem.is_string() {
            result.push(crate::js_to_rust_string(cx, elem));
        }
    }
    result
}}

unsafe fn js_stdio_mode(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> ::std::process::Stdio { unsafe {
    match js_str_prop(cx, obj_h, name).as_deref() {
        Some("pipe") => ::std::process::Stdio::piped(),
        Some("inherit") => ::std::process::Stdio::inherit(),
        Some("ignore") | Some("null") => ::std::process::Stdio::null(),
        _ => ::std::process::Stdio::piped(),
    }
}}

unsafe fn store_child_ptr(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, child: ::std::process::Child) { unsafe {
    let boxed = Box::new(child);
    let ptr = Box::into_raw(boxed);
    CHILD_PROCS.with(|p| p.borrow_mut().push(ptr));

    let ptr_bits = ptr as u64;
    let ptr_hi = (ptr_bits >> 32) as i32;
    let ptr_lo = (ptr_bits & 0xFFFFFFFF) as i32;
    let hi = Int32Value(ptr_hi);
    let hi_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hi };
    JS_DefineProperty(cx, obj_h, c"_ptrHi".as_ptr(), hi_h, 0);
    let lo = Int32Value(ptr_lo);
    let lo_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &lo };
    JS_DefineProperty(cx, obj_h, c"_ptrLo".as_ptr(), lo_h, 0);
}}

unsafe fn get_child_ptr(cx: *mut JSContext, obj_h: Handle<*mut JSObject>) -> Option<*mut ::std::process::Child> { unsafe {
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

unsafe fn build_command_from_opts(cx: *mut JSContext, opts_h: Handle<*mut JSObject>) -> Option<::std::process::Command> { unsafe {
    let cmd = js_str_prop(cx, opts_h, c"command".as_ptr())
        .or_else(|| js_str_prop(cx, opts_h, c"cmd".as_ptr()))?;
    let args = js_str_array_prop(cx, opts_h, c"args".as_ptr());
    let cwd = js_str_prop(cx, opts_h, c"cwd".as_ptr());

    let mut command = ::std::process::Command::new(&cmd);
    for arg in &args {
        command.arg(arg);
    }
    if let Some(ref dir) = cwd {
        command.current_dir(dir);
    }

    let stdin_mode = js_stdio_mode(cx, opts_h, c"stdio".as_ptr());
    command.stdout(js_stdio_mode(cx, opts_h, c"stdout".as_ptr()));
    command.stderr(js_stdio_mode(cx, opts_h, c"stderr".as_ptr()));
    command.stdin(stdin_mode);

    Some(command)
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_spawn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
        return false;
    }

    // First arg can be string (command) or object (options)
    let (cmd_str, opts_obj) = if argc > 0 {
        let first = *args.get(0).ptr;
        if first.is_string() {
            (Some(crate::js_to_rust_string(cx, first)), None)
        } else if first.is_object() {
            (None, Some(first.to_object()))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let second_obj = if argc > 1 {
        let second = *args.get(1).ptr;
        if second.is_object() { Some(second.to_object()) } else { None }
    } else {
        None
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
    if child_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let child_h = child_obj.handle().into();

    // Build command
    let mut command = if let Some(ref cmd) = cmd_str {
        let mut c = ::std::process::Command::new(cmd);
        if let Some(ref obj) = second_obj {
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: obj };
            let cargs = js_str_array_prop(cx, obj_h, c"args".as_ptr());
            // If second arg is array-like or has args
            for a in &cargs { c.arg(a); }
            let cwd = js_str_prop(cx, obj_h, c"cwd".as_ptr());
            if let Some(ref d) = cwd { c.current_dir(d); }
            c.stdout(js_stdio_mode(cx, obj_h, c"stdout".as_ptr()));
            c.stderr(js_stdio_mode(cx, obj_h, c"stderr".as_ptr()));
            c.stdin(js_stdio_mode(cx, obj_h, c"stdin".as_ptr()));
        } else {
            c.stdout(::std::process::Stdio::piped());
            c.stderr(::std::process::Stdio::piped());
        }
        c
    } else if let Some(ref obj) = opts_obj {
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: obj };
        match build_command_from_opts(cx, obj_h) {
            Some(c) => c,
            None => {
                JS_ReportErrorUTF8(cx, b"child_process.spawn: missing command\0".as_ptr() as *const ::std::os::raw::c_char);
                return false;
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, b"child_process.spawn requires arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    };

    match command.spawn() {
        Ok(child) => {
            let pid = child.id();
            store_child_ptr(cx, child_h, child);

            let pid_v = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_v);
            JS_DefineProperty(cx, child_h, c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

            let exited_v = BooleanValue(false);
            rooted!(&in(cx_ref) let ev = exited_v);
            JS_DefineProperty(cx, child_h, c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

            let ec_v = Int32Value(-1);
            rooted!(&in(cx_ref) let ecv = ec_v);
            JS_DefineProperty(cx, child_h, c"exitCode".as_ptr(), ecv.handle().into(), JSPROP_ENUMERATE as u32);

            let killed_v = BooleanValue(false);
            rooted!(&in(cx_ref) let kv = killed_v);
            JS_DefineProperty(cx, child_h, c"killed".as_ptr(), kv.handle().into(), JSPROP_ENUMERATE as u32);

            w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"wait".as_ptr(), Some(cp_child_wait), 0, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"kill".as_ptr(), Some(cp_child_kill), 0, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"stdout".as_ptr(), Some(cp_child_read_stdout), 0, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"stderr".as_ptr(), Some(cp_child_read_stderr), 0, JSPROP_ENUMERATE as u32);

            args.rval().set(ObjectValue(child_obj.get()));
        }
        Err(e) => {
            let msg = format!("spawn failed: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"child_process.exec requires a command string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
        return false;
    }

    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(cx, b"child_process.exec requires a string command\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let callback = if argc > 1 {
        let cb = *args.get(1).ptr;
        if cb.is_object() && JS_ObjectIsFunction(cb.to_object()) { Some(cb.to_object()) } else { None }
    } else {
        None
    };

    let cmd = crate::js_to_rust_string(cx, cmd_val);
    let shell = if cfg!(target_family = "unix") { "/bin/sh" } else { "cmd.exe" };
    let shell_flag = if cfg!(target_family = "unix") { "-c" } else { "/C" };

    let output = ::std::process::Command::new(shell)
        .arg(shell_flag)
        .arg(&cmd)
        .stdout(::std::process::Stdio::piped())
        .stderr(::std::process::Stdio::piped())
        .output();

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
    if child_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    match output {
        Ok(out) => {
            let stdout_str = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr_str = String::from_utf8_lossy(&out.stderr).into_owned();
            let exit_code = out.status.code().unwrap_or(-1);

            let child_h = child_obj.handle().into();

            if let Ok(c_stdout) = CString::new(stdout_str.as_str()) {
                let js_str = JS_NewStringCopyZ(cx, c_stdout.as_ptr());
                if !js_str.is_null() {
                    let v = StringValue(&*js_str);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(cx, child_h, c"stdout".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
                }
            }
            if let Ok(c_stderr) = CString::new(stderr_str.as_str()) {
                let js_str = JS_NewStringCopyZ(cx, c_stderr.as_ptr());
                if !js_str.is_null() {
                    let v = StringValue(&*js_str);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(cx, child_h, c"stderr".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
                }
            }
            let ec = Int32Value(exit_code);
            let ec_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ec };
            JS_DefineProperty(cx, child_h, c"exitCode".as_ptr(), ec_h, JSPROP_ENUMERATE as u32);

            // Call callback if provided: callback(error, stdout, stderr)
            if let Some(cb_obj) = callback {
                let global = CurrentGlobalOrNull(cx);
                let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

                let err_obj = if exit_code != 0 {
                    let e = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                    if !e.is_null() {
                        if let Ok(c_msg) = CString::new(format!("Command failed with exit code {}", exit_code)) {
                            let msg_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                            if !msg_str.is_null() {
                                let mv = StringValue(&*msg_str);
                                let mv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mv };
                                let e_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &e };
                                JS_SetProperty(cx, e_h, c"message".as_ptr(), mv_h);
                            }
                        }
                    }
                    e
                } else {
                    ::std::ptr::null_mut()
                };

                let mut call_vals: [Value; 3] = [
                    if err_obj.is_null() { NullValue() } else { ObjectValue(err_obj) },
                    UndefinedValue(),
                    UndefinedValue(),
                ];
                // Fill stdout/stderr from child_obj properties
                JS_GetProperty(cx, child_h, c"stdout".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut call_vals[1] });
                JS_GetProperty(cx, child_h, c"stderr".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut call_vals[2] });

                rooted!(&in(cx_ref) let cv0 = call_vals[0]);
                rooted!(&in(cx_ref) let cv1 = call_vals[1]);
                rooted!(&in(cx_ref) let cv2 = call_vals[2]);
                let elems = [&cv0.get(), &cv1.get(), &cv2.get()];
                let call_args = HandleValueArray {
                    length_: 3,
                    elements_: elems.as_ptr() as *const Value,
                };

                let cb_val = ObjectValue(cb_obj);
                let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };
                let mut rval = UndefinedValue();
                JS_CallFunctionValue(cx, global_h, cb_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            }

            args.rval().set(ObjectValue(child_obj.get()));
        }
        Err(e) => {
            let msg = format!("exec failed: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"child_process.execSync requires a command string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(cx, b"child_process.execSync requires a string command\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let cmd = crate::js_to_rust_string(cx, cmd_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
        return false;
    }

    let shell = if cfg!(target_family = "unix") { "/bin/sh" } else { "cmd.exe" };
    let shell_flag = if cfg!(target_family = "unix") { "-c" } else { "/C" };

    match ::std::process::Command::new(shell)
        .arg(shell_flag)
        .arg(&cmd)
        .stdout(::std::process::Stdio::piped())
        .stderr(::std::process::Stdio::piped())
        .output()
    {
        Ok(out) => {
            let stdout_str = String::from_utf8_lossy(&out.stdout).into_owned();
            if let Ok(c_out) = CString::new(stdout_str.as_str()) {
                let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
                if !js_str.is_null() {
                    args.rval().set(StringValue(&*js_str));
                    return true;
                }
            }
            args.rval().set(UndefinedValue());
        }
        Err(e) => {
            let msg = format!("execSync failed: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_fork(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // fork() spawns a new Node/Bao process with the given module
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"child_process.fork requires a module path\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let module_val = *args.get(0).ptr;
    if !module_val.is_string() {
        JS_ReportErrorUTF8(cx, b"child_process.fork requires a string module path\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let module = crate::js_to_rust_string(cx, module_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
        return false;
    }

    let executable = ::std::env::current_exe().unwrap_or_else(|_| ::std::path::PathBuf::from("bao"));

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    match ::std::process::Command::new(&executable)
        .arg("run")
        .arg(&module)
        .stdout(::std::process::Stdio::piped())
        .stderr(::std::process::Stdio::piped())
        .stdin(::std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
            rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
            if child_obj.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }

            let child_h = child_obj.handle().into();
            store_child_ptr(cx, child_h, child);

            let pid_v = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_v);
            JS_DefineProperty(cx, child_h, c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

            let exited_v = BooleanValue(false);
            rooted!(&in(cx_ref) let ev = exited_v);
            JS_DefineProperty(cx, child_h, c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

            w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"wait".as_ptr(), Some(cp_child_wait), 0, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"kill".as_ptr(), Some(cp_child_kill), 0, JSPROP_ENUMERATE as u32);

            args.rval().set(ObjectValue(child_obj.get()));
        }
        Err(e) => {
            let msg = format!("fork failed: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_child_wait(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(Int32Value(-1));
        return true;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let child_ptr = match get_child_ptr(cx, obj_h) {
        Some(p) => p,
        None => {
            args.rval().set(Int32Value(-1));
            return true;
        }
    };

    match (&mut *child_ptr).wait() {
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            let exited = BooleanValue(true);
            let exited_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exited };
            JS_SetProperty(cx, obj_h, c"exited".as_ptr(), exited_h);
            let ec = Int32Value(code);
            let ec_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ec };
            JS_SetProperty(cx, obj_h, c"exitCode".as_ptr(), ec_h);
            args.rval().set(Int32Value(code));
        }
        Err(_) => {
            args.rval().set(Int32Value(-1));
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_child_kill(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let child_ptr = match get_child_ptr(cx, obj_h) {
        Some(p) => p,
        None => {
            args.rval().set(BooleanValue(false));
            return true;
        }
    };

    let ok = (&mut *child_ptr).kill().is_ok();
    if ok {
        let killed = BooleanValue(true);
        let killed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &killed };
        JS_SetProperty(cx, obj_h, c"killed".as_ptr(), killed_h);
        let exited = BooleanValue(true);
        let exited_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exited };
        JS_SetProperty(cx, obj_h, c"exited".as_ptr(), exited_h);
    }
    args.rval().set(BooleanValue(ok));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_child_read_stdout(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(NullValue());
        return true;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let child_ptr = match get_child_ptr(cx, obj_h) {
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
        if let Ok(c_s) = CString::new(s.as_str()) {
            let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
            args.rval().set(if js_str.is_null() { NullValue() } else { StringValue(&*js_str) });
        } else {
            args.rval().set(NullValue());
        }
    } else {
        args.rval().set(NullValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_child_read_stderr(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(NullValue());
        return true;
    }
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let child_ptr = match get_child_ptr(cx, obj_h) {
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
        if let Ok(c_s) = CString::new(s.as_str()) {
            let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
            args.rval().set(if js_str.is_null() { NullValue() } else { StringValue(&*js_str) });
        } else {
            args.rval().set(NullValue());
        }
    } else {
        args.rval().set(NullValue());
    }
    true
}
