// REQ-ENG-003: ESM/CJS module resolver and loader
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ffi::CString;
use ::std::fs;
use ::std::path::{Path, PathBuf};
use ::std::ptr::NonNull;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{CompileModule1, ModuleEvaluate, ModuleLink};
use mozjs::rust::{
    transform_str_to_source_text, CompileOptionsWrapper, RealmOptions, Runtime,
    SIMPLE_GLOBAL_CLASS,
};

use crate::context::{GlobalSetupFn, PostEvalHook};
use crate::error::JsError;
use crate::job_queue::JobQueue;
use crate::value::{JsValue, jsval_to_jsvalue};

thread_local! {
    static MODULE_CACHE: RefCell<HashMap<::std::string::String, *mut JSObject>> = RefCell::new(HashMap::new());
    static CURRENT_DIR: RefCell<::std::option::Option<::std::path::PathBuf>> = RefCell::new(None);
}

pub struct ModuleLoader;

impl ModuleLoader {
    pub fn init(runtime: &Runtime) {
        let rt = runtime.rt();
        unsafe {
            SetModuleResolveHook(rt, Some(host_resolve_imported_module));
            SetModuleMetadataHook(rt, Some(host_populate_import_meta));
            SetModuleDynamicImportHook(rt, Some(host_dynamic_import));
        }
    }

