// @trace REQ-ENG-006 [api:Bun.inspect] — value formatter.
//
// Upstream Bun.inspect is a Zig-native formatter (BunObject.getInspect).
// This is the SM bridge: single-line, depth-capped, cycle-safe formatting
// for the common value space — primitives (strings double-quoted with
// escapes), arrays (elements first, then non-index own properties), plain
// objects (integer-index keys ascending first, then string keys in
// insertion order — Node/Bun inspect key order), Map/Set (native
// iterator reads: JS::MapEntries/JS::SetValues + install-time-captured
// pristine `next`), Date (native js::DateIsValid/js::DateGetMsecSinceEpoch
// + spec ISO math), RegExp, Error (own-property reads + prototype-identity
// class names), Promise (state via JS::GetPromiseState), functions,
// TypedArray / ArrayBuffer, and `nodejs.util.inspect.custom` dispatch
// (registered symbol + legacy string property).
//
// Built-in class reads never go through user-replaceable prototype
// members: Map/Set entries, the Date time value and Error class identity
// are read through SM native JSAPI, so prototype pollution cannot reach
// the output.
// domain-check 06d0ae8ac1/abe2ad4f00/a21f02a988 (own-idiom fix, upstream oracle)
//
// Swallowed exceptions are counted (`swallowed_exception_count`) and render
// as `<exception>` placeholders — never as silent empty strings.
//
// Options (2nd arg, upstream-compatible subset):
//   * `depth`    — max recursion (default 2; `Infinity` = unlimited)
//   * `colors`   — ANSI colourisation (default false)
use ::std::cell::Cell;

use mozjs::jsapi::*;
use mozjs::jsval::{
    DoubleValue, JSVal, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2::GetBuiltinClass;

const RESET: &str = "\x1b[0m";
const COLOR_STRING: &str = "\x1b[32m"; // green
const COLOR_NUMBER: &str = "\x1b[33m"; // yellow
const COLOR_KEY: &str = "\x1b[36m"; // cyan
const COLOR_SPECIAL: &str = "\x1b[33m"; // bool/null/undefined — yellow
const COLOR_ERROR: &str = "\x1b[31m"; // red

/// Placeholder rendered where a user-side throw or engine failure left no
/// usable value. Never an empty string — empty output hid failures (the
/// silent-blank defect class this module eradicates).
const PLACEHOLDER_THREW: &str = "<exception>";

thread_local! {
    /// Diagnostics counter: exceptions swallowed (cleared + placeholder
    /// rendered) during Bun.inspect formatting. Reset per root call and
    /// surfaced via `swallowed_exception_count()` so a silent-blank
    /// regression can never go unnoticed.
    static SWALLOWED_EXCEPTIONS: Cell<u32> = const { Cell::new(0) };
}

fn note_swallowed() {
    SWALLOWED_EXCEPTIONS.with(|c| c.set(c.get().saturating_add(1)));
}

/// Exceptions swallowed during the most recent Bun.inspect call.
pub fn swallowed_exception_count() -> u32 {
    SWALLOWED_EXCEPTIONS.with(|c| c.get())
}

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

/// days-since-epoch → (year, month, day) on the proleptic Gregorian
/// calendar — Howard Hinnant's `civil_from_days` (deterministic integer
/// math, no user code).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// ECMA-262 `Date.prototype.toISOString` rendering from the epoch-ms value:
/// UTC, milliseconds always 3 digits, extended (signed 6-digit) years
/// outside 0..=9999. Pure spec math over the native time-value slot — the
/// user-replaceable `Date.prototype.toISOString` is never invoked.
fn iso_string_from_msec(msec: f64) -> String {
    let ms = msec as i64;
    let days = ms.div_euclid(86_400_000);
    let mut tod = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let hour = tod / 3_600_000;
    tod -= hour * 3_600_000;
    let min = tod / 60_000;
    tod -= min * 60_000;
    let sec = tod / 1_000;
    let frac = tod - sec * 1_000;
    let year = if (0..=9999).contains(&y) {
        format!("{:04}", y)
    } else {
        let sign = if y < 0 { "-" } else { "+" };
        format!("{}{:06}", sign, y.abs())
    };
    format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, m, d, hour, min, sec, frac
    )
}

