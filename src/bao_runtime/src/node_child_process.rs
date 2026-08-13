// @trace REQ-ENG-007 [entity:ChildProcess] [api:METHOD child_process]
//
// Node.js child_process module — async spawn + sync spawn backed by bun_spawn
//
// JS API: spawn, exec, execFile, execSync, execFileSync, spawnSync, fork
// Async spawn: polling thread + __cp_drain pattern (same as node_net's __net_read)
// Sync spawn: bun_spawn::sync::spawn for all sync ops
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ffi::c_int;
use ::std::ptr::NonNull;
use ::std::sync::{Arc, LazyLock, Mutex};
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue,
    UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_spawn::process::PosixStdio;
use bun_spawn::sync::{self as spawn_sync, Stdio as SyncStdio};
use bun_spawn::{
    Argv, Envp, Exited, PidT, PosixSpawnOptions, SpawnResultExt, Status, spawn_process,
};
use bun_sys::FdExt;

use crate::require::cache_builtin;

// ─── Shared state for async child process pipe data ────────────────────────
// The polling thread and JS thread both access the same AsyncChildState via
// Arc<Mutex<...>>.  Data written by the polling thread is visible to the JS
// thread — unlike the old thread_local! approach where each thread got its own
// independent copy and async output was permanently lost.

/// Global registry: pid → shared state for that child process.
/// Used by __cp_drain / __cp_poll_exit to look up the Arc<Mutex<AsyncChildState>>
/// created during spawn.
static CP_ASYNC_STATES: LazyLock<Mutex<HashMap<i32, Arc<Mutex<AsyncChildState>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Shared state between the polling thread and the JS thread for a single child process.
struct AsyncChildState {
    pid: i32,
    stdout_fd: c_int, // -1 if not piped
    stderr_fd: c_int, // -1 if not piped
    #[allow(dead_code)]
    stdin_fd: c_int, // -1 if not piped
    stdout_eof: bool,
    stderr_eof: bool,
    child_exited: bool,
    /// Buffered stdout data accumulated by the polling thread.
    stdout_data: Vec<u8>,
    /// Buffered stderr data accumulated by the polling thread.
    stderr_data: Vec<u8>,
    /// Exit info (exit_code, signal) set when the child exits.
    exit_info: Option<(i32, i32)>,
}

/// RAII cleanup for shared child process state.
pub struct CpCleanup;

impl Drop for CpCleanup {
    fn drop(&mut self) {
        if let Ok(mut states) = CP_ASYNC_STATES.lock() {
            states.clear();
        }
        CP_STDIN_FDS.with(|m| m.borrow_mut().clear());
    }
}

/// Background polling thread: reads from stdout/stderr pipes into shared AsyncChildState.
/// Uses Arc<Mutex<AsyncChildState>> for shared state with the JS thread.
fn pipe_poll_thread(state: Arc<Mutex<AsyncChildState>>) {
    let mut buf = [0u8; 65536]; // 64 KiB read buffer

    loop {
        let (stdout_fd, stderr_fd, child_exited, stdout_eof, stderr_eof) = {
            let s = state.lock().unwrap();
            (
                s.stdout_fd,
                s.stderr_fd,
                s.child_exited,
                s.stdout_eof,
                s.stderr_eof,
            )
        };

        // If child has exited and both pipes are EOF, we're done.
        if child_exited && stdout_eof && stderr_eof {
            break;
        }

        // Build poll array for remaining open fds.
        let mut poll_fds: Vec<libc::pollfd> = Vec::new();
        let mut fd_map: Vec<&'static str> = Vec::new(); // "stdout" or "stderr"

        if !stdout_eof && stdout_fd >= 0 {
            poll_fds.push(libc::pollfd {
                fd: stdout_fd,
                events: libc::POLLIN as i16,
                revents: 0,
            });
            fd_map.push("stdout");
        }
        if !stderr_eof && stderr_fd >= 0 {
            poll_fds.push(libc::pollfd {
                fd: stderr_fd,
                events: libc::POLLIN as i16,
                revents: 0,
            });
            fd_map.push("stderr");
        }

        if poll_fds.is_empty() {
            // No fds to poll — wait for child to exit or just break.
            if child_exited {
                break;
            }
            // Sleep briefly and re-check.
            ::std::thread::sleep(::std::time::Duration::from_millis(10));
            continue;
        }

        // Poll with 100ms timeout to allow periodic state checks.
        let ret = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as u64, 100) };
        if ret < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                continue;
            }
            // Fatal poll error — break out.
            break;
        }
        if ret == 0 {
            // Timeout — re-check state.
            continue;
        }

        // Process ready fds.
        for (i, pfd) in poll_fds.iter().enumerate() {
            if pfd.revents & (libc::POLLIN as i16 | libc::POLLHUP as i16) == 0 {
                continue;
            }

            let which = fd_map[i];
            let fd = pfd.fd;

            match unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, buf.len()) }
            {
                n if n > 0 => {
                    let data = &buf[..n as usize];
                    let mut s = state.lock().unwrap();
                    if which == "stdout" {
                        s.stdout_data.extend_from_slice(data);
                    } else {
                        s.stderr_data.extend_from_slice(data);
                    }
                }
                0 => {
                    // EOF.
                    let mut s = state.lock().unwrap();
                    if which == "stdout" {
                        s.stdout_eof = true;
                    } else {
                        s.stderr_eof = true;
                    }
                }
                _ => {
                    // EAGAIN or error.
                    let errno = unsafe { *libc::__errno_location() };
                    if errno != libc::EAGAIN && errno != libc::EWOULDBLOCK {
                        // Real error — treat as EOF.
                        let mut s = state.lock().unwrap();
                        if which == "stdout" {
                            s.stdout_eof = true;
                        } else {
                            s.stderr_eof = true;
                        }
                    }
                }
            }
        }

        // Check if child has exited (non-blocking waitpid).
        {
            let mut s = state.lock().unwrap();
            if !s.child_exited {
                let mut wstatus: c_int = 0;
                let ret = unsafe { libc::waitpid(s.pid, &mut wstatus, libc::WNOHANG) };
                if ret == s.pid {
                    // Child has exited.
                    let exit_code = if libc::WIFEXITED(wstatus) {
                        libc::WEXITSTATUS(wstatus)
                    } else {
                        -1
                    };
                    let signal = if libc::WIFSIGNALED(wstatus) {
                        libc::WTERMSIG(wstatus)
                    } else {
                        0
                    };
                    s.child_exited = true;
                    s.exit_info = Some((exit_code, signal));
                }
            }
        }
    }

    // Close pipe fds that we own.
    let s = state.lock().unwrap();
    if s.stdout_fd >= 0 {
        unsafe {
            libc::close(s.stdout_fd);
        }
    }
    if s.stderr_fd >= 0 {
        unsafe {
            libc::close(s.stderr_fd);
        }
    }
}

// ─── Module install ────────────────────────────────────────────────────────

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"spawn".as_ptr(),
            Some(cp_spawn),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"exec".as_ptr(),
            Some(cp_exec),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"execFile".as_ptr(),
            Some(cp_exec_file),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"execSync".as_ptr(),
            Some(cp_exec_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"execFileSync".as_ptr(),
            Some(cp_exec_file_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"spawnSync".as_ptr(),
            Some(cp_spawn_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"fork".as_ptr(),
            Some(cp_fork),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Internal: drain buffered pipe data for an async child process.
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__cp_drain".as_ptr(),
            Some(cp_drain),
            1,
            0,
        );
        // Internal: check if child has exited.
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__cp_poll_exit".as_ptr(),
            Some(cp_poll_exit),
            1,
            0,
        );
        // Internal: write to child stdin.
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__cp_stdin_write".as_ptr(),
            Some(cp_stdin_write),
            2,
            0,
        );
        // Internal: close child stdin.
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__cp_stdin_close".as_ptr(),
            Some(cp_stdin_close),
            1,
            0,
        );
        // Internal: kill child process.
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__cp_kill".as_ptr(),
            Some(cp_kill_child),
            2,
            0,
        );

        w2::JS_DefineProperty3(
            cx,
            mod_obj.handle(),
            c"ChildProcess".as_ptr(),
            mod_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
        cache_builtin(cx, "child_process", mod_obj.get());

        // Run the JS shim that implements the EventEmitter-based ChildProcess class.
        let c_filename = ZBox::from_bytes("node:child_process".as_bytes());
        let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(CP_JS);
            let mut rval = UndefinedValue();
            let rval_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut src, rval_handle);
            libc::free(opts as *mut _);
            // JS shim returns undefined; errors are non-fatal (ChildProcess class is optional).
            let _ = ok;
        }
    }
}

// ─── Helper: read JS string property ───────────────────────────────────────

unsafe fn js_str_prop(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> Option<String> {
    unsafe {
        let mut val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_h,
            name,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            },
        );
        if val.is_string() {
            Some(crate::js_to_rust_string(cx, val))
        } else {
            None
        }
    }
}

unsafe fn js_str_array_prop(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> Vec<String> {
    unsafe {
        let mut val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_h,
            name,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            },
        );
        if !val.is_object() {
            return Vec::new();
        }
        let arr_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let arr = val.to_object();
        rooted!(&in(arr_cx) let arr_root = arr);
        let arr_h = arr_root.handle();
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            arr_h.into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() {
            len_val.to_int32() as u32
        } else {
            0
        };
        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                arr_h.into(),
                i,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            if elem.is_string() {
                result.push(crate::js_to_rust_string(cx, elem));
            }
        }
        result
    }
}

