// @trace REQ-ENG-005
use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::fs;
use ::std::path::{Path, PathBuf};
use ::std::ptr;

use mozjs::conversions::jsstr_to_string;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};

use crate::gc_store;

thread_local! {
    static REQUIRE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub fn cache_builtin(cx: &mut mozjs::context::JSContext, name: &str, obj: *mut JSObject) {
    let cache_key = format!("builtin:{}", name);
    gc_store::gc_store_insert(unsafe { cx.raw_cx() }, &cache_key, obj);
}

pub fn cache_assert_strict(cx: &mut mozjs::context::JSContext) {
    use mozjs::jsval::{ObjectValue, UndefinedValue};
    use mozjs::rooted;
    use mozjs::rust::wrappers2 as w2;

    let assert_obj = gc_store::gc_store_get(unsafe { cx.raw_cx() }, "builtin:assert");
    let Some(assert_obj) = assert_obj else { return };
    if assert_obj.is_null() { return; }

    rooted!(&in(cx) let strict_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if strict_obj.get().is_null() { return; }

    unsafe {
        let assert_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &assert_obj };
        let strict_h = strict_obj.handle();

        for (name, _n_args) in &[
            ("ok", 1), ("equal", 2), ("notEqual", 2),
            ("deepEqual", 2), ("notDeepEqual", 2),
            ("strictEqual", 2), ("notStrictEqual", 2),
            ("deepStrictEqual", 2), ("throws", 1),
            ("rejects", 1), ("doesNotThrow", 1),
            ("fail", 0), ("ifError", 1),
        ] {
            let mut fn_val = UndefinedValue();
            let c_name = CString::new(*name).unwrap_or_default();
            JS_GetProperty(cx.raw_cx(), assert_h, c_name.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val });
            if fn_val.is_object() {
                let fn_obj = fn_val.to_object();
                let fn_obj_val = ObjectValue(fn_obj);
                let fn_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_obj_val };
                JS_DefineProperty(cx.raw_cx(), strict_h.into(), c_name.as_ptr(), fn_h, JSPROP_ENUMERATE as u32);
            }
        }

        let mut ae_val = UndefinedValue();
        JS_GetProperty(cx.raw_cx(), assert_h, c"AssertionError".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ae_val });
        if ae_val.is_object() {
            let ae_obj = ae_val.to_object();
            let ae_val2 = ObjectValue(ae_obj);
            let ae_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ae_val2 };
            JS_DefineProperty(cx.raw_cx(), strict_h.into(), c"AssertionError".as_ptr(), ae_h, JSPROP_ENUMERATE as u32);
        }
    }

    cache_builtin(cx, "assert/strict", strict_obj.get());
}

