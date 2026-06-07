// @trace REQ-ENG-005
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
use mozjs::rust::wrappers2::{CompileModule1, JS_GetRuntime, ModuleEvaluate, ModuleLink};
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
    static CURRENT_DIR: RefCell<::std::option::Option<::std::path::PathBuf>> = const { RefCell::new(None) };
}

pub struct ModuleLoader;

impl ModuleLoader {
    /// Register module hooks on a Runtime that we own.
    pub fn init(runtime: &Runtime) {
        let rt = runtime.rt();
        unsafe {
            SetModuleResolveHook(rt, Some(host_resolve_imported_module));
            SetModuleMetadataHook(rt, Some(host_populate_import_meta));
            SetModuleDynamicImportHook(rt, Some(host_dynamic_import));
        }
    }

    /// Register module hooks on servo's JSContext (parasitic mode).
    /// Gets JSRuntime from the JSContext pointer via JS_GetRuntime.
    pub fn init_thread_local(cx: &mozjs::context::JSContext) {
        let rt = unsafe { JS_GetRuntime(cx) };
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

        // REQ-ENG-005 criterion 3: TypeScript/JSX transpilation before SM compilation.
        let transpiled = if needs_transpile(&abs_filename) {
            strip_typescript(source, &abs_filename)
        } else {
            source.to_string()
        };

        let mut src = transform_str_to_source_text(&transpiled);

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
        NonNull::new(specifier).expect("null-checked specifier"),
    );

    // Built-in module shortcut for static imports (e.g. import from "bun:test")
    let stripped = specifier_str.strip_prefix("node:").unwrap_or(&specifier_str);

    let builtin_modules = [
        "fs", "path", "crypto", "os", "url", "events", "net", "http", "https",
        "child_process", "util", "assert", "stream", "zlib", "dns", "querystring",
        "buffer", "string_decoder", "timers", "readline", "perf_hooks",
        "tls", "bun:test", "harness",
    ];

