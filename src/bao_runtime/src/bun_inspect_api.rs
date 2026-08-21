// @trace REQ-ENG-006 [api:Bun.inspect] — value formatter.
//
// Upstream Bun.inspect is a Zig-native formatter (BunObject.getInspect).
// This is the SM bridge: single-line, depth-capped, cycle-safe formatting
// for the common value space — primitives (strings double-quoted with
// escapes), arrays, plain objects, Map/Set, Date (toISOString), RegExp,
// Error, Promise (state via JS::GetPromiseState), functions, TypedArray /
// ArrayBuffer, and `nodejs.util.inspect.custom` dispatch.
//
// Options (2nd arg, upstream-compatible subset):
//   * `depth`    — max recursion (default 2; `Infinity` = unlimited)
//   * `colors`   — ANSI colourisation (default false)
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2::{GetBuiltinClass, JS_DefineFunction};

const RESET: &str = "\x1b[0m";
const COLOR_STRING: &str = "\x1b[32m"; // green
const COLOR_NUMBER: &str = "\x1b[33m"; // yellow
const COLOR_KEY: &str = "\x1b[36m"; // cyan
const COLOR_SPECIAL: &str = "\x1b[33m"; // bool/null/undefined — yellow
const COLOR_ERROR: &str = "\x1b[31m"; // red

struct InspectOpts {
    depth: u32,
    colors: bool,
}

fn paint(opts: &InspectOpts, s: String, color: &str) -> String {
    if opts.colors {
        format!("{}{}{}", color, s, RESET)
    } else {
        s
    }
}

/// JS identifier-ish key → bare, otherwise double-quote it.
fn format_key(key: &str) -> String {
    let valid = !key.is_empty()
        && key
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c == '$' || c.is_alphanumeric() && (i > 0 || !c.is_ascii_digit()));
    if valid {
        key.to_string()
    } else {
        escape_quoted(key)
    }
}

/// Double-quote a string with JSON-style escapes (control chars → \u00XX).
fn escape_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Format a number the way JS displays it (integers without a fraction).
fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }
    } else if n == n.trunc() && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

/// Read a string property off an object ("" when absent/non-string).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn obj_string_prop(cx: *mut JSContext, obj: mozjs::rust::Handle<*mut JSObject>, name: &[u8]) -> String {
    let name_z = bun_core::ZBox::from_bytes(name);
    let mut v = UndefinedValue();
    if !JS_GetProperty(
        cx,
        obj.into(),
        name_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    ) {
        JS_ClearPendingException(cx);
        return String::new();
    }
    if v.is_string() {
        crate::js_to_rust_string(cx, v)
    } else {
        String::new()
    }
}

/// Call a 0-arg method on an object and take its string result ("" on failure).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn call_to_string_method(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    method: &[u8],
) -> String {
    let method_z = bun_core::ZBox::from_bytes(method);
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        cx,
        obj.into(),
        method_z.as_ptr(),
        &HandleValueArray {
            length_: 0,
            elements_: ::std::ptr::null(),
        },
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if ok && rval.is_string() {
        crate::js_to_rust_string(cx, rval)
    } else {
        if !ok {
            JS_ClearPendingException(cx);
        }
        String::new()
    }
}

