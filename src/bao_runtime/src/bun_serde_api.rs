// @trace REQ-ENG-006 [api:Bun.TOML / Bun.YAML / Bun.JSONC] — serializer face.
//
// Bridge to the workspace `bun_parsers` crate (the upstream Bun port of
// TOML/YAML/JSON5 over the `bun_ast::Expr` value tree):
//
//   * `Bun.TOML.parse(text)`    — bun_parsers::toml::TOML::parse → Expr → JS
//   * `Bun.TOML.stringify(v)`   — JS value → TOML text (tables, arrays of
//                                 tables, scalars; throws on unrepresentable
//                                 values — TOML has no null/undefined)
//   * `Bun.YAML.parse(text)`    — bun_parsers::yaml::YAML::parse → Expr → JS
//                                 (multi-document streams become arrays,
//                                 matching upstream)
//   * `Bun.JSONC.parse(text)`   — comment/trailing-comma stripping scanner
//                                 + the engine's own `JSON.parse` (SpiderMonkey
//                                 parser — exact JS number/key-order semantics)
//
// No hand-written TOML/YAML parser: the grammar lives in bun_parsers (upstream
// port). The JS→TOML direction is serialization (writing), not parsing.
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, JSVal, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use bun_alloc::Arena as Bump;
use bun_ast::expr::Data as ExprData;
use bun_ast::{E, Expr};

/// Per-call parse environment: owned bump arena (freed on drop — unlike
/// Bunfig's `borrowing_default` process-heap borrow, a JS API can be called
/// unboundedly often) + the thread-local Expr store created/reset around the
/// parse+convert window (the load_bunfig pattern from bunfig/arguments.rs).
struct ParseArena {
    bump: Bump,
}

impl ParseArena {
    fn new() -> (Self, bun_ast::StoreResetGuard) {
        // Owned arena first so it drops LAST (Rust drops in reverse order):
        // StoreResetGuard resets the Expr slab while its nodes (which point
        // into the arena for string data) are still addressable, then the
        // arena frees.
        let env = ParseArena { bump: Bump::new() };
        bun_ast::initialize_store();
        let guard = bun_ast::StoreResetGuard::new();
        (env, guard)
    }
}

/// Build an in-memory `bun_ast::Source` (label `<toml>` / `<yaml>` / `<jsonc>`).
fn source_from_text(label: &[u8], text: &str) -> bun_ast::Source {
    let mut source = bun_ast::Source::init_empty_file_interned(label);
    source.contents = ::std::borrow::Cow::Owned(text.as_bytes().to_vec());
    source
}

/// Take the first error message text out of the log (for error surfacing).
fn log_first_error(log: &bun_ast::Log, fallback: &str) -> String {
    if log.errors > 0 {
        format!("{} ({} error(s))", fallback, log.errors)
    } else {
        fallback.to_string()
    }
}

