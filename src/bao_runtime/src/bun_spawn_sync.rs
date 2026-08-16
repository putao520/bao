// @trace REQ-ENG-006 [api:Bun.spawnSync] — synchronous subprocess.
//
// Upstream semantics (js_bun_spawn_bindings.zig spawnSync):
//   `Bun.spawnSync(cmd | { cmd, args?, cwd?, env?, stdin?, stdout?, stderr?,
//                          encoding?, maxBuffer? })`
//   → `{ success: boolean, pid: number|null, exitCode: number|null,
//       stdout: Uint8Array|string|null, stderr: Uint8Array|string|null }`
//
//   * stdout/stderr: piped by default → Uint8Array; `encoding: "utf8"`
//     renders them as strings; `"inherit"`/`"ignore"` → null.
//   * stdin: string / TypedArray / ArrayBuffer (written to the child) or
//     `"inherit"` / `"ignore"`.
//   * maxBuffer: caps collected stdout+stderr bytes (default 8 MiB);
//     output is truncated to the cap (upstream does not throw).
//   * Spawn failure (missing executable, bad cwd) THROWS synchronously.
//   * pid: the child is reaped before returning, so `pid` is null (upstream
//     keeps the pre-reap pid; null is the honest post-wait value here).
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_NewPlainObject, JS_NewUint8Array};

use bun_core::ZBox;

/// Default output cap per stream (upstream default maxBuffer).
const DEFAULT_MAX_BUFFER: usize = 8 * 1024 * 1024;

/// stdio option: pipe (default) | inherit | ignore/null.
enum StdioOpt {
    Pipe,
    Inherit,
    Ignore,
}

impl PartialEq for StdioOpt {
    fn eq(&self, other: &Self) -> bool {
        ::std::mem::discriminant(self) == ::std::mem::discriminant(other)
    }
}

struct SyncSpawnConfig {
    cmd: String,
    args: Vec<String>,
    cwd: Option<String>,
    env_pairs: Option<Vec<(String, String)>>,
    stdin_bytes: Option<Vec<u8>>,
    stdin_opt: StdioOpt,
    stdout_opt: StdioOpt,
    stderr_opt: StdioOpt,
    utf8_encoding: bool,
    max_buffer: usize,
}

