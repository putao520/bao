// @trace REQ-ENG-005
use ::std::cell::RefCell;
use ::std::os::unix::ffi::OsStrExt;
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
            let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
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
            let c_prop = c"process".as_ptr();
            unsafe {
                JS_GetProperty(
                    cx,
                    Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global },
                    c_prop,
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
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // D57: bun_sys::realpath instead of Path::canonicalize
    let canonical = {
        let mut buf = bun_paths::path_buffer_pool::get();
        let c_path = bun_core::ZBox::from_bytes(resolved.as_os_str().as_bytes());
        let zpath = unsafe { bun_core::ZStr::from_c_ptr(c_path.as_ptr()) };
        match bun_sys::realpath(zpath, &mut buf) {
            Ok(bytes) => ::std::path::PathBuf::from(::std::str::from_utf8(bytes).unwrap_or(".")),
            Err(_) => resolved.clone(),
        }
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

    let content = match bun_sys::File::read_from(bun_sys::Fd::cwd(), resolved.as_os_str().as_bytes()) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            let msg = format!("Cannot read module '{}': {}", specifier, e);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let exports_val = if resolved.extension().is_some_and(|e| e == "json") {
        let obj = load_json_module(cx, &content, &specifier);
        if obj.is_null() {
            let msg = format!("Failed to parse JSON module '{}'", specifier);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        mozjs::jsval::ObjectValue(obj)
    } else {
        match load_cjs_module(cx, &content, &resolved, base_dir.as_deref()) {
            Some(val) => val,
            None => {
                let msg = format!("Failed to load module '{}'", specifier);
                let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
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
    let js_str = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(content.as_bytes()).as_ptr());
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
    let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
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
    let c_filename = bun_core::ZBox::from_bytes(path.to_string_lossy().as_bytes());
    let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(NewCompileOptions(cx, c_filename.as_ptr(), 1) as *mut _) else {
        JS_DeleteProperty1(cx, global_h, c"exports".as_ptr());
        JS_DeleteProperty1(cx, global_h, c"module".as_ptr());
        REQUIRE_DIR.with(|d| *d.borrow_mut() = saved_dir);
        return None;
    };
    let opts = _opts_guard.as_ptr();

    let mut src = mozjs::rust::transform_str_to_source_text(source);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts as *const _, &mut src, rval_h);

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

pub fn resolve_specifier(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    // 铁律 0: use bun_resolver exclusively, no hand-written fallback
    bao_engine::module_loader::try_external_resolve(specifier, base_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as IoWrite;

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // resolve_specifier now delegates entirely to bun_resolver via
    // bao_engine::module_loader::try_external_resolve. Module resolution
    // tests live in bun_resolver's test suite.

    #[test]
    fn test_load_cjs_module_exports_x_pattern() {
        let tmp = tmp_dir();
        let js = tmp.path().join("mod.js");
        fs::write(&js, "exports.x = 42; exports.y = 'hello';").expect("write");
        let ctx = crate::runtime::JSContext::get();
        let result = load_cjs_module(&ctx, &js);
        assert!(result.is_object());
    }

    #[test]
    fn test_load_cjs_module_module_dot_exports_object() {
        let tmp = tmp_dir();
        let js = tmp.path().join("mod.js");
        fs::write(&js, "module.exports = { a: 1, b: 2 };").expect("write");
        let ctx = crate::runtime::JSContext::get();
        let result = load_cjs_module(&ctx, &js);
        assert!(result.is_object());
    }

    #[test]
    fn test_load_cjs_module_module_dot_exports_primitive() {
        let tmp = tmp_dir();
        let js = tmp.path().join("mod.js");
        fs::write(&js, "module.exports = 42;").expect("write");
        let ctx = crate::runtime::JSContext::get();
        let result = load_cjs_module(&ctx, &js);
        assert!(result.is_number());
    }

    #[test]
    fn test_load_cjs_module_module_dot_exports_string() {
        let tmp = tmp_dir();
        let js = tmp.path().join("mod.js");
        fs::write(&js, "module.exports = 'hello';").expect("write");
        let ctx = crate::runtime::JSContext::get();
        let result = load_cjs_module(&ctx, &js);
        assert!(result.is_string());
    }
}