/// Install require() on a target object (REQ-SEC-002 parameter injection).
///
/// Same as `install_require` but attaches the require function to `target`
/// instead of `global`. Used by `create_node_api_scope_values` to build
/// the temporary scope object for privileged evaluate_js.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `target` is a
/// valid handle to the scope JSObject.
pub unsafe fn install_require_on_target(
    cx: &mut mozjs::context::JSContext,
    target: mozjs::rust::Handle<*mut JSObject>,
) {
    mozjs::rust::wrappers2::JS_DefineFunction(
        cx, target, c"require".as_ptr(),
        ::std::option::Option::Some(require_fn), 1, JSPROP_ENUMERATE as u32,
    );
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

pub fn get_require_dir() -> Option<PathBuf> {
    REQUIRE_DIR.with(|d| d.borrow().clone())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn require_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"require() requires a module specifier".as_ptr());
        return false;
    }

    let spec_val = *args.get(0).ptr;
    if !spec_val.is_string() {
        JS_ReportErrorUTF8(cx, c"require() requires a string argument".as_ptr());
        return false;
    }

    let specifier = crate::js_to_rust_string(cx, spec_val);

    // Check built-in modules first (node:fs, node:path, fs, path, etc.)
    let builtin_key = specifier.strip_prefix("node:").unwrap_or(&specifier);
    let cache_key = format!("builtin:{}", builtin_key);
    let cached = gc_store::gc_store_get(cx, &cache_key);
    if let Some(existing) = cached
        && !existing.is_null() {
            args.rval().set(mozjs::jsval::ObjectValue(existing));
            return true;
        }

    // process is a global — return it directly for require("process") / require("node:process")
    if builtin_key == "process" {
        let global = JS::CurrentGlobalOrNull(cx);
        if !global.is_null() {
            let mut val = mozjs::jsval::UndefinedValue();
            let c_prop = CString::new("process").unwrap_or_default();
            unsafe {
                JS_GetProperty(
                    cx,
                    Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
                    c_prop.as_ptr(),
                    MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val },
                );
            }
            if val.is_object() {
                args.rval().set(val);
                return true;
            }
        }
    }

    let base_dir = REQUIRE_DIR.with(|d| d.borrow().clone());

    let resolved = match resolve_specifier(&specifier, base_dir.as_deref()) {
        Some(p) => p,
        None => {
            let msg = format!("Cannot find module '{}'", specifier);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(_) => resolved.clone(),
    };
    let cache_key = canonical.to_string_lossy().into_owned();

    let cached = gc_store::gc_store_get(cx, &cache_key);
    if let Some(existing) = cached
        && !existing.is_null() {
            // Check if this is a primitive wrapper {__primitive__: val}
            let existing_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &existing };
            let mut prim_check = mozjs::jsval::UndefinedValue();
            let prim_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut prim_check };
            unsafe { JS_GetProperty(cx, existing_h, c"__primitive__".as_ptr(), prim_h); }
            if !prim_check.is_undefined() {
                args.rval().set(prim_check);
            } else {
                args.rval().set(mozjs::jsval::ObjectValue(existing));
            }
            return true;
        }

    let content = match fs::read_to_string(&resolved) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Cannot read module '{}': {}", specifier, e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exports_val = if resolved.extension().is_some_and(|e| e == "json") {
        let obj = load_json_module(cx, &content, &specifier);
        if obj.is_null() {
            let msg = format!("Failed to parse JSON module '{}'", specifier);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        mozjs::jsval::ObjectValue(obj)
    } else {
        match load_cjs_module(cx, &content, &resolved, base_dir.as_deref()) {
            Some(val) => val,
            None => {
                let msg = format!("Failed to load module '{}'", specifier);
                let c_msg = CString::new(msg).unwrap_or_default();
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                return false;
            }
        }
    };

    // Cache: for objects, store directly; for primitives, wrap in {__primitive__: val}
    let cache_obj = if exports_val.is_object() {
        exports_val.to_object()
    } else {
        let wrapper = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if !wrapper.is_null() {
            let wrapper_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &wrapper };
            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exports_val };
            JS_DefineProperty(cx, wrapper_h, c"__primitive__".as_ptr(), val_h, 0);
        }
        wrapper
    };
    if !cache_obj.is_null() {
        gc_store::gc_store_insert(cx, &cache_key, cache_obj);
    }

    args.rval().set(exports_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn load_json_module(cx: *mut JSContext, content: &str, specifier: &str) -> *mut JSObject {
    let js_str = JS_NewStringCopyZ(cx, CString::new(content.as_bytes()).unwrap_or_default().as_ptr());
    if js_str.is_null() {
        return ptr::null_mut();
    }
    let str_handle = Handle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &js_str };
    let mut rval = mozjs::jsval::UndefinedValue();
    let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_handle, rval_handle);
    if ok && rval.is_object() {
        return rval.to_object();
    }
    JS_ClearPendingException(cx);
    let msg = format!("Invalid JSON in module '{}'", specifier);
    let c_msg = CString::new(msg).unwrap_or_default();
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    ptr::null_mut()
}

/// Helper: read a property from a JS object as Value.
/// Returns UndefinedValue if anything goes wrong.
#[inline]
unsafe fn get_prop(cx: *mut JSContext, obj: *mut JSObject, name: *const i8) -> Value {
    let mut val = UndefinedValue();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_h, name, val_h);
    val
}

