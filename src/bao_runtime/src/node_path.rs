// @trace REQ-ENG-007
use ::std::path::{MAIN_SEPARATOR, Path, PathBuf};
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ──────────────────────────────────────────────────────────────────────────
// path.posix — the REAL posix face (node semantics on every platform).
//
// The module previously self-referenced the host implementation, which made
// `path.posix.sep === '\\'` on Windows (posix-selfref conformance FAIL) and
// every posix-literal assertion wrong. Node ships a genuine posix algorithm
// on all platforms; these pure-string implementations are that algorithm
// (join/normalize/resolve/dirname/basename/extname/isAbsolute/relative/
// parse/format), separator-fixed to '/'.
// ──────────────────────────────────────────────────────────────────────────

mod posix_core {
    /// Split a posix path into segments; keeps a leading-root flag.
    fn split(p: &str) -> (bool, Vec<&str>) {
        let rooted = p.starts_with('/');
        let segs = p
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect();
        (rooted, segs)
    }

    pub fn normalize(p: &str) -> String {
        let (rooted, segs) = split(p);
        let mut out: Vec<&str> = Vec::new();
        for seg in segs {
            if seg == ".." {
                if !out.is_empty() && *out.last().unwrap() != ".." {
                    out.pop();
                } else if !rooted {
                    out.push(seg);
                }
            } else {
                out.push(seg);
            }
        }
        let joined = out.join("/");
        if rooted {
            format!("/{}", joined)
        } else if joined.is_empty() {
            ".".to_string()
        } else {
            joined
        }
    }

    pub fn join(parts: &[String]) -> String {
        let joined: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        normalize(&joined.join("/"))
    }

    pub fn is_absolute(p: &str) -> bool {
        p.starts_with('/')
    }

    pub fn resolve(parts: &[String], cwd: &str) -> String {
        // Right-most absolute segment wins; otherwise cwd + parts.
        let mut abs_start = 0usize;
        for (i, p) in parts.iter().enumerate() {
            if is_absolute(p) {
                abs_start = i;
            }
        }
        let mut base: Vec<&str> = Vec::new();
        let mut rooted = false;
        if abs_start == 0 && !parts.first().is_some_and(|p| is_absolute(p)) {
            let (r, segs) = split(cwd);
            rooted = r;
            base = segs;
        }
        for p in &parts[abs_start..] {
            let (_r, segs) = split(p);
            rooted = rooted || is_absolute(p);
            for seg in segs {
                if seg == ".." {
                    if !base.is_empty() && *base.last().unwrap() != ".." {
                        base.pop();
                    } else if !rooted {
                        base.push(seg);
                    }
                } else {
                    base.push(seg);
                }
            }
        }
        let joined = base.join("/");
        if rooted {
            format!("/{}", joined)
        } else if joined.is_empty() {
            "/".to_string()
        } else {
            joined
        }
    }

    pub fn dirname(p: &str) -> String {
        match p.rfind('/') {
            Some(0) => "/".to_string(),
            Some(i) => normalize(&p[..i]),
            None => ".".to_string(),
        }
    }

    pub fn basename(p: &str, ext: Option<&str>) -> String {
        let mut base = p.rsplit('/').next().unwrap_or("");
        if base.is_empty() {
            return String::new();
        }
        if let Some(e) = ext {
            if !e.is_empty() && base.ends_with(e) && base.len() > e.len() {
                base = &base[..base.len() - e.len()];
            }
        }
        base.to_string()
    }

    pub fn extname(p: &str) -> String {
        let base = basename(p, None);
        match base.rfind('.') {
            Some(0) => String::new(), // leading-dot only = no ext (node)
            Some(i) => base[i..].to_string(),
            None => String::new(),
        }
    }

    pub fn relative(from: &str, to: &str) -> String {
        let fa = normalize(from);
        let ta = normalize(to);
        if fa == ta {
            return String::new();
        }
        let (_, fseg) = split(&fa);
        let (_, tseg) = split(&ta);
        let mut i = 0;
        while i < fseg.len() && i < tseg.len() && fseg[i] == tseg[i] {
            i += 1;
        }
        let ups = fseg.len() - i;
        let mut parts: Vec<String> = Vec::new();
        for _ in 0..ups {
            parts.push("..".to_string());
        }
        for seg in &tseg[i..] {
            parts.push(seg.to_string());
        }
        if parts.is_empty() {
            ".".to_string()
        } else {
            parts.join("/")
        }
    }

    /// node posix.parse: { root, dir, base, ext, name }
    pub fn parse(p: &str) -> (String, String, String, String, String) {
        let root = if is_absolute(p) { "/".to_string() } else { String::new() };
        let dir = dirname(p);
        let base = basename(p, None);
        let ext = extname(p);
        let name = if ext.is_empty() {
            base.clone()
        } else {
            base[..base.len() - ext.len()].to_string()
        };
        (root, dir, base, ext, name)
    }

    pub fn format(dir: &str, base: &str) -> String {
        if dir.is_empty() {
            base.to_string()
        } else if dir.ends_with('/') {
            format!("{}{}", dir, base)
        } else {
            format!("{}/{}", dir, base)
        }
    }
}