/// Convert a parsed `bun_ast::Expr` value tree into a JS value.
///
/// `bump` must be the same arena the parser allocated from (string data and
/// UTF-16→UTF8 conversions borrow it).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn expr_to_js(
    cx: *mut JSContext,
    expr: &Expr,
    bump: &Bump,
) -> JSVal {
    match &expr.data {
        ExprData::ENull(_) => mozjs::jsval::NullValue(),
        ExprData::EUndefined(_) => UndefinedValue(),
        ExprData::EBoolean(b) => BooleanValue(b.value),
        ExprData::ENumber(n) => DoubleValue(n.value),
        ExprData::EString(s) => {
            let bytes = match (**s).string(bump) {
                Ok(b) => b,
                Err(_) => b"",
            };
            let utf16: Vec<u16> = String::from_utf8_lossy(bytes).encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            if js_str.is_null() {
                return UndefinedValue();
            }
            StringValue(&*js_str)
        }
        ExprData::EBigInt(big) => {
            // Decimal digit string → BigInt via the JS `BigInt()` constructor
            // (the constructor parses decimal strings by spec).
            let digits = String::from_utf8_lossy((**big).value.slice()).into_owned();
            let c_digits = bun_core::ZBox::from_bytes(digits.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_digits.as_ptr());
            if js_str.is_null() {
                return UndefinedValue();
            }
            let global = CurrentGlobalOrNull(cx);
            if global.is_null() {
                return UndefinedValue();
            }
            let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let global_root = global);
            let mut big_ctor = UndefinedValue();
            JS_GetProperty(
                cx,
                global_root.handle().into(),
                c"BigInt".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut big_ctor,
                },
            );
            if !big_ctor.is_object() {
                return UndefinedValue();
            }
            rooted!(&in(cx_ref) let ctor_obj = big_ctor.to_object());
            rooted!(&in(cx_ref) let ctor_val = ObjectValue(ctor_obj.get()));
            let arg = StringValue(&*js_str);
            let call_args = HandleValueArray {
                length_: 1,
                elements_: &arg,
            };
            let mut rval = UndefinedValue();
            let ok = JS_CallFunctionValue(
                cx,
                global_root.handle().into(),
                ctor_val.handle().into(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            if !ok {
                JS_ClearPendingException(cx);
                return UndefinedValue();
            }
            rval
        }
        ExprData::EArray(arr) => {
            let items = &(**arr).items;
            let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let js_arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, items.len()));
            if js_arr.get().is_null() {
                return UndefinedValue();
            }
            for (i, item) in items.iter().enumerate() {
                let v = expr_to_js(cx, item, bump);
                rooted!(&in(cx_ref) let v_root = v);
                let _ = JS_DefineElement(
                    cx,
                    js_arr.handle().into(),
                    i as u32,
                    v_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            ObjectValue(js_arr.get())
        }
        ExprData::EObject(obj) => {
            let props = &(**obj).properties;
            let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let js_obj = JS_NewPlainObject(cx_ref));
            if js_obj.get().is_null() {
                return UndefinedValue();
            }
            for prop in props.iter() {
                if prop.kind == bun_ast::g::PropertyKind::Spread {
                    continue;
                }
                let Some(key_expr) = prop.key.as_ref() else { continue };
                let Some(value_expr) = prop.value.as_ref() else { continue };
                // Object keys render as their JS string form (TOML keys are
                // strings; YAML numbers/bools stringify like JS ToPropertyKey).
                let key = expr_key_to_string(key_expr, bump);
                let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
                let v = expr_to_js(cx, value_expr, bump);
                rooted!(&in(cx_ref) let v_root = v);
                let _ = JS_DefineProperty(
                    cx,
                    js_obj.handle().into(),
                    c_key.as_ptr(),
                    v_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            ObjectValue(js_obj.get())
        }
        // Parser-internal shapes never appear in TOML/YAML value position.
        // Anything unexpected is surfaced explicitly rather than faked.
        other => {
            let kind = match other {
                ExprData::EUnary(_) => "unary",
                ExprData::EBinary(_) => "binary",
                ExprData::ETemplate(_) => "template",
                ExprData::ERegExp(_) => "regexp",
                _ => "unsupported",
            };
            let msg = format!("Bun parser produced an unsupported value node ({})", kind);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            UndefinedValue()
        }
    }
}

/// Render an object-key Expr as its JS string key.
fn expr_key_to_string(key: &Expr, bump: &Bump) -> String {
    match &key.data {
        ExprData::EString(s) => String::from_utf8_lossy((**s).string(bump).unwrap_or(b"")).into_owned(),
        ExprData::ENumber(n) => {
            if n.value == n.value.trunc() && n.value.abs() < 1e21 {
                format!("{}", n.value as i64)
            } else {
                format!("{}", n.value)
            }
        }
        ExprData::EBoolean(b) => if b.value { "true" } else { "false" }.to_string(),
        ExprData::ENull(_) => "null".to_string(),
        _ => String::new(),
    }
}

/// Whether an exception is pending after a conversion that may have reported.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn exception_pending(cx: *mut JSContext) -> bool {
    mozjs_sys::jsapi::JS_IsExceptionPending(cx)
}