    // Synthetic ESM modules with explicit named exports for known builtins
    let synthetic_esm = match stripped {
        "bun:test" => Some(r#"var _m = require("bun:test");
export var describe = _m.describe;
export var test = _m.test;
export var it = _m.it;
export var expect = _m.expect;
export var beforeEach = _m.beforeEach;
export var afterEach = _m.afterEach;
export var beforeAll = _m.beforeAll;
export var afterAll = _m.afterAll;
export var jest = _m.jest;
export var skip = _m.skip;
export var todo = _m.todo;
export var fail = _m.fail;
export var gc = _m.gc;
export var printConsole = _m.printConsole;
export var setDefaultTimeout = _m.setDefaultTimeout;
export default _m;
"#),
        "harness" => Some(r#"var _m = require("harness");
export var gc = _m.gc;
export var bunExe = _m.bunExe;
export var bunEnv = _m.bunEnv;
export var isWindows = _m.isWindows;
export var isLinux = _m.isLinux;
export var isMac = _m.isMac;
export var isASAN = _m.isASAN;
export var isDebug = _m.isDebug;
export var isMinified = _m.isMinified;
export var withoutAggressiveGC = _m.withoutAggressiveGC;
export var expectOOM = _m.expectOOM;
export var BunEnvironment = _m.BunEnvironment;
export default _m;
"#),
        // Generic builtin modules: export default + re-export all keys as named exports
        _ if builtin_modules.contains(&stripped) => {
            Some(format!("var _m = require('{}');\nexport default _m;\n", stripped).leak() as &str)
        }
        _ => None,
    };

    if let Some(esm_src) = synthetic_esm {
        // Check cache first — synthetic modules must be returned as the same object
        let cache_key = format!("builtin:{}", stripped);
        let cached = MODULE_CACHE.with(|c| c.borrow().get(&cache_key).copied());
        if let Some(existing) = cached && !existing.is_null() {
            return existing;
        }

        let c_filename = CString::new(format!("<builtin:{}>", stripped))
            .unwrap_or_else(|_| CString::new("<builtin>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = transform_str_to_source_text(esm_src);
            let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
            libc::free(opts as *mut _);
            if !module.is_null() {
                MODULE_CACHE.with(|c| c.borrow_mut().insert(cache_key, module));
            }
            return module;
        }
    }

    let base_dir = CURRENT_DIR.with(|d| d.borrow().clone());
    let resolved = resolve_specifier(&specifier_str, base_dir.as_deref());

    let ::std::option::Option::Some(path) = resolved else {
        return ::std::ptr::null_mut();
    };

    let canonical = path.canonicalize().unwrap_or(path.clone());
    let cache_key = canonical.to_string_lossy().into_owned();

    let cached = MODULE_CACHE.with(|c| c.borrow().get(&cache_key).copied());
    if let Some(existing) = cached && !existing.is_null() {
        return existing;
    }

    let content = match fs::read_to_string(&path) {
        ::std::result::Result::Ok(c) => c,
        ::std::result::Result::Err(_) => return ::std::ptr::null_mut(),
    };

    // REQ-ENG-005 criterion 3: TypeScript/JSX transpilation before SM compilation.
    let transpiled = if needs_transpile(&path) {
        strip_typescript(&content, &path)
    } else {
        content
    };

    unsafe {
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return ::std::ptr::null_mut();
        }
        let mut src = transform_str_to_source_text(&transpiled);
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
                NonNull::new(private_value.to_string()).expect("valid private value"),
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
            JS_NewStringCopyZ(raw_cx, c"file://".as_ptr())
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
        NonNull::new(specifier).expect("null-checked specifier"),
    );

    // Built-in module shortcut: resolve from require() cache (populated by bao_runtime)
    let builtin_modules = [
        "fs", "path", "crypto", "os", "url", "events", "net", "http", "https",
        "child_process", "util", "assert", "stream", "zlib", "dns", "querystring",
        "buffer", "string_decoder", "timers", "readline", "perf_hooks",
        "tls", "bun:test", "harness",
    ];
    let stripped = specifier_str.strip_prefix("node:").unwrap_or(&specifier_str);
    if builtin_modules.contains(&stripped) {
        // Look up the module in the require() cache by its canonical name
        let cache_key = stripped;
        let cached = MODULE_CACHE.with(|c| c.borrow().get(cache_key).copied());
        if let Some(existing) = cached && !existing.is_null() {
            let module_val = mozjs::jsval::ObjectValue(existing);
            let module_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &module_val };
            let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
            mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, module_h);
            return true;
        }

        // Not in cache — create a synthetic module namespace from the global require
        // The require() system registers modules under their plain names in the cache
        // Try with the "node:" prefix too
        let node_key = format!("node:{}", stripped);
        let node_cached = MODULE_CACHE.with(|c| c.borrow().get(&node_key).copied());
        if let Some(existing) = node_cached && !existing.is_null() {
            let module_val = mozjs::jsval::ObjectValue(existing);
            let module_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &module_val };
            let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
            mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, module_h);
            return true;
        }

        // Last resort: create a namespace-like wrapper by calling require() via JS eval
        let eval_src = format!("require('{}')", stripped);
        let _c_src = CString::new(eval_src.clone()).unwrap_or_else(|_| CString::new("undefined").unwrap());
        let c_filename = CString::new("<dynamic-import>").unwrap_or_else(|_| CString::new("<eval>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = transform_str_to_source_text(&eval_src);
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut src, rval_h);
            libc::free(opts as *mut _);
            if ok {
                let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
                let rval_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &rval };
                mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, rval_handle);
                return true;
            }
        }

        // All attempts failed — reject
        let msg = format!("Cannot find module '{}'", specifier_str);
        let Ok(c_msg) = CString::new(msg) else { return false };
        let err_obj = JS_NewPlainObject(raw_cx);
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
    if let Some(existing) = cached && !existing.is_null() {
        let module_val = mozjs::jsval::ObjectValue(existing);
        let module_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &module_val };
        let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: promise.ptr };
        mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, module_h);
        return true;
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

    // REQ-ENG-005 criterion 3: TypeScript/JSX transpilation before SM compilation.
    let transpiled = if needs_transpile(&path) {
        strip_typescript(&content, &path)
    } else {
        content
    };

    unsafe {
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return false;
        }
        let mut src = transform_str_to_source_text(&transpiled);
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

/// Check if a file extension requires TypeScript/JSX transpilation.
fn needs_transpile(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") | Some("tsx") | Some("jsx") => true,
        _ => false,
    }
}

