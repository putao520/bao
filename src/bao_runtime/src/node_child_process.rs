// @trace REQ-ENG-007 [entity:ChildProcess] [api:METHOD child_process]
//
// Node.js child_process module — backed by bun_spawn (posix_spawn + sync::spawn)
//
// JS API: spawn, exec, execFile, execSync, execFileSync, spawnSync, fork
// Internal: bun_spawn::sync::spawn for all sync ops
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_spawn::sync::{self as spawn_sync, Stdio as SyncStdio};
use bun_spawn::{Status, Exited};

use crate::require::cache_builtin;

// Sync spawn stores results directly on JS objects — no separate Process storage needed.

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"spawn".as_ptr(), Some(cp_spawn), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"exec".as_ptr(), Some(cp_exec), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"execFile".as_ptr(), Some(cp_exec_file), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"execSync".as_ptr(), Some(cp_exec_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"execFileSync".as_ptr(), Some(cp_exec_file_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"spawnSync".as_ptr(), Some(cp_spawn_sync), 1, JSPROP_ENUMERATE as u32);
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

/// Map JS stdio string to bun_spawn::sync::Stdio variant.
unsafe fn js_stdio_mode(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, name: *const ::std::os::raw::c_char) -> SyncStdio { unsafe {
    match js_str_prop(cx, obj_h, name).as_deref() {
        Some("pipe") | Some("piped") => SyncStdio::Buffer,
        Some("inherit") | Some("ipc") => SyncStdio::Inherit,
        Some("ignore") | Some("null") => SyncStdio::Ignore,
        _ => SyncStdio::Buffer,
    }
}}

/// Build bun_spawn::sync::Options from JS opts object.
unsafe fn build_sync_opts_from_js(cx: *mut JSContext, opts_h: Handle<*mut JSObject>) -> Option<spawn_sync::Options> { unsafe {
    let cmd = js_str_prop(cx, opts_h, c"command".as_ptr())
        .or_else(|| js_str_prop(cx, opts_h, c"cmd".as_ptr()))?;
    let args = js_str_array_prop(cx, opts_h, c"args".as_ptr());
    let cwd = js_str_prop(cx, opts_h, c"cwd".as_ptr());

    let mut argv: Vec<Box<[u8]>> = Vec::with_capacity(args.len() + 1);
    argv.push(cmd.as_bytes().to_vec().into_boxed_slice());
    for arg in &args {
        argv.push(arg.as_bytes().to_vec().into_boxed_slice());
    }

    let cwd_bytes = if let Some(ref d) = cwd {
        d.as_bytes().to_vec().into_boxed_slice()
    } else {
        Box::new([])
    };

    let detached_val = js_str_prop(cx, opts_h, c"detached".as_ptr());
    let detached = detached_val.as_deref() == Some("true");

    Some(spawn_sync::Options {
        stdin: js_stdio_mode(cx, opts_h, c"stdin".as_ptr()),
        stdout: js_stdio_mode(cx, opts_h, c"stdout".as_ptr()),
        stderr: js_stdio_mode(cx, opts_h, c"stderr".as_ptr()),
        ipc: None,
        cwd: cwd_bytes,
        detached,
        argv,
        envp: None,
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    })
}}

/// Extract exit code from bun_spawn::Status.
fn status_to_exit_code(status: &Status) -> i32 {
    match status {
        Status::Exited(Exited { code, signal: 0 }) => *code as i32,
        Status::Exited(Exited { signal, .. }) => -(*signal as i32),
        Status::Signaled(sig) => -(*sig as i32),
        _ => -1,
    }
}

/// Build sync::Options for a shell command (exec/execSync).
fn shell_sync_opts(command: &str) -> spawn_sync::Options {
    let shell = if cfg!(target_family = "unix") { "/bin/sh" } else { "cmd.exe" };
    let shell_flag = if cfg!(target_family = "unix") { "-c" } else { "/C" };
    spawn_sync::Options {
        stdin: SyncStdio::Ignore,
        stdout: SyncStdio::Buffer,
        stderr: SyncStdio::Buffer,
        ipc: None,
        cwd: Box::new([]),
        detached: false,
        argv: vec![
            shell.as_bytes().to_vec().into_boxed_slice(),
            shell_flag.as_bytes().to_vec().into_boxed_slice(),
            command.as_bytes().to_vec().into_boxed_slice(),
        ],
        envp: None,
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    }
}

// ─── cp_spawn ────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_spawn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

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

    // Build sync::Options
    let sync_opts = if let Some(ref cmd) = cmd_str {
        let mut opts = spawn_sync::Options {
            stdin: SyncStdio::Buffer,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: Box::new([]),
            detached: false,
            argv: vec![cmd.as_bytes().to_vec().into_boxed_slice()],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };
        if let Some(ref obj) = second_obj {
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: obj };
            let cargs = js_str_array_prop(cx, obj_h, c"args".as_ptr());
            for a in &cargs {
                opts.argv.push(a.as_bytes().to_vec().into_boxed_slice());
            }
            let cwd = js_str_prop(cx, obj_h, c"cwd".as_ptr());
            if let Some(ref d) = cwd {
                opts.cwd = d.as_bytes().to_vec().into_boxed_slice();
            }
            opts.stdout = js_stdio_mode(cx, obj_h, c"stdout".as_ptr());
            opts.stderr = js_stdio_mode(cx, obj_h, c"stderr".as_ptr());
            opts.stdin = js_stdio_mode(cx, obj_h, c"stdin".as_ptr());
        }
        opts
    } else if let Some(ref obj) = opts_obj {
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: obj };
        match build_sync_opts_from_js(cx, obj_h) {
            Some(o) => o,
            None => {
                JS_ReportErrorUTF8(cx, c"child_process.spawn: missing command".as_ptr());
                return false;
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"child_process.spawn requires arguments".as_ptr());
        return false;
    };

    // Spawn synchronously (bun_spawn::sync::spawn)
    // spawn() returns Result<Maybe<Result>, Error> where Maybe = Result<T, Error>
    // Use ?? to unwrap both layers, then match the inner Result
    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("spawn system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("spawn failed: {:?}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exit_code = status_to_exit_code(&spawn_result.status);
    let pid = unsafe { libc::getpid() };

    let pid_v = Int32Value(pid as i32);
    rooted!(&in(cx_ref) let pv = pid_v);
    JS_DefineProperty(cx, child_h, c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

    let exited_v = BooleanValue(true);
    rooted!(&in(cx_ref) let ev = exited_v);
    JS_DefineProperty(cx, child_h, c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

    let ec_v = Int32Value(exit_code);
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
    true
}

// ─── cp_exec ─────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.exec requires a command string".as_ptr());
        return false;
    }

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(cx, c"child_process.exec requires a string command".as_ptr());
        return false;
    }

    let callback = if argc > 1 {
        let cb = *args.get(1).ptr;
        if cb.is_object() && JS_ObjectIsFunction(cb.to_object()) { Some(cb.to_object()) } else { None }
    } else {
        None
    };

    let cmd = crate::js_to_rust_string(cx, cmd_val);
    let sync_opts = shell_sync_opts(&cmd);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
    if child_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("exec system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("exec failed: {:?}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let stdout_bytes = spawn_result.stdout.clone();
    let stderr_bytes = spawn_result.stderr.clone();
    let exit_code = status_to_exit_code(&spawn_result.status);
    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();

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
            if !e.is_null()
                && let Ok(c_msg) = CString::new(format!("Command failed with exit code {}", exit_code)) {
                    let msg_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                    if !msg_str.is_null() {
                        let mv = StringValue(&*msg_str);
                        let mv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mv };
                        let e_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &e };
                        JS_SetProperty(cx, e_h, c"message".as_ptr(), mv_h);
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
    true
}

