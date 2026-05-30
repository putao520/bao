use ::std::ffi::CString;
use ::std::path::{Path, PathBuf, MAIN_SEPARATOR};

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

        let sep_cstr = CString::new(if MAIN_SEPARATOR == '/' { "/" } else { "\\" }).unwrap_or_default();
        let sep_str = JS_NewStringCopyZ(cx.raw_cx(), sep_cstr.as_ptr());
        if !sep_str.is_null() {
            let sep_val = mozjs::jsval::StringValue(&*sep_str);
            rooted!(&in(cx) let sep_root = sep_val);
            JS_DefineProperty(cx.raw_cx(), path_obj.handle().into(), c"sep".as_ptr(), sep_root.handle().into(), JSPROP_ENUMERATE as u32);
        }

        let delim_cstr = CString::new(if cfg!(windows) { ";" } else { ":" }).unwrap_or_default();
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

    cache_builtin("path", path_obj.get());
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
    let c_str = CString::new(s).unwrap_or_default();
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
    for i in 0..argc {
        let val = *args.get(i).ptr;
        match arg_to_string(cx, val) {
            Some(s) => parts.push(s),
            None => {
                JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
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
    let cwd = ::std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut resolved = cwd;

    for i in 0..argc {
        let val = *args.get(i).ptr;
        match arg_to_string(cx, val) {
            Some(s) => {
                let p = Path::new(&s);
                if p.is_absolute() {
                    resolved = p.to_path_buf();
                } else {
                    resolved = resolved.join(p);
                }
            }
            None => {
                JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
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
        JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
            return false;
        }
    };
    let result = Path::new(&s).parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    return_string(cx, &args, &result)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_basename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
            return false;
        }
    };
    let mut base = Path::new(&s).file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.clone());

    if argc >= 2 {
        let ext_val = *args.get(1).ptr;
        if let Some(ext) = arg_to_string(cx, ext_val) {
            if base.ends_with(&ext) && !ext.is_empty() {
                base.truncate(base.len() - ext.len());
            }
        }
    }
    return_string(cx, &args, &base)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_extname(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
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
        JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
            return false;
        }
    };
    let p = Path::new(&s);
    let normalized = normalize_path(&p.to_path_buf());
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
        JS_ReportErrorUTF8(cx, b"The \"from\" and \"to\" arguments must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
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
        JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let val = *args.get(0).ptr;
    let s = match arg_to_string(cx, val) {
        Some(s) => s,
        None => {
            JS_ReportErrorUTF8(cx, b"The \"path\" argument must be of type string\0".as_ptr() as *const ::std::os::raw::c_char);
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
    let parsed_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &parsed };

    define_string_prop(cx, parsed_handle, "root", &root);
    define_string_prop(cx, parsed_handle, "dir", &dir);
    define_string_prop(cx, parsed_handle, "base", &file_name);
    define_string_prop(cx, parsed_handle, "ext", &ext);
    define_string_prop(cx, parsed_handle, "name", &name);

    args.rval().set(mozjs::jsval::ObjectValue(parsed));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn path_format(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"The \"pathObject\" argument must be of type object\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let val = *args.get(0).ptr;
    if !val.is_object() {
        JS_ReportErrorUTF8(cx, b"The \"pathObject\" argument must be of type object\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let obj = val.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let dir = get_string_prop(cx, obj_h, "dir");
    let base = get_string_prop(cx, obj_h, "base");
    let name = get_string_prop(cx, obj_h, "name");
    let ext = get_string_prop(cx, obj_h, "ext");

    let result = if let Some(b) = base {
        if dir.as_ref().map_or(false, |d| !d.is_empty()) {
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

fn posix_join(parts: &[::std::string::String]) -> ::std::string::String {
    if parts.is_empty() {
        return ".".to_string();
    }
    let mut result = ::std::string::String::new();
    for part in parts {
        if part.starts_with('/') {
            result.clear();
        }
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(part);
    }
    if result.is_empty() {
        return ".".to_string();
    }
    let mut segments: Vec<&str> = Vec::new();
    let has_root = result.starts_with('/');
    for seg in result.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if !segments.is_empty() && *segments.last().expect("non-empty segments") != ".." {
                    segments.pop();
                } else if !has_root {
                    segments.push("..");
                }
            }
            _ => segments.push(seg),
        }
    }
    let mut normalized = if has_root { "/".to_string() } else { String::new() };
    normalized.push_str(&segments.join("/"));
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

fn normalize_path(path: &PathBuf) -> PathBuf {
    let mut components = Vec::new();
    let has_root = path.is_absolute();
    for comp in path.components() {
        match comp {
            ::std::path::Component::CurDir => {}
            ::std::path::Component::ParentDir => {
                if let Some(last) = components.last() {
                    if last != &".." {
                        components.pop();
                        continue;
                    }
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

fn make_absolute(s: &str) -> PathBuf {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        normalize_path(&p)
    } else {
        let cwd = ::std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        normalize_path(&cwd.join(&p))
    }
}

fn pathdiff(to: &Path, from: &Path) -> ::std::option::Option<PathBuf> {
    let to_abs = if to.is_absolute() { to.to_path_buf() } else { ::std::env::current_dir().ok()?.join(to) };
    let from_abs = if from.is_absolute() { from.to_path_buf() } else { ::std::env::current_dir().ok()?.join(from) };

    let mut to_components: Vec<_> = to_abs.components().collect();
    let mut from_components: Vec<_> = from_abs.components().collect();

    while !to_components.is_empty() && !from_components.is_empty() && to_components[0] == from_components[0] {
        to_components.remove(0);
        from_components.remove(0);
    }

    let mut result = PathBuf::new();
    for _ in from_components.iter() {
        result.push("..");
    }
    for comp in to_components {
        result.push(comp);
    }
    ::std::option::Option::Some(result)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_string_prop(cx: *mut JSContext, obj: Handle<*mut JSObject>, name: &str, value: &str) {
    let c_name = CString::new(name).unwrap_or_default();
    let c_val = CString::new(value).unwrap_or_default();
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if !js_str.is_null() {
        let val = mozjs::jsval::StringValue(&*js_str);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj, c_name.as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop(cx: *mut JSContext, obj: Handle<*mut JSObject>, name: &str) -> ::std::option::Option<::std::string::String> {
    let c_name = CString::new(name).unwrap_or_default();
    let mut val = UndefinedValue();
    let handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj, c_name.as_ptr(), handle);
    arg_to_string(cx, val)
}