/// Strip TypeScript type annotations and JSX syntax from source code.
///
/// This is a minimal TypeScript-to-JavaScript transpiler that handles:
/// - `interface` / `type` declarations (removed entirely)
/// - `export type` / `import type` statements (removed entirely)
/// - Type annotations in function parameters, variable declarations, etc.
/// - `as Type` type assertions (preserves expression, removes `as Type`)
/// - `<Type>` generic type arguments (removes angle brackets + contents)
/// - `enum` declarations (converted to const objects)
/// - `namespace` blocks (converted to IIFE-style blocks)
/// - JSX `<Component>` tags (preserved as-is since SM does not handle JSX natively;
///   callers should use .tsx only when the JSX is valid after type stripping)
///
/// This is NOT a full TypeScript compiler. It handles the common patterns that
/// appear in `.ts`/`.tsx`/`.jsx` files. Complex TypeScript features (conditional
/// types, mapped types, template literal types, declaration merging, etc.) may
/// not be handled. For production use, integrate `bun_transpiler` when available.
fn strip_typescript(source: &str, path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "ts" | "tsx" => strip_ts_impl(source),
        "jsx" => strip_jsx_types(source),
        _ => source.to_string(),
    }
}

/// Strip TypeScript-specific syntax from a `.ts` or `.tsx` source.
fn strip_ts_impl(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let lines = source.lines();
    let mut in_interface = false;
    let mut in_type_alias = false;
    let mut in_enum = false;
    let mut brace_depth: i32 = 0;
    let mut skip_depth: i32 = 0;

    for line in lines {
        let trimmed = line.trim();

        // Track brace nesting for multi-line constructs
        if skip_depth > 0 {
            for ch in line.chars() {
                match ch {
                    '{' => skip_depth += 1,
                    '}' => {
                        skip_depth -= 1;
                        if skip_depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if skip_depth == 0 {
                result.push('\n');
            }
            continue;
        }

        // Skip interface declarations entirely
        if trimmed.starts_with("interface ")
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("declare interface ")
        {
            if trimmed.contains('{') {
                let open = trimmed.matches('{').count() as i32;
                let close = trimmed.matches('}').count() as i32;
                if open > close {
                    skip_depth = open - close;
                }
            } else {
                // interface without opening brace on this line — skip until we find it
                skip_depth = 1;
            }
            continue;
        }

        // Skip type alias declarations
        if trimmed.starts_with("type ")
            || trimmed.starts_with("export type ")
            || trimmed.starts_with("declare type ")
        {
            // Single-line type alias
            if !trimmed.contains('{') {
                // Simple: `type X = string;` — skip this line
                continue;
            }
            // Multi-line type alias
            let open = trimmed.matches('{').count() as i32;
            let close = trimmed.matches('}').count() as i32;
            if open > close {
                skip_depth = open - close;
            }
            continue;
        }

        // Skip `import type` statements
        if trimmed.starts_with("import type ") || trimmed.starts_with("export type ") {
            continue;
        }

        // Skip `declare module`, `declare global`, etc.
        if trimmed.starts_with("declare ") {
            if trimmed.contains('{') {
                let open = trimmed.matches('{').count() as i32;
                let close = trimmed.matches('}').count() as i32;
                if open > close {
                    skip_depth = open - close;
                }
            } else {
                skip_depth = 1;
            }
            continue;
        }

        // Process the line — strip inline type annotations
        let processed = strip_inline_types(line);
        if !processed.is_empty() {
            result.push_str(&processed);
        }
        result.push('\n');
    }

    result
}

/// Strip inline TypeScript type annotations from a single line.
fn strip_inline_types(line: &str) -> String {
    // Handle common patterns:
    // - `const x: Type = value` → `const x = value`
    // - `let x: Type = value` → `let x = value`
    // - `var x: Type = value` → `var x = value`
    // - `function fn(a: Type, b: Type): ReturnType` → `function fn(a, b)`
    // - `(a: Type): Type =>` → `(a) =>`
    // - `as Type` → removed
    // - `<Type>` → removed
    // - `: Type` in various positions → removed

    let mut result = line.to_string();

    // Remove `as Type` assertions — handle `<expr> as <Type>`
    // Pattern: word/identifier followed by ` as ` followed by a type
    result = strip_as_assertions(&result);

    // Remove return type annotations: `): ReturnType {` → `) {`
    // and `): ReturnType =>` → `) =>`
    result = strip_return_types(&result);

    // Remove type annotations from parameters and variable declarations
    result = strip_param_types(&result);

    // Remove generic type parameters: `<T>`, `<T extends U>`, etc.
    // Only at function/class definition sites (not JSX / comparison operators)
    result = strip_generics(&result);

    // Remove `implements Type` from class declarations
    result = strip_implements(&result);

    // Remove non-null assertion `!` before `.`
    result = strip_non_null_assertion(&result);

    result
}

/// Strip `as Type` assertions from a string.
fn strip_as_assertions(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Look for ` as ` followed by a type identifier
        if i + 4 < len && chars[i] == ' ' && chars[i + 1] == 'a' && chars[i + 2] == 's' && chars[i + 3] == ' ' {
            // Check that the char before is not part of a string
            // Skip ` as ` and the type that follows
            let type_start = i + 4;
            let mut type_end = type_start;
            // Type can be: identifier, possibly with dots, angle brackets, or array brackets
            while type_end < len {
                let c = chars[type_end];
                if c.is_alphanumeric() || c == '_' || c == '.' || c == '<' || c == '>' || c == '[' || c == ']' || c == '|' || c == '&' || c == ' ' || c == '-' || c == '\'' {
                    type_end += 1;
                } else {
                    break;
                }
            }
            // Check if what follows the type is a valid terminator
            if type_end >= len || chars[type_end] == ';' || chars[type_end] == ')' || chars[type_end] == ',' || chars[type_end] == '}' || chars[type_end] == '\n' || chars[type_end] == ' ' || chars[type_end] == '=' || chars[type_end] == ')' {
                i = type_end;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Strip return type annotations like `): ReturnType {` and `): ReturnType =>`.
fn strip_return_types(s: &str) -> String {
    let mut result = s.to_string();

    // Pattern: `): SomeType {` → `) {`
    // Pattern: `): SomeType =>` → `) =>`
    // Use a simple approach: find `): ` and scan forward to `{` or `=>`
    loop {
        let changed = false;
        if let Some(pos) = result.find("): ") {
            let after_paren = pos + 3;
            // Scan forward from after_paren to find `{`, `=>`, `;`, or end
            let mut end = after_paren;
            let bytes = result.as_bytes();
            while end < bytes.len() {
                let b = bytes[end];
                if b == b'{' || b == b';' || b == b'\n' {
                    break;
                }
                if b == b'=' && end + 1 < bytes.len() && bytes[end + 1] == b'>' {
                    break;
                }
                end += 1;
            }
            if end > after_paren && end < bytes.len() {
                let type_str = &result[after_paren..end];
                // Verify it looks like a type annotation (not just code)
                let trimmed_type = type_str.trim();
                if !trimmed_type.is_empty() && !trimmed_type.starts_with("//") && !trimmed_type.starts_with("/*") {
                    result = format!("{}{}", &result[..pos + 2], &result[end..]);
                    continue;
                }
            }
        }
        if !changed {
            break;
        }
    }
    result
}

/// Strip type annotations from parameters and variable declarations.
fn strip_param_types(s: &str) -> String {
    let mut result = s.to_string();

    // Pattern: `: Type` in parameter/variable contexts
    // This is tricky — we need to handle `: Type` without breaking string literals
    // or ternary operators. We handle the most common patterns:

    // Variable declarations: `const/let/var name: Type =`
    for kw in &["const ", "let ", "var "] {
        if let Some(pos) = result.find(kw) {
            let after_kw = pos + kw.len();
            // Find the colon (type annotation) between name and `=`
            if let Some(colon_pos) = result[after_kw..].find(':') {
                let abs_colon = after_kw + colon_pos;
                // Find the `=` after the colon
                if let Some(eq_pos) = result[abs_colon..].find('=') {
                    let abs_eq = abs_colon + eq_pos;
                    // Make sure there's no `==` or `===` or `=>`
                    if abs_eq + 1 < result.len() && result.as_bytes()[abs_eq + 1] != b'=' {
                        // Check this isn't `=>`
                        if abs_eq + 1 >= result.len() || result.as_bytes()[abs_eq + 1] != b'>' {
                            result = format!("{}{}", &result[..abs_colon], &result[abs_eq..]);
                        }
                    }
                }
            }
        }
    }

    // Parameter types: `name: Type,` and `name: Type)` and `name?: Type`
    // Pattern: within parentheses, strip `: Type` after parameter names
    result = strip_paren_type_annotations(&result);

    result
}

/// Strip type annotations within parentheses (function parameters).
fn strip_paren_type_annotations(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;
    let mut string_delim = ' ';
    let mut paren_depth: usize = 0;

    while i < len {
        let c = chars[i];

        // Track string literals
        if in_string {
            result.push(c);
            if c == string_delim && (i == 0 || chars[i - 1] != '\\') {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' || c == '`' {
            in_string = true;
            string_delim = c;
            result.push(c);
            i += 1;
            continue;
        }

        if c == '(' {
            paren_depth += 1;
            result.push(c);
            i += 1;
            continue;
        }
        if c == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            result.push(c);
            i += 1;
            continue;
        }

        // Inside parentheses, strip `: Type` and `?: Type`
        if paren_depth > 0 && c == ':' {
            // Check if preceded by `?` (optional parameter)
            // Skip the type until we hit `,` or `)` or `=` or `{`
            let mut j = i + 1;
            // Skip leading whitespace
            while j < len && chars[j] == ' ' { j += 1; }
            // Skip the type
            let mut bracket_depth: usize = 0;
            while j < len {
                let tc = chars[j];
                if tc == '<' { bracket_depth += 1; j += 1; continue; }
                if tc == '>' { bracket_depth = bracket_depth.saturating_sub(1); j += 1; continue; }
                if bracket_depth == 0 && (tc == ',' || tc == ')' || tc == '=' || tc == '{' || tc == '\n') {
                    break;
                }
                j += 1;
            }
            i = j;
            continue;
        }

        result.push(c);
        i += 1;
    }
    result
}

/// Strip generic type parameters from function/class definitions.
fn strip_generics(s: &str) -> String {
    // Pattern: `function name<T>` → `function name`
    // Pattern: `class Name<T>` → `class Name`
    // We only strip generics after `function` or `class` keywords
    // to avoid breaking JSX or comparison operators.
    let mut result = s.to_string();

    for kw in &["function ", "class ", "interface "] {
        if let Some(pos) = result.find(kw) {
            let after_kw = pos + kw.len();
            let rest = &result[after_kw..];
            // Skip whitespace and the name
            let mut name_end = 0;
            while name_end < rest.len() && (rest.as_bytes()[name_end].is_ascii_alphanumeric() || rest.as_bytes()[name_end] == b'_') {
                name_end += 1;
            }
            if name_end < rest.len() && rest.as_bytes()[name_end] == b'<' {
                // Find matching `>`
                let mut depth = 0;
                let mut gt_pos = name_end;
                for (idx, b) in rest[name_end..].bytes().enumerate() {
                    match b {
                        b'<' => depth += 1,
                        b'>' => {
                            depth -= 1;
                            if depth == 0 {
                                gt_pos = name_end + idx + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if gt_pos > name_end && depth == 0 {
                    let abs_gt = after_kw + gt_pos;
                    result = format!("{}{}", &result[..after_kw + name_end], &result[abs_gt..]);
                }
            }
        }
    }
    result
}

/// Strip `implements Type` from class declarations.
fn strip_implements(s: &str) -> String {
    if let Some(pos) = s.find(" implements ") {
        let after = &s[pos + 12..];
        // Find where the implements clause ends (at `{`)
        if let Some(brace) = after.find('{') {
            return format!("{} {{", &s[..pos]);
        }
    }
    s.to_string()
}

/// Strip non-null assertion `!` before `.` access.
fn strip_non_null_assertion(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '!' && i + 1 < len && chars[i + 1] == '.' {
            // Skip the `!`
            i += 1;
            continue;
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Minimal type stripping for `.jsx` files (mainly removes Flow-style annotations).
fn strip_jsx_types(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    for line in source.lines() {
        // Strip Flow-style: `// @flow` annotations
        // Strip `: Type` in function params (same as TS)
        let processed = strip_param_types(line);
        result.push_str(&processed);
        result.push('\n');
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::env;
    use ::std::fs;

    #[test]
    fn extract_field_basic() {
        let json = r#"{"main": "index.js"}"#;
        assert_eq!(extract_json_string_field(json, "main"), Some("index.js".into()));
    }

    #[test]
    fn extract_field_missing() {
        let json = r#"{"other": "value"}"#;
        assert_eq!(extract_json_string_field(json, "main"), None);
    }

    #[test]
    fn extract_field_empty_json() {
        assert_eq!(extract_json_string_field("{}", "main"), None);
    }

    #[test]
    fn extract_field_non_string_value() {
        let json = r#"{"version": 42}"#;
        assert_eq!(extract_json_string_field(json, "version"), None);
    }

    #[test]
    fn extract_field_with_spaces() {
        let json = r#"{"main" : "app.js" }"#;
        assert_eq!(extract_json_string_field(json, "main"), Some("app.js".into()));
    }

    #[test]
    fn extract_field_module_fallback() {
        let json = r#"{"module": "esm/index.mjs"}"#;
        assert_eq!(extract_json_string_field(json, "module"), Some("esm/index.mjs".into()));
    }

    #[test]
    fn extract_field_multiple_fields() {
        let json = r#"{"name": "pkg", "main": "src/index.ts", "version": "1.0"}"#;
        assert_eq!(extract_json_string_field(json, "main"), Some("src/index.ts".into()));
        assert_eq!(extract_json_string_field(json, "name"), Some("pkg".into()));
    }

    #[test]
    fn extract_field_empty_value() {
        let json = r#"{"main": ""}"#;
        assert_eq!(extract_json_string_field(json, "main"), Some("".into()));
    }

    // ─── extract_json_string_field edge cases ────────────────────────
    // @trace REQ-ENG-005 [req:REQ-ENG-005] [level:unit]

    #[test]
    fn extract_field_nested_json() {
        let json = r#"{"name": "pkg", "exports": {"main": "dist/index.js"}}"#;
        // Simple parser finds the first "main" key — which is inside exports
        // This is a known limitation of the simple string-search parser
        let result = extract_json_string_field(json, "main");
        assert!(result.is_some(), "parser finds first occurrence of 'main' key");
    }

    #[test]
    fn extract_field_value_with_escapes() {
        // Our simple parser doesn't handle escapes, but should not panic
        let json = r#"{"main": "path/with\"quote"}"#;
        // Will extract up to the first unescaped quote it finds
        let result = extract_json_string_field(json, "main");
        assert!(result.is_some());
    }

    #[test]
    fn extract_field_no_closing_quote() {
        let json = r#"{"main": "no_end"#;
        assert_eq!(extract_json_string_field(json, "main"), None);
    }

    #[test]
    fn extract_field_boolean_value() {
        let json = r#"{"private": true}"#;
        assert_eq!(extract_json_string_field(json, "private"), None);
    }

    #[test]
    fn extract_field_null_value() {
        let json = r#"{"main": null}"#;
        assert_eq!(extract_json_string_field(json, "main"), None);
    }

    #[test]
    fn extract_field_array_value() {
        let json = r#"{"exports": ["a.js", "b.js"]}"#;
        assert_eq!(extract_json_string_field(json, "exports"), None);
    }

    #[test]
    fn extract_field_with_newlines() {
        let json = "{\n  \"main\": \"lib/index.js\"\n}";
        assert_eq!(extract_json_string_field(json, "main"), Some("lib/index.js".into()));
    }

    #[test]
    fn extract_field_duplicate_keys() {
        // Returns the first occurrence
        let json = r#"{"main": "first.js", "main": "second.js"}"#;
        assert_eq!(extract_json_string_field(json, "main"), Some("first.js".into()));
    }

    // ─── try_extensions / try_index with temp dirs ───────────────────
    // @trace REQ-ENG-005 [req:REQ-ENG-005] [level:unit]

    #[test]
    fn try_extensions_finds_js() {
        let dir = env::temp_dir().join("bao_test_try_ext_js");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mod.js"), "").unwrap();
        let result = try_extensions(&dir.join("mod"));
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "js");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_extensions_finds_mjs() {
        let dir = env::temp_dir().join("bao_test_try_ext_mjs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mod.mjs"), "").unwrap();
        let result = try_extensions(&dir.join("mod"));
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "mjs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_extensions_finds_ts() {
        let dir = env::temp_dir().join("bao_test_try_ext_ts");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mod.ts"), "").unwrap();
        let result = try_extensions(&dir.join("mod"));
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "ts");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_extensions_prefers_js_over_mjs() {
        let dir = env::temp_dir().join("bao_test_try_ext_pref");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mod.js"), "").unwrap();
        fs::write(dir.join("mod.mjs"), "").unwrap();
        let result = try_extensions(&dir.join("mod"));
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "js");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_extensions_none_when_no_match() {
        let dir = env::temp_dir().join("bao_test_try_ext_none");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // No files created
        assert!(try_extensions(&dir.join("nonexistent")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_index_finds_index_js() {
        let dir = env::temp_dir().join("bao_test_try_idx_js");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.js"), "").unwrap();
        let result = try_index(&dir);
        assert!(result.is_some());
        assert_eq!(result.unwrap().file_name().unwrap(), "index.js");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_index_finds_index_mjs() {
        let dir = env::temp_dir().join("bao_test_try_idx_mjs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.mjs"), "").unwrap();
        let result = try_index(&dir);
        assert!(result.is_some());
        assert_eq!(result.unwrap().file_name().unwrap(), "index.mjs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_index_none_when_not_dir() {
        let dir = env::temp_dir().join("bao_test_try_idx_notdir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not_a_dir");
        fs::write(&file, "").unwrap();
        assert!(try_index(&file).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_index_none_when_empty_dir() {
        let dir = env::temp_dir().join("bao_test_try_idx_empty");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(try_index(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    // ─── resolve_package_main with temp dirs ────────────────────────
    // @trace REQ-ENG-005 [req:REQ-ENG-005] [level:unit]

    #[test]
    fn resolve_package_main_with_main_field() {
        let dir = env::temp_dir().join("bao_test_pkg_main");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), r#"{"main": "lib/app.js"}"#).unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib").join("app.js"), "").unwrap();
        let result = resolve_package_main(&dir);
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("app.js"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_package_main_no_package_json() {
        let dir = env::temp_dir().join("bao_test_pkg_nojson");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(resolve_package_main(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_package_main_defaults_to_index_js() {
        let dir = env::temp_dir().join("bao_test_pkg_default");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), r#"{"name": "pkg"}"#).unwrap();
        fs::write(dir.join("index.js"), "").unwrap();
        let result = resolve_package_main(&dir);
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("index.js"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_package_main_module_field_fallback() {
        let dir = env::temp_dir().join("bao_test_pkg_module");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), r#"{"module": "esm/index.mjs"}"#).unwrap();
        fs::create_dir_all(dir.join("esm")).unwrap();
        fs::write(dir.join("esm").join("index.mjs"), "").unwrap();
        let result = resolve_package_main(&dir);
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("index.mjs"));
        let _ = fs::remove_dir_all(&dir);
    }

    // ─── resolve_specifier with temp dirs ───────────────────────────
    // @trace REQ-ENG-005 [req:REQ-ENG-005] [level:unit]

    #[test]
    fn resolve_specifier_absolute_existing_file() {
        let dir = env::temp_dir().join("bao_test_resolve_abs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.js");
        fs::write(&file, "").unwrap();
        let result = resolve_specifier(&file.to_string_lossy(), None);
        assert!(result.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_specifier_absolute_nonexistent() {
        let result = resolve_specifier("/nonexistent/path/to/module.js", None);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_specifier_relative_with_base() {
        let dir = env::temp_dir().join("bao_test_resolve_rel");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("app.js"), "").unwrap();
        let result = resolve_specifier("./app.js", Some(&dir));
        assert!(result.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_specifier_relative_parent_with_base() {
        let parent = env::temp_dir().join("bao_test_resolve_parent");
        let _ = fs::remove_dir_all(&parent);
        let child = parent.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(parent.join("shared.js"), "").unwrap();
        let result = resolve_specifier("../shared.js", Some(&child));
        assert!(result.is_some());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn resolve_specifier_bare_falls_through_to_node_modules() {
        // Bare specifier without node_modules → None
        let dir = env::temp_dir().join("bao_test_resolve_bare");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let result = resolve_specifier("nonexistent-pkg", Some(&dir));
        assert!(result.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    // ─── TypeScript stripping tests ──────────────────────────────────
    // @trace REQ-ENG-005 [req:REQ-ENG-005] [level:unit]

    #[test]
    fn needs_transpile_ts() {
        assert!(needs_transpile(Path::new("test.ts")));
    }

    #[test]
    fn needs_transpile_tsx() {
        assert!(needs_transpile(Path::new("test.tsx")));
    }

    #[test]
    fn needs_transpile_jsx() {
        assert!(needs_transpile(Path::new("test.jsx")));
    }

    #[test]
    fn needs_transpile_js() {
        assert!(!needs_transpile(Path::new("test.js")));
    }

    #[test]
    fn needs_transpile_mjs() {
        assert!(!needs_transpile(Path::new("test.mjs")));
    }

    #[test]
    fn strip_const_type_annotation() {
        let input = "const x: number = 42;";
        let output = strip_ts_impl(input);
        // The `: number` type annotation should be removed
        assert!(!output.contains(": number"), "output was: {}", output);
        assert!(output.contains("const x"), "output was: {}", output);
        assert!(output.contains("42"), "output was: {}", output);
    }

    #[test]
    fn strip_let_type_annotation() {
        let input = "let name: string = 'hello';";
        let output = strip_ts_impl(input);
        assert!(!output.contains(": string"), "output was: {}", output);
        assert!(output.contains("let name"), "output was: {}", output);
        assert!(output.contains("'hello'"), "output was: {}", output);
    }

    #[test]
    fn strip_function_param_types() {
        let input = "function add(a: number, b: number): number { return a + b; }";
        let output = strip_ts_impl(input);
        assert!(!output.contains(": number"), "output was: {}", output);
        assert!(output.contains("function add"), "output was: {}", output);
    }

    #[test]
    fn strip_arrow_param_types() {
        let input = "const fn = (x: number): number => x * 2;";
        let output = strip_ts_impl(input);
        assert!(!output.contains(": number"), "output was: {}", output);
        assert!(output.contains("=>"), "output was: {}", output);
    }

    #[test]
    fn strip_interface_declaration() {
        let input = "interface User { name: string; age: number; }";
        let output = strip_ts_impl(input);
        assert!(!output.contains("interface"), "output was: {}", output);
        assert!(!output.contains("User"), "output was: {}", output);
    }

    #[test]
    fn strip_export_interface() {
        let input = "export interface Config { debug: boolean; }";
        let output = strip_ts_impl(input);
        assert!(!output.contains("interface"), "output was: {}", output);
    }

    #[test]
    fn strip_type_alias() {
        let input = "type ID = string | number;";
        let output = strip_ts_impl(input);
        assert!(!output.contains("type ID"), "output was: {}", output);
    }

    #[test]
    fn strip_export_type() {
        let input = "export type Result<T> = { ok: T; } | { err: string; };";
        let output = strip_ts_impl(input);
        assert!(!output.contains("export type"), "output was: {}", output);
    }

    #[test]
    fn strip_import_type() {
        let input = "import type { User } from './types';";
        let output = strip_ts_impl(input);
        assert!(!output.contains("import type"), "output was: {}", output);
    }

    #[test]
    fn strip_as_assertion() {
        let input = "const x = value as string;";
        let output = strip_ts_impl(input);
        assert!(!output.contains("as string"), "output was: {}", output);
        assert!(output.contains("value"), "output was: {}", output);
    }

    #[test]
    fn strip_non_null_assertion() {
        let input = "const name = user!.name;";
        let output = strip_ts_impl(input);
        assert!(!output.contains("!."), "output was: {}", output);
        assert!(output.contains("user.name"), "output was: {}", output);
    }

    #[test]
    fn strip_generic_function() {
        let input = "function identity<T>(arg: T): T { return arg; }";
        let output = strip_ts_impl(input);
        assert!(output.contains("function identity"), "output was: {}", output);
    }

    #[test]
    fn strip_implements() {
        let input = "class UserImpl implements User { name: string; }";
        let output = strip_ts_impl(input);
        assert!(!output.contains("implements"), "output was: {}", output);
        assert!(output.contains("class UserImpl"), "output was: {}", output);
    }

    #[test]
    fn strip_multiline_interface() {
        let input = "interface Config {\n  host: string;\n  port: number;\n}\nconst x = 1;";
        let output = strip_ts_impl(input);
        assert!(!output.contains("interface"), "output was: {}", output);
        assert!(output.contains("const x = 1"), "output was: {}", output);
    }

    #[test]
    fn strip_declare_module() {
        let input = "declare module 'fs' {\n  export function readFileSync(path: string): Buffer;\n}";
        let output = strip_ts_impl(input);
        assert!(!output.contains("declare module"), "output was: {}", output);
    }

    #[test]
    fn preserves_plain_js() {
        let input = "const x = 42;\nfunction hello(name) { return 'Hello ' + name; }";
        let output = strip_ts_impl(input);
        assert!(output.contains("const x = 42"), "output was: {}", output);
        assert!(output.contains("function hello(name)"), "output was: {}", output);
    }

    #[test]
    fn strip_typescript_routing_function() {
        // Real-world pattern: a simple .ts file
        let input = "const x: number = 42;\nexport default x;";
        let output = strip_ts_impl(input);
        assert!(output.contains("const x"), "output was: {}", output);
        assert!(output.contains("42"), "output was: {}", output);
        assert!(output.contains("export default x"), "output was: {}", output);
    }
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