/// Drive a JS iterator object to completion, collecting `value` payloads.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn drain_iterator(
    cx: *mut JSContext,
    iter: mozjs::rust::Handle<*mut JSObject>,
    out: &mut Vec<JSVal>,
    cap: usize,
) {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let iter_root = iter.get());
    loop {
        if out.len() >= cap {
            return;
        }
        let mut next_rv = UndefinedValue();
        let ok = JS_CallFunctionName(
            cx,
            iter_root.handle().into(),
            c"next".as_ptr(),
            &HandleValueArray {
                length_: 0,
                elements_: ::std::ptr::null(),
            },
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut next_rv,
            },
        );
        if !ok || !next_rv.is_object() {
            if !ok {
                JS_ClearPendingException(cx);
            }
            return;
        }
        rooted!(&in(cx_ref) let res = next_rv.to_object());
        let mut done_v = UndefinedValue();
        if !JS_GetProperty(
            cx,
            res.handle().into(),
            c"done".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut done_v,
            },
        ) {
            JS_ClearPendingException(cx);
            return;
        }
        if done_v.to_boolean() {
            return;
        }
        let mut value_v = UndefinedValue();
        if !JS_GetProperty(
            cx,
            res.handle().into(),
            c"value".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut value_v,
            },
        ) {
            JS_ClearPendingException(cx);
            return;
        }
        out.push(value_v);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn inspect_val(
    cx: *mut JSContext,
    val: JSVal,
    depth: u32,
    opts: &InspectOpts,
    seen: &mut Vec<usize>,
) -> String {
    if val.is_undefined() {
        return paint(opts, "undefined".into(), COLOR_SPECIAL);
    }
    if val.is_null() {
        return paint(opts, "null".into(), COLOR_SPECIAL);
    }
    if val.is_boolean() {
        return paint(
            opts,
            if val.to_boolean() { "true" } else { "false" }.into(),
            COLOR_SPECIAL,
        );
    }
    if val.is_int32() {
        return paint(opts, format_number(val.to_int32() as f64), COLOR_NUMBER);
    }
    if val.is_double() {
        return paint(opts, format_number(val.to_double()), COLOR_NUMBER);
    }
    if val.is_string() {
        let s = crate::js_to_rust_string(cx, val);
        return paint(opts, escape_quoted(&s), COLOR_STRING);
    }
    if val.is_symbol() || val.is_bigint() {
        // Symbols/BigInts: canonical via SM ToString.
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(
            cx,
        ));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let sv = val);
        let jsstr = mozjs::rust::ToString(cx_ref, sv.handle());
        if !jsstr.is_null() {
            let str_val = mozjs::jsval::StringValue(&*jsstr);
            return paint(
                opts,
                crate::js_to_rust_string(cx, str_val),
                if val.is_bigint() { COLOR_NUMBER } else { COLOR_SPECIAL },
            );
        }
        JS_ClearPendingException(cx);
        return paint(opts, "Symbol()".into(), COLOR_SPECIAL);
    }
    if !val.is_object() {
        return "undefined".to_string();
    }

    // ── objects ──────────────────────────────────────────────────────────
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let obj = val.to_object());
    let obj_ptr = obj.get() as usize;

    // Cycle guard — every nested visit checks the ancestry chain.
    if seen.contains(&obj_ptr) {
        return "[Circular]".to_string();
    }

    let at_depth_cap = depth == 0;

    // Classify via JS::GetBuiltinClass.
    let mut cls = ESClass::Other;
    let classified = GetBuiltinClass(cx_ref, obj.handle(), &mut cls);
    if !classified {
        JS_ClearPendingException(cx);
        cls = ESClass::Other;
    }

    // Functions (functions are not an ESClass member — probe separately).
    if JS_ObjectIsFunction(obj.get()) {
        let name = obj_string_prop(cx, obj.handle(), b"name");
        return if name.is_empty() {
            "[Function (anonymous)]".to_string()
        } else {
            format!("[Function: {}]", name)
        };
    }

    match cls {
        ESClass::RegExp => {
            let src = obj_string_prop(cx, obj.handle(), b"source");
            let flags = obj_string_prop(cx, obj.handle(), b"flags");
            return format!("/{}/{}", src, flags);
        }
        ESClass::Date => {
            let iso = call_to_string_method(cx, obj.handle(), b"toISOString");
            if iso.is_empty() {
                let s = call_to_string_method(cx, obj.handle(), b"toString");
                return format!("Invalid Date ({})", if s.is_empty() { "Invalid Date".into() } else { s });
            }
            return iso;
        }
        ESClass::Error => {
            let name = obj_string_prop(cx, obj.handle(), b"name");
            let msg = obj_string_prop(cx, obj.handle(), b"message");
            let head = if msg.is_empty() {
                if name.is_empty() { "Error".to_string() } else { name }
            } else if name.is_empty() {
                format!("Error: {}", msg)
            } else {
                format!("{}: {}", name, msg)
            };
            // First stack frame when present (upstream prints the stack).
            let stack = obj_string_prop(cx, obj.handle(), b"stack");
            if !stack.is_empty() {
                if let Some(second) = stack.split('\n').nth(1) {
                    let trimmed = second.trim();
                    if !trimmed.is_empty() {
                        return paint(
                            opts,
                            format!("{}\n  {}", head, trimmed.trim_start_matches("at ")),
                            COLOR_ERROR,
                        );
                    }
                }
            }
            return paint(opts, head, COLOR_ERROR);
        }
        ESClass::Promise => {
            let state = JS::GetPromiseState(obj.handle().into());
            let body = match state {
                PromiseState::Fulfilled => {
                    let mut rv = UndefinedValue();
                    mozjs::glue::JS_GetPromiseResult(
                        obj.handle().into(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rv,
                        },
                    );
                    seen.push(obj_ptr);
                    let s = inspect_val(cx, rv, depth.saturating_sub(1), opts, seen);
                    seen.pop();
                    s
                }
                PromiseState::Rejected => {
                    let mut rv = UndefinedValue();
                    mozjs::glue::JS_GetPromiseResult(
                        obj.handle().into(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rv,
                        },
                    );
                    JS_ClearPendingException(cx);
                    seen.push(obj_ptr);
                    let s = inspect_val(cx, rv, depth.saturating_sub(1), opts, seen);
                    seen.pop();
                    format!("<rejected> {}", s)
                }
                _ => "<pending>".to_string(),
            };
            return format!("Promise {{ {} }}", body);
        }
        ESClass::Map | ESClass::Set => {
            if at_depth_cap {
                return if cls == ESClass::Map { "[Map]".into() } else { "[Set]".into() };
            }
            seen.push(obj_ptr);
            let method: &[u8] = if cls == ESClass::Map { b"entries" } else { b"values" };
            let name_z = bun_core::ZBox::from_bytes(method);
            let mut iter_v = UndefinedValue();
            let ok = JS_CallFunctionName(
                cx,
                obj.handle().into(),
                name_z.as_ptr(),
                &HandleValueArray {
                    length_: 0,
                    elements_: ::std::ptr::null(),
                },
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut iter_v,
                },
            );
            let mut items: Vec<JSVal> = Vec::new();
            if ok && iter_v.is_object() {
                rooted!(&in(cx_ref) let iter_obj = iter_v.to_object());
                drain_iterator(cx, iter_obj.handle(), &mut items, 100);
            } else {
                JS_ClearPendingException(cx);
            }
            let n = items.len();
            let mut parts: Vec<String> = Vec::with_capacity(n);
            for item in items {
                if cls == ESClass::Map && item.is_object() {
                    rooted!(&in(cx_ref) let pair = item.to_object());
                    let mut k = UndefinedValue();
                    JS_GetElement(cx, pair.handle().into(), 0, MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut k,
                    });
                    let mut v = UndefinedValue();
                    JS_GetElement(cx, pair.handle().into(), 1, MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut v,
                    });
                    let ks = inspect_val(cx, k, depth - 1, opts, seen);
                    let vs = inspect_val(cx, v, depth - 1, opts, seen);
                    parts.push(format!("{} => {}", ks, vs));
                } else {
                    parts.push(inspect_val(cx, item, depth - 1, opts, seen));
                }
            }
            seen.pop();
            let head = if cls == ESClass::Map { "Map" } else { "Set" };
            return format!("{}({}) {{ {} }}", head, n, parts.join(", "));
        }
        ESClass::ArrayBuffer => {
            let byte_len = obj_string_prop(cx, obj.handle(), b"byteLength");
            return format!("ArrayBuffer {{ byteLength: {} }}", byte_len);
        }
        _ => {}
    }

    // TypedArrays / DataView: ESClass has no per-kind variants in SM; detect
    // views through the object-as-view API (same probe node_buffer uses).
    let ta_bytes = crate::node_buffer::collect_byte_view(cx, val);
    {
        let mut view_length: usize = 0;
        let mut view_shared = false;
        let mut view_data: *mut u8 = ::std::ptr::null_mut();
        let view_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
            obj.get(),
            &mut view_length,
            &mut view_shared,
            &mut view_data,
        );
        if view_unwrapped.is_null() {
            return plain_object_or_array(cx, val, obj_ptr, at_depth_cap, depth, opts, seen);
        }
    }
    {
        let ctor_name = obj_string_prop(cx, obj.handle(), b"constructor");
        // constructor.name travels through the prototype chain.
        let tname = if ctor_name.is_empty() {
            "TypedArray".to_string()
        } else {
            ctor_name
        };
        if at_depth_cap {
            return format!("{}(...)", tname);
        }
        if let Some(bytes) = ta_bytes {
            if tname == "Uint8Array" || tname == "Uint8ClampedArray" || tname == "Int8Array" {
                let elems: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                return format!("{}({}) [ {} ]", tname, bytes.len(), elems.join(", "));
            }
        }
        // Non-byte-width views: render via the index space.
        let len = {
            let mut lv = UndefinedValue();
            JS_GetProperty(cx, obj.handle().into(), c"length".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut lv,
            });
            if lv.is_number() { lv.to_number() as u32 } else { 0 }
        };
        seen.push(obj_ptr);
        let mut elems: Vec<String> = Vec::new();
        for i in 0..len.min(100) {
            let mut ev = UndefinedValue();
            JS_GetElement(cx, obj.handle().into(), i, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ev,
            });
            elems.push(inspect_val(cx, ev, depth - 1, opts, seen));
        }
        seen.pop();
        return format!("{}({}) [ {} ]", tname, len, elems.join(", "));
    }
}