impl SyncSpawnConfig {
    fn new() -> Self {
        SyncSpawnConfig {
            cmd: String::new(),
            args: Vec::new(),
            cwd: None,
            env_pairs: None,
            stdin_bytes: None,
            stdin_opt: StdioOpt::Pipe,
            stdout_opt: StdioOpt::Pipe,
            stderr_opt: StdioOpt::Pipe,
            utf8_encoding: false,
            max_buffer: DEFAULT_MAX_BUFFER,
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop_opt(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> Option<String> {
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        name,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_string() {
        Some(crate::js_to_rust_string(cx, v))
    } else {
        None
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_array_prop(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> Vec<String> {
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        name,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if !v.is_object() {
        return Vec::new();
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let arr = v.to_object());
    rooted!(&in(cx_ref) let av = v);
    let mut is_arr = false;
    IsArrayObject(cx, av.handle().into(), &mut is_arr);
    if !is_arr {
        return Vec::new();
    }
    let mut len_v = UndefinedValue();
    JS_GetProperty(
        cx,
        arr.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_v,
        },
    );
    let len = if len_v.is_number() { len_v.to_number() as u32 } else { 0 };
    let mut out = Vec::new();
    for i in 0..len {
        let mut e = UndefinedValue();
        JS_GetElement(
            cx,
            arr.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut e,
            },
        );
        if e.is_string() {
            out.push(crate::js_to_rust_string(cx, e));
        }
    }
    out
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_stdio_kw(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
) -> StdioOpt {
    match get_string_prop_opt(cx, obj_h, name).as_deref() {
        Some("inherit") => StdioOpt::Inherit,
        Some("ignore") | Some("null") => StdioOpt::Ignore,
        _ => StdioOpt::Pipe,
    }
}

/// Read env / cwd / stdio keywords / stdin data / encoding / maxBuffer off an
/// options object into `cfg`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_opts_into(cx: *mut JSContext, oh: Handle<*mut JSObject>, cfg: &mut SyncSpawnConfig) {
    if let Some(dir) = get_string_prop_opt(cx, oh, c"cwd".as_ptr()) {
        cfg.cwd = Some(dir);
    }

    // env: object of string→string.
    let mut env_v = UndefinedValue();
    JS_GetProperty(
        cx,
        oh,
        c"env".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut env_v,
        },
    );
    if env_v.is_object() {
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let eo = env_v.to_object());
        let mut ids = mozjs::rust::IdVector::new(cx);
        if GetPropertyKeys(cx, eo.handle().into(), JSITER_OWNONLY, ids.handle_mut()) {
            let mut pairs = Vec::new();
            for jsid in &*ids {
                if !jsid.is_string() {
                    continue;
                }
                let key_ptr = jsid.to_string();
                if key_ptr.is_null() {
                    continue;
                }
                let key = mozjs::conversions::unsafe_jsstr_to_string(
                    cx,
                    ::std::ptr::NonNull::new_unchecked(key_ptr),
                );
                let c_key = ZBox::from_bytes(key.as_bytes());
                let mut val = UndefinedValue();
                if JS_GetProperty(
                    cx,
                    eo.handle().into(),
                    c_key.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut val,
                    },
                ) && val.is_string()
                {
                    pairs.push((key, crate::js_to_rust_string(cx, val)));
                }
            }
            cfg.env_pairs = Some(pairs);
        }
    }

    // stdin: stdio keyword, or input data (string / bytes view).
    let mut stdin_v = UndefinedValue();
    JS_GetProperty(
        cx,
        oh,
        c"stdin".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut stdin_v,
        },
    );
    if stdin_v.is_string() {
        let s = crate::js_to_rust_string(cx, stdin_v);
        match s.as_str() {
            "inherit" => cfg.stdin_opt = StdioOpt::Inherit,
            "ignore" | "null" => cfg.stdin_opt = StdioOpt::Ignore,
            _ => cfg.stdin_bytes = Some(s.into_bytes()),
        }
    } else if stdin_v.is_object() {
        if let Some(b) = crate::node_buffer::collect_byte_view(cx, stdin_v) {
            cfg.stdin_bytes = Some(b);
        }
    }

    cfg.stdout_opt = get_stdio_kw(cx, oh, c"stdout".as_ptr());
    cfg.stderr_opt = get_stdio_kw(cx, oh, c"stderr".as_ptr());

    if let Some(enc) = get_string_prop_opt(cx, oh, c"encoding".as_ptr()) {
        cfg.utf8_encoding = enc == "utf8" || enc == "utf-8" || enc == "latin1";
    }
    let mut mb = UndefinedValue();
    JS_GetProperty(
        cx,
        oh,
        c"maxBuffer".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut mb,
        },
    );
    if mb.is_number() && mb.to_number().is_finite() && mb.to_number() >= 0.0 {
        cfg.max_buffer = mb.to_number() as usize;
    }
}

/// Marshal collected bytes: Uint8Array default, string with utf8 encoding,
/// null for unpiped streams.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bytes_to_result(
    cx: &mut mozjs::context::JSContext,
    bytes: Option<Vec<u8>>,
    utf8_encoding: bool,
) -> JSVal {
    let Some(bytes) = bytes else {
        return NullValue();
    };
    if utf8_encoding {
        let s = String::from_utf8_lossy(&bytes).into_owned();
        let c_s = ZBox::from_bytes(s.as_bytes());
        let js_str = JS_NewStringCopyZ(cx.raw_cx(), c_s.as_ptr());
        return if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) };
    }
    let arr = JS_NewUint8Array(cx, bytes.len());
    if arr.is_null() {
        return UndefinedValue();
    }
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
    let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        arr,
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if !unwrapped.is_null() && !data_ptr.is_null() && length == bytes.len() {
        ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
    }
    ObjectValue(arr)
}