/// Intrinsic error prototypes (JSProtoKey) → spec class names. Identity
/// against these — never property lookup — resolves an Error's `name`, so
/// a polluted `Error.prototype.name` cannot reach inspect output.
const ERROR_PROTO_TABLE: &[(JSProtoKey, &str)] = &[
    (JSProtoKey::JSProto_Error, "Error"),
    (JSProtoKey::JSProto_TypeError, "TypeError"),
    (JSProtoKey::JSProto_RangeError, "RangeError"),
    (JSProtoKey::JSProto_EvalError, "EvalError"),
    (JSProtoKey::JSProto_ReferenceError, "ReferenceError"),
    (JSProtoKey::JSProto_SyntaxError, "SyntaxError"),
    (JSProtoKey::JSProto_URIError, "URIError"),
    (JSProtoKey::JSProto_AggregateError, "AggregateError"),
];

/// Spec class name for a native-error instance: walk the prototype chain
/// (bounded — exotic proxies can fake depth) and compare each hop against
/// the realm's intrinsic error prototypes. Pointer identity is truth:
/// which intrinsic an object derives from determines its spec class name,
/// regardless of property pollution. Falls back to "Error".
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn native_error_class_name(cx: *mut JSContext, obj: *mut JSObject) -> String {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    let mut cur: *mut JSObject = obj;
    for _ in 0..10 {
        rooted!(&in(cx_ref) let cur_root = cur);
        let mut next: *mut JSObject = ::std::ptr::null_mut();
        if !JS_GetPrototype(
            cx,
            cur_root.handle().into(),
            MutableHandle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut next,
            },
        ) {
            JS_ClearPendingException(cx);
            note_swallowed();
            break;
        }
        if next.is_null() {
            break;
        }
        for (key, name) in ERROR_PROTO_TABLE {
            let mut proto: *mut JSObject = ::std::ptr::null_mut();
            if JS_GetClassPrototype(
                cx,
                *key,
                MutableHandle::<*mut JSObject> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut proto,
                },
            ) && proto == next
            {
                return name.to_string();
            }
        }
        cur = next;
    }
    "Error".to_string()
}

/// Read a string property off an object ("" when absent/non-string,
/// placeholder + count when the read itself threw).
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
        note_swallowed();
        return PLACEHOLDER_THREW.to_string();
    }
    if v.is_string() {
        crate::js_to_rust_string(cx, v)
    } else {
        String::new()
    }
}

/// Stringify a JSVal the way Node stringifies Error name/message fields
/// (`String(value)`); engine ToString failures count and degrade to the
/// placeholder.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn value_to_display_string(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    v: JSVal,
) -> String {
    if v.is_string() {
        return crate::js_to_rust_string(cx, v);
    }
    rooted!(&in(cx_ref) let hv = v);
    let jsstr = mozjs::rust::ToString(cx_ref, hv.handle());
    if !jsstr.is_null() {
        let sval = StringValue(&*jsstr);
        return crate::js_to_rust_string(cx, sval);
    }
    JS_ClearPendingException(cx);
    note_swallowed();
    PLACEHOLDER_THREW.to_string()
}

/// Own-property string read: JS_HasOwnProperty gates a [[Get]] so the value
/// comes from the object itself (own data or own accessor — the owner's
/// explicit definition), never from a polluted prototype. Absent → None;
/// engine failures count as swallowed exceptions.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn own_prop_string(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    name: &[u8],
) -> Option<String> {
    let name_z = bun_core::ZBox::from_bytes(name);
    let mut found = false;
    if !JS_HasOwnProperty(cx, obj.into(), name_z.as_ptr(), &mut found) {
        JS_ClearPendingException(cx);
        note_swallowed();
        return None;
    }
    if !found {
        return None;
    }
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
        note_swallowed();
        return None;
    }
    Some(value_to_display_string(cx, cx_ref, v))
}

