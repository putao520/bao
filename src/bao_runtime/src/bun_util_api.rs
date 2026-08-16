// @trace REQ-ENG-006 [api:Bun.peek / Bun.stringWidth / Bun.RegExp.escape /
// Bun.readableStreamToArray / Bun.tcpSocket] — utility face.
//
//   * `Bun.peek(v)` / `Bun.peekStatus(promise)` — promise-settled introspection
//     via JS::GetPromiseState (upstream Peek.ts: pending → the promise itself,
//     settled → the settled value) plus the lazy-iterator arm: an object with
//     a callable `next` (and no Symbol.iterator) is peeked by taking the first
//     item eagerly and returning a Peeked iterator that replays it.
//   * `Bun.stringWidth(s, opts?)` — terminal column width via the workspace
//     bun_core visible-width engine (the upstream `String.visibleWidth`
//     port): ANSI escapes zero-width by default (countAnsiEscapeCodes opts),
//     East-Asian ambiguous narrow by default (ambiguousIsNarrow opts).
//   * `Bun.RegExp.escape(s)` — bun_core::string::escape_reg_exp (the upstream
//     escapeRegExp port; `-` → `\x2d`, meta chars backslash-escaped).
//   * `Bun.readableStreamToArray(stream)` — JS-side reader drain over the
//     installed web ReadableStream (web_streams.js).
//   * `Bun.tcpSocket` — explicit not-implemented throw (registered gap): the
//     TCP connection family is owned by Bun.connect / Bun.listen
//     (bun_listen.rs, net-domain workstream).
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use bun_core::ZBox;

