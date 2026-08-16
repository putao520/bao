// @trace REQ-ENG-006 [api:Bun.Glob] — glob pattern object.
//
// Upstream face (js/builtins/Glob.ts + runtime/api/glob.zig):
// `new Bun.Glob(pattern)` →
//   * `glob.match(path)`            — bun_glob::r#match (Bun-faithful matcher)
//   * `glob.scanSync({ cwd, dot, onlyFiles, absolute, followSymlinks })`
//                                   — GlobWalker collection → Array
//                                     (arrays are iterable; upstream returns a
//                                     generator over the same values)
//   * `glob.scan({...})`            — same collection wrapped as an async
//                                     iterable (upstream: async generator)
//
// Upstream option defaults (runtime/api/glob.zig ScanOpts.fromJS): dot=false,
// onlyFiles=true, followSymlinks=false, errorOnBrokenSymlinks=false,
// absolute=false. Yielded paths are joined with `cwd` (cwd-rooted).
//
// BCE-20260817-GLOB-STRCWD — scan/scanSync(arg) only read options when arg
// was an object, so the upstream string form (`g.scanSync('/abs/dir')` —
// ScanOpts.fromJS treats a string arg as the `cwd` shorthand, parseCWD
// resolving relative values against the process cwd) was silently ignored
// and the scan ran against the process cwd instead → empty result. Non-
// object/non-string args and a non-string `cwd` now fail closed with the
// upstream messages instead of being dropped.
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::ToBoolean;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use bun_core::ZBox;

#[derive(Default, Clone, Copy)]
struct ScanOpts {
    dot: bool,
    only_files: bool,
    absolute: bool,
    follow_symlinks: bool,
}

impl ScanOpts {
    /// Upstream defaults: only_files=true, everything else false.
    fn upstream_defaults() -> Self {
        ScanOpts { dot: false, only_files: true, absolute: false, follow_symlinks: false }
    }
}

/// Read the pattern stored on the Glob instance (`_pattern` private prop).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_pattern_of(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) -> Option<String> {
    let mut v = UndefinedValue();
    if !JS_GetProperty(
        cx,
        obj.into(),
        c"_pattern".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    ) || !v.is_string()
    {
        JS_ClearPendingException(cx);
        return None;
    }
    Some(crate::js_to_rust_string(cx, v))
}

fn process_cwd() -> String {
    ::std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string())
}

/// Upstream `parseCWD` (runtime/api/glob.zig): "" → process cwd (i.e. cwd
/// left unset), absolute → as-is, relative → resolved against process cwd.
fn resolve_scan_cwd(cwd_val: &str) -> String {
    if cwd_val.is_empty() {
        return process_cwd();
    }
    if cwd_val.starts_with('/') {
        return cwd_val.to_string();
    }
    let base = process_cwd();
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        format!("/{}", cwd_val)
    } else {
        format!("{}/{}", trimmed, cwd_val)
    }
}

/// Read scan options off arg 0 — upstream `ScanOpts.fromJS` semantics:
///   * absent / undefined / null → defaults (process cwd)
///   * string → the string IS the `cwd` (shorthand form; parseCWD-resolved)
///   * object → truthy reads of cwd/dot/onlyFiles/absolute/followSymlinks
///     (a truthy non-boolean reads as false, mirroring upstream getTruthy +
///     `if (v.isBoolean()) v.asBoolean() else false`)
///   * anything else → Err (caller reports; upstream throws)
/// A truthy non-string `cwd` → Err (upstream: "invalid `cwd`, not a string").
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_scan_opts(
    cx: *mut JSContext,
    argc: u32,
    args: &CallArgs,
    fn_name: &str,
) -> ::std::result::Result<(String, ScanOpts), String> {
    let mut cwd = process_cwd();
    let mut opts = ScanOpts::upstream_defaults();
    if argc == 0 {
        return Ok((cwd, opts));
    }
    let arg0 = *args.get(0).ptr;
    if arg0.is_undefined() || arg0.is_null() {
        return Ok((cwd, opts));
    }
    if !arg0.is_object() {
        if arg0.is_string() {
            let s = crate::js_to_rust_string(cx, arg0);
            cwd = resolve_scan_cwd(&s);
            return Ok((cwd, opts));
        }
        return Err(format!("{}: expected first argument to be an object", fn_name));
    }

    let mut wrapped =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let oobj = arg0.to_object());
    let o_h = oobj.handle().into();

    // `cwd` — only a truthy string is accepted (getTruthy + isString check).
    let mut v = UndefinedValue();
    if JS_GetProperty(
        cx,
        o_h,
        c"cwd".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    ) {
        rooted!(&in(cx_ref) let v_root = v);
        if ToBoolean(v_root.handle()) {
            if !v.is_string() {
                return Err(format!("{}: invalid `cwd`, not a string", fn_name));
            }
            let s = crate::js_to_rust_string(cx, v);
            cwd = resolve_scan_cwd(&s);
        }
    } else {
        JS_ClearPendingException(cx);
    }

    let bools: &[(&::std::ffi::CStr, fn(&mut ScanOpts, bool))] = &[
        (c"dot", |o, b| o.dot = b),
        (c"onlyFiles", |o, b| o.only_files = b),
        (c"absolute", |o, b| o.absolute = b),
        (c"followSymlinks", |o, b| o.follow_symlinks = b),
    ];
    for (name, setter) in bools {
        let mut bv = UndefinedValue();
        if JS_GetProperty(
            cx,
            o_h,
            name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut bv,
            },
        ) {
            rooted!(&in(cx_ref) let bv_root = bv);
            if ToBoolean(bv_root.handle()) {
                setter(&mut opts, bv.is_boolean() && bv.to_boolean());
            }
        } else {
            JS_ClearPendingException(cx);
        }
    }
    Ok((cwd, opts))
}

