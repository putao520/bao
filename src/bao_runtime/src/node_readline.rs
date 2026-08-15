// @trace REQ-ENG-007
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let rl_mod = unsafe { w2::JS_NewPlainObject(cx) });
    if rl_mod.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            rl_mod.handle(),
            c"createInterface".as_ptr(),
            Some(rl_create_interface),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            rl_mod.handle(),
            c"clearLine".as_ptr(),
            Some(rl_clear_line),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            rl_mod.handle(),
            c"clearScreenDown".as_ptr(),
            Some(rl_clear_screen),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            rl_mod.handle(),
            c"cursorTo".as_ptr(),
            Some(rl_cursor_to),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            rl_mod.handle(),
            c"moveCursor".as_ptr(),
            Some(rl_move_cursor),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            rl_mod.handle(),
            c"emitKeypressEvents".as_ptr(),
            Some(rl_emit_keypress),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Create the readline.promises namespace with a Promise-based
        // createInterface and an Interface class that supports .question()
        // returning a Promise (matching Bun's readline.promises shape).
        rooted!(&in(cx) let promises_obj = w2::JS_NewPlainObject(cx));
        if !promises_obj.get().is_null() {
            w2::JS_DefineFunction(
                cx,
                promises_obj.handle(),
                c"createInterface".as_ptr(),
                Some(rl_promises_create_interface),
                1,
                JSPROP_ENUMERATE as u32,
            );
            w2::JS_DefineFunction(
                cx,
                promises_obj.handle(),
                c"Interface".as_ptr(),
                Some(rl_promises_interface_ctor),
                1,
                JSPROP_ENUMERATE as u32,
            );

            rooted!(&in(cx) let prom_val = ObjectValue(promises_obj.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                rl_mod.handle().into(),
                c"promises".as_ptr(),
                prom_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    cache_builtin(cx, "readline", rl_mod.get());

    // Cache the promises sub-object as a standalone module so
    // require("readline/promises") works via node_subpath_aliases.
    let rl_cached = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, "builtin:readline");
    if let Some(rl_obj) = rl_cached {
        if !rl_obj.is_null() {
            unsafe {
                let raw_cx = cx.raw_cx();
                rooted!(&in(cx) let rl_root = rl_obj);
                let mut prom_val = UndefinedValue();
                JS_GetProperty(
                    raw_cx,
                    rl_root.handle().into(),
                    c"promises".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut prom_val,
                    },
                );
                if prom_val.is_object() {
                    cache_builtin(cx, "readline/promises", prom_val.to_object());
                }
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_create_interface(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let iface = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if iface.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut input_val = UndefinedValue();
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        rooted!(&in(wrapped_cx) let opts_root = opts);
        JS_GetProperty(
            cx,
            opts_root.handle().into(),
            c"input".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut input_val,
            },
        );
    }
    rooted!(&in(wrapped_cx) let input_val_root = input_val);
    JS_DefineProperty(
        cx,
        iface.handle().into(),
        c"input".as_ptr(),
        input_val_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(wrapped_cx) let closed_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        iface.handle().into(),
        c"closed".as_ptr(),
        closed_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(wrapped_cx) let paused_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        iface.handle().into(),
        c"paused".as_ptr(),
        paused_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // on — delegate to EventEmitter
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"on".as_ptr(),
        Some(crate::node_events::ee_on),
        2,
        JSPROP_ENUMERATE as u32,
    );
    // close — mark as closed
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"close".as_ptr(),
        Some(rl_close),
        0,
        JSPROP_ENUMERATE as u32,
    );
    // pause/resume — toggle paused flag
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"pause".as_ptr(),
        Some(rl_pause),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"resume".as_ptr(),
        Some(rl_resume),
        0,
        JSPROP_ENUMERATE as u32,
    );
    // write/prompt/setPrompt — return this for chaining. question is NOT in
    // this list: it has real stdin-reading semantics (see rl_question).
    for name in &["write", "prompt", "setPrompt"] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        JS_DefineFunction(
            cx,
            iface.handle().into(),
            c_name.as_ptr(),
            Some(rl_chain),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }
    // question(query, callback) — writes the prompt, reads one real line
    // from stdin, invokes callback(answer).
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"question".as_ptr(),
        Some(rl_question),
        2,
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(ObjectValue(iface.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = this.to_object());
    rooted!(&in(wrapped_cx) let closed_v = BooleanValue(true));
    JS_DefineProperty(
        cx,
        this_obj.handle().into(),
        c"closed".as_ptr(),
        closed_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_pause(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = this.to_object());
    rooted!(&in(wrapped_cx) let paused_v = BooleanValue(true));
    JS_DefineProperty(
        cx,
        this_obj.handle().into(),
        c"paused".as_ptr(),
        paused_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_resume(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj = this.to_object());
    rooted!(&in(wrapped_cx) let paused_v = BooleanValue(false));
    JS_DefineProperty(
        cx,
        this_obj.handle().into(),
        c"paused".as_ptr(),
        paused_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// Read one line (up to and including `\n`) from a raw file descriptor.
/// Returns the line without the trailing `\n`/`\r\n`, `None` on EOF before
/// any byte was read, or `Err` on a read error.
///
/// Byte-wise reads on purpose: wrapping fd 0 in a `File` would close the fd
/// on drop, and a line of interactive input is far below syscall overhead
/// thresholds.
fn read_line_from_fd(fd: i32) -> ::std::result::Result<Option<String>, ::std::io::Error> {
    let mut out: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        // SAFETY: plain read(2) on a valid fd; buffer is a valid 1-byte ptr.
        let n = unsafe { libc::read(fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
        if n < 0 {
            return Err(::std::io::Error::last_os_error());
        }
        if n == 0 {
            // EOF — no data at all means "stdin closed", a partial line
            // without trailing newline is still returned as an answer.
            return Ok(if out.is_empty() {
                None
            } else {
                Some(trim_line_ending(out))
            });
        }
        if byte[0] == b'\n' {
            return Ok(Some(trim_line_ending(out)));
        }
        out.push(byte[0]);
    }
}

/// Strip a single trailing `\r` (CRLF input), if present.
fn trim_line_ending(mut line: Vec<u8>) -> String {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).unwrap_or_default()
}

/// Write the `question()` prompt (args[`idx`], when a string) to stdout
/// without a trailing newline. Prompt display is best-effort: a closed
/// stdout must not prevent reading the answer.
fn print_question_prompt(cx: *mut JSContext, args: &CallArgs, idx: u32) {
    if idx >= args.argc_ {
        return;
    }
    // SAFETY: reading an argv slot handed to us by SpiderMonkey; no GC can
    // run between the read and the string conversion.
    let v = unsafe { *args.get(idx).ptr };
    if !v.is_string() {
        return;
    }
    // SAFETY: v is a JS string value on a live cx.
    let s = unsafe { crate::js_to_rust_string(cx, v) };
    if s.is_empty() {
        return;
    }
    use ::std::io::Write;
    let mut out = ::std::io::stdout();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// readline Interface .question(query, callback) — writes the prompt, reads
/// one real line from stdin, then invokes `callback(answer)`.
///
/// Fails closed: EOF or a read error throws instead of fabricating an empty
/// answer (silent-fake eradication group D — this previously resolved
/// with `''` via a no-op chain method).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_question(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if argc < 2 || !(*args.get(1).ptr).is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"readline.question(query, callback): callback must be a function".as_ptr(),
        );
        return false;
    }

    print_question_prompt(cx, &args, 0);

    match read_line_from_fd(0) {
        Ok(Some(line)) => {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let callback = (*args.get(1).ptr).to_object());
            let js_str = JS_NewStringCopyN(
                cx,
                line.as_ptr() as *const libc::c_char,
                line.len(),
            );
            if js_str.is_null() {
                JS_ReportErrorUTF8(cx, c"readline.question: failed to allocate answer string".as_ptr());
                return false;
            }
            rooted!(&in(wrapped_cx) let str_val = StringValue(&*js_str));
            let elems = [str_val.get()];
            let call_args = HandleValueArray {
                length_: 1,
                elements_: elems.as_ptr(),
            };
            rooted!(&in(wrapped_cx) let cb_val = ObjectValue(callback.get()));
            rooted!(&in(wrapped_cx) let global = CurrentGlobalOrNull(cx));
            rooted!(&in(wrapped_cx) let mut call_rval = UndefinedValue());
            if !JS_CallFunctionValue(
                cx,
                global.handle().into(),
                cb_val.handle().into(),
                &call_args,
                call_rval.handle_mut().into(),
            ) {
                return false;
            }
            let this = args.thisv();
            if this.is_object() {
                args.rval().set(*this.ptr);
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
        Ok(None) => {
            JS_ReportErrorUTF8(
                cx,
                c"readline.question: stdin closed before an answer was read (EOF)".as_ptr(),
            );
            false
        }
        Err(e) => {
            let msg = format!("readline.question: failed to read from stdin: {}", e);
            if let Ok(c_msg) = ::std::ffi::CString::new(msg) {
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            }
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_chain(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if this.is_object() {
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(_cx));
        rooted!(&in(wrapped_cx) let this_obj = this.to_object());
        args.rval().set(ObjectValue(this_obj.get()));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_clear_line(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_clear_screen(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_cursor_to(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_move_cursor(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_emit_keypress(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

/// readline.promises.createInterface — returns the Interface synchronously;
/// its question() returns a Promise (see rl_promises_question).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_promises_create_interface(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    // Create an Interface object the same way rl_create_interface does
    rooted!(&in(wrapped_cx) let iface = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if iface.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut input_val = UndefinedValue();
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        rooted!(&in(wrapped_cx) let opts_root = opts);
        JS_GetProperty(
            cx,
            opts_root.handle().into(),
            c"input".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut input_val,
            },
        );
    }
    rooted!(&in(wrapped_cx) let input_val_root = input_val);
    JS_DefineProperty(
        cx,
        iface.handle().into(),
        c"input".as_ptr(),
        input_val_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(wrapped_cx) let closed_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        iface.handle().into(),
        c"closed".as_ptr(),
        closed_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(wrapped_cx) let paused_val = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(
        cx,
        iface.handle().into(),
        c"paused".as_ptr(),
        paused_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"on".as_ptr(),
        Some(crate::node_events::ee_on),
        2,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"close".as_ptr(),
        Some(rl_close),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"pause".as_ptr(),
        Some(rl_pause),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"resume".as_ptr(),
        Some(rl_resume),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // question() returns a Promise (readline/promises spec)
    JS_DefineFunction(
        cx,
        iface.handle().into(),
        c"question".as_ptr(),
        Some(rl_promises_question),
        1,
        JSPROP_ENUMERATE as u32,
    );

    for name in &["write", "prompt", "setPrompt"] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        JS_DefineFunction(
            cx,
            iface.handle().into(),
            c_name.as_ptr(),
            Some(rl_chain),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    // Return the Interface synchronously (node:readline/promises semantics:
    // createInterface() yields the Interface directly — only question()
    // returns a Promise). The previous Promise.resolve(iface) wrapper broke
    // the standard `const rl = createInterface(...); rl.question(...)` shape.
    args.rval().set(ObjectValue(iface.get()));
    true
}

/// readline.promises.Interface — constructor for the promises Interface class.
/// Same as rl_promises_create_interface but callable as `new Interface(options)`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_promises_interface_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // Delegate to rl_promises_create_interface which creates an Interface
    // with question() returning Promise.
    rl_promises_create_interface(cx, argc, vp)
}

/// readline/promises Interface .question() — writes the prompt, reads one
/// real line from stdin, returns a Promise resolved with the answer string
/// (or rejected on EOF/read error — never a fabricated empty answer).
///
/// Silent-fake eradication group D: this previously resolved `''`
/// immediately without reading anything.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn rl_promises_question(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    print_question_prompt(cx, &args, 0);

    // Read the real answer, then wrap it in Promise.resolve / Promise.reject
    // via the same eval'd-thunk pattern used by rl_promises_create_interface.
    let (thunk_src, answer) = match read_line_from_fd(0) {
        Ok(Some(line)) => ("(function(v) { return Promise.resolve(v); })", Ok(line)),
        Ok(None) => (
            "(function(m) { return Promise.reject(new Error(m)); })",
            Err("readline question(): stdin closed before an answer was read (EOF)".to_string()),
        ),
        Err(e) => (
            "(function(m) { return Promise.reject(new Error(m)); })",
            Err(format!("readline question(): failed to read from stdin: {}", e)),
        ),
    };

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let opts = mozjs::glue::NewCompileOptions(cx, c"<rl_question>".as_ptr(), 1);
    if opts.is_null() {
        JS_ReportErrorUTF8(cx, c"readline question(): failed to create compile options".as_ptr());
        return false;
    }
    let mut thunk = UndefinedValue();
    let ok = mozjs_sys::jsapi::JS::Evaluate2(
        cx,
        opts,
        &mut mozjs::rust::transform_str_to_source_text(thunk_src),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut thunk,
        },
    );
    libc::free(opts as *mut _);
    if !ok || !thunk.is_object() {
        JS_ReportErrorUTF8(cx, c"readline question(): failed to build Promise wrapper".as_ptr());
        return false;
    }

    rooted!(&in(wrapped_cx) let thunk_obj = thunk.to_object());
    let payload = match &answer {
        Ok(line) => {
            let js_str = JS_NewStringCopyN(cx, line.as_ptr() as *const libc::c_char, line.len());
            if js_str.is_null() {
                JS_ReportErrorUTF8(
                    cx,
                    c"readline question(): failed to allocate answer string".as_ptr(),
                );
                return false;
            }
            StringValue(&*js_str)
        }
        Err(msg) => match ::std::ffi::CString::new(msg.as_str()) {
            Ok(c_msg) => {
                let js_str = JS_NewStringCopyN(cx, c_msg.as_ptr(), msg.len());
                if !js_str.is_null() {
                    StringValue(&*js_str)
                } else {
                    UndefinedValue()
                }
            }
            Err(_) => UndefinedValue(),
        },
    };
    rooted!(&in(wrapped_cx) let payload_root = payload);
    let elems = [payload_root.get()];
    let call_args = HandleValueArray {
        length_: 1,
        elements_: elems.as_ptr(),
    };
    rooted!(&in(wrapped_cx) let thunk_val = ObjectValue(thunk_obj.get()));
    rooted!(&in(wrapped_cx) let global = CurrentGlobalOrNull(cx));
    rooted!(&in(wrapped_cx) let mut call_rval = UndefinedValue());
    if !JS_CallFunctionValue(
        cx,
        global.handle().into(),
        thunk_val.handle().into(),
        &call_args,
        call_rval.handle_mut().into(),
    ) {
        return false;
    }
    args.rval().set(call_rval.get());
    true
}
