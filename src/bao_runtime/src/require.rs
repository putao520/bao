use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ffi::CString;
use ::std::fs;
use ::std::path::{Path, PathBuf};
use ::std::ptr;

use mozjs::conversions::jsstr_to_string;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};

thread_local! {
    static MODULE_CACHE: RefCell<HashMap<::std::string::String, *mut JSObject>> = RefCell::new(HashMap::new());
    static REQUIRE_DIR: RefCell<Option<PathBuf>> = RefCell::new(None);
}

pub fn install_require(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        mozjs::rust::wrappers2::JS_DefineFunction(
            cx, global, c"require".as_ptr(),
            ::std::option::Option::Some(require_fn), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

pub fn set_require_dir(dir: PathBuf) {
    REQUIRE_DIR.with(|d| *d.borrow_mut() = Some(dir));
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn require_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"require() requires a module specifier\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let spec_val = *args.get(0).ptr;
    if !spec_val.is_string() {
        JS_ReportErrorUTF8(cx, b"require() requires a string argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let specifier = jsstr_to_string(cx, ptr::NonNull::new(spec_val.to_string()).unwrap());
    let base_dir = REQUIRE_DIR.with(|d| d.borrow().clone());

    let resolved = match resolve_specifier(&specifier, base_dir.as_deref()) {
        Some(p) => p,
        None => {
            let msg = format!("Cannot find module '{}'", specifier);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(_) => resolved.clone(),
    };
    let cache_key = canonical.to_string_lossy().into_owned();

    let cached = MODULE_CACHE.with(|c| c.borrow().get(&cache_key).copied());
    if let Some(existing) = cached {
        if !existing.is_null() {
            let exports_val = mozjs::jsval::ObjectValue(existing);
            args.rval().set(exports_val);
            return true;
        }
    }

    let content = match fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Cannot read module '{}': {}", specifier, e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let exports_obj = load_cjs_module(cx, &content, &resolved, base_dir.as_deref());

    if exports_obj.is_null() {
        let msg = format!("Failed to load module '{}'", specifier);
        let c_msg = CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
        return false;
    }

    MODULE_CACHE.with(|c| c.borrow_mut().insert(cache_key, exports_obj));
    args.rval().set(mozjs::jsval::ObjectValue(exports_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn load_cjs_module(
    cx: *mut JSContext,
    source: &str,
    path: &Path,
    _base_dir: Option<&Path>,
) -> *mut JSObject {
    let exports_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if exports_obj.is_null() {
        return ptr::null_mut();
    }

    let dir = match path.parent() {
        Some(d) => d,
        None => return exports_obj,
    };

    let saved_dir = REQUIRE_DIR.with(|d| d.borrow().clone());
    REQUIRE_DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return ptr::null_mut();
    }

    let global_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let mut old_exports = UndefinedValue();
    let old_exports_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut old_exports };
    JS_GetProperty(cx, global_handle, c"exports".as_ptr(), old_exports_h);
    let mut old_module = UndefinedValue();
    let old_module_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut old_module };
    JS_GetProperty(cx, global_handle, c"module".as_ptr(), old_module_h);

    let exports_val = mozjs::jsval::ObjectValue(exports_obj);
    let exports_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exports_val };
    JS_SetProperty(cx, global_handle, c"exports".as_ptr(), exports_h);

    let module_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !module_obj.is_null() {
        let module_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &module_obj };
        let mod_val = mozjs::jsval::ObjectValue(module_obj);
        let mod_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_val };
        JS_SetProperty(cx, global_handle, c"module".as_ptr(), mod_h);
        JS_DefineProperty(cx, module_h, c"exports".as_ptr(), exports_h, JSPROP_ENUMERATE as u32);
    }

    let filename_str = path.to_string_lossy().into_owned();
    let c_filename = CString::new(filename_str)
        .unwrap_or_else(|_| CString::new("<module>").unwrap());
    let opts = NewCompileOptions(cx, c_filename.as_ptr(), 1);
    if opts.is_null() {
        JS_DeleteProperty1(cx, global_handle, c"exports".as_ptr());
        JS_DeleteProperty1(cx, global_handle, c"module".as_ptr());
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return ptr::null_mut();
    }

    let mut src = mozjs::rust::transform_str_to_source_text(source);
    let mut rval = UndefinedValue();
    let mut rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_handle);
    libc::free(opts as *mut _);

    JS_DeleteProperty1(cx, global_handle, c"exports".as_ptr());
    JS_DeleteProperty1(cx, global_handle, c"module".as_ptr());
    if !old_exports.is_undefined() {
        let restore_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &old_exports };
        JS_SetProperty(cx, global_handle, c"exports".as_ptr(), restore_h);
    }
    if !old_module.is_undefined() {
        let restore_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &old_module };
        JS_SetProperty(cx, global_handle, c"module".as_ptr(), restore_h);
    }

    if !ok {
        JS_ClearPendingException(cx);
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return ptr::null_mut();
    }

    mozjs_sys::jsapi::js::RunJobs(cx);
    REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
    exports_obj
}

fn wrap_cjs(source: &str, filename: &str, dirname: &str) -> ::std::string::String {
    format!(
        r#"(function(exports, require, module, __filename, __dirname) {{
{}
}})(exports, require, module, "{}", "{}")"#,
        source,
        filename.replace('\\', "\\\\").replace('"', "\\\""),
        dirname.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn resolve_specifier(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    let path = Path::new(specifier);

    if path.is_absolute() {
        return try_resolve(path);
    }

    if specifier.starts_with("./") || specifier.starts_with("../") {
        let base = base_dir.unwrap_or_else(|| Path::new("."));
        let full = base.join(specifier);
        return try_resolve(&full);
    }

    resolve_node_modules(specifier, base_dir)
}

fn try_resolve(path: &Path) -> ::std::option::Option<PathBuf> {
    for ext in [".js", ".mjs", ".json", ".ts", ".tsx"] {
        let candidate = PathBuf::from(format!("{}{}", path.display(), ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    if path.exists() {
        return Some(path.to_path_buf());
    }
    if path.is_dir() {
        for name in ["index.js", "index.mjs", "index.ts"] {
            let candidate = path.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

fn resolve_node_modules(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    let start = match base_dir {
        Some(d) => d.to_path_buf(),
        None => ::std::env::current_dir().ok()?,
    };
    let mut dir = start.as_path();
    loop {
        let nm = dir.join("node_modules");
        if nm.is_dir() {
            let target = nm.join(specifier);
            if let Some(r) = try_resolve(&target) {
                return Some(r);
            }
        }
        dir = dir.parent()?;
    }
}