/// Node `determineSpecificType`-shaped "Received …" fragment for
/// ERR_INVALID_ARG_TYPE messages. Shared with node_util's util.inspect
/// options validation.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn received_type_fragment(cx: *mut JSContext, val: JSVal) -> String {
    if val.is_number() {
        return format!("type number ({})", format_number(val.to_number()));
    }
    if val.is_string() {
        return format!("type string ('{}')", crate::js_to_rust_string(cx, val));
    }
    if val.is_boolean() {
        return format!("type boolean ({})", val.to_boolean());
    }
    if val.is_symbol() || val.is_bigint() {
        let mut wrapped =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let hv = val);
        let jsstr = mozjs::rust::ToString(cx_ref, hv.handle());
        if !jsstr.is_null() {
            let s = crate::js_to_rust_string(cx, StringValue(&*jsstr));
            return if val.is_bigint() {
                format!("type bigint ({}n)", s)
            } else {
                format!("type symbol ({})", s)
            };
        }
        JS_ClearPendingException(cx);
        note_swallowed();
        return if val.is_bigint() { "type bigint".to_string() } else { "type symbol".to_string() };
    }
    "type object".to_string()
}

/// TypeError with `code: "ERR_INVALID_ARG_TYPE"` (house pattern:
/// globals.rs throw_error_with_code / node_crypto argon2_throw_with_code).
/// Returns false with the exception pending — the JSNative return contract.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn throw_invalid_arg_type(cx: *mut JSContext, msg: &str) -> bool {
    let c_msg = ::std::ffi::CString::new(msg)
        .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
    {
        let mut cx_s = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        mozjs::error::throw_type_error_safe(&mut cx_s, c_msg.as_ref());
    }
    if JS_IsExceptionPending(cx) {
        rooted!(in(cx) let mut exn = UndefinedValue());
        JS_GetPendingException(cx, exn.handle_mut().into());
        let exn_val = exn.get();
        if !exn_val.is_undefined() && exn_val.is_object() {
            let cx_ref_err =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref_err) let exn_root = exn_val.to_object());
            let code_str = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(b"ERR_INVALID_ARG_TYPE").as_ptr());
            if !code_str.is_null() {
                let code_val = StringValue(&*code_str);
                rooted!(&in(cx_ref_err) let code_r = code_val);
                JS_DefineProperty(
                    cx,
                    exn_root.handle().into(),
                    c"code".as_ptr(),
                    code_r.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                JS_SetPendingException(
                    cx,
                    exn.handle().into(),
                    ExceptionStackBehavior::DoNotCapture,
                );
            }
        }
    }
    false
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
            let str_val = StringValue(&*jsstr);
            return paint(
                opts,
                crate::js_to_rust_string(cx, str_val),
                if val.is_bigint() { COLOR_NUMBER } else { COLOR_SPECIAL },
            );
        }
        JS_ClearPendingException(cx);
        note_swallowed();
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
        note_swallowed();
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
            // Native slot reads (js::DateIsValid / js::DateGetMsecSinceEpoch)
            // + spec ISO math — the user-replaceable
            // Date.prototype.toISOString is never invoked.
            // domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
            let mut valid = false;
            if !mozjs_sys::jsapi::js::DateIsValid(cx, obj.handle().into(), &mut valid) {
                JS_ClearPendingException(cx);
                note_swallowed();
                valid = false;
            }
            if !valid {
                return "Invalid Date".to_string();
            }
            let mut msec: f64 = 0.0;
            if !mozjs_sys::jsapi::js::DateGetMsecSinceEpoch(cx, obj.handle().into(), &mut msec)
                || !msec.is_finite()
            {
                JS_ClearPendingException(cx);
                note_swallowed();
                return "Invalid Date".to_string();
            }
            return iso_string_from_msec(msec);
        }
        ESClass::Error => {
            // `name`: own property first (the owner's explicit definition),
            // else prototype IDENTITY against the realm's intrinsic error
            // prototypes — never a [[Get]] that could surface a polluted
            // Error.prototype.name. `message` is own on every
            // constructor-made error; non-own reads as empty (Node prints
            // the name alone). `stack` stays a [[Get]]: SM has no JSAPI
            // stack reader, and stack is an own per-instance accessor
            // (registered divergence).
            // domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
            let name = match own_prop_string(cx, cx_ref, obj.handle(), b"name") {
                Some(n) => n,
                None => native_error_class_name(cx, obj.get()),
            };
            let msg = own_prop_string(cx, cx_ref, obj.handle(), b"message").unwrap_or_default();
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
            let is_map = cls == ESClass::Map;
            let head = if is_map { "Map" } else { "Set" };
            // Native table iteration: JS::MapEntries / JS::SetValues create
            // the iterator through the map's internal table (SM: MapObject.cpp
            // CreateIterator — never the user-replaceable
            // Map.prototype.entries / Set.prototype.values), and the drain
            // calls the install-time-captured pristine
            // %Map/SetIteratorPrototype%.next. No user code runs.
            // domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
            let mut iter_val = UndefinedValue();
            let made_iter = if is_map {
                mozjs_sys::jsapi::JS::MapEntries(
                    cx,
                    obj.handle().into(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut iter_val,
                    },
                )
            } else {
                mozjs_sys::jsapi::JS::SetValues(
                    cx,
                    obj.handle().into(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut iter_val,
                    },
                )
            };
            if !made_iter || !iter_val.is_object() {
                JS_ClearPendingException(cx);
                note_swallowed();
                seen.pop();
                return format!("{} {{ {} }}", head, PLACEHOLDER_THREW);
            }
            rooted!(&in(cx_ref) let iter_root = iter_val.to_object());
            // Captured pristine `next`; without the bag (pre-install or
            // tampered global) fall back to the prototype name call.
            let mut next_val = UndefinedValue();
            let have_next = primordial_iter_next(cx, cx_ref, is_map, &mut next_val);
            rooted!(&in(cx_ref) let next_root = next_val);
            // Header count is the native table size even when rendering
            // truncates (Node prints Map(150) for a 150-entry map).
            let n = if is_map {
                mozjs_sys::jsapi::JS::MapSize(cx, obj.handle().into())
            } else {
                mozjs_sys::jsapi::JS::SetSize(cx, obj.handle().into())
            } as usize;
            let render_n = n.min(100);
            let mut parts: Vec<String> = Vec::with_capacity(render_n);
            for _ in 0..render_n {
                let mut res_val = UndefinedValue();
                let called = if have_next {
                    JS_CallFunctionValue(
                        cx,
                        iter_root.handle().into(),
                        next_root.handle().into(),
                        &HandleValueArray {
                            length_: 0,
                            elements_: ::std::ptr::null(),
                        },
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut res_val,
                        },
                    )
                } else {
                    JS_CallFunctionName(
                        cx,
                        iter_root.handle().into(),
                        c"next".as_ptr(),
                        &HandleValueArray {
                            length_: 0,
                            elements_: ::std::ptr::null(),
                        },
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut res_val,
                        },
                    )
                };
                if !called || !res_val.is_object() {
                    if !called {
                        JS_ClearPendingException(cx);
                        note_swallowed();
                    }
                    break;
                }
                // Fresh iterator-result object: `done`/`value` are own
                // properties, so these reads cannot pick up prototype noise.
                rooted!(&in(cx_ref) let res_root = res_val.to_object());
                let mut done_v = UndefinedValue();
                if !JS_GetProperty(
                    cx,
                    res_root.handle().into(),
                    c"done".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut done_v,
                    },
                ) {
                    JS_ClearPendingException(cx);
                    note_swallowed();
                    break;
                }
                if done_v.to_boolean() {
                    break;
                }
                let mut value_v = UndefinedValue();
                if !JS_GetProperty(
                    cx,
                    res_root.handle().into(),
                    c"value".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut value_v,
                    },
                ) {
                    JS_ClearPendingException(cx);
                    note_swallowed();
                    break;
                }
                // Map iterators yield fresh [key, value] pair arrays; Set
                // iterators yield the element directly (a Set of undefined
                // is legitimate, a Map entry that is not a pair is not).
                if is_map && !value_v.is_object() {
                    // Unreachable for the native entries iterator — kept as
                    // a self-diagnosing guard: counted + placeholder, never
                    // a silent "undefined" part.
                    note_swallowed();
                    parts.push(PLACEHOLDER_THREW.to_string());
                    continue;
                }
                if is_map {
                    rooted!(&in(cx_ref) let pair = value_v.to_object());
                    let mut k = UndefinedValue();
                    let got_k = JS_GetElement(cx, pair.handle().into(), 0, MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut k,
                    });
                    let mut v = UndefinedValue();
                    let got_v = JS_GetElement(cx, pair.handle().into(), 1, MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut v,
                    });
                    if !got_k || !got_v {
                        JS_ClearPendingException(cx);
                        note_swallowed();
                        continue;
                    }
                    parts.push(format!(
                        "{} => {}",
                        inspect_val(cx, k, depth - 1, opts, seen),
                        inspect_val(cx, v, depth - 1, opts, seen)
                    ));
                } else {
                    parts.push(inspect_val(cx, value_v, depth - 1, opts, seen));
                }
            }
            seen.pop();
            return format!("{}({}) {{ {} }}", head, n, parts.join(", "));
        }
        ESClass::ArrayBuffer => {
            // byteLength is a number — read the value, not a string
            // coercion (the old string read rendered
            // `ArrayBuffer { byteLength:  }` for every ArrayBuffer).
            let mut bl = UndefinedValue();
            if !JS_GetProperty(
                cx,
                obj.handle().into(),
                c"byteLength".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut bl,
                },
            ) {
                JS_ClearPendingException(cx);
                note_swallowed();
                return format!("ArrayBuffer {{ byteLength: {} }}", PLACEHOLDER_THREW);
            }
            if bl.is_number() {
                return format!("ArrayBuffer {{ byteLength: {} }}", format_number(bl.to_number()));
            }
            note_swallowed();
            return format!("ArrayBuffer {{ byteLength: {} }}", PLACEHOLDER_THREW);
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
        // Constructor display name: `.constructor.name` (the old code read
        // `.constructor` itself — always a function, never a string — so
        // every view rendered as "TypedArray").
        let mut ctor_name = String::new();
        let mut ctor_val = UndefinedValue();
        let got_ctor = JS_GetProperty(
            cx,
            obj.handle().into(),
            c"constructor".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctor_val,
            },
        );
        if got_ctor && ctor_val.is_object() {
            rooted!(&in(cx_ref) let ctor_obj = ctor_val.to_object());
            ctor_name = obj_string_prop(cx, ctor_obj.handle(), b"name");
        } else if !got_ctor {
            JS_ClearPendingException(cx);
            note_swallowed();
        }
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

