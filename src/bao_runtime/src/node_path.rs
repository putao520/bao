// @trace REQ-ENG-007
use bun_core::ZBox;
use ::std::path::{Path, PathBuf, MAIN_SEPARATOR};
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let path_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if path_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, path_obj.handle(), c"join".as_ptr(), Some(path_join), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"resolve".as_ptr(), Some(path_resolve), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"dirname".as_ptr(), Some(path_dirname), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"basename".as_ptr(), Some(path_basename), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"extname".as_ptr(), Some(path_extname), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"normalize".as_ptr(), Some(path_normalize), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"isAbsolute".as_ptr(), Some(path_is_absolute), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"relative".as_ptr(), Some(path_relative), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"parse".as_ptr(), Some(path_parse), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"format".as_ptr(), Some(path_format), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, path_obj.handle(), c"toNamespacedPath".as_ptr(), Some(path_to_namespaced), 1, JSPROP_ENUMERATE as u32);

        let sep_cstr = ZBox::from_bytes(if MAIN_SEPARATOR == '/' { "/" } else { "\\" }.as_bytes());
        let sep_str = JS_NewStringCopyZ(cx.raw_cx(), sep_cstr.as_ptr());
        if !sep_str.is_null() {
            let sep_val = mozjs::jsval::StringValue(&*sep_str);
            rooted!(&in(cx) let sep_root = sep_val);
            JS_DefineProperty(cx.raw_cx(), path_obj.handle().into(), c"sep".as_ptr(), sep_root.handle().into(), JSPROP_ENUMERATE as u32);
        }

        let delim_cstr = ZBox::from_bytes(if cfg!(windows) { b";" } else { b":" });
        let delim_str = JS_NewStringCopyZ(cx.raw_cx(), delim_cstr.as_ptr());
        if !delim_str.is_null() {
            let delim_val = mozjs::jsval::StringValue(&*delim_str);
            rooted!(&in(cx) let delim_root = delim_val);
            JS_DefineProperty(cx.raw_cx(), path_obj.handle().into(), c"delimiter".as_ptr(), delim_root.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // path.posix / path.win32 — self-references to the path module
    unsafe {
        w2::JS_DefineProperty3(cx, path_obj.handle(), c"posix".as_ptr(), path_obj.handle(), JSPROP_ENUMERATE as u32);
        w2::JS_DefineProperty3(cx, path_obj.handle(), c"win32".as_ptr(), path_obj.handle(), JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "path", path_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn arg_to_string(cx: *mut JSContext, val: JSVal) -> ::std::option::Option<::std::string::String> {
    if val.is_undefined() || val.is_null() {
        return ::std::option::Option::None;
    }
    let raw_handle = mozjs::rust::HandleValue::from_marked_location(&val);
    let s = mozjs::rust::ToString(cx, raw_handle);
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
        let mut buf = bun_core::PathBuffer::default();
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
    let result = Path::new(&s).parent()
        .map(|p| {
            let pstr = p.to_string_lossy().into_owned();
            if pstr.is_empty() { ".".to_string() } else { pstr }
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
    let mut base = Path::new(&s).file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.clone());

    if argc >= 2 {
        let ext_val = *args.get(1).ptr;
        if let Some(ext) = arg_to_string(cx, ext_val)
            && base.ends_with(&ext) && !ext.is_empty() {
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
    let ext = Path::new(&s).extension()
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
    args.rval().set(mozjs::jsval::BooleanValue(Path::new(&s).is_absolute()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_relative(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, c"The \"from\" and \"to\" arguments must be of type string".as_ptr());
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
    return_string(cx, &args, result.unwrap_or_default().to_string_lossy().as_ref())
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
    let root = if p.is_absolute() { "/".to_string() } else { String::new() };
    let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let file_name = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
        JS_ReportErrorUTF8(cx, c"The \"pathObject\" argument must be of type object".as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    if !val.is_object() {
        JS_ReportErrorUTF8(cx, c"The \"pathObject\" argument must be of type object".as_ptr());
        return false;
    }
    let obj = val.to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
    let mut buf = bun_core::PathBuffer::default();
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

    let mut result = if has_root { "/".to_string() } else { String::new() };
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
                    && last != &".." {
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
        let resolved = resolve_path::join_abs_string::<Posix>(&cwd, &[to.to_string_lossy().as_bytes()]);
        String::from_utf8_lossy(resolved).into_owned()
    };
    let from_abs = if from.is_absolute() {
        from.to_string_lossy().into_owned()
    } else {
        let resolved = resolve_path::join_abs_string::<Posix>(&cwd, &[from.to_string_lossy().as_bytes()]);
        String::from_utf8_lossy(resolved).into_owned()
    };

    // @trace REQ-ENG-007 [code:bun_paths] — relative-path computation delegated
    // to bun_paths::resolve_path::relative_platform (Zig `relativePath`, POSIX).
    // ALWAYS_COPY=true so the result owns its bytes (does not alias TLS scratch).
    let rel = resolve_path::relative_platform::<Posix, true>(
        from_abs.as_bytes(),
        to_abs.as_bytes(),
    );
    ::std::option::Option::Some(PathBuf::from(String::from_utf8_lossy(rel).into_owned()))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_string_prop(cx: *mut JSContext, obj: Handle<*mut JSObject>, name: &str, value: &str) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let c_val = ZBox::from_bytes(value.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if !js_str.is_null() {
        let val = mozjs::jsval::StringValue(&*js_str);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let val_root = val);
        JS_DefineProperty(cx, obj, c_name.as_ptr(), val_root.handle().into(), JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop(cx: *mut JSContext, obj: Handle<*mut JSObject>, name: &str) -> ::std::option::Option<::std::string::String> {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut val = UndefinedValue();
    let handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
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
        assert_eq!(posix_join(&["a".to_string(), "b".to_string(), "c".to_string()]), "a/b/c");
    }

    // posix_join: absolute parts after first are joined as relative, NOT overriding
    #[test]
    fn test_posix_join_absolute_part() {
        // Leading / in non-first part is stripped (posix join behavior)
        assert_eq!(posix_join(&["a".to_string(), "/b".to_string(), "c".to_string()]), "a/b/c");
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
        assert_eq!(posix_join(&["a".to_string(), "".to_string(), "b".to_string()]), "a/b");
    }

    #[test]
    fn test_posix_join_root() {
        assert_eq!(posix_join(&["/".to_string()]), "/");
    }

    #[test]
    fn test_posix_join_dot_dot_normalizes() {
        assert_eq!(posix_join(&["a".to_string(), "b".to_string(), "..".to_string()]), "a");
    }

    // posix_join: .. beyond root resolves within root (absolute path can't go beyond root)
    #[test]
    fn test_posix_join_dot_dot_beyond_root_stays() {
        // For relative path, .. resolves upward; extra .. stays as ..
        assert_eq!(posix_join(&["a".to_string(), "..".to_string(), "..".to_string()]), "..");
    }

    // --- normalize_path (Path-based) ---

    #[test]
    fn test_normalize_path_dot_dot() {
        assert_eq!(normalize_path(::std::path::Path::new("/a/b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn test_normalize_path_dot() {
        assert_eq!(normalize_path(::std::path::Path::new("/a/./b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn test_normalize_path_root() {
        assert_eq!(normalize_path(::std::path::Path::new("/")), PathBuf::from("/"));
    }

    #[test]
    fn test_normalize_path_relative() {
        assert_eq!(normalize_path(::std::path::Path::new("a/b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn test_normalize_path_double_dot_beyond_root() {
        // Implementation preserves .. beyond root as /../b
        assert_eq!(normalize_path(::std::path::Path::new("/a/../../b")), PathBuf::from("/../b"));
    }

    #[test]
    fn test_normalize_path_empty_relative() {
        assert_eq!(normalize_path(::std::path::Path::new(".")), PathBuf::from("."));
    }

    #[test]
    fn test_normalize_path_multiple_dots() {
        assert_eq!(normalize_path(::std::path::Path::new("/a/b/c/../../d")), PathBuf::from("/a/d"));
    }

    // --- make_absolute ---

    #[test]
    fn test_make_absolute_already_absolute() {
        assert_eq!(make_absolute("/foo/bar"), PathBuf::from("/foo/bar"));
    }

    #[test]
    fn test_make_absolute_relative() {
        let result = make_absolute("foo/bar");
        assert!(result.is_absolute(), "result should be absolute: {:?}", result);
        assert!(result.to_str().unwrap().contains("foo/bar"));
    }

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