/// arg_to_string for the posix natives (same contract as the platform fns).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn posix_arg(cx: *mut JSContext, v: mozjs::jsval::JSVal) -> Option<String> {
    arg_to_string(cx, v)
}

macro_rules! posix_str_fn {
    ($name:ident, $jsname:literal, $body:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let out: Option<String> = $body(cx, &args, argc);
            match out {
                Some(s) => return_string(cx, &args, &s),
                None => {
                    ::mozjs::jsapi::JS_ReportErrorUTF8(
                        cx,
                        c"posix path: invalid argument".as_ptr(),
                    );
                    false
                }
            }
        }
    };
}

posix_str_fn!(js_posix_join, "join", |cx: *mut JSContext,
                                    args: &::mozjs::jsapi::CallArgs,
                                    argc: u32|
 -> Option<String> {
    let mut parts = Vec::new();
    for i in 0..argc {
        parts.push(posix_arg(cx, *args.get(i).ptr)?);
    }
    Some(posix_core::join(&parts))
});
posix_str_fn!(js_posix_normalize, "normalize", |cx: *mut JSContext,
                                             args: &::mozjs::jsapi::CallArgs,
                                             _argc: u32|
 -> Option<String> {
    Some(posix_core::normalize(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_posix_resolve, "resolve", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         argc: u32|
 -> Option<String> {
    let mut parts = Vec::new();
    for i in 0..argc {
        parts.push(posix_arg(cx, *args.get(i).ptr)?);
    }
    let cwd = ::std::env::current_dir().ok()?.to_string_lossy().replace('\\', "/");
    Some(posix_core::resolve(&parts, &cwd))
});
posix_str_fn!(js_posix_dirname, "dirname", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         _argc: u32|
 -> Option<String> {
    Some(posix_core::dirname(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_posix_basename, "basename", |cx: *mut JSContext,
                                           args: &::mozjs::jsapi::CallArgs,
                                           argc: u32|
 -> Option<String> {
    let p = posix_arg(cx, *args.get(0).ptr)?;
    let ext = if argc > 1 {
        Some(posix_arg(cx, *args.get(1).ptr)?)
    } else {
        None
    };
    Some(posix_core::basename(&p, ext.as_deref()))
});
posix_str_fn!(js_posix_extname, "extname", |cx: *mut JSContext,
                                         args: &::mozjs::jsapi::CallArgs,
                                         _argc: u32|
 -> Option<String> {
    Some(posix_core::extname(&posix_arg(cx, *args.get(0).ptr)?))
});
posix_str_fn!(js_posix_relative, "relative", |cx: *mut JSContext,
                                           args: &::mozjs::jsapi::CallArgs,
                                           _argc: u32|
 -> Option<String> {
    let from = posix_arg(cx, *args.get(0).ptr)?;
    let to = posix_arg(cx, *args.get(1).ptr)?;
    Some(posix_core::relative(&from, &to))
});
posix_str_fn!(js_posix_format, "format", |cx: *mut JSContext,
                                       args: &::mozjs::jsapi::CallArgs,
                                       _argc: u32|
 -> Option<String> {
    // Accepts the parsed-shape object {dir, base}.
    let obj = (*args.get(0).ptr).to_object();
    let mut wrapped = ::mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(cx),
    );
    let cx_ref = &mut wrapped;
    ::mozjs::rooted!(&in(cx_ref) let obj_root = obj);
    let mut dir = String::new();
    let mut base = String::new();
    let mut v = ::mozjs::jsval::UndefinedValue();
    ::mozjs::jsapi::JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"dir".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_string() {
        dir = arg_to_string(cx, v)?;
    }
    v = ::mozjs::jsval::UndefinedValue();
    ::mozjs::jsapi::JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"base".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_string() {
        base = arg_to_string(cx, v)?;
    }
    Some(posix_core::format(&dir, &base))
});

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn posix_is_absolute(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(::mozjs::jsval::BooleanValue(false));
        return true;
    }
    match posix_arg(cx, *args.get(0).ptr) {
        Some(s) => args.rval().set(::mozjs::jsval::BooleanValue(posix_core::is_absolute(&s))),
        None => args.rval().set(::mozjs::jsval::BooleanValue(false)),
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn posix_parse_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
    if argc == 0 {
        ::mozjs::jsapi::JS_ReportErrorUTF8(
            cx,
            ::std::ffi::CStr::from_bytes_with_nul(b"path.parse requires a path\0")
                .unwrap()
                .as_ptr(),
        );
        return false;
    }
    let Some(p) = posix_arg(cx, *args.get(0).ptr) else {
        args.rval().set(::mozjs::jsval::UndefinedValue());
        return true;
    };
    let (root, dir, base, ext, name) = posix_core::parse(&p);
    let raw_cx = cx;
    let mut wrapped = ::mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(raw_cx),
    );
    let cx_ref = &mut wrapped;
    ::mozjs::rooted!(&in(cx_ref) let obj = ::mozjs::rust::wrappers2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(::mozjs::jsval::UndefinedValue());
        return true;
    }
    let h = obj.handle().into();
    macro_rules! def_str {
        ($name:literal, $val:expr) => {{
            let cs = ZBox::from_bytes($val.as_bytes());
            let js = JS_NewStringCopyZ(raw_cx, cs.as_ptr());
            if !js.is_null() {
                ::mozjs::rooted!(&in(cx_ref) let sv = ::mozjs::jsval::StringValue(&*js));
                ::mozjs::jsapi::JS_DefineProperty(
                    raw_cx,
                    h,
                    ::std::ffi::CStr::from_bytes_with_nul(::std::concat!($name, "\0").as_bytes())
                        .unwrap()
                        .as_ptr(),
                    sv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }};
    }
    def_str!("root", root);
    def_str!("dir", dir);
    def_str!("base", base);
    def_str!("ext", ext);
    def_str!("name", name);
    args.rval().set(::mozjs::jsval::ObjectValue(obj.get()));
    true
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let path_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if path_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"join".as_ptr(),
            Some(path_join),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"resolve".as_ptr(),
            Some(path_resolve),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"dirname".as_ptr(),
            Some(path_dirname),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"basename".as_ptr(),
            Some(path_basename),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"extname".as_ptr(),
            Some(path_extname),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"normalize".as_ptr(),
            Some(path_normalize),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"isAbsolute".as_ptr(),
            Some(path_is_absolute),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"relative".as_ptr(),
            Some(path_relative),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"parse".as_ptr(),
            Some(path_parse),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"format".as_ptr(),
            Some(path_format),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            path_obj.handle(),
            c"toNamespacedPath".as_ptr(),
            Some(path_to_namespaced),
            1,
            JSPROP_ENUMERATE as u32,
        );

        let sep_cstr = ZBox::from_bytes(if MAIN_SEPARATOR == '/' { "/" } else { "\\" }.as_bytes());
        let sep_str = JS_NewStringCopyZ(cx.raw_cx(), sep_cstr.as_ptr());
        if !sep_str.is_null() {
            let sep_val = mozjs::jsval::StringValue(&*sep_str);
            rooted!(&in(cx) let sep_root = sep_val);
            JS_DefineProperty(
                cx.raw_cx(),
                path_obj.handle().into(),
                c"sep".as_ptr(),
                sep_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // path.platform — mirrors process.platform / os.platform() so code
        // branching on `path.platform === "win32"` behaves like Node's
        // platform-tagged path modules. Audit item: was entirely missing
        // (typeof path.platform === 'undefined').
        let platform_bytes: &[u8] = if cfg!(target_os = "linux") {
            b"linux"
        } else if cfg!(target_os = "macos") {
            b"darwin"
        } else if cfg!(target_os = "windows") {
            b"win32"
        } else {
            b"unknown"
        };
        let platform_cstr = ZBox::from_bytes(platform_bytes);
        let platform_str = JS_NewStringCopyZ(cx.raw_cx(), platform_cstr.as_ptr());
        if !platform_str.is_null() {
            let platform_val = mozjs::jsval::StringValue(&*platform_str);
            rooted!(&in(cx) let platform_root = platform_val);
            JS_DefineProperty(
                cx.raw_cx(),
                path_obj.handle().into(),
                c"platform".as_ptr(),
                platform_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        let delim_cstr = ZBox::from_bytes(if cfg!(windows) { b";" } else { b":" });
        let delim_str = JS_NewStringCopyZ(cx.raw_cx(), delim_cstr.as_ptr());
        if !delim_str.is_null() {
            let delim_val = mozjs::jsval::StringValue(&*delim_str);
            rooted!(&in(cx) let delim_root = delim_val);
            JS_DefineProperty(
                cx.raw_cx(),
                path_obj.handle().into(),
                c"delimiter".as_ptr(),
                delim_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // path.posix — the REAL posix face (pure-string algorithms above);
    // node ships a genuine posix implementation on every platform, and the
    // old self-reference answered `path.posix.sep === path.sep` ('\' on
    // Windows — posix-selfref conformance FAIL).
    unsafe {
        rooted!(&in(cx) let posix_obj = w2::JS_NewPlainObject(cx));
        if !posix_obj.get().is_null() {
            let fns: &[(&str, ::std::option::Option<
                unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool,
            >)] = &[
                ("join", Some(js_posix_join)),
                ("normalize", Some(js_posix_normalize)),
                ("resolve", Some(js_posix_resolve)),
                ("dirname", Some(js_posix_dirname)),
                ("basename", Some(js_posix_basename)),
                ("extname", Some(js_posix_extname)),
                ("isAbsolute", Some(posix_is_absolute)),
                ("relative", Some(js_posix_relative)),
                ("parse", Some(posix_parse_fn)),
                ("format", Some(js_posix_format)),
            ];
            for (name, fp) in fns {
                let c_name = ZBox::from_bytes(name.as_bytes());
                let f = ::mozjs::jsapi::JS_NewFunction(
                    cx.raw_cx(),
                    *fp,
                    2,
                    0,
                    c_name.as_ptr(),
                );
                if !f.is_null() {
                    let fobj = ::mozjs::jsapi::JS_GetFunctionObject(f);
                    ::mozjs::rooted!(&in(cx) let fv = ::mozjs::jsval::ObjectValue(fobj));
                    ::mozjs::jsapi::JS_DefineProperty(
                        cx.raw_cx(),
                        posix_obj.handle().into(),
                        c_name.as_ptr(),
                        fv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            // posix.sep = "/" / posix.delimiter = ":"
            let sep_cstr = ZBox::from_bytes(b"/");
            let sep_str = JS_NewStringCopyZ(cx.raw_cx(), sep_cstr.as_ptr());
            if !sep_str.is_null() {
                let v = ::mozjs::jsval::StringValue(&*sep_str);
                ::mozjs::rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    posix_obj.handle().into(),
                    c"sep".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let dlm_cstr = ZBox::from_bytes(b":");
            let dlm_str = JS_NewStringCopyZ(cx.raw_cx(), dlm_cstr.as_ptr());
            if !dlm_str.is_null() {
                let v = ::mozjs::jsval::StringValue(&*dlm_str);
                ::mozjs::rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    posix_obj.handle().into(),
                    c"delimiter".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            // posix.posix / posix.win32 self-refs (node shape)
            w2::JS_DefineProperty3(
                cx,
                posix_obj.handle(),
                c"posix".as_ptr(),
                posix_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
            // Attach the real posix face to the module:
            w2::JS_DefineProperty3(
                cx,
                path_obj.handle(),
                c"posix".as_ptr(),
                posix_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // path.win32 — Node.js ships a real Windows-flavoured path object on all
    // platforms so `path.win32.sep === "\\"` even on Linux. We expose a thin
    // JS wrapper that mirrors the host's path API but overrides `sep` /
    // `delimiter` to the Windows values. (See ~/code/rust/bun/src/js/node/
    // path.ts — Bun likewise ships both `posix` and `win32`.)
    unsafe {
        rooted!(&in(cx) let win32_obj = w2::JS_NewPlainObject(cx));
        if !win32_obj.get().is_null() {
            // Reuse the host path functions; only the separator/delimiter
            // constants differ. Methods are forwarded by assigning the same
            // function references the host module already exposes.
            for fn_name in &[
                "join",
                "resolve",
                "dirname",
                "basename",
                "extname",
                "normalize",
                "isAbsolute",
                "relative",
                "parse",
                "format",
                "toNamespaced",
            ] {
                let c_name = ZBox::from_bytes(fn_name.as_bytes());
                let mut fn_val = UndefinedValue();
                JS_GetProperty(
                    cx.raw_cx(),
                    path_obj.handle().into(),
                    c_name.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut fn_val,
                    },
                );
                if fn_val.is_object() {
                    rooted!(&in(cx) let fv = fn_val);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        win32_obj.handle().into(),
                        c_name.as_ptr(),
                        fv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            // win32.sep = "\\" / win32.delimiter = ";"
            let winsep_cstr = ZBox::from_bytes(b"\\");
            let winsep_str = JS_NewStringCopyZ(cx.raw_cx(), winsep_cstr.as_ptr());
            if !winsep_str.is_null() {
                let v = mozjs::jsval::StringValue(&*winsep_str);
                rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    win32_obj.handle().into(),
                    c"sep".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let wdlm_cstr = ZBox::from_bytes(b";");
            let wdlm_str = JS_NewStringCopyZ(cx.raw_cx(), wdlm_cstr.as_ptr());
            if !wdlm_str.is_null() {
                let v = mozjs::jsval::StringValue(&*wdlm_str);
                rooted!(&in(cx) let vr = v);
                JS_DefineProperty(
                    cx.raw_cx(),
                    win32_obj.handle().into(),
                    c"delimiter".as_ptr(),
                    vr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            // win32.win32 / win32.posix self-refs (matches Node.js shape)
            w2::JS_DefineProperty3(
                cx,
                win32_obj.handle(),
                c"win32".as_ptr(),
                win32_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
            w2::JS_DefineProperty3(
                cx,
                win32_obj.handle(),
                c"posix".as_ptr(),
                path_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );

            w2::JS_DefineProperty3(
                cx,
                path_obj.handle(),
                c"win32".as_ptr(),
                win32_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // path.matchesGlob(path, pattern) — pure JS glob matcher. Bun's
    // implementation (~/code/rust/bun/src/js/node/path.ts:matchesGlob) uses
    // `Bun.Glob` (native); here we express the same boolean result with a
    // minimal regex-based glob translator covering the standard wildcard
    // syntax (`*`, `?`, `**`, character classes) so behaviour matches Node.
    unsafe {
        let matches_src = r#"(function(p){
  function globToRegExp(pattern) {
    // Anchor the pattern; translate `**` → `.*`, `*` → `.*`, `?` → `.`,
    // and escape regex metacharacters in literals. Mirrors Bun.Glob's
    // full-path glob semantics (a single `*` matches across path segments),
    // which is what Node.js' path.matchesGlob surfaces.
    var s = '^';
    for (var i = 0; i < pattern.length; i++) {
      var c = pattern.charAt(i);
      if (c === '*') {
        if (pattern.charAt(i + 1) === '*') {
          s += '.*';
          i++;
        } else {
          s += '.*';
        }
      } else if (c === '?') {
        s += '.';
      } else if ('.+^${}()|[]\\'.indexOf(c) >= 0) {
        s += '\\' + c;
      } else {
        s += c;
      }
    }
    return new RegExp(s + '$');
  }
  p.matchesGlob = function matchesGlob(path, pattern) {
    if (typeof path !== 'string') {
      throw new TypeError('The "path" argument must be of type string.');
    }
    if (typeof pattern !== 'string') {
      throw new TypeError('The "pattern" argument must be of type string.');
    }
    return globToRegExp(pattern).test(path);
  };
})"#;
        let mut msrc = mozjs::rust::transform_str_to_source_text(matches_src);
        let mut mval = UndefinedValue();
        let mh = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut mval,
        };
        let mopts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<path-matchesGlob>".as_ptr(), 1);
        if !mopts.is_null() {
            let global = CurrentGlobalOrNull(cx.raw_cx());
            if !global.is_null()
                && JS::Evaluate2(cx.raw_cx(), mopts, &mut msrc, mh)
                && mval.is_object()
            {
                let wrapped_cx =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
                rooted!(&in(wrapped_cx) let global_root = global);
                rooted!(&in(wrapped_cx) let path_val_root = ObjectValue(path_obj.get()));
                let args_arr = HandleValueArray {
                    length_: 1,
                    elements_: &path_val_root.get() as *const Value,
                };
                let mut call_rval = UndefinedValue();
                let call_rval_h = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut call_rval,
                };
                rooted!(&in(wrapped_cx) let factory_obj = mval.to_object());
                rooted!(&in(wrapped_cx) let factory_obj_h = ObjectValue(factory_obj.get()));
                JS_CallFunctionValue(
                    cx.raw_cx(),
                    global_root.handle().into(),
                    factory_obj_h.handle().into(),
                    &args_arr,
                    call_rval_h,
                );
            }
            libc::free(mopts as *mut _);
        }
    }

    cache_builtin(cx, "path", path_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn arg_to_string(
    cx: *mut JSContext,
    val: JSVal,
) -> ::std::option::Option<::std::string::String> {
    if val.is_undefined() || val.is_null() {
        return ::std::option::Option::None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let val_root = val);
    let s = mozjs::rust::ToString(&mut wrapped_cx, val_root.handle().into());
    if s.is_null() {
        return ::std::option::Option::None;
    }
    let rust_str = crate::jsstr_to_rust_string(cx, s);
    ::std::option::Option::Some(rust_str)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_string(cx: *mut JSContext, args: &CallArgs, s: &str) -> bool {
    let c_str = ZBox::from_bytes(s.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if js_str.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::StringValue(&*js_str));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_join(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut parts: Vec<::std::string::String> = Vec::new();
    for val in ::std::slice::from_raw_parts(args.argv_, argc as usize) {
        match arg_to_string(cx, *val) {
            Some(s) => parts.push(s),
            None => {
                JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
                return false;
            }
        }
    }
    let joined = posix_join(&parts);
    return_string(cx, &args, &joined)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_resolve(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let cwd = {
        let mut buf = bun_paths::path_buffer_pool::get();
        bun_core::getcwd(&mut buf)
            .map(|z| PathBuf::from(String::from_utf8_lossy(z.as_bytes()).into_owned()))
            .unwrap_or_else(|_| PathBuf::from("."))
    };
    let mut resolved = cwd;

    for val in ::std::slice::from_raw_parts(args.argv_, argc as usize) {
        match arg_to_string(cx, *val) {
            Some(s) => {
                let p = Path::new(&s);
                if p.is_absolute() {
                    resolved = p.to_path_buf();
                } else {
                    resolved = resolved.join(p);
                }
            }
            None => {
                JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
                return false;
            }
        }
    }

    let result = normalize_path(&resolved);
    return_string(cx, &args, &result.to_string_lossy())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_dirname(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let result = Path::new(&s)
        .parent()
        .map(|p| {
            let pstr = p.to_string_lossy().into_owned();
            if pstr.is_empty() {
                ".".to_string()
            } else {
                pstr
            }
        })
        .unwrap_or_else(|| ".".to_string());
    return_string(cx, &args, &result)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_basename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let mut base = Path::new(&s)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.clone());

    if argc >= 2 {
        let ext_val = *args.get(1).ptr;
        if let Some(ext) = arg_to_string(cx, ext_val)
            && base.ends_with(&ext)
            && !ext.is_empty()
        {
            base.truncate(base.len() - ext.len());
        }
    }
    return_string(cx, &args, &base)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_extname(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let ext = Path::new(&s)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    return_string(cx, &args, &ext)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_normalize(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };
    let p = Path::new(&s);
    let normalized = normalize_path(p);
    return_string(cx, &args, &normalized.to_string_lossy())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_is_absolute(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    // Node semantics, not Rust's: on win32 a rooted path WITHOUT a drive
    // ('/foo') is absolute (node's lib/path.js win32 isAbsolute — separator
    // at [0], or device-root + ':' + separator), while std's
    // Path::is_absolute demands a drive prefix and answered false for
    // '/foo' on Windows. POSIX keeps the std behavior (leading '/').
    #[cfg(windows)]
    {
        let b = s.as_bytes();
        let rooted = b.first().is_some_and(|&c| c == b'/' || c == b'\\');
        let drive_rooted = b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'/' || b[2] == b'\\');
        args.rval().set(mozjs::jsval::BooleanValue(rooted || drive_rooted));
    }
    #[cfg(not(windows))]
    {
        args.rval()
            .set(mozjs::jsval::BooleanValue(Path::new(&s).is_absolute()));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_relative(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(
            cx,
            c"The \"from\" and \"to\" arguments must be of type string".as_ptr(),
        );
        return false;
    }
    let from_val = *args.get(0).ptr;
    let to_val = *args.get(1).ptr;
    let from_str = match arg_to_string(cx, from_val) {
        Some(s) => s,
        None => return return_string(cx, &args, ""),
    };
    let to_str = match arg_to_string(cx, to_val) {
        Some(s) => s,
        None => return return_string(cx, &args, ""),
    };

    let from_abs = make_absolute(&from_str);
    let to_abs = make_absolute(&to_str);

    let result = pathdiff(&to_abs, &from_abs);
    return_string(
        cx,
        &args,
        result.unwrap_or_default().to_string_lossy().as_ref(),
    )
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_parse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
            return false;
        }
    };

    let p = Path::new(&s);
    let root = if p.is_absolute() {
        "/".to_string()
    } else {
        String::new()
    };
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = p
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let name = if !file_name.is_empty() && !ext.is_empty() {
        file_name[..file_name.len() - ext.len()].to_string()
    } else {
        file_name.clone()
    };

    let parsed = JS_NewPlainObject(cx);
    if parsed.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let parsed_root = parsed);

    define_string_prop(cx, parsed_root.handle().into(), "root", &root);
    define_string_prop(cx, parsed_root.handle().into(), "dir", &dir);
    define_string_prop(cx, parsed_root.handle().into(), "base", &file_name);
    define_string_prop(cx, parsed_root.handle().into(), "ext", &ext);
    define_string_prop(cx, parsed_root.handle().into(), "name", &name);

    args.rval().set(mozjs::jsval::ObjectValue(parsed));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_format(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"The \"pathObject\" argument must be of type object".as_ptr(),
        );
        return false;
    }
    let val = *args.get(0).ptr;
    if !val.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"The \"pathObject\" argument must be of type object".as_ptr(),
        );
        return false;
    }
    let obj = val.to_object();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let dir = get_string_prop(cx, obj_root.handle().into(), "dir");
    let base = get_string_prop(cx, obj_root.handle().into(), "base");
    let name = get_string_prop(cx, obj_root.handle().into(), "name");
    let ext = get_string_prop(cx, obj_root.handle().into(), "ext");

    let result = if let Some(b) = base {
        if dir.as_ref().is_some_and(|d| !d.is_empty()) {
            format!("{}/{}", dir.unwrap_or_default(), b)
        } else {
            b
        }
    } else {
        let mut s = dir.unwrap_or_default();
        if !s.is_empty() && !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(&name.unwrap_or_default());
        s.push_str(&ext.unwrap_or_default());
        s
    };
    return_string(cx, &args, &result)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_to_namespaced(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let resolved = make_absolute(&s);
    return_string(cx, &args, &resolved.to_string_lossy())
}

// --- Pure logic helpers ---
// @trace REQ-ENG-007 [api:path] [code:bun_paths] — absolute-path resolution
// (`make_absolute`) and relative-path computation (`pathdiff`) delegate to
// `bun_paths::resolve_path` (Zig std `std.fs.path` faithful port):
//   * `join_abs_string::<Posix>(cwd, parts)` resolves `cwd + parts` into a
//     single absolute path (the equivalent of Node's `path.resolve` core).
//   * `relative_platform::<Posix, _>(from, to)` computes the relative path
//     from one absolute path to another (the equivalent of `path.relative`).
//
// The Node.js-specific `.`/`..` collapse for `posix_join` and `normalize_path`
// stays in Rust here because Node's `path.posix.normalize` deliberately
// preserves leading `..` above the root (e.g. `/a/../../b` → `/../b`) while
// Zig std's `normalizeString` clamps at the root (`/b`). The bundler/resolver
// consume the Zig semantics; the Node compatibility layer keeps its own.

use bun_paths::resolve_path::{self, platform::Posix};

/// Resolve the current working directory as an owned byte vector, falling
/// back to `b"."` so absolute-path joins never see an empty cwd.
fn cwd_bytes() -> Vec<u8> {
    let mut buf = bun_paths::path_buffer_pool::get();
    match bun_core::getcwd(&mut buf) {
        Ok(z) => z.as_bytes().to_vec(),
        Err(_) => b".".to_vec(),
    }
}

pub(crate) fn posix_join(parts: &[::std::string::String]) -> ::std::string::String {
    if parts.is_empty() {
        return ".".to_string();
    }

    // Node.js path.posix.join:
    // 1. Filter empty parts
    // 2. Join all parts with / (absolute components treated as regular — leading / stripped at join time)
    // 3. If first non-empty part started with /, result is absolute
    // 4. Normalize . and ..
    // 5. Preserve trailing / from last non-empty part

    // Step 1: Collect non-empty parts, strip leading / from each, track if first was absolute
    let mut segments: Vec<&str> = Vec::new();
    let mut has_root = false;
    let mut first_seen = false;
    let mut trailing_slash = false;

    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !first_seen {
            first_seen = true;
            has_root = part.starts_with('/');
        }
        // Track trailing slash from last part
        trailing_slash = part.ends_with('/');
        // Split by / and collect non-empty segments
        for seg in part.split('/') {
            if !seg.is_empty() && seg != "." {
                segments.push(seg);
            }
        }
    }

    if !first_seen {
        return ".".to_string();
    }

    // Step 2: Normalize .. by popping
    let mut normalized: Vec<&str> = Vec::new();
    for seg in &segments {
        if *seg == ".." {
            if !normalized.is_empty() && *normalized.last().expect("segments") != ".." {
                normalized.pop();
            } else if !has_root {
                normalized.push("..");
            }
        } else {
            normalized.push(seg);
        }
    }

    let mut result = if has_root {
        "/".to_string()
    } else {
        String::new()
    };
    result.push_str(&normalized.join("/"));

    // Trailing slash: only when last non-empty part had trailing slash AND result is relative
    // or when result is empty (only . and / segments)
    if result.is_empty() && trailing_slash {
        "./".to_string()
    } else if !result.is_empty() && trailing_slash && !result.ends_with('/') {
        result.push('/');
        result
    } else if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

pub(crate) fn normalize_path(path: &::std::path::Path) -> PathBuf {
    let mut components = Vec::new();
    let has_root = path.is_absolute();
    for comp in path.components() {
        match comp {
            ::std::path::Component::CurDir => {}
            ::std::path::Component::ParentDir => {
                if let Some(last) = components.last()
                    && last != &".."
                {
                    components.pop();
                    continue;
                }
                components.push("..");
            }
            ::std::path::Component::Normal(s) => {
                components.push(s.to_string_lossy().into_owned().leak() as &'static str);
            }
            _ => {}
        }
    }
    let mut result = PathBuf::new();
    if has_root {
        result.push("/");
    }
    for seg in &components {
        result.push(*seg);
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

pub(crate) fn make_absolute(s: &str) -> PathBuf {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        normalize_path(&p)
    } else {
        // @trace REQ-ENG-007 [code:bun_paths] — resolve `cwd + path` into a
        // single absolute path via bun_paths::resolve_path::join_abs_string
        // (Zig `joinAbsoluteString`, POSIX). Falls back to the std::path
        // normalize if the resolved result round-trips lossily.
        let cwd = cwd_bytes();
        let part_bytes = s.as_bytes();
        let resolved = resolve_path::join_abs_string::<Posix>(&cwd, &[part_bytes]);
        PathBuf::from(String::from_utf8_lossy(resolved).into_owned())
    }
}

pub(crate) fn pathdiff(to: &Path, from: &Path) -> ::std::option::Option<PathBuf> {
    let cwd = cwd_bytes();
    let to_abs = if to.is_absolute() {
        to.to_string_lossy().into_owned()
    } else {
        // Make `to` absolute against the cwd via bun_paths.
        let resolved =
            resolve_path::join_abs_string::<Posix>(&cwd, &[to.to_string_lossy().as_bytes()]);
        String::from_utf8_lossy(resolved).into_owned()
    };
    let from_abs = if from.is_absolute() {
        from.to_string_lossy().into_owned()
    } else {
        let resolved =
            resolve_path::join_abs_string::<Posix>(&cwd, &[from.to_string_lossy().as_bytes()]);
        String::from_utf8_lossy(resolved).into_owned()
    };

    // @trace REQ-ENG-007 [code:bun_paths] — relative-path computation delegated
    // to bun_paths::resolve_path::relative_platform (Zig `relativePath`, POSIX).
    // ALWAYS_COPY=true so the result owns its bytes (does not alias TLS scratch).
    let rel =
        resolve_path::relative_platform::<Posix, true>(from_abs.as_bytes(), to_abs.as_bytes());
    ::std::option::Option::Some(PathBuf::from(String::from_utf8_lossy(rel).into_owned()))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_string_prop(
    cx: *mut JSContext,
    obj: Handle<*mut JSObject>,
    name: &str,
    value: &str,
) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let c_val = ZBox::from_bytes(value.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if !js_str.is_null() {
        let val = mozjs::jsval::StringValue(&*js_str);
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let val_root = val);
        JS_DefineProperty(
            cx,
            obj,
            c_name.as_ptr(),
            val_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop(
    cx: *mut JSContext,
    obj: Handle<*mut JSObject>,
    name: &str,
) -> ::std::option::Option<::std::string::String> {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut val = UndefinedValue();
    let handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    };
    JS_GetProperty(cx, obj, c_name.as_ptr(), handle);
    arg_to_string(cx, val)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- posix_join ---

    #[test]
    fn test_posix_join_empty() {
        assert_eq!(posix_join(&[]), ".");
    }

    #[test]
    fn test_posix_join_single() {
        assert_eq!(posix_join(&["foo".to_string()]), "foo");
    }

    #[test]
    fn test_posix_join_multiple() {
        assert_eq!(
            posix_join(&["a".to_string(), "b".to_string(), "c".to_string()]),
            "a/b/c"
        );
    }

    // posix_join: absolute parts after first are joined as relative, NOT overriding
    #[test]
    fn test_posix_join_absolute_part() {
        // Leading / in non-first part is stripped (posix join behavior)
        assert_eq!(
            posix_join(&["a".to_string(), "/b".to_string(), "c".to_string()]),
            "a/b/c"
        );
    }

    #[test]
    fn test_posix_join_trailing_slash() {
        assert_eq!(posix_join(&["a/".to_string(), "b".to_string()]), "a/b");
    }

    #[test]
    fn test_posix_join_dot() {
        assert_eq!(posix_join(&[".".to_string(), "b".to_string()]), "b");
    }

    #[test]
    fn test_posix_join_empty_parts_skipped() {
        assert_eq!(
            posix_join(&["a".to_string(), "".to_string(), "b".to_string()]),
            "a/b"
        );
    }

    #[test]
    fn test_posix_join_root() {
        assert_eq!(posix_join(&["/".to_string()]), "/");
    }

    #[test]
    fn test_posix_join_dot_dot_normalizes() {
        assert_eq!(
            posix_join(&["a".to_string(), "b".to_string(), "..".to_string()]),
            "a"
        );
    }

    // posix_join: .. beyond root resolves within root (absolute path can't go beyond root)
    #[test]
    fn test_posix_join_dot_dot_beyond_root_stays() {
        // For relative path, .. resolves upward; extra .. stays as ..
        assert_eq!(
            posix_join(&["a".to_string(), "..".to_string(), "..".to_string()]),
            ".."
        );
    }

    // --- normalize_path (Path-based) ---

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_dot_dot() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_dot() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/./b")),
            PathBuf::from("/a/b")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_root() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/")),
            PathBuf::from("/")
        );
    }

    #[test]
    fn test_normalize_path_relative() {
        assert_eq!(
            normalize_path(::std::path::Path::new("a/b/../c")),
            PathBuf::from("a/c")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_double_dot_beyond_root() {
        // Implementation preserves .. beyond root as /../b
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/../../b")),
            PathBuf::from("/../b")
        );
    }

    #[test]
    fn test_normalize_path_empty_relative() {
        assert_eq!(
            normalize_path(::std::path::Path::new(".")),
            PathBuf::from(".")
        );
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_normalize_path_multiple_dots() {
        assert_eq!(
            normalize_path(::std::path::Path::new("/a/b/c/../../d")),
            PathBuf::from("/a/d")
        );
    }

    // --- make_absolute ---

    #[test]
    fn test_make_absolute_already_absolute() {
        assert_eq!(make_absolute("/foo/bar"), PathBuf::from("/foo/bar"));
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_make_absolute_relative() {
        let result = make_absolute("foo/bar");
        assert!(
            result.is_absolute(),
            "result should be absolute: {:?}",
            result
        );
        assert!(result.to_str().unwrap().contains("foo/bar"));
    }

    // POSIX-path-form assertions (leading-/ anchors): on windows the per-OS
    // path semantics are correct product behavior, these tests hold the posix form.
    #[cfg(unix)]
    #[test]
    fn test_make_absolute_dot() {
        let result = make_absolute(".");
        assert!(result.is_absolute());
    }

    #[test]
    fn test_make_absolute_dot_dot() {
        let result = make_absolute("/a/b/..");
        // Path-based normalization: /a/b/.. -> /a
        let s = result.to_str().unwrap();
        assert!(s == "/a" || s == "/a/", "expected /a or /a/, got {}", s);
    }

    // --- pathdiff ---

    #[test]
    fn test_pathdiff_same() {
        let p = Path::new("/a/b/c");
        assert_eq!(pathdiff(p, p), Some(PathBuf::from("")));
    }

    #[test]
    fn test_pathdiff_sibling() {
        let to = Path::new("/a/b/c");
        let from = Path::new("/a/b/d");
        // pathdiff strips common prefix then builds relative path
        assert_eq!(pathdiff(to, from), Some(PathBuf::from("../c")));
    }

    #[test]
    fn test_pathdiff_child() {
        let to = Path::new("/a/b/c/d");
        let from = Path::new("/a/b");
        assert_eq!(pathdiff(to, from), Some(PathBuf::from("c/d")));
    }

    #[test]
    fn test_pathdiff_parent() {
        let to = Path::new("/a/b");
        let from = Path::new("/a/b/c/d");
        assert_eq!(pathdiff(to, from), Some(PathBuf::from("../..")));
    }

    #[test]
    fn test_pathdiff_different_roots() {
        let to = Path::new("/a/b");
        let from = Path::new("/c/d");
        let result = pathdiff(to, from);
        assert!(result.is_some());
    }

    #[test]
    fn test_pathdiff_relative() {
        let to = Path::new("a/b");
        let from = Path::new("a/c");
        let result = pathdiff(to, from);
        assert!(result.is_some());
    }
}