// ──────────────────────────────────────────────────────────────────────────
// Bun.TOML
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn toml_parse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.TOML.parse expects a string".as_ptr());
        return false;
    }
    let text = crate::js_to_rust_string(cx, *args.get(0).ptr);

    let (env, _store_guard) = ParseArena::new();
    let mut log = bun_ast::Log::new();
    let source = source_from_text(b"<toml>", &text);
    match bun_parsers::toml::TOML::parse(&source, &mut log, &env.bump, false) {
        Ok(expr) => {
            let v = expr_to_js(cx, &expr, &env.bump);
            if exception_pending(cx) {
                return false;
            }
            args.rval().set(v);
            true
        }
        Err(e) => {
            let msg = log_first_error(&log, "Failed to parse TOML");
            let full = format!("{}: {}", msg, e);
            let c_full = bun_core::ZBox::from_bytes(full.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_full.as_ptr());
            false
        }
    }
}

// ── Bun.TOML.stringify: JS value → TOML text ─────────────────────────────

fn toml_quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml_bare_key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn toml_key(k: &str) -> String {
    if toml_bare_key_ok(k) {
        k.to_string()
    } else {
        toml_quote_string(k)
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn toml_scalar(
    cx: *mut JSContext,
    val: JSVal,
    seen: &mut Vec<usize>,
) -> ::std::result::Result<Option<String>, String> {
    if val.is_string() {
        return Ok(Some(toml_quote_string(&crate::js_to_rust_string(cx, val))));
    }
    if val.is_boolean() {
        return Ok(Some(if val.to_boolean() { "true".into() } else { "false".into() }));
    }
    if val.is_number() {
        let n = if val.is_int32() { val.to_int32() as f64 } else { val.to_double() };
        if n.is_nan() || n.is_infinite() {
            return Err("TOML cannot represent NaN/Infinity".to_string());
        }
        if n == n.trunc() && n.abs() <= 9.007199254740992e15 {
            return Ok(Some(format!("{}", n as i64)));
        }
        let s = format!("{}", n);
        // TOML floats need a fractional part or exponent.
        return Ok(Some(if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{}.0", s)
        }));
    }
    if val.is_bigint() {
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let bv = val);
        let jsstr = mozjs::rust::ToString(cx_ref, bv.handle());
        if !jsstr.is_null() {
            let str_val = mozjs::jsval::StringValue(&*jsstr);
            return Ok(Some(crate::js_to_rust_string(cx, str_val)));
        }
        JS_ClearPendingException(cx);
        return Err("TOML: BigInt conversion failed".to_string());
    }
    if val.is_object() {
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let obj = val.to_object());
        let mut cls = ESClass::Other;
        if mozjs::rust::wrappers2::GetBuiltinClass(cx_ref, obj.handle(), &mut cls) && cls == ESClass::Date {
            let name_z = bun_core::ZBox::from_bytes(b"toISOString");
            let mut rval = UndefinedValue();
            let ok = JS_CallFunctionName(
                cx,
                obj.handle().into(),
                name_z.as_ptr(),
                &HandleValueArray { length_: 0, elements_: ::std::ptr::null() },
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            if ok && rval.is_string() {
                // RFC 3339 (toISOString) is a valid TOML offset date-time.
                return Ok(Some(crate::js_to_rust_string(cx, rval)));
            }
            if !ok {
                JS_ClearPendingException(cx);
            }
            return Ok(Some("1970-01-01T00:00:00.000Z".to_string()));
        }
        let _ = seen;
        return Ok(None); // composite — caller decides table/array handling
    }
    if val.is_null() || val.is_undefined() {
        return Err("TOML cannot represent null/undefined".to_string());
    }
    Err("TOML cannot represent this value type".to_string())
}