/// Render own enumerable entries — integer-index keys bare and ascending
/// first, then string keys in insertion order (engine enumeration order,
/// which is Node/Bun inspect key order) — into `parts` as `key: value`
/// lines. `skip_indices_below` skips array-index keys the caller already
/// rendered as elements (those below `length`).
/// domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn render_own_keyed_entries(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    skip_indices_below: Option<u32>,
    depth: u32,
    opts: &InspectOpts,
    seen: &mut Vec<usize>,
    parts: &mut Vec<String>,
) {
    let mut ids = mozjs::rust::IdVector::new(cx_ref);
    // `obj` is already a `mozjs::rust::Handle` (the parameter type) — no
    // rooted-guard `.handle()` re-wrap, only the raw-conversion `.into()`.
    let ok = GetPropertyKeys(cx, obj.into(), JSITER_OWNONLY, ids.handle_mut());
    if !ok {
        JS_ClearPendingException(cx);
        note_swallowed();
        return;
    }
    for jsid in &*ids {
        if jsid.is_int() {
            let idx = jsid.to_int() as u32;
            if let Some(below) = skip_indices_below {
                if idx < below {
                    continue;
                }
            }
            let mut v = UndefinedValue();
            if !JS_GetElement(cx, obj.into(), idx, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            }) {
                JS_ClearPendingException(cx);
                note_swallowed();
                continue;
            }
            let rendered_key = paint(opts, idx.to_string(), COLOR_KEY);
            parts.push(format!("{}: {}", rendered_key, inspect_val(cx, v, depth - 1, opts, seen)));
            continue;
        }
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
        if !JS_GetProperty(
            cx,
            obj.into(),
            key_z.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        ) {
            JS_ClearPendingException(cx);
            note_swallowed();
            continue;
        }
        let rendered_key = paint(opts, format_key(&key), COLOR_KEY);
        parts.push(format!("{}: {}", rendered_key, inspect_val(cx, v, depth - 1, opts, seen)));
    }
}