/// Plain object / array rendering (shared tail of the object space).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn plain_object_or_array(
    cx: *mut JSContext,
    val: JSVal,
    obj_ptr: usize,
    at_depth_cap: bool,
    depth: u32,
    opts: &InspectOpts,
    seen: &mut Vec<usize>,
) -> String {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let obj = val.to_object());
    let mut is_arr = false;
    rooted!(&in(cx_ref) let v_root = val);
    IsArrayObject(cx, v_root.handle().into(), &mut is_arr);
    if is_arr {
        if at_depth_cap {
            return "[Array]".to_string();
        }
        let mut len_v = UndefinedValue();
        JS_GetProperty(cx, obj.handle().into(), c"length".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_v,
        });
        let len = if len_v.is_number() { len_v.to_number() as u32 } else { 0 };
        seen.push(obj_ptr);
        let mut parts: Vec<String> = Vec::new();
        for i in 0..len.min(100) {
            let mut ev = UndefinedValue();
            JS_GetElement(cx, obj.handle().into(), i, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ev,
            });
            parts.push(inspect_val(cx, ev, depth - 1, opts, seen));
        }
        seen.pop();
        if parts.is_empty() {
            return "[]".to_string();
        }
        return format!("[ {} ]", parts.join(", "));
    }

    if at_depth_cap {
        return "[Object]".to_string();
    }

    // nodejs.util.inspect.custom dispatch (upstream honours it).
    {
        let mut custom_v = UndefinedValue();
        let has_custom = JS_GetProperty(
            cx,
            obj.handle().into(),
            c"nodejs.util.inspect.custom".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut custom_v,
            },
        );
        if has_custom && custom_v.is_object() {
            rooted!(&in(cx_ref) let custom_fn = custom_v.to_object());
            if JS_ObjectIsFunction(custom_fn.get()) {
                let depth_arg = DoubleValue(depth as f64);
                let args = [depth_arg];
                let call_args = HandleValueArray {
                    length_: 1,
                    elements_: args.as_ptr(),
                };
                rooted!(&in(cx_ref) let this_val = val);
                rooted!(&in(cx_ref) let custom_val = ObjectValue(custom_fn.get()));
                let mut rval = UndefinedValue();
                let ok = JS_CallFunctionValue(
                    cx,
                    obj.handle().into(),
                    custom_val.handle().into(),
                    &call_args,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
                if ok && rval.is_string() {
                    let _ = &this_val;
                    return crate::js_to_rust_string(cx, rval);
                }
                if !ok {
                    JS_ClearPendingException(cx);
                }
            }
        } else if !has_custom {
            JS_ClearPendingException(cx);
        }
    }

    // Plain object: own enumerable string keys.
    seen.push(obj_ptr);
    let mut ids = mozjs::rust::IdVector::new(cx_ref);
    let ok = GetPropertyKeys(cx, obj.handle().into(), JSITER_OWNONLY, ids.handle_mut());
    let mut parts: Vec<String> = Vec::new();
    if ok {
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
            let key_z = bun_core::ZBox::from_bytes(key.as_bytes());
            let mut v = UndefinedValue();
            JS_GetProperty(
                cx,
                obj.handle().into(),
                key_z.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            );
            let rendered_key = paint(opts, format_key(&key), COLOR_KEY);
            parts.push(format!("{}: {}", rendered_key, inspect_val(cx, v, depth - 1, opts, seen)));
        }
    }
    seen.pop();
    if parts.is_empty() {
        return "{}".to_string();
    }
    format!("{{ {} }}", parts.join(", "))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_inspect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 {
        let js_str = JS_NewStringCopyZ(cx, c"undefined".as_ptr());
        args.rval().set(if js_str.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*js_str)
        });
        return true;
    }

    // Options: { depth?: number, colors?: boolean } (upstream-compatible).
    let mut opts = InspectOpts { depth: 2, colors: false };
    if args.argc_ > 1 {
        let o = *args.get(1).ptr;
        if o.is_object() {
            let mut wrapped =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let oobj = o.to_object());
            let mut dv = UndefinedValue();
            if JS_GetProperty(
                cx,
                oobj.handle().into(),
                c"depth".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut dv,
                },
            ) && dv.is_number()
            {
                // to_number (not to_double): is_number() covers int32-tagged
                // values too, and to_double asserts is_double.
                let d = dv.to_number();
                opts.depth = if d.is_infinite() { u32::MAX } else { d.max(0.0) as u32 };
            }
            let mut cv = UndefinedValue();
            if JS_GetProperty(
                cx,
                oobj.handle().into(),
                c"colors".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut cv,
                },
            ) && cv.is_boolean()
            {
                opts.colors = cv.to_boolean();
            }
        }
    }

    let val = *args.get(0).ptr;
    // Root call carries one extra level of budget: `depth: N` caps NESTED
    // levels — the root itself always renders its keys (node semantics:
    // inspect({d:{e:{f:1}}}, {depth:0}) → "{ d: [Object] }").
    let s = inspect_val(cx, val, opts.depth.saturating_add(1), &opts, &mut Vec::new());
    let utf16: Vec<u16> = s.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*js_str)
    });
    let _ = Int32Value(0);
    let _ = BooleanValue(false);
    true
}

/// Install `Bun.inspect` on the Bun object.
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
        c"inspect".as_ptr(),
        Some(bun_inspect),
        2,
        JSPROP_ENUMERATE as u32,
    );
}