/// Collect matches via the workspace GlobWalker (the same engine fs.glob
/// uses — node_fs.rs glob_collect pattern). Returns cwd-rooted paths.
fn glob_collect_impl(pattern: &str, cwd: &str, opts: ScanOpts) -> Vec<String> {
    type Walker = bun_glob::GlobWalker<bun_glob::walk::SyscallAccessor, false>;
    let absolute = opts.absolute || pattern.starts_with('/');
    let mut walker = match Walker::init_with_cwd(
        pattern.as_bytes(),
        cwd.as_bytes(),
        opts.dot,
        absolute,
        opts.follow_symlinks,
        false, // error_on_broken_symlinks (upstream default)
        opts.only_files,
        None,
    ) {
        Ok(Ok(w)) => w,
        // Malformed pattern (unbalanced brace/class) → no matches.
        _ => return Vec::new(),
    };
    let mut iter = bun_glob::walk::Iterator::new(&mut walker);
    if iter.init().is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    loop {
        match iter.next() {
            Ok(Ok(Some(path))) => out.push(String::from_utf8_lossy(&path).into_owned()),
            _ => break,
        }
    }
    out
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn glob_ctor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if args.argc_ == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.Glob expects a pattern string".as_ptr());
        return false;
    }
    let pattern = crate::js_to_rust_string(cx, *args.get(0).ptr);

    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let g = JS_NewPlainObject(cx_ref));
    if g.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let c_pattern = ZBox::from_bytes(pattern.as_bytes());
    let js_pattern = JS_NewStringCopyZ(cx, c_pattern.as_ptr());
    if !js_pattern.is_null() {
        rooted!(&in(cx_ref) let pv = StringValue(&*js_pattern));
        JS_DefineProperty(
            cx,
            g.handle().into(),
            c"_pattern".as_ptr(),
            pv.handle().into(),
            0,
        );
    }
    // Bun.Glob instances expose the pattern as `source` too (Glob.ts).
    if !js_pattern.is_null() {
        rooted!(&in(cx_ref) let pv = StringValue(&*js_pattern));
        JS_DefineProperty(
            cx,
            g.handle().into(),
            c"source".as_ptr(),
            pv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"match".as_ptr(),
        Some(glob_match),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"scanSync".as_ptr(),
        Some(glob_scan_sync),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"scan".as_ptr(),
        Some(glob_scan),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(ObjectValue(g.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn glob_match(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"Bun.Glob.prototype.match requires a Glob instance".as_ptr());
        return false;
    }
    rooted!(&in(cx_ref) let this_root = this.to_object());
    let Some(pattern) = glob_pattern_of(cx, this_root.handle()) else {
        JS_ReportErrorUTF8(cx, c"Bun.Glob instance has no pattern".as_ptr());
        return false;
    };
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Bun.Glob.match expects a path string".as_ptr());
        return false;
    }
    let path = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let matched = matches!(
        bun_glob::r#match(pattern.as_bytes(), path.as_bytes()),
        bun_glob::MatchResult::Match
    );
    args.rval().set(mozjs::jsval::BooleanValue(matched));
    true
}

/// The canonical scan body both scan entry points call after resolving `this`:
/// collect matches and build the JS array. `Ok(null)` = unusable `this`
/// (caller yields undefined); `Err` = invalid scan argument (caller reports).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_scan_body(
    cx: *mut JSContext,
    this: *mut JSObject,
    argc: u32,
    args: &CallArgs,
    fn_name: &str,
) -> ::std::result::Result<*mut JSObject, String> {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let this_root = this);
    let Some(pattern) = glob_pattern_of(cx, this_root.handle()) else {
        return Ok(::std::ptr::null_mut());
    };
    let (cwd, opts) = glob_scan_opts(cx, argc, args, fn_name)?;
    let matches = glob_collect_impl(&pattern, &cwd, opts);

    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, matches.len()));
    if arr.get().is_null() {
        return Ok(::std::ptr::null_mut());
    }
    for (i, m) in matches.iter().enumerate() {
        let c_m = ZBox::from_bytes(m.as_bytes());
        let js_m = JS_NewStringCopyZ(cx, c_m.as_ptr());
        if !js_m.is_null() {
            rooted!(&in(cx_ref) let mv = StringValue(&*js_m));
            let _ = JS_DefineElement(
                cx,
                arr.handle().into(),
                i as u32,
                mv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    Ok(arr.get())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn glob_scan_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"Bun.Glob.scanSync requires a Glob instance".as_ptr());
        return false;
    }
    match glob_scan_body(cx, this.to_object(), argc, &args, "scanSync") {
        Ok(arr) if !arr.is_null() => args.rval().set(ObjectValue(arr)),
        Ok(_) => args.rval().set(UndefinedValue()),
        Err(msg) => {
            let m = ZBox::from_vec(msg.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), m.as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn glob_scan(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // scan: synchronous collection wrapped as an async iterable (upstream
    // returns an async generator over the same match set).
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"Bun.Glob.scan requires a Glob instance".as_ptr());
        return false;
    }
    let arr = match glob_scan_body(cx, this.to_object(), argc, &args, "scan") {
        Ok(arr) if !arr.is_null() => arr,
        Ok(_) => {
            args.rval().set(UndefinedValue());
            return true;
        }
        Err(msg) => {
            let m = ZBox::from_vec(msg.into_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), m.as_ptr());
            return false;
        }
    };
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    let wrap_src = r#"(function(arr) {
  return {
    [Symbol.asyncIterator]: async function* () { yield* arr; },
    [Symbol.iterator]: function* () { yield* arr; },
  };
})"#;
    let mut text = mozjs::rust::transform_str_to_source_text(wrap_src);
    let opts = mozjs::glue::NewCompileOptions(cx, c"<bun:glob-scan>".as_ptr(), 1);
    if opts.is_null() {
        args.rval().set(ObjectValue(arr));
        return true;
    }
    let mut ctor = UndefinedValue();
    let ctor_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut ctor,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut text, ctor_h);
    libc::free(opts as *mut _);
    if !ok || !ctor.is_object() {
        JS_ClearPendingException(cx);
        args.rval().set(ObjectValue(arr));
        return true;
    }
    rooted!(&in(cx_ref) let ctor_obj = ctor.to_object());
    rooted!(&in(cx_ref) let ctor_val = ObjectValue(ctor_obj.get()));
    rooted!(&in(cx_ref) let arr_val = ObjectValue(arr));
    let call_vals = [arr_val.handle().get()];
    let call_arr = HandleValueArray {
        length_: 1,
        elements_: call_vals.as_ptr(),
    };
    let mut out = UndefinedValue();
    let called = JS_CallFunctionValue(
        cx,
        ctor_obj.handle().into(),
        ctor_val.handle().into(),
        &call_arr,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut out,
        },
    );
    if !called || !out.is_object() {
        if !called {
            JS_ClearPendingException(cx);
        }
        args.rval().set(ObjectValue(arr));
        return true;
    }
    args.rval().set(out);
    true
}

/// Install `Bun.Glob` (constructor) on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    // JSFUN_CONSTRUCTOR (0x400): constructible so `new Bun.Glob(pattern)`
    // dispatches here (node_buffer.rs JSFUN_CONSTRUCTOR pattern — SM's
    // [[Construct]] for natives honours the rval we set).
    const JSFUN_CONSTRUCTOR: u32 = 0x400;
    let ctor = JS_NewFunction(
        cx.raw_cx(),
        Some(glob_ctor),
        1,
        JSFUN_CONSTRUCTOR,
        c"Glob".as_ptr(),
    );
    if ctor.is_null() {
        return;
    }
    let ctor_obj = JS_GetFunctionObject(ctor);
    if ctor_obj.is_null() {
        return;
    }
    rooted!(&in(cx) let ctor_root = ctor_obj);
    JS_DefineProperty3(
        cx,
        bun_obj,
        c"Glob".as_ptr(),
        ctor_root.handle(),
        JSPROP_ENUMERATE as u32,
    );
}