/// Invoke a `nodejs.util.inspect.custom` hook (0-arg dispatch carrying the
/// current depth). Returns the hook's string output; None = absent /
/// non-callable / non-string result / throwing hook (throwing hooks are
/// counted and fall through to normal rendering — never a silent blank).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn invoke_custom_inspect(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    custom_v: JSVal,
    depth: u32,
) -> Option<String> {
    if !custom_v.is_object() {
        return None;
    }
    rooted!(&in(cx_ref) let custom_fn = custom_v.to_object());
    if !JS_ObjectIsFunction(custom_fn.get()) {
        return None;
    }
    let depth_arg = DoubleValue(depth as f64);
    let args = [depth_arg];
    let call_args = HandleValueArray {
        length_: 1,
        elements_: args.as_ptr(),
    };
    rooted!(&in(cx_ref) let custom_val = ObjectValue(custom_fn.get()));
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        obj.into(),
        custom_val.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if ok && rval.is_string() {
        return Some(crate::js_to_rust_string(cx, rval));
    }
    if !ok {
        JS_ClearPendingException(cx);
        note_swallowed();
    }
    None
}

/// nodejs.util.inspect.custom dispatch — the registered
/// Symbol.for("nodejs.util.inspect.custom") key (the Node/Bun entry) first,
/// then the legacy plain-string property. Both stay user entry points.
/// domain-check a21f02a988 (own-idiom fix, upstream oracle)
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn try_custom_inspect(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    depth: u32,
) -> Option<String> {
    rooted!(&in(cx_ref) let key_str = JS_NewStringCopyZ(cx, c"nodejs.util.inspect.custom".as_ptr()));
    if !key_str.is_null() {
        // Registered symbols are pinned by the runtime's symbol registry —
        // the stack jsid needs no rooting for this call window.
        let sym = mozjs_sys::jsapi::JS::GetSymbolFor(cx, key_str.handle().into());
        if !sym.is_null() {
            let custom_id = mozjs::jsid::SymbolId(sym);
            let mut custom_v = UndefinedValue();
            if JS_GetPropertyById(
                cx,
                obj.into(),
                Handle::from_marked_location(&custom_id),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut custom_v,
                },
            ) {
                if let Some(out) = invoke_custom_inspect(cx, cx_ref, obj, custom_v, depth) {
                    return Some(out);
                }
            } else {
                JS_ClearPendingException(cx);
                note_swallowed();
            }
        }
    }
    // Legacy string-keyed property (the pre-symbol entry, kept working).
    let mut custom_v = UndefinedValue();
    let has_custom = JS_GetProperty(
        cx,
        obj.into(),
        c"nodejs.util.inspect.custom".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut custom_v,
        },
    );
    if has_custom {
        invoke_custom_inspect(cx, cx_ref, obj, custom_v, depth)
    } else {
        JS_ClearPendingException(cx);
        note_swallowed();
        None
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
        // Non-index own properties render after the elements — Node renders
        // `[ 1, 2, 3, foo: 'x' ]`; own integer indices >= length render as
        // `N: value` keys (below-length indices are element space).
        // domain-check 06d0ae8ac1 (own-idiom fix, upstream oracle)
        render_own_keyed_entries(cx, cx_ref, obj.handle(), Some(len), depth, opts, seen, &mut parts);
        seen.pop();
        if parts.is_empty() {
            return "[]".to_string();
        }
        return format!("[ {} ]", parts.join(", "));
    }

    if at_depth_cap {
        return "[Object]".to_string();
    }

    // nodejs.util.inspect.custom dispatch (upstream honours it). The hook
    // receives the levels-from-root depth (Node's recurseTimes), which is
    // one below this frame's remaining budget.
    if let Some(custom_out) = try_custom_inspect(cx, cx_ref, obj.handle(), depth.saturating_sub(1)) {
        return custom_out;
    }

    // Plain object: own enumerable entries — integer-index keys ascending
    // first, then string keys in insertion order.
    seen.push(obj_ptr);
    let mut parts: Vec<String> = Vec::new();
    render_own_keyed_entries(cx, cx_ref, obj.handle(), None, depth, opts, seen, &mut parts);
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
    SWALLOWED_EXCEPTIONS.with(|c| c.set(0));

    // Options: { depth?: number, colors?: boolean } (upstream-compatible).
    // Node contract: null/undefined options are ignored; any other
    // non-object throws ERR_INVALID_ARG_TYPE (function objects are objects
    // in JSVal terms and parse like any options bag).
    // domain-check a21f02a988 (own-idiom fix, upstream oracle)
    let mut opts = InspectOpts { depth: 2, colors: false };
    if args.argc_ > 1 {
        let o = *args.get(1).ptr;
        if !o.is_undefined() && !o.is_null() {
            if !o.is_object() {
                let msg = format!(
                    "The \"options\" argument must be of type object. Received {}",
                    received_type_fragment(cx, o)
                );
                return throw_invalid_arg_type(cx, &msg);
            }
            let mut wrapped =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let oobj = o.to_object());
            let mut dv = UndefinedValue();
            let got_depth = JS_GetProperty(
                cx,
                oobj.handle().into(),
                c"depth".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut dv,
                },
            );
            if !got_depth {
                JS_ClearPendingException(cx);
                note_swallowed();
            } else if dv.is_number() {
                // to_number (not to_double): is_number() covers int32-tagged
                // values too, and to_double asserts is_double.
                let d = dv.to_number();
                opts.depth = if d.is_infinite() { u32::MAX } else { d.max(0.0) as u32 };
            }
            let mut cv = UndefinedValue();
            let got_colors = JS_GetProperty(
                cx,
                oobj.handle().into(),
                c"colors".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut cv,
                },
            );
            if !got_colors {
                JS_ClearPendingException(cx);
                note_swallowed();
            } else if cv.is_boolean() {
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
    true
}