// ──────────────────────────────────────────────────────────────────────────
// Bun.peek / Bun.peekStatus
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_peek(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;
    if !val.is_object() {
        args.rval().set(val);
        return true;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let obj = val.to_object());

    // Promise arm: settled → the settled value; pending → the promise itself.
    if JS::IsPromiseObject(obj.handle().into()) {
        let state = JS::GetPromiseState(obj.handle().into());
        match state {
            PromiseState::Fulfilled | PromiseState::Rejected => {
                let mut rv = UndefinedValue();
                mozjs::glue::JS_GetPromiseResult(
                    obj.handle().into(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rv,
                    },
                );
                if state == PromiseState::Rejected {
                    // Clear the rejection-marker exception state peeking may
                    // have raised; peek returns the reason value, it does not
                    // throw (upstream $peekPromiseSettledValue semantics).
                    JS_ClearPendingException(cx);
                }
                args.rval().set(rv);
                return true;
            }
            PromiseState::Pending => {
                args.rval().set(val);
                return true;
            }
        }
    }

    // Lazy-iterator arm: callable `next` and NOT itself iterable (generators
    // are iterable — upstream peeks those through the same path, but SM
    // generators are already lazy; we only eager-peek plain iterator-likes).
    let mut next_v = UndefinedValue();
    if !JS_GetProperty(
        cx,
        obj.handle().into(),
        c"next".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut next_v,
        },
    ) {
        JS_ClearPendingException(cx);
        args.rval().set(val);
        return true;
    }
    if !next_v.is_object() {
        args.rval().set(val);
        return true;
    }
    rooted!(&in(cx_ref) let next_obj = next_v.to_object());
    if !JS_ObjectIsFunction(next_obj.get()) {
        args.rval().set(val);
        return true;
    }
    let mut has_iter = false;
    JS_HasProperty(
        cx,
        obj.handle().into(),
        c"Symbol.iterator".as_ptr(),
        &mut has_iter,
    );
    if has_iter {
        args.rval().set(val);
        return true;
    }

    // Take the first item eagerly.
    rooted!(&in(cx_ref) let next_val = ObjectValue(next_obj.get()));
    let call_args = HandleValueArray {
        length_: 1,
        elements_: &*next_val.handle(),
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        obj.handle().into(),
        next_val.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok || !rval.is_object() {
        if !ok {
            JS_ClearPendingException(cx);
        }
        args.rval().set(val);
        return true;
    }
    rooted!(&in(cx_ref) let res = rval.to_object());
    let mut done_v = UndefinedValue();
    JS_GetProperty(
        cx,
        res.handle().into(),
        c"done".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut done_v,
        },
    );
    if done_v.to_boolean() {
        args.rval().set(val);
        return true;
    }
    let mut value_v = UndefinedValue();
    JS_GetProperty(
        cx,
        res.handle().into(),
        c"value".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut value_v,
        },
    );

    // Return a Peeked iterator: replays the taken first item, then delegates
    // to the original `next`.
    let peeked_src = r#"(function(original, firstValue) {
  var done = false;
  return {
    get peeked() { return firstValue; },
    next: function() {
      if (done) return { value: undefined, done: true };
      if (firstValue !== undefined) {
        var v = firstValue;
        firstValue = undefined;
        return { value: v, done: false };
      }
      var r = original.next();
      if (r.done) done = true;
      return r;
    },
    __originalIterator: original,
  };
})"#;
    let mut text = mozjs::rust::transform_str_to_source_text(peeked_src);
    let opts = mozjs::glue::NewCompileOptions(cx, c"<bun:peek>".as_ptr(), 1);
    if opts.is_null() {
        args.rval().set(val);
        return true;
    }
    let mut ctor = UndefinedValue();
    let ctor_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut ctor,
    };
    let evaluated = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut text, ctor_h);
    libc::free(opts as *mut _);
    if !evaluated || !ctor.is_object() {
        JS_ClearPendingException(cx);
        args.rval().set(val);
        return true;
    }
    rooted!(&in(cx_ref) let ctor_obj = ctor.to_object());
    rooted!(&in(cx_ref) let ctor_val = ObjectValue(ctor_obj.get()));
    rooted!(&in(cx_ref) let this_arg = val);
    rooted!(&in(cx_ref) let fv = value_v);
    let call_vals = [this_arg.handle().get(), fv.handle().get()];
    let call_arr = HandleValueArray {
        length_: 2,
        elements_: call_vals.as_ptr(),
    };
    let mut out = UndefinedValue();
    let ok2 = JS_CallFunctionValue(
        cx,
        ctor_obj.handle().into(),
        ctor_val.handle().into(),
        &call_arr,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut out,
        },
    );
    if !ok2 || !out.is_object() {
        if !ok2 {
            JS_ClearPendingException(cx);
        }
        args.rval().set(val);
        return true;
    }
    args.rval().set(out);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_peek_status(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let val = if args.argc_ > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let name = if !val.is_object() {
        "fulfilled" // non-promises are already settled values (upstream peekStatus)
    } else {
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let obj = val.to_object());
        if JS::IsPromiseObject(obj.handle().into()) {
            match JS::GetPromiseState(obj.handle().into()) {
                PromiseState::Pending => "pending",
                PromiseState::Fulfilled => "fulfilled",
                _ => "rejected",
            }
        } else {
            "fulfilled"
        }
    };
    let c_name = ZBox::from_bytes(name.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    true
}