    pub fn eval_module(
        cx: &mut mozjs::context::JSContext,
        source: &str,
        filename: &str,
        global_setup: Option<GlobalSetupFn>,
        post_eval_hook: Option<PostEvalHook>,
    ) -> ::std::result::Result<JsValue, JsError> {
        let abs_filename = if Path::new(filename).is_absolute() {
            PathBuf::from(filename)
        } else {
            ::std::env::current_dir().unwrap_or_default().join(filename)
        };
        let base_dir = abs_filename.parent().map(|p| p.to_path_buf())
            .or_else(|| ::std::env::current_dir().ok());

        CURRENT_DIR.with(|d| *d.borrow_mut() = base_dir.clone());

        let options = RealmOptions::default();

        rooted!(&in(cx) let global = unsafe {
            mozjs::rust::wrappers2::JS_NewGlobalObject(
                cx,
                &SIMPLE_GLOBAL_CLASS,
                ::std::ptr::null_mut(),
                OnNewGlobalHookOption::FireOnNewGlobalHook,
                &*options,
            )
        });

        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        crate::host_fn::install_console(realm_cx, global.handle());
        if let Some(setup) = global_setup {
            unsafe { setup(realm_cx, global.handle()) };
        }

        let c_filename = CString::new(filename)
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

        let mut src = transform_str_to_source_text(source);

        rooted!(&in(realm_cx) let mut module_obj = unsafe {
            CompileModule1(realm_cx, compile_opts.ptr, &mut src)
        });

        if module_obj.get().is_null() {
            return ::std::result::Result::Err(JsError {
                message: "Failed to compile module".into(),
                filename: filename.into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        if !unsafe { ModuleLink(realm_cx, module_obj.handle()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        if !unsafe { ModuleEvaluate(realm_cx, module_obj.handle(), rval.handle_mut()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        JobQueue::drain(realm_cx);

        if let Some(hook) = post_eval_hook {
            for _ in 0..1000 {
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
                hook(realm_cx);
                JobQueue::drain(realm_cx);
            }
        }

        ::std::result::Result::Ok(unsafe {
            jsval_to_jsvalue(realm_cx.raw_cx_no_gc(), rval.get())
        })
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_resolve_imported_module(
    raw_cx: *mut JSContext,
    _referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
) -> *mut JSObject {
    let specifier = unsafe { GetModuleRequestSpecifier(raw_cx, module_request) };
    if specifier.is_null() {
        return ::std::ptr::null_mut();
    }

    let specifier_str = mozjs::conversions::jsstr_to_string(
        raw_cx,
        NonNull::new(specifier).unwrap(),
    );

    let base_dir = CURRENT_DIR.with(|d| d.borrow().clone());
    let resolved = resolve_specifier(&specifier_str, base_dir.as_deref());

    let ::std::option::Option::Some(path) = resolved else {
        return ::std::ptr::null_mut();
    };

    let canonical = path.canonicalize().unwrap_or(path.clone());
    let cache_key = canonical.to_string_lossy().into_owned();

    let cached = MODULE_CACHE.with(|c| c.borrow().get(&cache_key).copied());
    if let Some(existing) = cached {
        if !existing.is_null() {
            return existing;
        }
    }

    let content = match fs::read_to_string(&path) {
        ::std::result::Result::Ok(c) => c,
        ::std::result::Result::Err(_) => return ::std::ptr::null_mut(),
    };

    unsafe {
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return ::std::ptr::null_mut();
        }
        let mut src = transform_str_to_source_text(&content);
        let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        libc::free(opts as *mut _);
        if !module.is_null() {
            MODULE_CACHE.with(|c| c.borrow_mut().insert(cache_key, module));
        }
        module
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_populate_import_meta(
    raw_cx: *mut JSContext,
    private_value: Handle<Value>,
    meta_object: Handle<*mut JSObject>,
) -> bool {
    unsafe {
        let url_str = if private_value.is_string() {
            let specifier = mozjs::conversions::jsstr_to_string(
                raw_cx,
                NonNull::new(private_value.to_string()).unwrap(),
            );
            let resolved = if specifier.starts_with("file://") {
                specifier
            } else {
                let base_dir = CURRENT_DIR.with(|d| d.borrow().clone());
                let path = resolve_specifier(&specifier, base_dir.as_deref());
                match path {
                    Some(p) => format!("file://{}", p.to_string_lossy()),
                    None => format!("file://{}", specifier),
                }
            };
            let Ok(c_url) = CString::new(resolved.as_str()) else {
                return false;
            };
            JS_NewStringCopyZ(raw_cx, c_url.as_ptr())
        } else {
            JS_NewStringCopyZ(raw_cx, b"file://\0".as_ptr() as *const ::std::os::raw::c_char)
        };
        if url_str.is_null() {
            return false;
        }
        let val = mozjs::jsval::StringValue(&*url_str);
        let handle_val = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(raw_cx, meta_object, c"url".as_ptr(), handle_val, JSPROP_ENUMERATE as u32)
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_dynamic_import(
    raw_cx: *mut JSContext,
    _referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
    promise: Handle<*mut JSObject>,
) -> bool {
    let specifier = unsafe { GetModuleRequestSpecifier(raw_cx, module_request) };
    if specifier.is_null() {
        return false;
    }
    let specifier_str = mozjs::conversions::jsstr_to_string(
        raw_cx,
        NonNull::new(specifier).unwrap(),
    );

    let base_dir = CURRENT_DIR.with(|d| d.borrow().clone());
    let resolved = resolve_specifier(&specifier_str, base_dir.as_deref());

    let ::std::option::Option::Some(path) = resolved else {
        let msg = format!("Cannot find module '{}'", specifier_str);
        let Ok(c_msg) = CString::new(msg) else { return false };
        let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw_cx);
        if !err_obj.is_null() {
            let err_msg = JS_NewStringCopyZ(raw_cx, c_msg.as_ptr());
            if !err_msg.is_null() {
                let msg_val = mozjs::jsval::StringValue(&*err_msg);
                let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
                let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
                JS_SetProperty(raw_cx, err_h, c"message".as_ptr(), msg_h);
            }
            let err_val = mozjs::jsval::ObjectValue(err_obj);
            let err_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
            let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
            mozjs_sys::jsapi::JS::RejectPromise(raw_cx, promise_h, err_handle);
        }
        return true;
    };

    let canonical = path.canonicalize().unwrap_or(path.clone());
    let cache_key = canonical.to_string_lossy().into_owned();

    let cached = MODULE_CACHE.with(|c| c.borrow().get(&cache_key).copied());
    if let Some(existing) = cached {
        if !existing.is_null() {
            let module_val = mozjs::jsval::ObjectValue(existing);
            let module_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &module_val };
            let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
            mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, module_h);
            return true;
        }
    }

    let content = match fs::read_to_string(&path) {
        ::std::result::Result::Ok(c) => c,
        ::std::result::Result::Err(e) => {
            let msg = format!("Cannot read module '{}': {}", specifier_str, e);
            let Ok(c_msg) = CString::new(msg) else { return false };
            let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw_cx);
            if !err_obj.is_null() {
                let err_msg = JS_NewStringCopyZ(raw_cx, c_msg.as_ptr());
                if !err_msg.is_null() {
                    let msg_val = mozjs::jsval::StringValue(&*err_msg);
                    let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
                    let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
                    JS_SetProperty(raw_cx, err_h, c"message".as_ptr(), msg_h);
                }
                let err_val = mozjs::jsval::ObjectValue(err_obj);
                let err_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
                let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
                mozjs_sys::jsapi::JS::RejectPromise(raw_cx, promise_h, err_handle);
            }
            return true;
        }
    };

    unsafe {
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return false;
        }
        let mut src = transform_str_to_source_text(&content);
        let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        libc::free(opts as *mut _);
        if module.is_null() {
            return false;
        }

        MODULE_CACHE.with(|c| c.borrow_mut().insert(cache_key, module));

        let module_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &module };
        if !mozjs_sys::jsapi::JS::ModuleLink(raw_cx, module_h) {
            return false;
        }

        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        if !mozjs_sys::jsapi::JS::ModuleEvaluate(raw_cx, module_h, rval_h) {
            return false;
        }

        mozjs_sys::jsapi::js::RunJobs(raw_cx);

        let ns_obj = mozjs_sys::jsapi::JS::GetModuleNamespace(raw_cx, module_h);
        let ns_val = mozjs::jsval::ObjectValue(ns_obj);
        let ns_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ns_val };
        let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
        mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, ns_h);
    }
    true
}

fn resolve_specifier(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    let path = Path::new(specifier);

    // Absolute path
    if path.is_absolute() {
        if let Some(resolved) = try_extensions(path) {
            return Some(resolved);
        }
        if let Some(resolved) = try_index(path) {
            return Some(resolved);
        }
        if path.exists() {
            return Some(path.to_path_buf());
        }
        return None;
    }

    // Relative path (./ or ../) — resolve against base_dir
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let base = base_dir.unwrap_or_else(|| Path::new("."));
        let full_path = base.join(specifier);
        if let Some(resolved) = try_extensions(&full_path) {
            return Some(resolved);
        }
        if let Some(resolved) = try_index(&full_path) {
            return Some(resolved);
        }
        if full_path.exists() {
            return Some(full_path);
        }
        return None;
    }

    // Bare specifier → node_modules lookup from base_dir or CWD
    resolve_node_modules(specifier, base_dir)
}

fn try_extensions(path: &Path) -> ::std::option::Option<PathBuf> {
    for ext in [".js", ".mjs", ".ts", ".tsx", ".jsx"] {
        let candidate = PathBuf::from(format!("{}{}", path.display(), ext));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn try_index(dir: &Path) -> ::std::option::Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    for name in ["index.js", "index.mjs", "index.ts", "index.tsx"] {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
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
        let nm_dir = dir.join("node_modules");
        if nm_dir.is_dir() {
            let target = nm_dir.join(specifier);
            if let Some(resolved) = try_extensions(&target) {
                return Some(resolved);
            }
            if let Some(resolved) = try_index(&target) {
                return Some(resolved);
            }
            // Check package.json "main" field
            if let Some(resolved) = resolve_package_main(&target) {
                return Some(resolved);
            }
        }

        dir = dir.parent()?;
    }
}

fn resolve_package_main(pkg_dir: &Path) -> ::std::option::Option<PathBuf> {
    let pkg_json_path = pkg_dir.join("package.json");
    if !pkg_json_path.exists() {
        return None;
    }

    let content = ::std::fs::read_to_string(&pkg_json_path).ok()?;
    let main_field = extract_json_string_field(&content, "main")
        .or_else(|| extract_json_string_field(&content, "module"))
        .unwrap_or_else(|| "index.js".to_string());

    let main_path = pkg_dir.join(&main_field);
    if let Some(resolved) = try_extensions(&main_path) {
        return Some(resolved);
    }
    if main_path.exists() {
        return Some(main_path);
    }
    None
}

fn extract_json_string_field(json: &str, field: &str) -> ::std::option::Option<String> {
    let pattern = format!("\"{}\"", field);
    let start = json.find(&pattern)?;
    let after = &json[start + pattern.len()..];
    let colon_pos = after.find(':')?;
    let after_colon = &after[colon_pos + 1..];
    let trimmed = after_colon.trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let value_start = &trimmed[1..];
    let end = value_start.find('"')?;
    Some(value_start[..end].to_string())
}

fn extract_module_error(cx: &mut mozjs::context::JSContext) -> JsError {
    rooted!(&in(cx) let mut exn = UndefinedValue());
    if let ::std::option::Option::Some(info) = unsafe {
        mozjs::rust::error_info_from_exception_stack(cx.raw_cx_no_gc(), exn.handle_mut().into())
    } {
        JsError {
            message: info.message,
            filename: info.filename,
            line: info.line,
            column: info.col,
            stack: None,
        }
    } else {
        JsError {
            message: "Unknown module error".into(),
            filename: "<module>".into(),
            line: 0,
            column: 0,
            stack: None,
        }
    }
}