/// Name of the hidden global property pinning this module's captured
/// primordials (non-enumerable + readonly + permanent — invisible to
/// enumeration and untamperable from script).
const PRIMORDIAL_BAG_PROP: &[u8] = b"__baoInspectPrims";

/// Capture one pristine iterator `next` intrinsic into the bag: build the
/// native iterator off a fresh empty table (JS::NewMapObject /
/// JS::NewSetObject + JS::MapEntries / JS::SetValues — the internal-table
/// path), then read `next` off the iterator's REAL prototype. Runs at
/// install time, before any user script, so the read yields the engine
/// original no matter what user code later does to the prototype.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn capture_iter_next_into_bag(
    cx: &mut mozjs::context::JSContext,
    bag: mozjs::rust::Handle<*mut JSObject>,
    table: *mut JSObject,
    map: bool,
    prop: &[u8],
) {
    let raw = cx.raw_cx();
    rooted!(&in(cx) let table_root = table);
    let mut iter_val = UndefinedValue();
    let made_iter = if map {
        mozjs_sys::jsapi::JS::MapEntries(raw, table_root.handle().into(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut iter_val,
        })
    } else {
        mozjs_sys::jsapi::JS::SetValues(raw, table_root.handle().into(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut iter_val,
        })
    };
    if !made_iter || !iter_val.is_object() {
        JS_ClearPendingException(raw);
        return;
    }
    rooted!(&in(cx) let iter_root = iter_val.to_object());
    let mut proto: *mut JSObject = ::std::ptr::null_mut();
    if !JS_GetPrototype(
        raw,
        iter_root.handle().into(),
        MutableHandle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto,
        },
    ) || proto.is_null() {
        JS_ClearPendingException(raw);
        return;
    }
    rooted!(&in(cx) let proto_root = proto);
    let mut next_val = UndefinedValue();
    let next_z = bun_core::ZBox::from_bytes(b"next");
    if !JS_GetProperty(
        raw,
        proto_root.handle().into(),
        next_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut next_val,
        },
    ) || !next_val.is_object() || !JS_ObjectIsFunction(next_val.to_object()) {
        JS_ClearPendingException(raw);
        return;
    }
    rooted!(&in(cx) let next_root = next_val.to_object());
    let prop_z = bun_core::ZBox::from_bytes(prop);
    mozjs::rust::wrappers2::JS_DefineProperty3(
        cx,
        bag,
        prop_z.as_ptr(),
        next_root.handle(),
        (JSPROP_READONLY | JSPROP_PERMANENT) as u32,
    );
}