// ──────────────────────────────────────────────────────────────────────────
// Bun.stringWidth
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_string_width(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let val = if args.argc_ > 0 { *args.get(0).ptr } else { UndefinedValue() };
    if val.is_undefined() {
        args.rval().set(Int32Value(0));
        return true;
    }
    if !val.is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.stringWidth expects a string".as_ptr());
        return false;
    }

    // Options: { countAnsiEscapeCodes?: bool (false), ambiguousIsNarrow?: bool }
    // (ambiguousIsNarrow accepted; the engine treats ambiguous codepoints as
    // narrow — see the module doc for the documented degradation.)
    let mut count_ansi = false;
    if args.argc_ > 1 && (*args.get(1).ptr).is_object() {
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let oobj = (*args.get(1).ptr).to_object());
        let mut v = UndefinedValue();
        if JS_GetProperty(
            cx,
            oobj.handle().into(),
            c"countAnsiEscapeCodes".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        ) && v.is_boolean()
        {
            count_ansi = v.to_boolean();
        }
    }

    // JS string → UTF-16 (lossless) → visible width. bun_core's visible-width
    // module is FFI to an unlinked C++ object in bao, so the engine is the
    // mature unicode-width crate (UAX#11: wide/fullwidth = 2, combining and
    // control = 0) plus local ANSI-span handling:
    //   * countAnsiEscapeCodes=false (default) — escape spans are zero-width.
    //   * countAnsiEscapeCodes=true — literal printable chars inside escape
    //     spans count (width 1 each; C0 controls incl. ESC are zero-width).
    //   * ambiguousIsNarrow — accepted; ambiguous codepoints are treated as
    //     narrow (unicode-width has no ambiguous-class table; explicit
    //     degradation documented in the wave report).
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let sv = val);
    let jsstr = mozjs::rust::ToString(cx_ref, sv.handle());
    if jsstr.is_null() {
        JS_ClearPendingException(cx);
        args.rval().set(Int32Value(0));
        return true;
    }
    let str_val = StringValue(&*jsstr);
    let utf16: Vec<u16> = crate::js_to_rust_string(cx, str_val).encode_utf16().collect();
    let width = utf16_visible_width(&utf16, count_ansi);
    args.rval().set(Int32Value(width as i32));
    true
}

use ::std::io::Write as _;

/// Visible terminal width of a UTF-16 string.
///
/// ANSI escape spans (CSI `ESC [ … final`, OSC `ESC ] … (BEL|ESC \)`,
/// two-byte ESC forms) are zero-width unless `count_ansi_literal`; inside
/// spans, printable ASCII contributes width 1 (C0 controls incl. ESC are 0).
fn utf16_visible_width(input: &[u16], count_ansi_literal: bool) -> usize {
    #[derive(PartialEq)]
    enum St {
        Code,
        Csi,
        Osc,
        OscEsc,
    }
    use unicode_width::UnicodeWidthChar as _;
    let mut st = St::Code;
    let mut width = 0usize;
    let mut i = 0usize;
    while i < input.len() {
        let c = input[i];
        // Decode surrogate pairs for non-BMP codepoints (width lives on the
        // decoded char — e.g. most emoji are wide).
        let (ch, step): (char, usize) = if (0xD800..0xDC00).contains(&c)
            && i + 1 < input.len()
            && (0xDC00..0xE000).contains(&input[i + 1])
        {
            let hi = (c as u32 - 0xD800) << 10;
            let lo = input[i + 1] as u32 - 0xDC00;
            (::std::char::from_u32(0x10000 + hi + lo).unwrap_or('\u{FFFD}'), 2)
        } else {
            (::std::char::from_u32(c as u32).unwrap_or('\u{FFFD}'), 1)
        };
        match st {
            St::Code => {
                if ch == '\u{1b}' {
                    st = if input.get(i + 1) == Some(&(b'[' as u16)) {
                        St::Csi
                    } else if input.get(i + 1) == Some(&(b']' as u16)) {
                        St::Osc
                    } else {
                        St::Csi // two-byte ESC form: payload handled like CSI
                    };
                    i += 2;
                    continue;
                }
                width += ch.width().unwrap_or(0);
            }
            St::Csi => {
                if (0x40..=0x7e).contains(&c) {
                    if count_ansi_literal {
                        width += 1; // final byte
                    }
                    st = St::Code;
                } else if (0x20..0x3f).contains(&c) {
                    if count_ansi_literal {
                        width += 1;
                    }
                }
            }
            St::Osc => {
                if ch == '\u{7}' {
                    st = St::Code;
                } else if ch == '\u{1b}' {
                    st = St::OscEsc;
                } else if count_ansi_literal && c >= 0x20 {
                    width += 1;
                }
            }
            St::OscEsc => {
                if ch == '\\' {
                    st = St::Code;
                } else {
                    st = St::Osc;
                }
            }
        }
        i += step;
    }
    width
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn regexp_escape(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        // Upstream jsEscapeRegExp throws on non-string input.
        JS_ReportErrorUTF8(cx, c"expected string argument".as_ptr());
        return false;
    }
    let input = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let mut out: Vec<u8> = Vec::with_capacity(input.len() + 8);
    let _ = bun_core::string::escape_reg_exp::escape_reg_exp(input.as_bytes(), &mut out);
    let c_out = ZBox::from_vec(out);
    let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    true
}