/// Helper: read a property from a JS object stored in a Value.
/// Returns UndefinedValue if the Value is not an object.
#[inline]
unsafe fn get_prop_from_val(cx: *mut JSContext, val: Value, name: *const i8) -> Value {
    if !val.is_object() {
        return UndefinedValue();
    }
    get_prop(cx, val.to_object(), name)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn load_cjs_module(
    cx: *mut JSContext,
    source: &str,
    path: &Path,
    _base_dir: Option<&Path>,
) -> Option<Value> {
    // Create exports object and set on global immediately (roots via global's property table).
    let exports_obj = JS_NewPlainObject(cx);
    if exports_obj.is_null() {
        return None;
    }

    let dir = match path.parent() {
        Some(d) => d,
        None => return Some(mozjs::jsval::ObjectValue(exports_obj)),
    };

    let saved_dir = REQUIRE_DIR.with(|d| d.borrow().clone());
    REQUIRE_DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return None;
    }

    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    // Save old globals so we can restore them after evaluation.
    let old_exports = get_prop(cx, global, c"exports".as_ptr());
    let old_module = get_prop(cx, global, c"module".as_ptr());

    // Set exports on global — this makes it GC-reachable via global's property table.
    {
        let ev = mozjs::jsval::ObjectValue(exports_obj);
        let ev_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ev };
        JS_SetProperty(cx, global_h, c"exports".as_ptr(), ev_h);
    }

    // Create module object — JS_NewPlainObject CAN trigger GC.
    // exports is safe because it's reachable from global, and SpiderMonkey updates
    // all heap pointers (property values) when objects move.
    {
        let module_obj = JS_NewPlainObject(cx);
        if !module_obj.is_null() {
            let mv = mozjs::jsval::ObjectValue(module_obj);
            let mv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mv };
            JS_SetProperty(cx, global_h, c"module".as_ptr(), mv_h);

            // Re-read exports from global to get a fresh pointer after potential GC.
            // GC updates global.exports if the object moved; raw local `exports_obj` is stale.
            let fresh_exports = get_prop(cx, global, c"exports".as_ptr());
            if fresh_exports.is_object() {
                let fresh_exp_obj = fresh_exports.to_object();
                let fev = mozjs::jsval::ObjectValue(fresh_exp_obj);
                let fev_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fev };
                // Re-read module from global for the same reason.
                let fresh_module = get_prop(cx, global, c"module".as_ptr());
                if fresh_module.is_object() {
                    let fm_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &fresh_module.to_object() };
                    JS_DefineProperty(cx, fm_h, c"exports".as_ptr(), fev_h, JSPROP_ENUMERATE as u32);
                }
            }
        }
    }

    // Compile and evaluate the module source.
    let filename_str = path.to_string_lossy().into_owned();
    let c_filename = CString::new(filename_str)
        .unwrap_or_else(|_| CString::new("<module>").unwrap());
    let opts = NewCompileOptions(cx, c_filename.as_ptr(), 1);
    if opts.is_null() {
        JS_DeleteProperty1(cx, global_h, c"exports".as_ptr());
        JS_DeleteProperty1(cx, global_h, c"module".as_ptr());
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return None;
    }

    let mut src = mozjs::rust::transform_str_to_source_text(source);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h);
    libc::free(opts as *mut _);

    // IMPORTANT: read module.exports BEFORE restoring old globals.
    // After Evaluate2 (which can trigger GC), re-read from global to get fresh pointers.
    let module_after_eval = get_prop(cx, global, c"module".as_ptr());
    let final_exports = get_prop_from_val(cx, module_after_eval, c"exports".as_ptr());

    // Restore old globals.
    JS_DeleteProperty1(cx, global_h, c"exports".as_ptr());
    JS_DeleteProperty1(cx, global_h, c"module".as_ptr());
    if !old_exports.is_undefined() {
        let restore_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &old_exports };
        JS_SetProperty(cx, global_h, c"exports".as_ptr(), restore_h);
    }
    if !old_module.is_undefined() {
        let restore_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &old_module };
        JS_SetProperty(cx, global_h, c"module".as_ptr(), restore_h);
    }

    if !ok {
        JS_ClearPendingException(cx);
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return None;
    }

    mozjs_sys::jsapi::js::RunJobs(cx);
    REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);

    // module.exports was set (possibly to a primitive) — return it.
    if !final_exports.is_undefined() {
        return Some(final_exports);
    }

    // Fallback: re-read exports from global (in case module.exports was never set
    // but the module modified `exports.x = ...` via the global reference).
    let fallback = get_prop(cx, global, c"exports".as_ptr());
    if !fallback.is_undefined() {
        return Some(fallback);
    }

    Some(mozjs::jsval::UndefinedValue())
}