/// Pin the pristine Map/Set iterator `next` intrinsics on the context
/// global, under a hidden permanent property. Runtime Map/Set rendering
/// drains the NATIVE iterator by calling the captured original `next`, so
/// neither `Map.prototype.entries` / `Set.prototype.values` nor
/// `%Map/SetIteratorPrototype%.next` replacements can reach the output.
/// Best-effort: a failed capture only drops the runtime to the prototype
/// name-call fallback (registered), never blocks install.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn install_primordial_bag(cx: &mut mozjs::context::JSContext) {
    let global = CurrentGlobalOrNull(cx.raw_cx());
    if global.is_null() {
        return;
    }
    rooted!(&in(cx) let global_root = global);

    // Idempotent: a second install keeps the first (pristine) capture.
    {
        let mut existing = UndefinedValue();
        let bag_z = bun_core::ZBox::from_bytes(PRIMORDIAL_BAG_PROP);
        let got = JS_GetProperty(
            cx.raw_cx(),
            global_root.handle().into(),
            bag_z.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut existing,
            },
        );
        if !got {
            JS_ClearPendingException(cx.raw_cx());
        } else if existing.is_object() {
            return;
        }
    }

    rooted!(&in(cx) let bag = mozjs::rust::wrappers2::JS_NewPlainObject(cx));
    if bag.get().is_null() {
        return;
    }

    let map_obj = mozjs_sys::jsapi::JS::NewMapObject(cx.raw_cx());
    if !map_obj.is_null() {
        capture_iter_next_into_bag(cx, bag.handle(), map_obj, true, b"mapIterNext");
    }
    let set_obj = mozjs_sys::jsapi::JS::NewSetObject(cx.raw_cx());
    if !set_obj.is_null() {
        capture_iter_next_into_bag(cx, bag.handle(), set_obj, false, b"setIterNext");
    }

    let bag_z = bun_core::ZBox::from_bytes(PRIMORDIAL_BAG_PROP);
    mozjs::rust::wrappers2::JS_DefineProperty3(
        cx,
        global_root.handle(),
        bag_z.as_ptr(),
        bag.handle(),
        (JSPROP_READONLY | JSPROP_PERMANENT) as u32,
    );
}