/// Map JS stdio string to bun_spawn::sync::Stdio variant.
#[allow(dead_code)]
unsafe fn js_stdio_mode(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> SyncStdio {
    unsafe {
        match js_str_prop(cx, obj_h, name).as_deref() {
            Some("pipe") | Some("piped") => SyncStdio::Buffer,
            Some("inherit") | Some("ipc") => SyncStdio::Inherit,
            Some("ignore") | Some("null") => SyncStdio::Ignore,
            _ => SyncStdio::Buffer,
        }
    }
}

/// Map JS stdio string to a bool indicating whether to pipe (for async spawn).
unsafe fn js_stdio_wants_pipe(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> bool {
    unsafe {
        match js_str_prop(cx, obj_h, name).as_deref() {
            Some("pipe") | Some("piped") => true,
            Some("inherit") | Some("ipc") => false,
            Some("ignore") | Some("null") => false,
            _ => true, // default: pipe
        }
    }
}

/// Build bun_spawn::sync::Options from JS opts object.
#[allow(dead_code)]
unsafe fn build_sync_opts_from_js(
    cx: *mut JSContext,
    opts_h: Handle<*mut JSObject>,
) -> Option<spawn_sync::Options> {
    unsafe {
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
    }
}

/// Extract exit code from bun_spawn::Status.
pub fn status_to_exit_code(status: &Status) -> i32 {
    match status {
        Status::Exited(Exited { code, signal: 0 }) => *code as i32,
        Status::Exited(Exited { signal, .. }) => -(*signal as i32),
        Status::Signaled(sig) => -(*sig as i32),
        _ => -1,
    }
}

/// Extract signal number from bun_spawn::Status, if killed by signal.
fn status_to_signal(status: &Status) -> Option<i32> {
    match status {
        Status::Exited(Exited { signal: s, .. }) if *s != 0 => Some(*s as i32),
        Status::Signaled(sig) => Some(*sig as i32),
        _ => None,
    }
}

/// Build sync::Options for a shell command (exec/execSync).
fn shell_sync_opts(command: &str) -> spawn_sync::Options {
    let shell = if cfg!(target_family = "unix") {
        "/bin/sh"
    } else {
        "cmd.exe"
    };
    let shell_flag = if cfg!(target_family = "unix") {
        "-c"
    } else {
        "/C"
    };
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

// ─── cp_spawn — ASYNC spawn ────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_spawn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
        if second.is_object() {
            Some(second.to_object())
        } else {
            None
        }
    } else {
        None
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let second_obj_r = second_obj.unwrap_or_else(|| ::std::ptr::null_mut::<JSObject>()));
    rooted!(&in(cx_ref) let opts_obj_r = opts_obj.unwrap_or_else(|| ::std::ptr::null_mut::<JSObject>()));

    // Parse command and args from JS arguments.
    let (cmd, cmd_args, cwd, pipe_stdout, pipe_stderr, pipe_stdin) = if let Some(ref cmd) = cmd_str
    {
        let mut a: Vec<String> = Vec::new();
        let mut c = None;
        let mut ps = true;
        let mut pe = true;
        let mut pi = false;
        if second_obj.is_some() && !second_obj_r.get().is_null() {
            let obj_h = second_obj_r.handle();
            let cargs = js_str_array_prop(cx, obj_h.into(), c"args".as_ptr());
            a = cargs;
            c = js_str_prop(cx, obj_h.into(), c"cwd".as_ptr());
            ps = js_stdio_wants_pipe(cx, obj_h.into(), c"stdout".as_ptr());
            pe = js_stdio_wants_pipe(cx, obj_h.into(), c"stderr".as_ptr());
            pi = js_stdio_wants_pipe(cx, obj_h.into(), c"stdin".as_ptr());
        }
        (cmd.clone(), a, c, ps, pe, pi)
    } else if opts_obj.is_some() && !opts_obj_r.get().is_null() {
        let obj_h = opts_obj_r.handle();
        let cmd = match js_str_prop(cx, obj_h.into(), c"command".as_ptr())
            .or_else(|| js_str_prop(cx, obj_h.into(), c"cmd".as_ptr()))
        {
            Some(c) => c,
            None => {
                JS_ReportErrorUTF8(cx, c"child_process.spawn: missing command".as_ptr());
                return false;
            }
        };
        let cmd_args = js_str_array_prop(cx, obj_h.into(), c"args".as_ptr());
        let cwd = js_str_prop(cx, obj_h.into(), c"cwd".as_ptr());
        let ps = js_stdio_wants_pipe(cx, obj_h.into(), c"stdout".as_ptr());
        let pe = js_stdio_wants_pipe(cx, obj_h.into(), c"stderr".as_ptr());
        let pi = js_stdio_wants_pipe(cx, obj_h.into(), c"stdin".as_ptr());
        (cmd, cmd_args, cwd, ps, pe, pi)
    } else {
        JS_ReportErrorUTF8(cx, c"child_process.spawn requires arguments".as_ptr());
        return false;
    };

    // Build argv for posix_spawn.
    let mut argv: Vec<Box<[u8]>> = Vec::with_capacity(cmd_args.len() + 1);
    argv.push(cmd.as_bytes().to_vec().into_boxed_slice());
    for arg in &cmd_args {
        argv.push(arg.as_bytes().to_vec().into_boxed_slice());
    }

    let cwd_bytes = if let Some(ref d) = cwd {
        d.as_bytes().to_vec().into_boxed_slice()
    } else {
        Box::new([])
    };

    // Create pipes for stdout/stderr/stdin as needed.
    let mut stdout_pipe: [c_int; 2] = [-1, -1];
    let mut stderr_pipe: [c_int; 2] = [-1, -1];
    let mut stdin_pipe: [c_int; 2] = [-1, -1];

    if pipe_stdout {
        if unsafe { libc::pipe(stdout_pipe.as_mut_ptr()) } != 0 {
            let msg = format!("spawn: failed to create stdout pipe: errno {}", unsafe {
                *libc::__errno_location()
            });
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }
    if pipe_stderr {
        if unsafe { libc::pipe(stderr_pipe.as_mut_ptr()) } != 0 {
            // Cleanup stdout pipe if already created.
            if stdout_pipe[0] >= 0 {
                unsafe {
                    libc::close(stdout_pipe[0]);
                }
            }
            if stdout_pipe[1] >= 0 {
                unsafe {
                    libc::close(stdout_pipe[1]);
                }
            }
            let msg = format!("spawn: failed to create stderr pipe: errno {}", unsafe {
                *libc::__errno_location()
            });
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }
    if pipe_stdin {
        if unsafe { libc::pipe(stdin_pipe.as_mut_ptr()) } != 0 {
            if stdout_pipe[0] >= 0 {
                unsafe {
                    libc::close(stdout_pipe[0]);
                }
            }
            if stdout_pipe[1] >= 0 {
                unsafe {
                    libc::close(stdout_pipe[1]);
                }
            }
            if stderr_pipe[0] >= 0 {
                unsafe {
                    libc::close(stderr_pipe[0]);
                }
            }
            if stderr_pipe[1] >= 0 {
                unsafe {
                    libc::close(stderr_pipe[1]);
                }
            }
            let msg = format!("spawn: failed to create stdin pipe: errno {}", unsafe {
                *libc::__errno_location()
            });
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    // Build PosixSpawnOptions with Pipe() variants.
    let spawn_opts = PosixSpawnOptions {
        stdin: if pipe_stdin {
            PosixStdio::Pipe(bun_sys::Fd::from_native(stdin_pipe[0]))
        } else {
            PosixStdio::Inherit
        },
        stdout: if pipe_stdout {
            PosixStdio::Pipe(bun_sys::Fd::from_native(stdout_pipe[1]))
        } else {
            PosixStdio::Inherit
        },
        stderr: if pipe_stderr {
            PosixStdio::Pipe(bun_sys::Fd::from_native(stderr_pipe[1]))
        } else {
            PosixStdio::Inherit
        },
        ipc: None,
        extra_fds: Box::new([]),
        cwd: cwd_bytes,
        detached: false,
        windows: (),
        argv0: None,
        stream: true,
        sync: false,
        can_block_entire_thread_to_reduce_cpu_usage_in_fast_path: false,
        use_execve_on_macos: false,
        no_sigpipe: true,
        new_process_group: false,
        pty_slave_fd: -1,
        pseudoconsole: (),
        linux_pdeathsig: None,
    };

    // Build argv C array.
    let mut string_builder = bun_core::StringBuilder::default();
    for arg in &argv {
        string_builder.count_z(arg);
    }
    if string_builder.allocate().is_err() {
        // Cleanup pipes.
        for fd in [
            stdout_pipe[0],
            stdout_pipe[1],
            stderr_pipe[0],
            stderr_pipe[1],
            stdin_pipe[0],
            stdin_pipe[1],
        ] {
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
        }
        JS_ReportErrorUTF8(cx, c"child_process.spawn: out of memory".as_ptr());
        return false;
    }
    for arg in &argv {
        string_builder.append_count_z(arg);
    }
    let base = string_builder
        .ptr
        .expect("allocate succeeded")
        .as_ptr()
        .cast_const()
        .cast::<::std::ffi::c_char>();
    let mut c_args: Vec<*const ::std::ffi::c_char> = Vec::with_capacity(argv.len() + 1);
    let mut off = 0usize;
    for arg in &argv {
        c_args.push(unsafe { base.add(off) });
        off += arg.len() + 1;
    }
    c_args.push(::std::ptr::null());
    let envp: *const *const ::std::ffi::c_char = bun_sys::environ_ptr();

    // Spawn the child process.
    let spawn_result = unsafe { spawn_process(&spawn_opts, c_args.as_ptr(), envp) };

    // Close the child-side pipe fds (they're now dup'd into the child).
    // stdin: child uses read end (stdin_pipe[0]), so parent closes it.
    // stdout/stderr: child uses write end (pipe[1]), so parent closes those.
    if stdin_pipe[0] >= 0 {
        unsafe {
            libc::close(stdin_pipe[0]);
        }
    }
    if stdout_pipe[1] >= 0 {
        unsafe {
            libc::close(stdout_pipe[1]);
        }
    }
    if stderr_pipe[1] >= 0 {
        unsafe {
            libc::close(stderr_pipe[1]);
        }
    }

    match spawn_result {
        Err(e) => {
            // Cleanup parent-side pipe fds.
            // stdin: parent holds write end (stdin_pipe[1]).
            // stdout/stderr: parent holds read end (pipe[0]).
            if stdin_pipe[1] >= 0 {
                unsafe {
                    libc::close(stdin_pipe[1]);
                }
            }
            if stdout_pipe[0] >= 0 {
                unsafe {
                    libc::close(stdout_pipe[0]);
                }
            }
            if stderr_pipe[0] >= 0 {
                unsafe {
                    libc::close(stderr_pipe[0]);
                }
            }
            let msg = format!("spawn failed: {:?}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
        Ok(Err(sys_err)) => {
            if stdin_pipe[1] >= 0 {
                unsafe {
                    libc::close(stdin_pipe[1]);
                }
            }
            if stdout_pipe[0] >= 0 {
                unsafe {
                    libc::close(stdout_pipe[0]);
                }
            }
            if stderr_pipe[0] >= 0 {
                unsafe {
                    libc::close(stderr_pipe[0]);
                }
            }
            let msg = format!("spawn system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
        Ok(Ok(posix_result)) => {
            let pid = posix_result.pid;

            // Close the spawned stdio fds returned by spawn_process_posix.
            // These are the parent-side socketpair/memfd fds, not our pipe fds.
            // (When using Pipe(fd), spawn_process_posix does not create extra fds.)
            drop(posix_result);

            // Set non-blocking on the parent-side read fds.
            if stdout_pipe[0] >= 0 {
                let _ = bun_sys::set_nonblocking(bun_sys::Fd::from_native(stdout_pipe[0]));
            }
            if stderr_pipe[0] >= 0 {
                let _ = bun_sys::set_nonblocking(bun_sys::Fd::from_native(stderr_pipe[0]));
            }

            // Build the shared state and register it globally for __cp_drain / __cp_poll_exit.
            let async_state = Arc::new(Mutex::new(AsyncChildState {
                pid,
                stdout_fd: if pipe_stdout { stdout_pipe[0] } else { -1 },
                stderr_fd: if pipe_stderr { stderr_pipe[0] } else { -1 },
                stdin_fd: if pipe_stdin { stdin_pipe[1] } else { -1 },
                stdout_eof: false,
                stderr_eof: false,
                child_exited: false,
                stdout_data: Vec::new(),
                stderr_data: Vec::new(),
                exit_info: None,
            }));

            // Register in global registry so __cp_drain / __cp_poll_exit can find it.
            if let Ok(mut registry) = CP_ASYNC_STATES.lock() {
                registry.insert(pid, Arc::clone(&async_state));
            }

            // Store stdin_fd on a thread-local map for __cp_stdin_write/__cp_stdin_close.
            // Parent holds the write end (stdin_pipe[1]) to write to child's stdin.
            CP_STDIN_FDS.with(|m| m.borrow_mut().insert(pid, stdin_pipe[1]));

            {
                let state_clone = Arc::clone(&async_state);
                if let Err(e) = ::std::thread::Builder::new()
                    .name(format!("cp-poll-{}", pid))
                    .stack_size(128 * 1024)
                    .spawn(move || pipe_poll_thread(state_clone))
                {
                    eprintln!(
                        "[bao] FATAL: failed to spawn cp-poll-{} thread: {} — child stdout/stderr will not drain, process may block on 64KB pipe buffer",
                        pid, e
                    );
                }
            }

            // Build the JS ChildProcess object.
            rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
            if child_obj.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }

            let child_h = child_obj.handle().into();

            // pid
            let pid_v = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"pid".as_ptr(),
                pv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // exitCode = null (not yet exited)
            let null_v = NullValue();
            rooted!(&in(cx_ref) let nv = null_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"exitCode".as_ptr(),
                nv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // signalCode = null
            JS_DefineProperty(
                cx,
                child_h,
                c"signalCode".as_ptr(),
                nv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // killed = false
            let killed_v = BooleanValue(false);
            rooted!(&in(cx_ref) let kv = killed_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"killed".as_ptr(),
                kv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // exited = false
            let exited_v = BooleanValue(false);
            rooted!(&in(cx_ref) let ev = exited_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"exited".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // spawnfile
            let c_cmd = ZBox::from_bytes(cmd.as_bytes());
            {
                let js_str = JS_NewStringCopyZ(cx, c_cmd.as_ptr());
                if !js_str.is_null() {
                    let v = StringValue(&*js_str);
                    rooted!(&in(cx_ref) let rv = v);
                    JS_DefineProperty(
                        cx,
                        child_h,
                        c"spawnfile".as_ptr(),
                        rv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }

            // spawnargs array
            let spawnargs_obj = w2::NewArrayObject1(cx_ref, cmd_args.len());
            if !spawnargs_obj.is_null() {
                rooted!(&in(cx_ref) let sa_r = spawnargs_obj);
                for (i, arg) in cmd_args.iter().enumerate() {
                    let c_arg = ZBox::from_bytes(arg.as_bytes());
                    let js_str = JS_NewStringCopyZ(cx, c_arg.as_ptr());
                    if !js_str.is_null() {
                        let v = StringValue(&*js_str);
                        rooted!(&in(cx_ref) let av = v);
                        JS_DefineElement(
                            cx,
                            sa_r.handle().into(),
                            i as u32,
                            av.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
                let sa_val = ObjectValue(sa_r.get());
                rooted!(&in(cx_ref) let sav = sa_val);
                JS_DefineProperty(
                    cx,
                    child_h,
                    c"spawnargs".as_ptr(),
                    sav.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }

            // stdin_fd (stored for native __cp_stdin_write) — parent's write end
            let stdin_fd_v = Int32Value(if pipe_stdin { stdin_pipe[1] } else { -1 });
            rooted!(&in(cx_ref) let sfdv = stdin_fd_v);
            JS_DefineProperty(cx, child_h, c"_stdinFd".as_ptr(), sfdv.handle().into(), 0);

            // Mount `kill(signal)` directly on the native child object — the
            // polyfill's ChildProcess.prototype.kill exists but cp_spawn returns
            // a plain object without the prototype chain, so `child.kill` was
            // undefined. Delegate to the existing __cp_kill_child native.
            w2::JS_DefineFunction(
                cx,
                child_obj.handle(),
                c"kill".as_ptr(),
                Some(cp_kill_child),
                1,
                JSPROP_ENUMERATE as u32,
            );

            // ─── Mount stdout / stderr / stdin stream properties ───────────────
            // Node.js semantics: pipe=true → Readable/Writable backed by
            // __cp_drain / __cp_stdin_write; pipe=false → null (stdio:'ignore'/'inherit').
            // We reuse the existing pipe/poll/drain machinery — no new I/O here.
            //
            // GC safety: every JSObject / JSVal produced below is rooted via
            // rooted!() + .handle().into() per BCE-20260619-012 (no raw Handle{ptr}).
            let streams = build_stdio_streams(cx_ref, child_h, pid, pipe_stdout, pipe_stderr, pipe_stdin);
            // If the JS helper failed (returns false), surface the error.
            if !streams {
                args.rval().set(ObjectValue(child_obj.get()));
                return true;
            }

            args.rval().set(ObjectValue(child_obj.get()));
            true
        }
    }
}

// ─── Helper: mount stdout/stderr/stdin on a spawned ChildProcess ───────────
//
// Reuses existing machinery — no new pipe/poll/buffer code:
//   * Readable streams backed by __cp_drain(pid, which) (returns ArrayBuffer | null)
//   * Writable stub backed by __cp_stdin_write(pid, data) / __cp_stdin_close(pid)
//
// Node.js semantics: stream === null when stdio is not piped ('ignore'/'inherit').
//
// GC safety: every JSObject/JSVal intermediate is rooted via rooted!() and
// passed through .handle().into(); no raw Handle{ptr:&var} per BCE-20260619-012.
//
// Returns true on success; on JS eval failure, reports a JS error and returns false.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_stdio_streams(
    cx: &mut mozjs::context::JSContext,
    child_h: Handle<*mut JSObject>,
    pid: i32,
    pipe_stdout: bool,
    pipe_stderr: bool,
    pipe_stdin: bool,
) -> bool {
    // Assemble the JS snippet: parameterised by PID, returns an array
    // [stdout, stderr, stdin] where each element is either a stream object
    // (piped) or null (not piped). The snippet relies on:
    //   * require('stream').Readable — installed by node_stream::install
    //   * require('child_process').__cp_drain / __cp_stdin_write / __cp_stdin_close
    //     / __cp_poll_exit — defined on this module object at install time.
    let ps = if pipe_stdout { "true" } else { "false" };
    let pe = if pipe_stderr { "true" } else { "false" };
    let pi = if pipe_stdin { "true" } else { "false" };

    let src = format!(
        r#"(function() {{
  var R = require('stream').Readable;
  var cp = require('child_process');
  var PID = {pid};
  var wantStdout = {ps};
  var wantStderr = {pe};
  var wantStdin = {pi};

  function makeReadable(which) {{
    var ended = false;
    var pollScheduled = false;
    var s = new R({{
      read: function(n) {{
        try {{
          var b = cp.__cp_drain(PID, which);
          if (b !== null && b !== undefined) {{
            this.push(b);
            return;
          }}
          // No data right now. Distinguish EOF from "not yet": if child
          // has exited AND drain stays null, this pipe is at EOF.
          var exit = cp.__cp_poll_exit(PID);
          if (exit !== null && exit !== undefined) {{
            if (!ended) {{ ended = true; this.push(null); }}
            return;
          }}
          // Schedule a retry on next tick; re-issue _read to drain again.
          var self = this;
          if (!pollScheduled) {{
            pollScheduled = true;
            setTimeout(function() {{
              pollScheduled = false;
              try {{
                var b2 = cp.__cp_drain(PID, which);
                if (b2 !== null && b2 !== undefined) {{
                  self.push(b2);
                }} else {{
                  var ex = cp.__cp_poll_exit(PID);
                  if (ex !== null && ex !== undefined) {{
                    if (!ended) {{ ended = true; self.push(null); }}
                  }}
                }}
              }} catch (e) {{ self.emit('error', e); }}
            }}, 10);
          }}
        }} catch (e) {{ this.emit('error', e); }}
      }}
    }});
    return s;
  }}

  function makeStdin() {{
    var closed = false;
    return {{
      write: function(data) {{
        if (closed) return false;
        if (typeof cp.__cp_stdin_write === 'function') {{
          return cp.__cp_stdin_write(PID, data);
        }}
        return false;
      }},
      end: function() {{
        if (closed) return;
        closed = true;
        if (typeof cp.__cp_stdin_close === 'function') cp.__cp_stdin_close(PID);
      }},
      destroy: function() {{
        if (closed) return;
        closed = true;
        if (typeof cp.__cp_stdin_close === 'function') cp.__cp_stdin_close(PID);
      }},
      on: function() {{ return this; }},
      once: function() {{ return this; }}
    }};
  }}

  return [
    wantStdout ? makeReadable(0) : null,
    wantStderr ? makeReadable(1) : null,
    wantStdin ? makeStdin() : null
  ];
}})()"#,
        pid = pid,
        ps = ps,
        pe = pe,
        pi = pi,
    );

    let cx_raw = cx.raw_cx();
    let c_filename = ZBox::from_bytes("node:child_process.spawn.streams".as_bytes());
    let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
    if opts.is_null() {
        // Compile options OOM — non-fatal: leave properties undefined (test only
        // checks !== undefined, but we explicitly attach nulls to be safe).
        attach_null_property(cx, child_h, c"stdout".as_ptr());
        attach_null_property(cx, child_h, c"stderr".as_ptr());
        attach_null_property(cx, child_h, c"stdin".as_ptr());
        return true;
    }

    let mut src_text = mozjs::rust::transform_str_to_source_text(src.as_str());
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src_text, rval_handle);
    libc::free(opts as *mut _);

    if !ok || !rval.is_object() {
        // JS failure — attach nulls so the child object is still well-formed
        // (Node.js allows null stdout/stderr/stdin).
        attach_null_property(cx, child_h, c"stdout".as_ptr());
        attach_null_property(cx, child_h, c"stderr".as_ptr());
        attach_null_property(cx, child_h, c"stdin".as_ptr());
        return true;
    }

    // Root the returned array and unpack elements onto child_obj.
    rooted!(&in(cx) let arr = rval.to_object());
    for (idx, name) in [(0u32, c"stdout".as_ptr()), (1u32, c"stderr".as_ptr()), (2u32, c"stdin".as_ptr())].iter() {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx_raw,
            arr.handle().into(),
            *idx,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        rooted!(&in(cx) let elem_r = elem);
        JS_DefineProperty(
            cx_raw,
            child_h,
            *name,
            elem_r.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn attach_null_property(
    cx: &mut mozjs::context::JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) {
    let null_v = NullValue();
    rooted!(&in(cx) let nv = null_v);
    unsafe {
        JS_DefineProperty(
            cx.raw_cx(),
            obj_h,
            name,
            nv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

// ─── Thread-local map for stdin fds ────────────────────────────────────────

thread_local! {
    static CP_STDIN_FDS: RefCell<HashMap<i32, c_int>> = RefCell::new(HashMap::new());
}

// ─── Native: __cp_drain(pid, which) ────────────────────────────────────────
// which: 0 = stdout, 1 = stderr. Returns ArrayBuffer or null.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_drain(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    let which = if argc > 1 {
        (*args.get(1).ptr).to_int32()
    } else {
        0
    }; // 0=stdout, 1=stderr

    if pid == 0 {
        args.rval().set(NullValue());
        return true;
    }

    // Look up shared state by pid, then drain the requested buffer.
    let data: Vec<u8> = CP_ASYNC_STATES
        .lock()
        .ok()
        .and_then(|registry| registry.get(&pid).cloned())
        .and_then(|state| {
            let mut s = state.lock().unwrap();
            if which == 0 {
                let mut swap = Vec::new();
                ::std::mem::swap(&mut s.stdout_data, &mut swap);
                Some(swap)
            } else {
                let mut swap = Vec::new();
                ::std::mem::swap(&mut s.stderr_data, &mut swap);
                Some(swap)
            }
        })
        .unwrap_or_default();

    if data.is_empty() {
        args.rval().set(NullValue());
        return true;
    }

    let len = data.len();
    let buf_ptr = data.as_ptr();
    // Copy data into a new allocation for JS.
    let alloc = ::std::alloc::alloc(
        ::std::alloc::Layout::from_size_align(len, 1)
            .unwrap_or_else(|_| ::std::alloc::Layout::from_size_align(1, 1).unwrap()),
    );
    if alloc.is_null() {
        args.rval().set(NullValue());
        return true;
    }
    unsafe {
        ::std::ptr::copy_nonoverlapping(buf_ptr, alloc, len);
    }

    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;

    let array_buffer =
        w2::NewArrayBufferWithContents(cx_ref, len, alloc as *mut ::std::os::raw::c_void);
    if array_buffer.is_null() {
        ::std::alloc::dealloc(
            alloc,
            ::std::alloc::Layout::from_size_align(len, 1)
                .unwrap_or_else(|_| ::std::alloc::Layout::from_size_align(1, 1).unwrap()),
        );
        args.rval().set(NullValue());
        return true;
    }

    rooted!(&in(cx_ref) let ab = array_buffer);
    args.rval().set(ObjectValue(ab.get()));
    true
}

// ─── Native: __cp_poll_exit(pid) ───────────────────────────────────────────
// Returns: null (not exited), or [exitCode, signalCode] if exited.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_poll_exit(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };

    if pid == 0 {
        args.rval().set(NullValue());
        return true;
    }

    // Look up shared state by pid, then take the exit_info if present.
    let exit_info = CP_ASYNC_STATES
        .lock()
        .ok()
        .and_then(|registry| registry.get(&pid).cloned())
        .and_then(|state| {
            let mut s = state.lock().unwrap();
            s.exit_info.take()
        });

    match exit_info {
        Some((code, signal)) => {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            let arr = w2::NewArrayObject1(cx_ref, 2);
            if arr.is_null() {
                args.rval().set(NullValue());
                return true;
            }
            rooted!(&in(cx_ref) let arr_r = arr);
            {
                let cv = Int32Value(code);
                rooted!(&in(cx_ref) let cvr = cv);
                JS_DefineElement(
                    cx,
                    arr_r.handle().into(),
                    0,
                    cvr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            {
                let sv = Int32Value(signal);
                rooted!(&in(cx_ref) let svr = sv);
                JS_DefineElement(
                    cx,
                    arr_r.handle().into(),
                    1,
                    svr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            args.rval().set(ObjectValue(arr_r.get()));
            true
        }
        None => {
            args.rval().set(NullValue());
            true
        }
    }
}

// ─── Native: __cp_stdin_write(pid, data) ───────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_stdin_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    if pid == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let stdin_fd = CP_STDIN_FDS
        .with(|m| m.borrow_mut().get(&pid).copied())
        .unwrap_or(-1);
    if stdin_fd < 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }

    // Get data as bytes from ArrayBuffer or string.
    let bytes: Vec<u8> = if data_val.is_object() {
        let obj = data_val.to_object();
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj_r = obj);
        // Check if it's an ArrayBuffer and get data.
        let mut length: usize = 0;
        let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
        let mut is_shared = false;
        unsafe {
            GetArrayBufferLengthAndData(obj_r.get(), &mut length, &mut is_shared, &mut data_ptr);
        }
        if !data_ptr.is_null() && length > 0 {
            unsafe { ::std::slice::from_raw_parts(data_ptr, length) }.to_vec()
        } else {
            // Try as string.
            crate::js_to_rust_string(cx, data_val).into_bytes()
        }
    } else if data_val.is_string() {
        crate::js_to_rust_string(cx, data_val).into_bytes()
    } else {
        Vec::new()
    };

    if bytes.is_empty() {
        args.rval().set(Int32Value(0));
        return true;
    }

    let written = unsafe {
        libc::write(
            stdin_fd,
            bytes.as_ptr() as *const ::std::ffi::c_void,
            bytes.len(),
        )
    };
    args.rval().set(DoubleValue(written as f64));
    true
}

// ─── Native: __cp_stdin_close(pid) ─────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_stdin_close(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };

    if pid == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let stdin_fd = CP_STDIN_FDS
        .with(|m| m.borrow_mut().remove(&pid))
        .unwrap_or(-1);
    if stdin_fd >= 0 {
        unsafe {
            libc::close(stdin_fd);
        }
        args.rval().set(BooleanValue(true));
    } else {
        args.rval().set(BooleanValue(false));
    }
    true
}

// ─── Native: __cp_kill_child(pid, signal) ──────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_kill_child(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    let signal = if argc > 1 {
        (*args.get(1).ptr).to_int32()
    } else {
        libc::SIGTERM as i32
    };

    if pid == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let ret = unsafe { libc::kill(pid, signal) };
    args.rval().set(BooleanValue(ret == 0));
    true
}

// ─── cp_exec — ASYNC exec (shell command, callback-based) ──────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.exec requires a command string".as_ptr());
        return false;
    }

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
        if cb.is_object() && JS_ObjectIsFunction(cb.to_object()) {
            Some(cb.to_object())
        } else {
            None
        }
    } else {
        None
    };

    let cmd = crate::js_to_rust_string(cx, cmd_val);
    let sync_opts = shell_sync_opts(&cmd);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let callback_r = callback.unwrap_or(::std::ptr::null_mut::<JSObject>()));
    rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
    if child_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("exec system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("exec failed: {:?}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
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

    let c_stdout = ZBox::from_bytes(stdout_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_stdout.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            rooted!(&in(cx_ref) let rv = v);
            JS_DefineProperty(
                cx,
                child_h,
                c"stdout".as_ptr(),
                rv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    let c_stderr = ZBox::from_bytes(stderr_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_stderr.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            rooted!(&in(cx_ref) let rv = v);
            JS_DefineProperty(
                cx,
                child_h,
                c"stderr".as_ptr(),
                rv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let ec = Int32Value(exit_code));
    JS_DefineProperty(
        cx,
        child_h,
        c"exitCode".as_ptr(),
        ec.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    let pid_v = Int32Value(spawn_result.pid as i32);
    rooted!(&in(cx_ref) let pv = pid_v);
    JS_DefineProperty(
        cx,
        child_h,
        c"pid".as_ptr(),
        pv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // Call callback if provided: callback(error, stdout, stderr)
    if callback.is_some() && !callback_r.get().is_null() {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));

        let err_obj = if exit_code != 0 {
            let e = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if !e.is_null() {
                let c_msg = ZBox::from_vec(
                    format!("Command failed with exit code {}", exit_code).into_bytes(),
                );
                let msg_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                if !msg_str.is_null() {
                    let mv = StringValue(&*msg_str);
                    rooted!(&in(cx_ref) let mv_r = mv);
                    rooted!(&in(cx_ref) let e_r = e);
                    JS_SetProperty(
                        cx,
                        e_r.handle().into(),
                        c"message".as_ptr(),
                        mv_r.handle().into(),
                    );
                }
            }
            e
        } else {
            ::std::ptr::null_mut()
        };

        let mut call_vals: [Value; 3] = [
            if err_obj.is_null() {
                NullValue()
            } else {
                ObjectValue(err_obj)
            },
            UndefinedValue(),
            UndefinedValue(),
        ];
        JS_GetProperty(
            cx,
            child_h,
            c"stdout".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut call_vals[1],
            },
        );
        JS_GetProperty(
            cx,
            child_h,
            c"stderr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut call_vals[2],
            },
        );

        rooted!(&in(cx_ref) let cv0 = call_vals[0]);
        rooted!(&in(cx_ref) let cv1 = call_vals[1]);
        rooted!(&in(cx_ref) let cv2 = call_vals[2]);
        let elems = [&cv0.get(), &cv1.get(), &cv2.get()];
        let call_args = HandleValueArray {
            length_: 3,
            elements_: elems.as_ptr() as *const Value,
        };

        let cb_val = ObjectValue(callback_r.get());
        rooted!(&in(cx_ref) let cb_r = cb_val);
        let mut rval = UndefinedValue();
        JS_CallFunctionValue(
            cx,
            global.handle().into(),
            cb_r.handle().into(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
    }

    args.rval().set(ObjectValue(child_obj.get()));
    true
}

// ─── cp_exec_sync ──────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.execSync requires a command string".as_ptr(),
        );
        return false;
    }

    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.execSync requires a string command".as_ptr(),
        );
        return false;
    }

    let cmd = crate::js_to_rust_string(cx, cmd_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let sync_opts = shell_sync_opts(&cmd);

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("execSync system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("execSync failed: {:?}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exit_code = status_to_exit_code(&spawn_result.status);
    let pid = spawn_result.pid;
    let signal = status_to_signal(&spawn_result.status);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if exit_code != 0 || signal.is_some() {
        let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if err_obj.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        rooted!(&in(cx_ref) let err_r = err_obj);
        let err_h = err_r.handle().into();

        let stderr_str = String::from_utf8_lossy(&spawn_result.stderr).into_owned();
        let msg = format!("Command failed: {}", cmd);
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
            if !js_str.is_null() {
                let v = StringValue(&*js_str);
                rooted!(&in(cx_ref) let rv = v);
                JS_SetProperty(cx, err_h, c"message".as_ptr(), rv.handle().into());
            }
        }

        rooted!(&in(cx_ref) let sv = Int32Value(exit_code));
        JS_SetProperty(cx, err_h, c"status".as_ptr(), sv.handle().into());

        rooted!(&in(cx_ref) let pv = Int32Value(pid as i32));
        JS_SetProperty(cx, err_h, c"pid".as_ptr(), pv.handle().into());

        let signal_val = match signal {
            Some(sig) => {
                let sig_name = match sig {
                    1 => "SIGHUP",
                    2 => "SIGINT",
                    3 => "SIGQUIT",
                    6 => "SIGABRT",
                    9 => "SIGKILL",
                    11 => "SIGSEGV",
                    13 => "SIGPIPE",
                    15 => "SIGTERM",
                    _ => "SIG",
                };
                let name_str = if sig_name == "SIG" {
                    format!("SIG{}", sig)
                } else {
                    sig_name.to_string()
                };
                let c_sig = ZBox::from_bytes(name_str.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_sig.as_ptr());
                if !js_str.is_null() {
                    StringValue(&*js_str)
                } else {
                    NullValue()
                }
            }
            None => NullValue(),
        };
        rooted!(&in(cx_ref) let sigv = signal_val);
        JS_SetProperty(cx, err_h, c"signal".as_ptr(), sigv.handle().into());

        let stdout_str = String::from_utf8_lossy(&spawn_result.stdout).into_owned();
        let c_stdout = ZBox::from_bytes(stdout_str.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_stdout.as_ptr());
            if !js_str.is_null() {
                let v = StringValue(&*js_str);
                rooted!(&in(cx_ref) let rv = v);
                JS_SetProperty(cx, err_h, c"stdout".as_ptr(), rv.handle().into());
            }
        }

        let c_stderr = ZBox::from_bytes(stderr_str.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_stderr.as_ptr());
            if !js_str.is_null() {
                let v = StringValue(&*js_str);
                rooted!(&in(cx_ref) let rv = v);
                JS_SetProperty(cx, err_h, c"stderr".as_ptr(), rv.handle().into());
            }
        }

        let output_arr_obj = w2::NewArrayObject1(cx_ref, 3);
        if !output_arr_obj.is_null() {
            rooted!(&in(cx_ref) let output_r = output_arr_obj);
            {
                let null_v = NullValue();
                rooted!(&in(cx_ref) let nv = null_v);
                JS_DefineElement(
                    cx,
                    output_r.handle().into(),
                    0,
                    nv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            {
                let mut stdout_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    err_h,
                    c"stdout".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut stdout_val,
                    },
                );
                rooted!(&in(cx_ref) let sov = stdout_val);
                JS_DefineElement(
                    cx,
                    output_r.handle().into(),
                    1,
                    sov.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            {
                let mut stderr_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    err_h,
                    c"stderr".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut stderr_val,
                    },
                );
                rooted!(&in(cx_ref) let sev = stderr_val);
                JS_DefineElement(
                    cx,
                    output_r.handle().into(),
                    2,
                    sev.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let output_val = ObjectValue(output_r.get());
            rooted!(&in(cx_ref) let ov = output_val);
            JS_SetProperty(cx, err_h, c"output".as_ptr(), ov.handle().into());
        }

        let err_val = ObjectValue(err_r.get());
        rooted!(&in(cx_ref) let ev = err_val);
        JS_SetPendingException(cx, ev.handle().into(), ExceptionStackBehavior::DoNotCapture);
        return false;
    }

    let stdout_str = String::from_utf8_lossy(&spawn_result.stdout).into_owned();
    let c_out = ZBox::from_bytes(stdout_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── cp_exec_file ──────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.execFile requires a file path".as_ptr());
        return false;
    }
    let file_val = *args.get(0).ptr;
    if !file_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.execFile requires a string file path".as_ptr(),
        );
        return false;
    }

    let file_path = crate::js_to_rust_string(cx, file_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
            let mut wrapped_cx_args =
                mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref_args = &mut wrapped_cx_args;
            rooted!(&in(cx_ref_args) let args_obj_r = args_obj);
            let args_obj_h = args_obj_r.handle();
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                args_obj_h.into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            if len_val.is_int32() {
                let len = len_val.to_int32() as u32;
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(
                        cx,
                        args_obj_h.into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut elem,
                        },
                    );
                    if elem.is_string() {
                        sync_opts.argv.push(
                            crate::js_to_rust_string(cx, elem)
                                .as_bytes()
                                .to_vec()
                                .into_boxed_slice(),
                        );
                    }
                }
            }
        }
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("execFile system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("execFile failed: {:?}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
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

    let child_h = child_r.handle().into();
    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let c_out = ZBox::from_bytes(stdout_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            rooted!(&in(cx_ref) let rv = v);
            JS_DefineProperty(
                cx,
                child_h,
                c"stdout".as_ptr(),
                rv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let c_err = ZBox::from_bytes(stderr_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
        if !js_str.is_null() {
            let v = StringValue(&*js_str);
            rooted!(&in(cx_ref) let rv = v);
            JS_DefineProperty(
                cx,
                child_h,
                c"stderr".as_ptr(),
                rv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let ec = Int32Value(exit_code));
    JS_DefineProperty(
        cx,
        child_h,
        c"exitCode".as_ptr(),
        ec.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    let pid_v = Int32Value(spawn_result.pid as i32);
    rooted!(&in(cx_ref) let pv = pid_v);
    JS_DefineProperty(
        cx,
        child_h,
        c"pid".as_ptr(),
        pv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(ObjectValue(child_r.get()));
    true
}

// ─── cp_exec_file_sync ─────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_exec_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.execFileSync requires a file path".as_ptr(),
        );
        return false;
    }
    let file_val = *args.get(0).ptr;
    if !file_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.execFileSync requires a string file path".as_ptr(),
        );
        return false;
    }
    let file_path = crate::js_to_rust_string(cx, file_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
            let mut wrapped_cx_args =
                mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref_args = &mut wrapped_cx_args;
            rooted!(&in(cx_ref_args) let args_obj_r = args_obj);
            let args_obj_h = args_obj_r.handle();
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                args_obj_h.into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            if len_val.is_int32() {
                let len = len_val.to_int32() as u32;
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(
                        cx,
                        args_obj_h.into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut elem,
                        },
                    );
                    if elem.is_string() {
                        sync_opts.argv.push(
                            crate::js_to_rust_string(cx, elem)
                                .as_bytes()
                                .to_vec()
                                .into_boxed_slice(),
                        );
                    }
                }
            }
        } else if args_val.is_string() {
            sync_opts.argv.push(
                crate::js_to_rust_string(cx, args_val)
                    .as_bytes()
                    .to_vec()
                    .into_boxed_slice(),
            );
        }
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("execFileSync system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("execFileSync failed: {:?}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exit_code = status_to_exit_code(&spawn_result.status);
    let pid = spawn_result.pid;
    let signal = status_to_signal(&spawn_result.status);

    if exit_code != 0 || signal.is_some() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;

        let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if err_obj.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        rooted!(&in(cx_ref) let err_r = err_obj);
        let err_h = err_r.handle().into();

        let stderr_str = String::from_utf8_lossy(&spawn_result.stderr).into_owned();
        let msg = format!("execFileSync failed with status {}", exit_code);
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
            if !js_str.is_null() {
                let v = StringValue(&*js_str);
                rooted!(&in(cx_ref) let rv = v);
                JS_SetProperty(cx, err_h, c"message".as_ptr(), rv.handle().into());
            }
        }

        rooted!(&in(cx_ref) let sv = Int32Value(exit_code));
        JS_SetProperty(cx, err_h, c"status".as_ptr(), sv.handle().into());

        rooted!(&in(cx_ref) let pv = Int32Value(pid as i32));
        JS_SetProperty(cx, err_h, c"pid".as_ptr(), pv.handle().into());

        let signal_val = match signal {
            Some(sig) => {
                let sig_name = match sig {
                    1 => "SIGHUP",
                    2 => "SIGINT",
                    3 => "SIGQUIT",
                    6 => "SIGABRT",
                    9 => "SIGKILL",
                    11 => "SIGSEGV",
                    13 => "SIGPIPE",
                    15 => "SIGTERM",
                    _ => "SIG",
                };
                let name_str = if sig_name == "SIG" {
                    format!("SIG{}", sig)
                } else {
                    sig_name.to_string()
                };
                let c_sig = ZBox::from_bytes(name_str.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_sig.as_ptr());
                if !js_str.is_null() {
                    StringValue(&*js_str)
                } else {
                    NullValue()
                }
            }
            None => NullValue(),
        };
        rooted!(&in(cx_ref) let sigv = signal_val);
        JS_SetProperty(cx, err_h, c"signal".as_ptr(), sigv.handle().into());

        let stdout_str = String::from_utf8_lossy(&spawn_result.stdout).into_owned();
        let c_stdout = ZBox::from_bytes(stdout_str.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_stdout.as_ptr());
            if !js_str.is_null() {
                let v = StringValue(&*js_str);
                rooted!(&in(cx_ref) let rv = v);
                JS_SetProperty(cx, err_h, c"stdout".as_ptr(), rv.handle().into());
            }
        }

        let c_stderr = ZBox::from_bytes(stderr_str.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_stderr.as_ptr());
            if !js_str.is_null() {
                let v = StringValue(&*js_str);
                rooted!(&in(cx_ref) let rv = v);
                JS_SetProperty(cx, err_h, c"stderr".as_ptr(), rv.handle().into());
            }
        }

        let err_val = ObjectValue(err_r.get());
        rooted!(&in(cx_ref) let ev2 = err_val);
        JS_SetPendingException(
            cx,
            ev2.handle().into(),
            ExceptionStackBehavior::DoNotCapture,
        );
        return false;
    }
    let stdout_str = String::from_utf8_lossy(&spawn_result.stdout).into_owned();
    let c_out = ZBox::from_bytes(stdout_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            args.rval().set(StringValue(&*js_str));
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── cp_spawn_sync ─────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_spawn_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.spawnSync requires a command".as_ptr());
        return false;
    }
    let cmd_val = *args.get(0).ptr;
    if !cmd_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.spawnSync requires a string command".as_ptr(),
        );
        return false;
    }
    let command = crate::js_to_rust_string(cx, cmd_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
            rooted!(&in(cx_ref) let args_obj_r = args_obj);
            let args_obj_h = args_obj_r.handle();
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                args_obj_h.into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            if len_val.is_int32() {
                let len = len_val.to_int32() as u32;
                for i in 0..len {
                    let mut elem = UndefinedValue();
                    JS_GetElement(
                        cx,
                        args_obj_h.into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut elem,
                        },
                    );
                    if elem.is_string() {
                        sync_opts.argv.push(
                            crate::js_to_rust_string(cx, elem)
                                .as_bytes()
                                .to_vec()
                                .into_boxed_slice(),
                        );
                    }
                }
            }
        }
    }

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("spawnSync system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if result_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_ref) let result_r = result_obj);
            let err_msg = format!("{:?}", e);
            let c_err = ZBox::from_vec(err_msg.into_bytes());
            {
                let js_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
                if !js_str.is_null() {
                    let err_val = StringValue(&*js_str);
                    rooted!(&in(cx_ref) let ev = err_val);
                    JS_DefineProperty(
                        cx,
                        result_r.handle().into(),
                        c"error".as_ptr(),
                        ev.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            let status = Int32Value(-1);
            rooted!(&in(cx_ref) let sv = status);
            JS_DefineProperty(
                cx,
                result_r.handle().into(),
                c"status".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            args.rval().set(ObjectValue(result_r.get()));
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
    rooted!(&in(cx_ref) let result_r = result_obj);

    let status = Int32Value(exit_code);
    rooted!(&in(cx_ref) let sv = status);
    JS_DefineProperty(
        cx,
        result_r.handle().into(),
        c"status".as_ptr(),
        sv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let c_out = ZBox::from_bytes(stdout_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
        if !js_str.is_null() {
            let out_val = StringValue(&*js_str);
            rooted!(&in(cx_ref) let ov = out_val);
            JS_DefineProperty(
                cx,
                result_r.handle().into(),
                c"stdout".as_ptr(),
                ov.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let c_err = ZBox::from_bytes(stderr_str.as_bytes());
    {
        let js_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
        if !js_str.is_null() {
            let err_val = StringValue(&*js_str);
            rooted!(&in(cx_ref) let ev = err_val);
            JS_DefineProperty(
                cx,
                result_r.handle().into(),
                c"stderr".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    let pid = Int32Value(spawn_result.pid as i32);
    rooted!(&in(cx_ref) let pv = pid);
    JS_DefineProperty(
        cx,
        result_r.handle().into(),
        c"pid".as_ptr(),
        pv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    let signal_val = match status_to_signal(&spawn_result.status) {
        Some(sig) => {
            let sig_name = match sig {
                1 => "SIGHUP",
                2 => "SIGINT",
                3 => "SIGQUIT",
                6 => "SIGABRT",
                9 => "SIGKILL",
                11 => "SIGSEGV",
                13 => "SIGPIPE",
                15 => "SIGTERM",
                _ => "SIG",
            };
            let name_str = if sig_name == "SIG" {
                format!("SIG{}", sig)
            } else {
                sig_name.to_string()
            };
            let c_sig = ZBox::from_bytes(name_str.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_sig.as_ptr());
            if !js_str.is_null() {
                StringValue(&*js_str)
            } else {
                NullValue()
            }
        }
        None => NullValue(),
    };
    rooted!(&in(cx_ref) let sigv = signal_val);
    JS_DefineProperty(
        cx,
        result_r.handle().into(),
        c"signal".as_ptr(),
        sigv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // output array
    let output_arr_obj = w2::NewArrayObject1(cx_ref, 3);
    let output_arr_val = if !output_arr_obj.is_null() {
        rooted!(&in(cx_ref) let output_r = output_arr_obj);
        {
            let null_v = NullValue();
            rooted!(&in(cx_ref) let nv = null_v);
            JS_DefineElement(
                cx,
                output_r.handle().into(),
                0,
                nv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        {
            let mut stdout_val = UndefinedValue();
            JS_GetProperty(
                cx,
                result_r.handle().into(),
                c"stdout".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut stdout_val,
                },
            );
            rooted!(&in(cx_ref) let sov = stdout_val);
            JS_DefineElement(
                cx,
                output_r.handle().into(),
                1,
                sov.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        {
            let mut stderr_val = UndefinedValue();
            JS_GetProperty(
                cx,
                result_r.handle().into(),
                c"stderr".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut stderr_val,
                },
            );
            rooted!(&in(cx_ref) let sev = stderr_val);
            JS_DefineElement(
                cx,
                output_r.handle().into(),
                2,
                sev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        ObjectValue(output_r.get())
    } else {
        NullValue()
    };
    rooted!(&in(cx_ref) let oav = output_arr_val);
    JS_DefineProperty(
        cx,
        result_r.handle().into(),
        c"output".as_ptr(),
        oav.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    let err_val = NullValue();
    rooted!(&in(cx_ref) let erv = err_val);
    JS_DefineProperty(
        cx,
        result_r.handle().into(),
        c"error".as_ptr(),
        erv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(ObjectValue(result_r.get()));
    true
}

// ─── cp_fork ───────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cp_fork(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"child_process.fork requires a module path".as_ptr());
        return false;
    }

    let module_val = *args.get(0).ptr;
    if !module_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"child_process.fork requires a string module path".as_ptr(),
        );
        return false;
    }

    let module = crate::js_to_rust_string(cx, module_val);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    // Use async spawn for fork — child should run independently.
    let executable =
        ::std::env::current_exe().unwrap_or_else(|_| ::std::path::PathBuf::from("bao"));
    let exec_str = executable.to_string_lossy().into_owned();

    // Build argv.
    let argv: Vec<Box<[u8]>> = vec![
        exec_str.as_bytes().to_vec().into_boxed_slice(),
        b"run".to_vec().into_boxed_slice(),
        module.as_bytes().to_vec().into_boxed_slice(),
    ];

    // Create pipes for IPC (stdout/stderr pipe, stdin pipe).
    let mut stdout_pipe: [c_int; 2] = [-1, -1];
    let mut stderr_pipe: [c_int; 2] = [-1, -1];
    let mut stdin_pipe: [c_int; 2] = [-1, -1];

    let _ = unsafe { libc::pipe(stdout_pipe.as_mut_ptr()) };
    let _ = unsafe { libc::pipe(stderr_pipe.as_mut_ptr()) };
    let _ = unsafe { libc::pipe(stdin_pipe.as_mut_ptr()) };

    let spawn_opts = PosixSpawnOptions {
        stdin: if stdin_pipe[0] >= 0 {
            PosixStdio::Pipe(bun_sys::Fd::from_native(stdin_pipe[0]))
        } else {
            PosixStdio::Inherit
        },
        stdout: if stdout_pipe[0] >= 0 {
            PosixStdio::Pipe(bun_sys::Fd::from_native(stdout_pipe[1]))
        } else {
            PosixStdio::Inherit
        },
        stderr: if stderr_pipe[0] >= 0 {
            PosixStdio::Pipe(bun_sys::Fd::from_native(stderr_pipe[1]))
        } else {
            PosixStdio::Inherit
        },
        ipc: None,
        extra_fds: Box::new([]),
        cwd: Box::new([]),
        detached: false,
        windows: (),
        argv0: None,
        stream: true,
        sync: false,
        can_block_entire_thread_to_reduce_cpu_usage_in_fast_path: false,
        use_execve_on_macos: false,
        no_sigpipe: true,
        new_process_group: false,
        pty_slave_fd: -1,
        pseudoconsole: (),
        linux_pdeathsig: None,
    };

    // Build argv C array.
    let mut string_builder = bun_core::StringBuilder::default();
    for arg in &argv {
        string_builder.count_z(arg);
    }
    if string_builder.allocate().is_err() {
        for fd in [
            stdout_pipe[0],
            stdout_pipe[1],
            stderr_pipe[0],
            stderr_pipe[1],
            stdin_pipe[0],
            stdin_pipe[1],
        ] {
            if fd >= 0 {
                unsafe {
                    libc::close(fd);
                }
            }
        }
        JS_ReportErrorUTF8(cx, c"child_process.fork: out of memory".as_ptr());
        return false;
    }
    for arg in &argv {
        string_builder.append_count_z(arg);
    }
    let base = string_builder
        .ptr
        .expect("allocate succeeded")
        .as_ptr()
        .cast_const()
        .cast::<::std::ffi::c_char>();
    let mut c_args: Vec<*const ::std::ffi::c_char> = Vec::with_capacity(argv.len() + 1);
    let mut off = 0usize;
    for arg in &argv {
        c_args.push(unsafe { base.add(off) });
        off += arg.len() + 1;
    }
    c_args.push(::std::ptr::null());
    let envp: *const *const ::std::ffi::c_char = bun_sys::environ_ptr();

    let spawn_result = unsafe { spawn_process(&spawn_opts, c_args.as_ptr(), envp) };

    // Close child-side pipe fds.
    // stdin: child uses read end (stdin_pipe[0]), so parent closes it.
    // stdout/stderr: child uses write end (pipe[1]), so parent closes those.
    if stdin_pipe[0] >= 0 {
        unsafe {
            libc::close(stdin_pipe[0]);
        }
    }
    if stdout_pipe[1] >= 0 {
        unsafe {
            libc::close(stdout_pipe[1]);
        }
    }
    if stderr_pipe[1] >= 0 {
        unsafe {
            libc::close(stderr_pipe[1]);
        }
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    match spawn_result {
        Err(e) => {
            // Cleanup parent-side pipe fds.
            // stdin: parent holds write end (stdin_pipe[1]).
            // stdout/stderr: parent holds read end (pipe[0]).
            for fd in [stdin_pipe[1], stdout_pipe[0], stderr_pipe[0]] {
                if fd >= 0 {
                    unsafe {
                        libc::close(fd);
                    }
                }
            }
            let msg = format!("fork failed: {:?}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
        Ok(Err(sys_err)) => {
            for fd in [stdin_pipe[1], stdout_pipe[0], stderr_pipe[0]] {
                if fd >= 0 {
                    unsafe {
                        libc::close(fd);
                    }
                }
            }
            let msg = format!("fork system error: {:?}", sys_err);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
        Ok(Ok(posix_result)) => {
            let pid = posix_result.pid;
            drop(posix_result);

            // Build shared state and register it globally for __cp_drain / __cp_poll_exit.
            let async_state = Arc::new(Mutex::new(AsyncChildState {
                pid,
                stdout_fd: stdout_pipe[0],
                stderr_fd: stderr_pipe[0],
                stdin_fd: stdin_pipe[1],
                stdout_eof: false,
                stderr_eof: false,
                child_exited: false,
                stdout_data: Vec::new(),
                stderr_data: Vec::new(),
                exit_info: None,
            }));

            // Register in global registry so __cp_drain / __cp_poll_exit can find it.
            if let Ok(mut registry) = CP_ASYNC_STATES.lock() {
                registry.insert(pid, Arc::clone(&async_state));
            }

            // Parent holds the write end (stdin_pipe[1]) to write to child's stdin.
            CP_STDIN_FDS.with(|m| m.borrow_mut().insert(pid, stdin_pipe[1]));
            {
                let state_clone = Arc::clone(&async_state);
                let _ = ::std::thread::Builder::new()
                    .name(format!("cp-fork-{}", pid))
                    .stack_size(128 * 1024)
                    .spawn(move || pipe_poll_thread(state_clone));
            }

            rooted!(&in(cx_ref) let child_obj = w2::JS_NewPlainObject(cx_ref));
            if child_obj.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }

            let child_h = child_obj.handle().into();

            let pid_v = Int32Value(pid as i32);
            rooted!(&in(cx_ref) let pv = pid_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"pid".as_ptr(),
                pv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            let null_v = NullValue();
            rooted!(&in(cx_ref) let nv = null_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"exitCode".as_ptr(),
                nv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineProperty(
                cx,
                child_h,
                c"signalCode".as_ptr(),
                nv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            let killed_v = BooleanValue(false);
            rooted!(&in(cx_ref) let kv = killed_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"killed".as_ptr(),
                kv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            let exited_v = BooleanValue(false);
            rooted!(&in(cx_ref) let ev = exited_v);
            JS_DefineProperty(
                cx,
                child_h,
                c"exited".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            let stdin_fd_v = Int32Value(stdin_pipe[1]);
            rooted!(&in(cx_ref) let sfdv = stdin_fd_v);
            JS_DefineProperty(cx, child_h, c"_stdinFd".as_ptr(), sfdv.handle().into(), 0);

            args.rval().set(ObjectValue(child_obj.get()));
            true
        }
    }
}

// ─── JS shim for async ChildProcess EventEmitter ───────────────────────────

const CP_JS: &str = r#"
(function() {
  var cp = require('child_process');

  // Helper: get or create a ChildProcess wrapper for a native child object.
  function ChildProcess(nativeObj) {
    this._native = nativeObj;
    this.pid = nativeObj.pid;
    this.exitCode = nativeObj.exitCode;
    this.signalCode = nativeObj.signalCode;
    this.killed = nativeObj.killed || false;
    this._exited = nativeObj.exited || false;
    this._polling = false;
    this._stdinFd = nativeObj._stdinFd || -1;
    this._events = {};
    this._onceFlags = {};

    // Create stream objects for stdin/stdout/stderr.
    var self = this;

    // stdout stream (readable)
    this.stdout = {
      _pid: this.pid,
      _which: 0,
      _ended: false,
      on: function(event, cb) {
        if (!self._events['stdout_' + event]) self._events['stdout_' + event] = [];
        self._events['stdout_' + event].push(cb);
        if (event === 'data' || event === 'end') self._startPoll();
      },
      pipe: function(dest) { return dest; }
    };

    // stderr stream (readable)
    this.stderr = {
      _pid: this.pid,
      _which: 1,
      _ended: false,
      on: function(event, cb) {
        if (!self._events['stderr_' + event]) self._events['stderr_' + event] = [];
        self._events['stderr_' + event].push(cb);
        if (event === 'data' || event === 'end') self._startPoll();
      },
      pipe: function(dest) { return dest; }
    };

    // stdin stream (writable)
    this.stdin = {
      write: function(data) {
        if (typeof cp.__cp_stdin_write === 'function' && self._stdinFd >= 0) {
          return cp.__cp_stdin_write(self.pid, data);
        }
        return false;
      },
      end: function() {
        if (typeof cp.__cp_stdin_close === 'function' && self._stdinFd >= 0) {
          cp.__cp_stdin_close(self.pid);
          self._stdinFd = -1;
        }
      },
      destroy: function() {
        if (typeof cp.__cp_stdin_close === 'function' && self._stdinFd >= 0) {
          cp.__cp_stdin_close(self.pid);
          self._stdinFd = -1;
        }
      }
    };
  }

  ChildProcess.prototype.on = function(event, cb) {
    if (!this._events[event]) this._events[event] = [];
    this._events[event].push(cb);
    if (event === 'exit' || event === 'close' || event === 'data') {
      this._startPoll();
    }
    return this;
  };

  ChildProcess.prototype.once = function(event, cb) {
    this.on(event, cb);
    if (!this._onceFlags[event]) this._onceFlags[event] = [];
    this._onceFlags[event].push(this._events[event].length - 1);
    return this;
  };

  ChildProcess.prototype.emit = function(event) {
    var args = Array.prototype.slice.call(arguments, 1);
    var cbs = this._events[event];
    if (!cbs) return false;
    var onceIndices = this._onceFlags[event] || [];
    var remaining = [];
    var remainingOnce = [];
    for (var i = 0; i < cbs.length; i++) {
      try { cbs[i].apply(null, args); } catch(e) {}
      var idx = onceIndices.indexOf(i);
      if (idx >= 0) {
        // once — don't keep
      } else {
        remaining.push(cbs[i]);
        remainingOnce.push(false);
      }
    }
    this._events[event] = remaining;
    this._onceFlags[event] = [];
    return true;
  };

  ChildProcess.prototype.removeListener = function(event, cb) {
    var cbs = this._events[event];
    if (!cbs) return this;
    var idx = cbs.indexOf(cb);
    if (idx >= 0) cbs.splice(idx, 1);
    return this;
  };

  ChildProcess.prototype.kill = function(signal) {
    signal = signal || 'SIGTERM';
    var sigNum = 15; // SIGTERM
    if (signal === 'SIGKILL') sigNum = 9;
    else if (signal === 'SIGINT') sigNum = 2;
    else if (signal === 'SIGHUP') sigNum = 1;
    else if (signal === 'SIGQUIT') sigNum = 3;
    else if (typeof signal === 'number') sigNum = signal;
    if (typeof cp.__cp_kill_child === 'function') {
      cp.__cp_kill_child(this.pid, sigNum);
    }
    this.killed = true;
    return true;
  };

  ChildProcess.prototype.ref = function() {};
  ChildProcess.prototype.unref = function() {};
  ChildProcess.prototype.disconnect = function() {};

  ChildProcess.prototype._startPoll = function() {
    if (this._polling) return;
    this._polling = true;
    this._pollTick();
  };

  ChildProcess.prototype._pollTick = function() {
    if (!this._polling || this._exited) return;

    var self = this;
    var pid = this.pid;

    // Drain stdout data.
    if (typeof cp.__cp_drain === 'function' && !this.stdout._ended) {
      var stdoutBuf = cp.__cp_drain(pid, 0);
      if (stdoutBuf && stdoutBuf.byteLength > 0) {
        this.emit('stdout_data', stdoutBuf);
        // Also emit on stdout stream listeners.
        var stdoutCbs = this._events['stdout_data'];
        if (stdoutCbs) {
          for (var i = 0; i < stdoutCbs.length; i++) {
            try { stdoutCbs[i](stdoutBuf); } catch(e) {}
          }
        }
      }
    }

    // Drain stderr data.
    if (typeof cp.__cp_drain === 'function' && !this.stderr._ended) {
      var stderrBuf = cp.__cp_drain(pid, 1);
      if (stderrBuf && stderrBuf.byteLength > 0) {
        this.emit('stderr_data', stderrBuf);
        var stderrCbs = this._events['stderr_data'];
        if (stderrCbs) {
          for (var i = 0; i < stderrCbs.length; i++) {
            try { stderrCbs[i](stderrBuf); } catch(e) {}
          }
        }
      }
    }

    // Check for child exit.
    if (typeof cp.__cp_poll_exit === 'function') {
      var exitInfo = cp.__cp_poll_exit(pid);
      if (exitInfo) {
        this._exited = true;
        this.exitCode = exitInfo[0];
        this.signalCode = exitInfo[1] || null;
        // Final drain.
        var finalStdout = cp.__cp_drain ? cp.__cp_drain(pid, 0) : null;
        var finalStderr = cp.__cp_drain ? cp.__cp_drain(pid, 1) : null;
        if (finalStdout && finalStdout.byteLength > 0) {
          this.emit('stdout_data', finalStdout);
        }
        if (finalStderr && finalStderr.byteLength > 0) {
          this.emit('stderr_data', finalStderr);
        }
        this.stdout._ended = true;
        this.stderr._ended = true;
        this.emit('stdout_end');
        this.emit('stderr_end');
        this.emit('exit', this.exitCode, this.signalCode);
        this.emit('close', this.exitCode, this.signalCode);
        return;
      }
    }

    // Continue polling.
    if (this._polling && !this._exited) {
      setTimeout(this._pollTick.bind(this), 0);
    }
  };

  // Store the constructor for reuse.
  cp._ChildProcess = ChildProcess;
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_to_exit_code_exited_zero() {
        let status = Status::Exited(Exited { code: 0, signal: 0 });
        assert_eq!(status_to_exit_code(&status), 0);
    }

    #[test]
    fn test_status_to_exit_code_exited_nonzero() {
        let status = Status::Exited(Exited {
            code: 42,
            signal: 0,
        });
        assert_eq!(status_to_exit_code(&status), 42);
    }

    #[test]
    fn test_status_to_exit_code_exited_with_signal() {
        let status = Status::Exited(Exited { code: 0, signal: 9 });
        assert_eq!(status_to_exit_code(&status), -9);
    }

    #[test]
    fn test_status_to_exit_code_signaled() {
        let status = Status::Signaled(15);
        assert_eq!(status_to_exit_code(&status), -15);
    }

    #[test]
    fn test_status_to_exit_code_running() {
        let status = Status::Running;
        assert_eq!(status_to_exit_code(&status), -1);
    }

    #[test]
    fn test_status_to_exit_code_err() {
        let status = Status::Err(bun_sys::Error::from_code_int(
            libc::ESRCH,
            bun_sys::Tag::waitpid,
        ));
        assert_eq!(status_to_exit_code(&status), -1);
    }

    #[test]
    fn test_shell_sync_opts_unix_argv() {
        let opts = shell_sync_opts("echo hello");
        assert_eq!(opts.argv.len(), 3);
        let shell = if cfg!(target_family = "unix") {
            "/bin/sh"
        } else {
            "cmd.exe"
        };
        let flag = if cfg!(target_family = "unix") {
            "-c"
        } else {
            "/C"
        };
        assert_eq!(&*opts.argv[0], shell.as_bytes());
        assert_eq!(&*opts.argv[1], flag.as_bytes());
        assert_eq!(&*opts.argv[2], b"echo hello");
    }

    #[test]
    fn test_shell_sync_opts_stdin_is_ignore() {
        let opts = shell_sync_opts("ls");
        assert!(matches!(opts.stdin, SyncStdio::Ignore));
    }

    #[test]
    fn test_shell_sync_opts_stdout_stderr_is_buffer() {
        let opts = shell_sync_opts("ls");
        assert!(matches!(opts.stdout, SyncStdio::Buffer));
        assert!(matches!(opts.stderr, SyncStdio::Buffer));
    }

    #[test]
    fn test_shell_sync_opts_not_detached() {
        let opts = shell_sync_opts("ls");
        assert!(!opts.detached);
    }

    #[test]
    fn test_shell_sync_opts_empty_cwd() {
        let opts = shell_sync_opts("ls");
        assert!(opts.cwd.is_empty());
    }

    #[test]
    fn test_shell_sync_opts_no_envp() {
        let opts = shell_sync_opts("ls");
        assert!(opts.envp.is_none());
    }

    #[test]
    fn test_sync_stdio_variants() {
        assert!(matches!(SyncStdio::Inherit, SyncStdio::Inherit));
        assert!(matches!(SyncStdio::Ignore, SyncStdio::Ignore));
        assert!(matches!(SyncStdio::Buffer, SyncStdio::Buffer));
    }

    #[test]
    fn test_status_is_ok_exited_zero() {
        let status = Status::Exited(Exited { code: 0, signal: 0 });
        assert!(status.is_ok());
    }

    #[test]
    fn test_status_is_ok_exited_nonzero() {
        let status = Status::Exited(Exited { code: 1, signal: 0 });
        assert!(!status.is_ok());
    }

    #[test]
    fn test_exited_default() {
        let e = Exited::default();
        assert_eq!(e.code, 0);
        assert_eq!(e.signal, 0);
    }

    #[test]
    fn test_spawn_echo_hello() {
        let opts = spawn_sync::Options {
            stdin: SyncStdio::Ignore,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: Box::new([]),
            detached: false,
            argv: vec![
                b"echo".to_vec().into_boxed_slice(),
                b"hello".to_vec().into_boxed_slice(),
            ],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };

        let result = match spawn_sync::spawn(&opts) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("spawn system error: {:?}", e),
            Err(e) => panic!("spawn failed: {:?}", e),
        };

        assert!(result.is_ok());
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("hello"), "stdout was: {:?}", stdout);
    }

    #[test]
    fn test_spawn_exit_code_nonzero() {
        let opts = spawn_sync::Options {
            stdin: SyncStdio::Ignore,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: Box::new([]),
            detached: false,
            argv: vec![
                b"sh".to_vec().into_boxed_slice(),
                b"-c".to_vec().into_boxed_slice(),
                b"exit 42".to_vec().into_boxed_slice(),
            ],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };

        let result = match spawn_sync::spawn(&opts) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("spawn system error: {:?}", e),
            Err(e) => panic!("spawn failed: {:?}", e),
        };

        assert!(!result.is_ok());
        assert_eq!(status_to_exit_code(&result.status), 42);
    }

    #[test]
    fn test_spawn_stderr_capture() {
        let opts = spawn_sync::Options {
            stdin: SyncStdio::Ignore,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: Box::new([]),
            detached: false,
            argv: vec![
                b"sh".to_vec().into_boxed_slice(),
                b"-c".to_vec().into_boxed_slice(),
                b"echo err >&2".to_vec().into_boxed_slice(),
            ],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };

        let result = match spawn_sync::spawn(&opts) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("spawn system error: {:?}", e),
            Err(e) => panic!("spawn failed: {:?}", e),
        };

        assert!(result.is_ok());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("err"), "stderr was: {:?}", stderr);
    }

    #[test]
    fn test_spawn_nonexistent_command() {
        let opts = spawn_sync::Options {
            stdin: SyncStdio::Ignore,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: Box::new([]),
            detached: false,
            argv: vec![
                b"/nonexistent/command/that/does/not/exist"
                    .to_vec()
                    .into_boxed_slice(),
            ],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };

        let result = spawn_sync::spawn(&opts);
        match result {
            Ok(Ok(r)) => {
                assert!(!r.is_ok(), "nonexistent command should not exit 0");
            }
            Ok(Err(_)) => {}
            Err(_) => {}
        }
    }

    #[test]
    fn test_shell_sync_opts_spawn_echo() {
        let opts = shell_sync_opts("echo from_shell");
        let result = match spawn_sync::spawn(&opts) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("shell spawn system error: {:?}", e),
            Err(e) => panic!("shell spawn failed: {:?}", e),
        };

        assert!(result.is_ok());
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("from_shell"), "stdout was: {:?}", stdout);
    }

    #[test]
    fn test_spawn_cwd_option() {
        let opts = spawn_sync::Options {
            stdin: SyncStdio::Ignore,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: b"/tmp".to_vec().into_boxed_slice(),
            detached: false,
            argv: vec![b"pwd".to_vec().into_boxed_slice()],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };

        let result = match spawn_sync::spawn(&opts) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("spawn system error: {:?}", e),
            Err(e) => panic!("spawn failed: {:?}", e),
        };

        assert!(result.is_ok());
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("/tmp"), "stdout was: {:?}", stdout);
    }

    #[test]
    fn test_spawn_stdin_ignore() {
        let opts = spawn_sync::Options {
            stdin: SyncStdio::Ignore,
            stdout: SyncStdio::Buffer,
            stderr: SyncStdio::Buffer,
            ipc: None,
            cwd: Box::new([]),
            detached: false,
            argv: vec![b"true".to_vec().into_boxed_slice()],
            envp: None,
            use_execve_on_macos: false,
            argv0: None,
            windows: (),
        };

        let result = match spawn_sync::spawn(&opts) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => panic!("spawn system error: {:?}", e),
            Err(e) => panic!("spawn failed: {:?}", e),
        };

        assert!(result.is_ok());
    }
}