fn resolve_specifier(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    // External resolver (bun_resolver via bao_engine hook) takes priority
    if let Some(result) = bao_engine::module_loader::try_external_resolve(specifier, base_dir) {
        return Some(result);
    }

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
    // Only return the path itself if it is a regular file.
    // Returning a bare directory here used to incorrectly resolve empty / "." /
    // ".." specifiers to whatever node_modules directory happened to exist on
    // the traversal path (root cause of test_resolve_node_modules_empty_specifier
    // and test_resolve_node_modules_with_dot_specifier failures).
    if path.is_file() {
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

pub fn resolve_node_modules(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    // Defensive guard: empty / "." / ".." are not valid package names and must
    // not produce false positives by falling through to directory traversal.
    if specifier.is_empty() || specifier == "." || specifier == ".." {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::fs;

    fn tempdir() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("create temp dir")
    }

    #[test]
    fn test_try_resolve_js_extension() {
        let dir = tempdir();
        let file = dir.path().join("mod.js");
        fs::write(&file, "").unwrap();
        let result = try_resolve(dir.path().join("mod").as_path());
        assert_eq!(result.unwrap().extension().unwrap(), "js");
    }

    #[test]
    fn test_try_resolve_ts_extension() {
        let dir = tempdir();
        let file = dir.path().join("mod.ts");
        fs::write(&file, "").unwrap();
        let result = try_resolve(dir.path().join("mod").as_path());
        assert!(result.is_some());
    }

    #[test]
    fn test_try_resolve_exact_match() {
        let dir = tempdir();
        let file = dir.path().join("data.json");
        fs::write(&file, "{}").unwrap();
        let result = try_resolve(dir.path().join("data.json").as_path());
        assert!(result.is_some());
    }

    #[test]
    fn test_try_resolve_index_js() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.js"), "").unwrap();
        // try_resolve checks directory → falls through to index.js
        // But since directory itself exists, it returns the dir path first.
        // This verifies the behavior: directory paths resolve to themselves.
        let result = try_resolve(pkg.as_path());
        assert!(result.is_some());
    }

    #[test]
    fn test_try_resolve_not_found() {
        let dir = tempdir();
        let result = try_resolve(dir.path().join("nonexistent").as_path());
        assert!(result.is_none());
    }

    #[test]
    fn test_try_resolve_priority_js_over_mjs() {
        let dir = tempdir();
        fs::write(dir.path().join("mod.js"), "").unwrap();
        fs::write(dir.path().join("mod.mjs"), "").unwrap();
        let result = try_resolve(dir.path().join("mod").as_path()).unwrap();
        assert_eq!(result.extension().unwrap(), "js");
    }

    #[test]
    fn test_resolve_node_modules_finds_package() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("lodash");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        let result = resolve_node_modules("lodash", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("lodash"));
    }

    #[test]
    fn test_resolve_node_modules_not_found() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        let result = resolve_node_modules("nonexistent", Some(dir.path()));
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_node_modules_traverses_up() {
        let dir = tempdir();
        let child = dir.path().join("sub").join("deep");
        fs::create_dir_all(&child).unwrap();
        let nm = dir.path().join("node_modules").join("pkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        let result = resolve_node_modules("pkg", Some(&child));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("pkg"));
    }

    #[test]
    fn test_resolve_specifier_absolute() {
        let dir = tempdir();
        let file = dir.path().join("target.js");
        fs::write(&file, "").unwrap();
        let abs = file.to_str().unwrap().to_string();
        let result = resolve_specifier(&abs, None);
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_relative() {
        let dir = tempdir();
        let file = dir.path().join("rel.js");
        fs::write(&file, "").unwrap();
        let result = resolve_specifier("./rel", Some(dir.path()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_parent_relative() {
        let dir = tempdir();
        let child = dir.path().join("sub");
        fs::create_dir_all(&child).unwrap();
        let file = dir.path().join("parent.js");
        fs::write(&file, "").unwrap();
        let result = resolve_specifier("../parent", Some(&child));
        assert!(result.is_some());
    }

    // =========================================================================
    // Additional tests for edge case coverage (20+ tests)
    // =========================================================================

    // --- try_resolve with .mjs, .json, .tsx extensions ---

    #[test]
    fn test_try_resolve_mjs_extension() {
        let dir = tempdir();
        let file = dir.path().join("mod.mjs");
        fs::write(&file, "export default 1;").unwrap();
        let result = try_resolve(dir.path().join("mod").as_path());
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "mjs");
    }

    #[test]
    fn test_try_resolve_json_extension() {
        let dir = tempdir();
        let file = dir.path().join("data.json");
        fs::write(&file, r#"{"key": "value"}"#).unwrap();
        let result = try_resolve(dir.path().join("data").as_path());
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "json");
    }

    #[test]
    fn test_try_resolve_tsx_extension() {
        let dir = tempdir();
        let file = dir.path().join("component.tsx");
        fs::write(&file, "export const X = () => <div/>;").unwrap();
        let result = try_resolve(dir.path().join("component").as_path());
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "tsx");
    }

    #[test]
    fn test_try_resolve_extension_priority_order() {
        // Verify extension priority: .js > .mjs > .json > .ts > .tsx
        let dir = tempdir();
        // Only .tsx exists
        fs::write(dir.path().join("only.tsx"), "").unwrap();
        let result = try_resolve(dir.path().join("only").as_path()).unwrap();
        assert_eq!(result.extension().unwrap(), "tsx");

        // Add .ts - should take precedence over .tsx
        fs::write(dir.path().join("only.ts"), "").unwrap();
        let result = try_resolve(dir.path().join("only").as_path()).unwrap();
        assert_eq!(result.extension().unwrap(), "ts");

        // Add .json - should take precedence over .ts
        fs::write(dir.path().join("only.json"), "{}").unwrap();
        let result = try_resolve(dir.path().join("only").as_path()).unwrap();
        assert_eq!(result.extension().unwrap(), "json");

        // Add .mjs - should take precedence over .json
        fs::write(dir.path().join("only.mjs"), "").unwrap();
        let result = try_resolve(dir.path().join("only").as_path()).unwrap();
        assert_eq!(result.extension().unwrap(), "mjs");

        // Add .js - should take precedence over .mjs
        fs::write(dir.path().join("only.js"), "").unwrap();
        let result = try_resolve(dir.path().join("only").as_path()).unwrap();
        assert_eq!(result.extension().unwrap(), "js");
    }

    // --- try_resolve with deeply nested directories ---

    #[test]
    fn test_try_resolve_deeply_nested_file() {
        let dir = tempdir();
        let deep = dir.path().join("a").join("b").join("c").join("d").join("e");
        fs::create_dir_all(&deep).unwrap();
        let file = deep.join("nested.js");
        fs::write(&file, "").unwrap();
        let result = try_resolve(deep.join("nested").as_path());
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("nested.js"));
    }

    #[test]
    fn test_try_resolve_deeply_nested_directory_with_index() {
        let dir = tempdir();
        let deep = dir.path().join("x").join("y").join("z").join("pkg");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("index.js"), "module.exports = {};").unwrap();
        let result = try_resolve(deep.as_path());
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("index.js"));
    }

    // --- try_resolve with empty directory ---

    #[test]
    fn test_try_resolve_empty_directory_no_index() {
        let dir = tempdir();
        let empty = dir.path().join("empty_dir");
        fs::create_dir_all(&empty).unwrap();
        // Empty directory with no index files should return None
        let result = try_resolve(empty.as_path());
        assert!(result.is_none());
    }

    // --- try_resolve with index.mjs and index.ts fallbacks ---

    #[test]
    fn test_try_resolve_index_mjs_fallback() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg_mjs");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.mjs"), "export default 1;").unwrap();
        let result = try_resolve(pkg.as_path());
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("index.mjs"));
    }

    #[test]
    fn test_try_resolve_index_ts_fallback() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg_ts");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.ts"), "export const x = 1;").unwrap();
        let result = try_resolve(pkg.as_path());
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("index.ts"));
    }

    #[test]
    fn test_try_resolve_index_priority_js_over_mjs() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg_priority");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.mjs"), "").unwrap();
        fs::write(pkg.join("index.js"), "").unwrap();
        let result = try_resolve(pkg.as_path()).unwrap();
        assert!(result.ends_with("index.js"));
    }

    #[test]
    fn test_try_resolve_index_priority_mjs_over_ts() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg_mjs_ts");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.ts"), "").unwrap();
        fs::write(pkg.join("index.mjs"), "").unwrap();
        let result = try_resolve(pkg.as_path()).unwrap();
        assert!(result.ends_with("index.mjs"));
    }

    // --- try_resolve with non-regular files (symlinks) ---

    #[test]
    fn test_try_resolve_symlink_to_file() {
        let dir = tempdir();
        let target = dir.path().join("target.js");
        fs::write(&target, "").unwrap();
        let link = dir.path().join("link.js");
        #[cfg(unix)]
        {
            use ::std::os::unix::fs::symlink;
            symlink(&target, &link).expect("create symlink");
        }
        #[cfg(windows)]
        {
            // Windows symlinks require admin privileges; skip test
            return;
        }
        // Symlink should resolve via is_file() check
        let result = try_resolve(dir.path().join("link").as_path());
        assert!(result.is_some());
    }

    #[test]
    fn test_try_resolve_symlink_to_directory_with_index() {
        let dir = tempdir();
        let target_dir = dir.path().join("target_pkg");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("index.js"), "").unwrap();
        let link_dir = dir.path().join("link_pkg");
        #[cfg(unix)]
        {
            use ::std::os::unix::fs::symlink;
            symlink(&target_dir, &link_dir).expect("create symlink");
        }
        #[cfg(windows)]
        {
            return;
        }
        let result = try_resolve(link_dir.as_path());
        assert!(result.is_some());
    }

    // --- resolve_specifier with absolute paths ---

    #[test]
    fn test_resolve_specifier_absolute_with_extension() {
        let dir = tempdir();
        let file = dir.path().join("absolute_target.js");
        fs::write(&file, "").unwrap();
        let abs = file.to_str().unwrap().to_string();
        let result = resolve_specifier(&abs, None);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), file);
    }

    #[test]
    fn test_resolve_specifier_absolute_without_extension() {
        let dir = tempdir();
        let file = dir.path().join("no_ext.js");
        fs::write(&file, "").unwrap();
        // Pass path without .js extension
        let abs_no_ext = file.with_extension("").to_str().unwrap().to_string();
        let result = resolve_specifier(&abs_no_ext, None);
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("no_ext.js"));
    }

    #[test]
    fn test_resolve_specifier_absolute_nonexistent() {
        let result = resolve_specifier("/nonexistent/path/to/module", None);
        assert!(result.is_none());
    }

    // --- resolve_specifier with "./" and "../" relative paths ---

    #[test]
    fn test_resolve_specifier_dot_slash_current_dir() {
        let dir = tempdir();
        let file = dir.path().join("current.js");
        fs::write(&file, "").unwrap();
        let result = resolve_specifier("./current", Some(dir.path()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_dot_slash_nested() {
        let dir = tempdir();
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("nested.js");
        fs::write(&file, "").unwrap();
        let result = resolve_specifier("./a/b/nested", Some(dir.path()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_double_dot_slash_traverses_up() {
        let dir = tempdir();
        let child = dir.path().join("child");
        fs::create_dir_all(&child).unwrap();
        let file = dir.path().join("up.js");
        fs::write(&file, "").unwrap();
        let result = resolve_specifier("../up", Some(&child));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_multiple_parent_traversals() {
        let dir = tempdir();
        let deep = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&deep).unwrap();
        let file = dir.path().join("root.js");
        fs::write(&file, "").unwrap();
        // From deep (a/b/c), go up 3 levels to reach dir
        let result = resolve_specifier("../../../root", Some(&deep));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_relative_no_base_dir() {
        // When base_dir is None, relative paths resolve from current_dir
        let dir = tempdir();
        let file = dir.path().join("rel.js");
        fs::write(&file, "").unwrap();
        // Save current dir and change to temp dir
        let original = ::std::env::current_dir().unwrap();
        ::std::env::set_current_dir(dir.path()).unwrap();
        let result = resolve_specifier("./rel", None);
        ::std::env::set_current_dir(&original).unwrap();
        assert!(result.is_some());
    }

    // --- resolve_specifier with bare specifiers falling through to node_modules ---

    #[test]
    fn test_resolve_specifier_bare_falls_through_to_node_modules() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("mylib");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        // Bare specifier (no ./ or ../ prefix) should resolve via node_modules
        let result = resolve_specifier("mylib", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("mylib"));
    }

    #[test]
    fn test_resolve_specifier_bare_not_in_node_modules() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        // Bare specifier that doesn't exist in node_modules
        let result = resolve_specifier("nonexistent_pkg", Some(dir.path()));
        assert!(result.is_none());
    }

    // --- resolve_node_modules with empty string, ".", ".." specifiers ---

    #[test]
    fn test_resolve_node_modules_empty_specifier() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        // Empty specifier must return None (defensive guard)
        let result = resolve_node_modules("", Some(dir.path()));
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_node_modules_with_dot_specifier() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        // "." specifier must return None (defensive guard)
        let result = resolve_node_modules(".", Some(dir.path()));
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_node_modules_with_double_dot_specifier() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        // ".." specifier must return None (defensive guard)
        let result = resolve_node_modules("..", Some(dir.path()));
        assert!(result.is_none());
    }

    // --- resolve_node_modules with base_dir=None ---

    #[test]
    fn test_resolve_node_modules_base_dir_none_uses_cwd() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("cwd_pkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        // Save and change current directory
        let original = ::std::env::current_dir().unwrap();
        ::std::env::set_current_dir(dir.path()).unwrap();
        let result = resolve_node_modules("cwd_pkg", None);
        ::std::env::set_current_dir(&original).unwrap();
        assert!(result.is_some());
    }

    // --- resolve_node_modules with deeply nested base_dir ---

    #[test]
    fn test_resolve_node_modules_deeply_nested_base_dir() {
        let dir = tempdir();
        let deep = dir.path().join("a").join("b").join("c").join("d").join("e");
        fs::create_dir_all(&deep).unwrap();
        let nm = dir.path().join("node_modules").join("deep_pkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        // Should traverse up from deep to find node_modules at root
        let result = resolve_node_modules("deep_pkg", Some(&deep));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_node_modules_finds_in_intermediate_node_modules() {
        // Test that it finds package in the first node_modules encountered,
        // not just the topmost one
        let dir = tempdir();
        let mid = dir.path().join("mid");
        fs::create_dir_all(&mid).unwrap();
        // node_modules at root level
        let nm_root = dir.path().join("node_modules").join("root_pkg");
        fs::create_dir_all(&nm_root).unwrap();
        fs::write(nm_root.join("index.js"), "// root").unwrap();
        // node_modules at mid level
        let nm_mid = mid.join("node_modules").join("mid_pkg");
        fs::create_dir_all(&nm_mid).unwrap();
        fs::write(nm_mid.join("index.js"), "// mid").unwrap();
        // From mid, should find mid_pkg first
        let result = resolve_node_modules("mid_pkg", Some(&mid)).unwrap();
        assert!(result.to_str().unwrap().contains("mid_pkg"));
    }

    // --- Edge case: specifier with special characters ---

    #[test]
    fn test_resolve_specifier_with_hyphen() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("my-lib");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        let result = resolve_node_modules("my-lib", Some(dir.path()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_with_underscore() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("my_lib");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        let result = resolve_node_modules("my_lib", Some(dir.path()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_with_dot_in_name() {
        // Scoped packages like @types/node or packages with dots
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("lib.core");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        let result = resolve_node_modules("lib.core", Some(dir.path()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_with_numbers() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("pkg123");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "").unwrap();
        let result = resolve_node_modules("pkg123", Some(dir.path()));
        assert!(result.is_some());
    }

    // --- Edge case: case sensitivity ---

    #[test]
    fn test_resolve_specifier_case_sensitive() {
        // On case-sensitive filesystems (Linux), different case = different file
        let dir = tempdir();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        // Create lowercase package
        let pkg_lower = nm.join("mypkg");
        fs::create_dir_all(&pkg_lower).unwrap();
        fs::write(pkg_lower.join("index.js"), "").unwrap();
        // Request uppercase - should not find on case-sensitive FS
        let result = resolve_node_modules("MyPkg", Some(dir.path()));
        // On case-insensitive FS (macOS/Windows), this might succeed
        // On Linux, it should fail
        #[cfg(target_os = "linux")]
        assert!(result.is_none());
    }

    // --- Edge case: path traversal (../) beyond root ---

    #[test]
    fn test_resolve_specifier_traversal_beyond_root() {
        let dir = tempdir();
        let file = dir.path().join("root.js");
        fs::write(&file, "").unwrap();
        // Try to traverse beyond the temp dir root
        // This should either resolve to something valid or return None
        // depending on what exists above the temp dir
        let result = resolve_specifier("../../../../../etc/passwd", Some(dir.path()));
        // Should not find /etc/passwd via relative path from temp dir
        // (unless temp dir happens to be very deep and /etc/passwd exists)
        // The key is it shouldn't panic
        let _ = result; // Just ensure no panic
    }

    #[test]
    fn test_resolve_node_modules_traversal_stops_at_root() {
        // resolve_node_modules should stop when it reaches filesystem root
        let dir = tempdir();
        // No node_modules anywhere
        let result = resolve_node_modules("nonexistent", Some(dir.path()));
        assert!(result.is_none());
    }

    // --- Additional edge cases ---

    #[test]
    fn test_try_resolve_with_trailing_slash() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg_with_slash");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.js"), "").unwrap();
        // Path with trailing slash (as string) - Path::new handles this
        let path_with_slash = pkg.to_str().unwrap().to_string() + "/";
        let result = try_resolve(Path::new(&path_with_slash));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_specifier_with_subpath() {
        // Test resolving a subpath within a package
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("pkg");
        let sub = nm.join("lib").join("sub.js");
        fs::create_dir_all(sub.parent().unwrap()).unwrap();
        fs::write(&sub, "").unwrap();
        let result = resolve_node_modules("pkg/lib/sub", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("sub.js"));
    }

    #[test]
    fn test_resolve_specifier_json_file_exact() {
        let dir = tempdir();
        let file = dir.path().join("config.json");
        fs::write(&file, r#"{"name": "test"}"#).unwrap();
        let result = resolve_specifier("./config.json", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("config.json"));
    }

    #[test]
    fn test_try_resolve_exact_file_path_no_extension_add() {
        // When exact file path is given (with extension), don't add extensions
        let dir = tempdir();
        let file = dir.path().join("exact.js");
        fs::write(&file, "").unwrap();
        // Pass the full path with extension
        let result = try_resolve(&file);
        assert!(result.is_some());
        // Should return the exact path, not try to add .js again
        assert_eq!(result.unwrap(), file);
    }
}