/// Fetch the install-time-captured pristine iterator `next` from the
/// context-global bag into `out` (false = bag absent/tampered; the caller
/// falls back to the iterator-prototype name call).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn primordial_iter_next(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    map: bool,
    out: &mut JSVal,
) -> bool {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return false;
    }
    rooted!(&in(cx_ref) let global_root = global);
    let mut bag_val = UndefinedValue();
    let bag_z = bun_core::ZBox::from_bytes(PRIMORDIAL_BAG_PROP);
    if !JS_GetProperty(
        cx,
        global_root.handle().into(),
        bag_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut bag_val,
        },
    ) || !bag_val.is_object() {
        JS_ClearPendingException(cx);
        return false;
    }
    rooted!(&in(cx_ref) let bag_root = bag_val.to_object());
    let mut fn_val = UndefinedValue();
    let name_z = bun_core::ZBox::from_bytes(if map { b"mapIterNext" } else { b"setIterNext" });
    if !JS_GetProperty(
        cx,
        bag_root.handle().into(),
        name_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut fn_val,
        },
    ) || !fn_val.is_object() || !JS_ObjectIsFunction(fn_val.to_object()) {
        JS_ClearPendingException(cx);
        return false;
    }
    *out = fn_val;
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
    install_primordial_bag(cx);
    mozjs::rust::wrappers2::JS_DefineFunction(
        cx,
        bun_obj,
        c"inspect".as_ptr(),
        Some(bun_inspect),
        2,
        JSPROP_ENUMERATE as u32,
    );
}