// ─── cp_exec_sync ────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.execSync requires a command string".as_ptr());
        return false;
    }

    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(cx, c"child_process.execSync requires a string command".as_ptr());
        return false;
    }

    let cmd = crate::js_to_rust_string(cx, cmd_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let sync_opts = shell_sync_opts(&cmd);

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("execSync system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("execSync failed: {:?}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let stdout_str = String::from_utf8_lossy(&spawn_result.stdout).into_owned();
    if let Ok(c_out) = CString::new(stdout_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── cp_exec_file ────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_file(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.execFile requires a file path".as_ptr());
        return false;
    }
    let file_val = *args.get(0).ptr;
    if !file_val.is_string() {
        JS_ReportErrorUTF8(cx, c"child_process.execFile requires a string file path".as_ptr());
        return false;
    }

    let file_path = crate::js_to_rust_string(cx, file_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let mut sync_opts = spawn_sync::Options {
        stdin: SyncStdio::Ignore,
        stdout: SyncStdio::Buffer,
        stderr: SyncStdio::Buffer,
        ipc: None,
        cwd: Box::new([]),
        detached: false,
        argv: vec![file_path.as_bytes().to_vec().into_boxed_slice()],
        envp: None,
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    };

    if argc > 1 {
        let args_val = *args.get(1).ptr;
        if args_val.is_object() {
            let args_obj = args_val.to_object();
            let args_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &args_obj };
            let mut len_val = UndefinedValue();
            JS_GetProperty(cx, args_obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
            if len_val.is_int32() {
                let len = len_val.to_int32() as u32;
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(cx, args_obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
                    if elem.is_string() {
                        sync_opts.argv.push(crate::js_to_rust_string(cx, elem).as_bytes().to_vec().into_boxed_slice());
                    }
                }
            }
        }
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("execFile system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("execFile failed: {:?}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let stdout_bytes = spawn_result.stdout.clone();
    let stderr_bytes = spawn_result.stderr.clone();
    let exit_code = status_to_exit_code(&spawn_result.status);

    let child_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if child_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let child_r = child_obj);
    w2::JS_DefineFunction(cx_ref, child_r.handle(), c"wait".as_ptr(), Some(cp_child_wait), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, child_r.handle(), c"kill".as_ptr(), Some(cp_child_kill), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, child_r.handle(), c"stdout".as_ptr(), Some(cp_child_read_stdout), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, child_r.handle(), c"stderr".as_ptr(), Some(cp_child_read_stderr), 0, JSPROP_ENUMERATE as u32);

    // Store stdout/stderr as properties
    let child_h = child_r.handle().into();
    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    if let Ok(c_out) = CString::new(stdout_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
            JS_DefineProperty(cx, child_h, c"stdout".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
        }
    }
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if let Ok(c_err) = CString::new(stderr_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
            JS_DefineProperty(cx, child_h, c"stderr".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
        }
    }
    let ec = Int32Value(exit_code);
    let ec_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ec };
    JS_DefineProperty(cx, child_h, c"exitCode".as_ptr(), ec_h, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(child_obj));
    true
}

// ─── cp_exec_file_sync ───────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_file_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.execFileSync requires a file path".as_ptr());
        return false;
    }
    let file_val = *args.get(0).ptr;
    if !file_val.is_string() {
        JS_ReportErrorUTF8(cx, c"child_process.execFileSync requires a string file path".as_ptr());
        return false;
    }
    let file_path = crate::js_to_rust_string(cx, file_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let mut sync_opts = spawn_sync::Options {
        stdin: SyncStdio::Ignore,
        stdout: SyncStdio::Buffer,
        stderr: SyncStdio::Buffer,
        ipc: None,
        cwd: Box::new([]),
        detached: false,
        argv: vec![file_path.as_bytes().to_vec().into_boxed_slice()],
        envp: None,
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    };

    if argc > 1 {
        let args_val = *args.get(1).ptr;
        if args_val.is_object() {
            let args_obj = args_val.to_object();
            let args_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &args_obj };
            let mut len_val = UndefinedValue();
            JS_GetProperty(cx, args_obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
            if len_val.is_int32() {
                let len = len_val.to_int32() as u32;
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(cx, args_obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
                    if elem.is_string() {
                        sync_opts.argv.push(crate::js_to_rust_string(cx, elem).as_bytes().to_vec().into_boxed_slice());
                    }
                }
            }
        } else if args_val.is_string() {
            sync_opts.argv.push(crate::js_to_rust_string(cx, args_val).as_bytes().to_vec().into_boxed_slice());
        }
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("execFileSync system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("execFileSync failed: {:?}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exit_code = status_to_exit_code(&spawn_result.status);
    if exit_code != 0 {
        let stderr_str = String::from_utf8_lossy(&spawn_result.stderr).into_owned();
        let msg = format!("execFileSync failed with status {}: {}", exit_code, stderr_str);
        let c_msg = CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let stdout_str = String::from_utf8_lossy(&spawn_result.stdout).into_owned();
    if let Ok(c_out) = CString::new(stdout_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── cp_spawn_sync ───────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_spawn_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(cx)
    );
    let cx_ref = &mut wrapped_cx;

    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.spawnSync requires a command".as_ptr());
        return false;
    }
    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(cx, c"child_process.spawnSync requires a string command".as_ptr());
        return false;
    }
    let command = crate::js_to_rust_string(cx, cmd_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let mut sync_opts = spawn_sync::Options {
        stdin: SyncStdio::Ignore,
        stdout: SyncStdio::Buffer,
        stderr: SyncStdio::Buffer,
        ipc: None,
        cwd: Box::new([]),
        detached: false,
        argv: vec![command.as_bytes().to_vec().into_boxed_slice()],
        envp: None,
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    };

    if argc > 1 {
        let args_val = *args.get(1).ptr;
        if args_val.is_object() {
            let args_obj = args_val.to_object();
            let args_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &args_obj };
            let mut len_val = UndefinedValue();
            JS_GetProperty(cx, args_obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
            if len_val.is_int32() {
                let len = len_val.to_int32() as u32;
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(cx, args_obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
                    if elem.is_string() {
                        sync_opts.argv.push(crate::js_to_rust_string(cx, elem).as_bytes().to_vec().into_boxed_slice());
                    }
                }
            }
        }
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("spawnSync system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if result_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            let result_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &result_obj };
            let err_msg = format!("{:?}", e);
            if let Ok(c_err) = CString::new(err_msg) {
                let js_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
                if !js_str.is_null() {
                    let err_val = StringValue(&*js_str);
                    rooted!(&in(cx_ref) let ev = err_val);
                    JS_DefineProperty(cx, result_h, c"error".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            let status = Int32Value(-1);
            rooted!(&in(cx_ref) let sv = status);
            JS_DefineProperty(cx, result_h, c"status".as_ptr(), sv.handle().into(), JSPROP_ENUMERATE as u32);
            args.rval().set(ObjectValue(result_obj));
            return true;
        }
    };

    let exit_code = status_to_exit_code(&spawn_result.status);
    let stdout_bytes = spawn_result.stdout.clone();
    let stderr_bytes = spawn_result.stderr.clone();

    let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let result_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &result_obj };

    let status = Int32Value(exit_code);
    rooted!(&in(cx_ref) let sv = status);
    JS_DefineProperty(cx, result_h, c"status".as_ptr(), sv.handle().into(), JSPROP_ENUMERATE as u32);

    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    if let Ok(c_out) = CString::new(stdout_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            let out_val = StringValue(&*js_str);
            rooted!(&in(cx_ref) let ov = out_val);
            JS_DefineProperty(cx, result_h, c"stdout".as_ptr(), ov.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if let Ok(c_err) = CString::new(stderr_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
        if !js_str.is_null() {
            let err_val = StringValue(&*js_str);
            rooted!(&in(cx_ref) let ev = err_val);
            JS_DefineProperty(cx, result_h, c"stderr".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    let pid = Int32Value(unsafe { libc::getpid() } as i32);
    rooted!(&in(cx_ref) let pv = pid);
    JS_DefineProperty(cx, result_h, c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

    let err_val = NullValue();
    rooted!(&in(cx_ref) let erv = err_val);
    JS_DefineProperty(cx, result_h, c"error".as_ptr(), erv.handle().into(), JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(result_obj));
    true
}

// ─── cp_fork ─────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_fork(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.fork requires a module path".as_ptr());
        return false;
    }

    let module_val = *args.get(0).ptr;
    if !module_val.is_string() {
        JS_ReportErrorUTF8(cx, c"child_process.fork requires a string module path".as_ptr());
        return false;
    }

    let module = crate::js_to_rust_string(cx, module_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let executable = ::std::env::current_exe().unwrap_or_else(|_| ::std::path::PathBuf::from("bao"));
    let exec_str = executable.to_string_lossy().into_owned();

    let sync_opts = spawn_sync::Options {
        stdin: SyncStdio::Inherit,
        stdout: SyncStdio::Inherit,
        stderr: SyncStdio::Inherit,
        ipc: None,
        cwd: Box::new([]),
        detached: false,
        argv: vec![
            exec_str.as_bytes().to_vec().into_boxed_slice(),
            b"run".to_vec().into_boxed_slice(),
            module.as_bytes().to_vec().into_boxed_slice(),
        ],
        envp: None,
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("fork system error: {:?}", sys_err);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("fork failed: {:?}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exit_code = status_to_exit_code(&spawn_result.status);
    let stdout_bytes = spawn_result.stdout.clone();

    rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
    if child_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let child_h = child_obj.handle().into();

    // Store stdout/stderr from fork (inherit mode may have empty output)
    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    if let Ok(c_out) = CString::new(stdout_str.as_str()) {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
            JS_DefineProperty(cx, child_h, c"stdout".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
        }
    }

    let pid_v = Int32Value(unsafe { libc::getpid() } as i32);
    rooted!(&in(cx_ref) let pv = pid_v);
    JS_DefineProperty(cx, child_h, c"pid".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);

    let exited_v = BooleanValue(true);
    rooted!(&in(cx_ref) let ev = exited_v);
    JS_DefineProperty(cx, child_h, c"exited".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);

    let ec_v = Int32Value(exit_code);
    rooted!(&in(cx_ref) let ecv = ec_v);
    JS_DefineProperty(cx, child_h, c"exitCode".as_ptr(), ecv.handle().into(), JSPROP_ENUMERATE as u32);

    w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"wait".as_ptr(), Some(cp_child_wait), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, child_obj.handle(), c"kill".as_ptr(), Some(cp_child_kill), 0, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(child_obj.get()));
    true
}

// ─── ChildProcess method: wait ───────────────────────────────────────

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

    // For sync spawn, the child has already exited — just read exitCode
    let mut ec_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"exitCode".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ec_val });
    let code = if ec_val.is_int32() { ec_val.to_int32() } else { -1 };

    let exited = BooleanValue(true);
    let exited_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exited };
    JS_SetProperty(cx, obj_h, c"exited".as_ptr(), exited_h);

    args.rval().set(Int32Value(code));
    true
}

// ─── ChildProcess method: kill ───────────────────────────────────────

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

    // For sync spawn, the child has already exited — just mark killed
    let killed = BooleanValue(true);
    let killed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &killed };
    JS_SetProperty(cx, obj_h, c"killed".as_ptr(), killed_h);
    let exited = BooleanValue(true);
    let exited_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exited };
    JS_SetProperty(cx, obj_h, c"exited".as_ptr(), exited_h);

    args.rval().set(BooleanValue(true));
    true
}

// ─── ChildProcess method: read stdout ────────────────────────────────

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

    // For sync spawn, stdout is already stored as a property
    let mut stdout_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"stdout".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut stdout_val });
    if stdout_val.is_string() {
        args.rval().set(stdout_val);
    } else {
        args.rval().set(NullValue());
    }
    true
}

// ─── ChildProcess method: read stderr ────────────────────────────────

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

    // For sync spawn, stderr is already stored as a property
    let mut stderr_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"stderr".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut stderr_val });
    if stderr_val.is_string() {
        args.rval().set(stderr_val);
    } else {
        args.rval().set(NullValue());
    }
    true
}