/// Own enumerable string keys in insertion order.
#[allow(unsafe_op_in_unsafe_fn, deprecated)]
unsafe fn js_own_keys(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) -> Vec<(String, JSVal)> {
    let mut out = Vec::new();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let mut ids = mozjs::rust::IdVector::new(&mut wrapped_cx);
    if !GetPropertyKeys(cx, obj.into(), JSITER_OWNONLY, ids.handle_mut()) {
        JS_ClearPendingException(cx);
        return out;
    }
    for jsid in &*ids {
        // Array indices arrive as INT jsids in SM — stringify them so
        // array-valued props enumerate their elements ("0", "1", …).
        let key: String = if jsid.is_string() {
            let key_ptr = jsid.to_string();
            if key_ptr.is_null() {
                continue;
            }
            mozjs::conversions::unsafe_jsstr_to_string(
                cx,
                ::std::ptr::NonNull::new_unchecked(key_ptr),
            )
        } else if jsid.is_int() {
            jsid.to_int().to_string()
        } else {
            continue;
        };
        let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
        let mut v = UndefinedValue();
        if !JS_GetProperty(
            cx,
            obj.into(),
            c_key.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        ) {
            JS_ClearPendingException(cx);
            continue;
        }
        out.push((key, v));
    }
    out
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn toml_emit_table(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    prefix: &str,
    out: &mut String,
    seen: &mut Vec<usize>,
) -> ::std::result::Result<(), String> {
    if seen.contains(&(obj.get() as usize)) {
        return Err("circular reference".to_string());
    }
    seen.push(obj.get() as usize);

    let props = js_own_keys(cx, obj);

    // Pass 1 — scalar / scalar-array pairs at this table level.
    for (key, v) in &props {
        if v.is_object() {
            let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped;
            rooted!(&in(cx_ref) let vobj = v.to_object());
            let mut is_arr = false;
            rooted!(&in(cx_ref) let v_root = *v);
            IsArrayObject(cx, v_root.handle().into(), &mut is_arr);
            if !is_arr {
                continue;
            }
            // Array: inline when every element is scalar.
            let elems = js_own_keys(cx, vobj.handle());
            let mut scalar_elems: Vec<String> = Vec::with_capacity(elems.len());
            let mut all_scalar = true;
            for (_, ev) in &elems {
                match toml_scalar(cx, *ev, seen)? {
                    Some(s) => scalar_elems.push(s),
                    None => {
                        all_scalar = false;
                        break;
                    }
                }
            }
            if all_scalar && !elems.is_empty() {
                out.push_str(&format!("{} = [{}]\n", toml_key(key), scalar_elems.join(", ")));
            }
            continue;
        }
        if let Some(s) = toml_scalar(cx, *v, seen)? {
            out.push_str(&format!("{} = {}\n", toml_key(key), s));
        }
    }

    // Pass 2 — nested tables and arrays of tables.
    for (key, v) in &props {
        if !v.is_object() {
            continue;
        }
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let vobj = v.to_object());
        let path = if prefix.is_empty() {
            toml_key(key)
        } else {
            format!("{}.{}", prefix, toml_key(key))
        };

        let mut is_arr = false;
        rooted!(&in(cx_ref) let v_root = *v);
        IsArrayObject(cx, v_root.handle().into(), &mut is_arr);
        if is_arr {
            let elems = js_own_keys(cx, vobj.handle());
            let mut has_object = false;
            for (_, ev) in &elems {
                if ev.is_object() {
                    has_object = true;
                    break;
                }
            }
            if !has_object {
                continue; // handled in pass 1 (or empty)
            }
            // Empty array of tables: `key = []` inline (pass 1 skipped empty).
            if elems.is_empty() {
                out.push_str(&format!("{} = []\n", toml_key(key)));
                continue;
            }
            for (_, ev) in &elems {
                if !ev.is_object() {
                    return Err("mixed scalar/table arrays are not valid TOML".to_string());
                }
                rooted!(&in(cx_ref) let eobj = ev.to_object());
                out.push_str(&format!("\n[[{}]]\n", path));
                toml_emit_table(cx, eobj.handle(), &path, out, seen)?;
            }
            continue;
        }

        let mut cls = ESClass::Other;
        let _ = mozjs::rust::wrappers2::GetBuiltinClass(cx_ref, vobj.handle(), &mut cls);
        // Date-valued object keys were already inlined in pass 1.
        if cls != ESClass::Object && cls != ESClass::Other {
            continue;
        }
        out.push_str(&format!("\n[{}]\n", path));
        toml_emit_table(cx, vobj.handle(), &path, out, seen)?;
    }

    seen.pop();
    Ok(())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn toml_stringify(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_object() {
        JS_ReportErrorUTF8(cx, c"Bun.TOML.stringify expects an object".as_ptr());
        return false;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let root = (*args.get(0).ptr).to_object());
    let mut is_arr = false;
    rooted!(&in(cx_ref) let rv = *args.get(0).ptr);
    IsArrayObject(cx, rv.handle().into(), &mut is_arr);
    if is_arr {
        JS_ReportErrorUTF8(cx, c"Bun.TOML.stringify: top-level value must be a table (object)".as_ptr());
        return false;
    }
    let mut out = String::new();
    match toml_emit_table(cx, root.handle(), "", &mut out, &mut Vec::new()) {
        Ok(()) => {
            let c_out = bun_core::ZBox::from_bytes(out.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_out.as_ptr());
            args.rval().set(if js_str.is_null() {
                UndefinedValue()
            } else {
                StringValue(&*js_str)
            });
            true
        }
        Err(e) => {
            let msg = format!("Bun.TOML.stringify failed: {}", e);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Bun.YAML
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn yaml_parse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.YAML.parse expects a string".as_ptr());
        return false;
    }
    let text = crate::js_to_rust_string(cx, *args.get(0).ptr);

    let (env, _store_guard) = ParseArena::new();
    let mut log = bun_ast::Log::new();
    let source = source_from_text(b"<yaml>", &text);
    match bun_parsers::yaml::YAML::parse(&source, &mut log, &env.bump) {
        Ok(expr) => {
            let v = expr_to_js(cx, &expr, &env.bump);
            if exception_pending(cx) {
                return false;
            }
            args.rval().set(v);
            true
        }
        Err(e) => {
            let msg = log_first_error(&log, "Failed to parse YAML");
            let full = format!("{}: {}", msg, e);
            let c_full = bun_core::ZBox::from_bytes(full.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_full.as_ptr());
            false
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Bun.JSONC — comment + trailing-comma strip, then engine JSON.parse
// ──────────────────────────────────────────────────────────────────────────

/// Strip `//` and `/* */` comments (outside strings) and trailing commas
/// (`,` directly followed by `}` / `]` outside strings). String state tracks
/// `"` with backslash escapes; JSONC strings never use `'`.
fn strip_jsonc(input: &str) -> ::std::result::Result<String, String> {
    #[derive(PartialEq)]
    enum St {
        Code,
        Str,
        LineComment,
        BlockComment,
    }
    let mut out = String::with_capacity(input.len());
    let mut st = St::Code;
    let bytes: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match st {
            St::Str => {
                out.push(c);
                match c {
                    '\\' if i + 1 < bytes.len() => {
                        out.push(bytes[i + 1]);
                        i += 2;
                        continue;
                    }
                    '"' => st = St::Code,
                    _ => {}
                }
            }
            St::LineComment => {
                if c == '\n' {
                    out.push('\n'); // preserve line numbering for error messages
                    st = St::Code;
                }
            }
            St::BlockComment => {
                if c == '*' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
                    st = St::Code;
                    i += 2;
                    continue;
                }
            }
            St::Code => {
                if c == '"' {
                    out.push(c);
                    st = St::Str;
                } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
                    st = St::LineComment;
                    i += 2;
                    continue;
                } else if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
                    st = St::BlockComment;
                    i += 2;
                    continue;
                } else if c == ',' {
                    // Trailing comma: `,` followed only by whitespace before a
                    // closer — drop the comma.
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_whitespace() {
                        j += 1;
                    }
                    if j < bytes.len() && (bytes[j] == '}' || bytes[j] == ']') {
                        // skip the comma (emit the whitespace as-is)
                        for k in (i + 1)..j {
                            out.push(bytes[k]);
                        }
                        i = j;
                        continue;
                    }
                    out.push(c);
                } else {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    if st == St::Str {
        return Err("unterminated string".to_string());
    }
    if st == St::BlockComment {
        return Err("unterminated block comment".to_string());
    }
    Ok(out)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn jsonc_parse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.JSONC.parse expects a string".as_ptr());
        return false;
    }
    let text = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let cleaned = match strip_jsonc(&text) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Bun.JSONC.parse failed: {}", e);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // Delegate to the engine's own JSON.parse (exact JS number/key-order
    // semantics — no reimplementation of the JSON grammar).
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        JS_ReportErrorUTF8(cx, c"Bun.JSONC.parse: no global object".as_ptr());
        return false;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let global_root = global);
    let mut json_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"JSON".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut json_val,
        },
    );
    if !json_val.is_object() {
        JS_ReportErrorUTF8(cx, c"Bun.JSONC.parse: JSON is not available".as_ptr());
        return false;
    }
    rooted!(&in(cx_ref) let json_obj = json_val.to_object());
    let c_cleaned = bun_core::ZBox::from_bytes(cleaned.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_cleaned.as_ptr());
    if js_str.is_null() {
        JS_ReportErrorUTF8(cx, c"Bun.JSONC.parse: string allocation failed".as_ptr());
        return false;
    }
    let arg = StringValue(&*js_str);
    let call_args = HandleValueArray {
        length_: 1,
        elements_: &arg,
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        cx,
        json_obj.handle().into(),
        c"parse".as_ptr(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        // JSON.parse already reported a syntax error against the cleaned
        // text; replace it with the JSONC origin message.
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, c"Bun.JSONC.parse: invalid JSON after comment stripping".as_ptr());
        return false;
    }
    args.rval().set(rval);
    true
}

// ──────────────────────────────────────────────────────────────────────────
// Install
// ──────────────────────────────────────────────────────────────────────────

/// Define a namespace object `{ parse }` / `{ parse, stringify }`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_ns(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
    name: &'static ::std::ffi::CStr,
    parse: unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool,
    stringify: Option<unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool>,
) {
    rooted!(&in(cx) let ns = JS_NewPlainObject(cx));
    if ns.get().is_null() {
        return;
    }
    JS_DefineFunction(
        cx,
        ns.handle(),
        c"parse".as_ptr(),
        Some(parse),
        1,
        JSPROP_ENUMERATE as u32,
    );
    if let Some(sf) = stringify {
        JS_DefineFunction(
            cx,
            ns.handle(),
            c"stringify".as_ptr(),
            Some(sf),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
    JS_DefineProperty3(
        cx,
        bun_obj,
        name.as_ptr(),
        ns.handle(),
        JSPROP_ENUMERATE as u32,
    );
}

/// Install `Bun.TOML`, `Bun.YAML`, `Bun.JSONC` on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    install_ns(cx, bun_obj, c"TOML", toml_parse, Some(toml_stringify));
    install_ns(cx, bun_obj, c"YAML", yaml_parse, None);
    install_ns(cx, bun_obj, c"JSONC", jsonc_parse, None);
    // Phantom uses of module-level imports kept for the walker.
    let _ = E::Boolean { value: false };
}