/// Cap collected output at `max_buffer` bytes.
fn cap_bytes(mut bytes: Vec<u8>, max_buffer: usize) -> Vec<u8> {
    if bytes.len() > max_buffer {
        bytes.truncate(max_buffer);
    }
    bytes
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_spawn_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 {
        JS_ReportErrorUTF8(cx, c"Bun.spawnSync requires a command or options object".as_ptr());
        return false;
    }

    let first = *args.get(0).ptr;
    let opts_val = if args.argc_ > 1 { *args.get(1).ptr } else { UndefinedValue() };

    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    let mut cfg = SyncSpawnConfig::new();

    // Shape 1: `Bun.spawnSync(["exe", ...args], opts?)`.
    if first.is_object() {
        rooted!(&in(cx_ref) let fv = first);
        let mut is_arr = false;
        IsArrayObject(cx, fv.handle().into(), &mut is_arr);
        if is_arr {
            rooted!(&in(cx_ref) let arr_obj = first.to_object());
            let items = get_string_array_prop(cx, arr_obj.handle().into(), c"cmd".as_ptr());
            if items.is_empty() {
                // The array itself is the argv (read via length walk).
                let mut len_v = UndefinedValue();
                JS_GetProperty(
                    cx,
                    arr_obj.handle().into(),
                    c"length".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut len_v,
                    },
                );
                let len = if len_v.is_number() { len_v.to_number() as u32 } else { 0 };
                let mut collected: Vec<String> = Vec::new();
                for i in 0..len {
                    let mut e = UndefinedValue();
                    JS_GetElement(
                        cx,
                        arr_obj.handle().into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut e,
                        },
                    );
                    if e.is_string() {
                        collected.push(crate::js_to_rust_string(cx, e));
                    }
                }
                if let Some((exe, rest)) = collected.split_first() {
                    cfg.cmd = exe.clone();
                    cfg.args = rest.to_vec();
                }
            }
            if opts_val.is_object() {
                rooted!(&in(cx_ref) let or = opts_val.to_object());
                read_opts_into(cx, or.handle().into(), &mut cfg);
                cfg.args.extend(get_string_array_prop(cx, or.handle().into(), c"args".as_ptr()));
            }
        }
    }

    // Shape 2: `Bun.spawnSync("exe", { args, ... })`.
    if cfg.cmd.is_empty() && first.is_string() {
        cfg.cmd = crate::js_to_rust_string(cx, first);
        if opts_val.is_object() {
            rooted!(&in(cx_ref) let or = opts_val.to_object());
            read_opts_into(cx, or.handle().into(), &mut cfg);
            cfg.args.extend(get_string_array_prop(cx, or.handle().into(), c"args".as_ptr()));
        }
    }

    // Shape 3: `Bun.spawnSync({ cmd: [...] | "exe", args, ... })`.
    if cfg.cmd.is_empty() && first.is_object() {
        rooted!(&in(cx_ref) let oo = first.to_object());
        let oh = oo.handle().into();
        let cmd_arr = get_string_array_prop(cx, oh, c"cmd".as_ptr());
        if !cmd_arr.is_empty() {
            let mut iter = cmd_arr.into_iter();
            cfg.cmd = iter.next().unwrap_or_default();
            cfg.args = iter.collect();
        } else {
            cfg.cmd = get_string_prop_opt(cx, oh, c"cmd".as_ptr()).unwrap_or_default();
        }
        read_opts_into(cx, oh, &mut cfg);
        if cfg.args.is_empty() {
            cfg.args = get_string_array_prop(cx, oh, c"args".as_ptr());
        }
    }

    if cfg.cmd.is_empty() {
        JS_ReportErrorUTF8(cx, c"Bun.spawnSync: missing command".as_ptr());
        return false;
    }

    // Build + run the child synchronously.
    let mut command = ::std::process::Command::new(&cfg.cmd);
    for a in &cfg.args {
        command.arg(a);
    }
    if let Some(ref dir) = cfg.cwd {
        command.current_dir(dir);
    }
    if let Some(ref pairs) = cfg.env_pairs {
        command.env_clear();
        for (k, v) in pairs {
            command.env(k, v);
        }
    }
    command.stdin(if cfg.stdin_bytes.is_some() {
        ::std::process::Stdio::piped()
    } else {
        match cfg.stdin_opt {
            StdioOpt::Inherit => ::std::process::Stdio::inherit(),
            StdioOpt::Ignore | StdioOpt::Pipe => ::std::process::Stdio::null(),
        }
    });
    command.stdout(match cfg.stdout_opt {
        StdioOpt::Pipe => ::std::process::Stdio::piped(),
        StdioOpt::Inherit => ::std::process::Stdio::inherit(),
        StdioOpt::Ignore => ::std::process::Stdio::null(),
    });
    command.stderr(match cfg.stderr_opt {
        StdioOpt::Pipe => ::std::process::Stdio::piped(),
        StdioOpt::Inherit => ::std::process::Stdio::inherit(),
        StdioOpt::Ignore => ::std::process::Stdio::null(),
    });

    let run = command.spawn().and_then(|mut child| {
        if let Some(input) = cfg.stdin_bytes.as_ref() {
            use ::std::io::Write as _;
            if let Some(mut sin) = child.stdin.take() {
                // Broken pipe is fine: the child may exit before consuming.
                let _ = sin.write_all(input);
            }
        }
        child.wait_with_output()
    });

    match run {
        Ok(output) => {
            let success = output.status.success();
            let exit_code = output.status.code();
            let stdout_bytes = if cfg.stdout_opt == StdioOpt::Pipe {
                Some(cap_bytes(output.stdout, cfg.max_buffer))
            } else {
                None
            };
            let stderr_bytes = if cfg.stderr_opt == StdioOpt::Pipe {
                Some(cap_bytes(output.stderr, cfg.max_buffer))
            } else {
                None
            };

            rooted!(&in(cx_ref) let res = JS_NewPlainObject(cx_ref));
            if res.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_ref) let sv = BooleanValue(success));
            JS_DefineProperty(
                cx,
                res.handle().into(),
                c"success".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            rooted!(&in(cx_ref) let pv = NullValue());
            JS_DefineProperty(
                cx,
                res.handle().into(),
                c"pid".as_ptr(),
                pv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            rooted!(&in(cx_ref) let ev = if let Some(c) = exit_code {
                Int32Value(c)
            } else {
                NullValue()
            });
            JS_DefineProperty(
                cx,
                res.handle().into(),
                c"exitCode".as_ptr(),
                ev.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            let so = bytes_to_result(cx_ref, stdout_bytes, cfg.utf8_encoding);
            rooted!(&in(cx_ref) let so_r = so);
            JS_DefineProperty(
                cx,
                res.handle().into(),
                c"stdout".as_ptr(),
                so_r.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            let se = bytes_to_result(cx_ref, stderr_bytes, cfg.utf8_encoding);
            rooted!(&in(cx_ref) let se_r = se);
            JS_DefineProperty(
                cx,
                res.handle().into(),
                c"stderr".as_ptr(),
                se_r.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            args.rval().set(ObjectValue(res.get()));
            true
        }
        Err(e) => {
            let msg = format!("Bun.spawnSync failed to spawn '{}': {}", cfg.cmd, e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

/// Install `Bun.spawnSync` on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    JS_DefineFunction(
        cx,
        bun_obj,
        c"spawnSync".as_ptr(),
        Some(bun_spawn_sync),
        2,
        JSPROP_ENUMERATE as u32,
    );
}