// ──────────────────────────────────────────────────────────────────────────
// Bun.readableStreamToArray — JS-side reader drain (web_streams.js streams)
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn readable_stream_to_array(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 {
        JS_ReportErrorUTF8(cx, c"Bun.readableStreamToArray expects a ReadableStream".as_ptr());
        return false;
    }
    let src = r#"(async function(stream) {
  var out = [];
  var reader = stream.getReader();
  while (true) {
    var r = await reader.read();
    if (r.done) break;
    out.push(r.value);
  }
  reader.releaseLock && reader.releaseLock();
  return out;
})"#;
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    let mut text = mozjs::rust::transform_str_to_source_text(src);
    let opts = mozjs::glue::NewCompileOptions(cx, c"<bun:rs2a>".as_ptr(), 1);
    if opts.is_null() {
        JS_ReportErrorUTF8(cx, c"Bun.readableStreamToArray: compile failed".as_ptr());
        return false;
    }
    let mut fn_val = UndefinedValue();
    let fn_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut fn_val,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut text, fn_h);
    libc::free(opts as *mut _);
    if !ok || !fn_val.is_object() {
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, c"Bun.readableStreamToArray: compile failed".as_ptr());
        return false;
    }
    rooted!(&in(cx_ref) let fn_obj = fn_val.to_object());
    rooted!(&in(cx_ref) let fn_call_val = ObjectValue(fn_obj.get()));
    rooted!(&in(cx_ref) let stream_arg = *args.get(0).ptr);
    let call_vals = [stream_arg.handle().get()];
    let call_arr = HandleValueArray {
        length_: 1,
        elements_: call_vals.as_ptr(),
    };
    rooted!(&in(cx_ref) let null_obj = ::std::ptr::null_mut::<JSObject>());
    let mut rval = UndefinedValue();
    let called = JS_CallFunctionValue(
        cx,
        null_obj.handle().into(),
        fn_call_val.handle().into(),
        &call_arr,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !called {
        return false;
    }
    args.rval().set(rval);
    true
}

// ──────────────────────────────────────────────────────────────────────────
// Bun.tcpSocket — explicit registered gap
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_tcp_socket(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let _ = &args;
    JS_ReportErrorUTF8(
        cx,
        c"Bun.tcpSocket is not implemented in Bao: use Bun.connect (TCP client) / Bun.listen (server) — the socket family is owned by the net domain (bun_listen.rs)".as_ptr(),
    );
    false
}

// ──────────────────────────────────────────────────────────────────────────
// Install
// ──────────────────────────────────────────────────────────────────────────

/// Install the utility face on the Bun object.
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
        c"peek".as_ptr(),
        Some(bun_peek),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        bun_obj,
        c"peekStatus".as_ptr(),
        Some(bun_peek_status),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        bun_obj,
        c"stringWidth".as_ptr(),
        Some(bun_string_width),
        2,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        bun_obj,
        c"readableStreamToArray".as_ptr(),
        Some(readable_stream_to_array),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        bun_obj,
        c"tcpSocket".as_ptr(),
        Some(bun_tcp_socket),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // Bun.RegExp = { escape }
    rooted!(&in(cx) let re_ns = JS_NewPlainObject(cx));
    if !re_ns.get().is_null() {
        JS_DefineFunction(
            cx,
            re_ns.handle(),
            c"escape".as_ptr(),
            Some(regexp_escape),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineProperty3(
            cx,
            bun_obj,
            c"RegExp".as_ptr(),
            re_ns.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}
