// @trace REQ-ENG-005 [bug:BUG-ENG-365]
use ::std::cell::RefCell;
use ::std::collections::HashSet;
use ::std::ffi::CString;
use ::std::fs;
use ::std::path::{Path, PathBuf};
use ::std::ptr::NonNull;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    CompileModule1, GetPromiseState, IsPromiseObject, JS_GetRuntime, ModuleEvaluate, ModuleLink,
    ThrowOnModuleEvaluationFailure,
};
use mozjs::rust::{
    CompileOptionsWrapper, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS, transform_str_to_source_text,
};

use crate::error::JsError;
use crate::host_fn::install_console;
use crate::value::{JsValue, jsval_to_jsvalue};

/// Function pointer type for global setup during module evaluation.
///
/// Called after creating a new global object and installing console,
/// before module compilation begins.
pub type GlobalSetupFn =
    unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>);

/// Function pointer type for post-evaluation hooks.
///
/// Called repeatedly after module evaluation until it returns `false`.
/// Used for waiting on async operations to complete.
pub type PostEvalHook = fn(&mut mozjs::context::JSContext) -> bool;

/// Function pointer type for draining the JS job queue (microtask queue).
///
/// Registered by bao_engine at startup. bun_sm calls this after module evaluation
/// to flush pending promise microtasks. This callback pattern avoids bun_sm
/// depending on bao_engine (which would create a circular dependency).
pub type JobQueueDrainFn = fn(&mut mozjs::context::JSContext);

/// Function pointer type for external module specifier resolver.
/// When set via `set_resolver`, this takes priority over the built-in
/// `resolve_specifier` logic, allowing `bun_resolver::Resolver` to drive resolution.
pub type ResolverFn = fn(&str, Option<&Path>) -> Option<PathBuf>;

thread_local! {
    /// GC-safe module cache: stores cached module objects as properties on the JS global.
    /// We only track which keys are set (a HashSet of strings). The actual `*mut JSObject`
    /// pointers are managed by SpiderMonkey's GC via global object properties.
    /// Property name format: `__gc_mod_{cache_key}`.
    static MODULE_CACHE: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static CURRENT_DIR: RefCell<::std::option::Option<::std::path::PathBuf>> = const { RefCell::new(None) };
    /// Absolute filesystem path of the entry module (the file passed to
    /// `bao test path` / `bao run path`). Used by host_populate_import_meta
    /// to set `import.meta.main = true` on the entry module.
    /// @trace REQ-ENG-006 [entity:JSContext]
    static ENTRY_MODULE: RefCell<::std::option::Option<::std::path::PathBuf>> = const { RefCell::new(None) };
    static EXTERNAL_RESOLVER: RefCell<Option<ResolverFn>> = const { RefCell::new(None) };
    static JOB_QUEUE_DRAIN: RefCell<Option<JobQueueDrainFn>> = const { RefCell::new(None) };
}

/// Register the job queue drain callback.
///
/// Called by bao_engine during initialization to register its `JobQueue::drain`
/// function. This allows bun_sm to drain microtasks without depending on bao_engine.
pub fn set_job_queue_drain(drain_fn: JobQueueDrainFn) {
    JOB_QUEUE_DRAIN.with(|d| *d.borrow_mut() = Some(drain_fn));
}

/// Drain the JS job queue using the registered callback.
///
/// If no callback has been registered, this is a no-op (Phase 1 safe default).
fn drain_job_queue(cx: &mut mozjs::context::JSContext) {
    JOB_QUEUE_DRAIN.with(|d| {
        if let Some(drain) = *d.borrow() {
            drain(cx);
        }
    });
}

/// Format a module cache property name: `__gc_mod_{key}`.
fn mod_prop_name(key: &str) -> CString {
    CString::new(format!("__gc_mod_{}", key)).unwrap_or_default()
}

/// Store a module object in the GC-safe MODULE_CACHE.
fn module_cache_insert(cx: *mut JSContext, key: &str, obj: *mut JSObject) {
    if obj.is_null() {
        return;
    }
    let global = unsafe { CurrentGlobalOrNull(cx) };
    if global.is_null() {
        return;
    }
    rooted!(in(cx) let global_root = global);
    let prop_name = mod_prop_name(key);
    rooted!(in(cx) let obj_val = mozjs::jsval::ObjectValue(obj));
    unsafe {
        JS_DefineProperty(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
            obj_val.handle().into(),
            JSPROP_READONLY as u32,
        );
    }
    MODULE_CACHE.with(|c| c.borrow_mut().insert(key.to_string()));
}

/// Retrieve a module object from the GC-safe MODULE_CACHE.
fn module_cache_get(cx: *mut JSContext, key: &str) -> Option<*mut JSObject> {
    if !MODULE_CACHE.with(|c| c.borrow().contains(key)) {
        return None;
    }
    let global = unsafe { CurrentGlobalOrNull(cx) };
    if global.is_null() {
        return None;
    }
    rooted!(in(cx) let global_root = global);
    let prop_name = mod_prop_name(key);
    let mut val = UndefinedValue();
    unsafe {
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            },
        );
    }
    if val.is_object() {
        Some(val.to_object())
    } else {
        None
    }
}

// ============================================================================
// BUG-ENG-365: SM Module API compliance helpers
//
// SM Module API contract (https://searchfox.org/mozilla-central/source/js/public/Modules.h):
//   1. Embedder MUST call JS::SetModulePrivate(module, urlValue) on every
//      module object returned by the resolve hook. The private value is the
//      module's identifying URL — it flows back into the resolve hook as
//      `referencingPrivate` and into the metadata hook as `privateValue`,
//      enabling correct import.meta.url and relative-import resolution.
//
//   2. Embedder MUST call JS::FinishDynamicModuleImport() to complete a
//      dynamic import() — never ResolvePromise the inner import promise
//      directly. FinishDynamicModuleImport drives the SM-side state machine
//      (record, link, evaluate, capability) and resolves the user-facing
//      promise with the module namespace.
//
//   3. Top-level await is handled automatically by SM (hasTopLevelAwait →
//      ExecuteAsyncModule). Embedder does NOT call SetModuleTopLevelCapability
//      — that API does not exist in the public SM Module API surface.
//
//   4. CJS↔ESM interop: when ESM imports a CJS module, the resolve hook
//      wraps the CJS module.exports in a synthetic ESM whose default export
//      is module.exports and whose named exports are exposed via Object.keys
//      re-exports.
// ============================================================================

/// Percent-encode a path segment for use in a `file://` URL.
/// Encodes spaces, control chars, and unsafe ASCII per RFC 3986 unreserved set.
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        // Unreserved per RFC 3986: A-Z a-z 0-9 - _ . ~
        // Plus path-allowed: / (separator)
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/');
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{:02X}", b));
        }
    }
    out
}

/// Build a `file://` URL from an absolute path, applying RFC 3986 percent-encoding.
/// This is the canonical form for JS::SetModulePrivate per WHATWG URL spec.
fn path_to_file_url(path: &Path) -> String {
    let s = path.to_string_lossy();
    // Absolute paths already begin with '/'
    format!("file://{}", percent_encode_path(&s))
}

/// Create a JS string value holding `url`. Returns a Value the caller may
/// pass to `JS::SetModulePrivate`. Caller-managed lifetime — must be
/// consumed before any GC.
///
/// # Safety
/// Caller must hold a valid `cx` and the returned Value must be used
/// immediately (before any GC).
unsafe fn make_string_value(raw_cx: *mut JSContext, url: &str) -> Value {
    let c_url = CString::new(url).unwrap_or_else(|_| CString::new("file://").unwrap());
    let js_str = unsafe { JS_NewStringCopyZ(raw_cx, c_url.as_ptr()) };
    if js_str.is_null() {
        return UndefinedValue();
    }
    unsafe { mozjs::jsval::StringValue(&*js_str) }
}

/// Attach `module_private` (a file:// URL string) to a compiled module.
///
/// Spec: `JS::SetModulePrivate(module, value)`. The private value is stored
/// on the module's ScriptSourceObject and surfaces in:
///   - host_resolve_imported_module(referencingPrivate, ...) for sub-imports
///   - host_populate_import_meta(privateValue, metaObject) for import.meta
///
/// Without this call, import.meta.url is incorrect and relative imports from
/// within the module resolve against CWD rather than the module's own URL.
///
/// # Safety
/// Caller must ensure `module` is a valid compiled module object.
unsafe fn set_module_private(raw_cx: *mut JSContext, module: *mut JSObject, url: &str) {
    if module.is_null() {
        return;
    }
    let private_val = unsafe { make_string_value(raw_cx, url) };
    if private_val.is_undefined() {
        return;
    }
    unsafe {
        mozjs_sys::jsapi::JS::SetModulePrivate(module, &private_val as *const Value);
    }
}

/// Resolve a specifier against a referencing module's private value (URL).
///
/// If `referencing_private` is a file:// URL string, the base directory is
/// extracted from it. Otherwise returns None (caller falls back to CURRENT_DIR).
/// This makes relative imports (`./foo`, `../bar`) resolve relative to the
/// importing module's location — required by ECMAScript module semantics.
///
/// # Safety
/// Caller must hold a valid `cx`.
unsafe fn base_dir_from_private_cx(
    raw_cx: *mut JSContext,
    referencing_private: Handle<Value>,
) -> Option<PathBuf> {
    if !referencing_private.is_string() {
        return None;
    }
    let url_jsstr = unsafe { referencing_private.to_string() };
    let Some(jsstr) = NonNull::new(url_jsstr) else {
        return None;
    };
    let url = unsafe { mozjs::conversions::unsafe_jsstr_to_string(raw_cx, jsstr) };
    let path_str = url.strip_prefix("file://")?;
    let decoded = percent_decode_path(path_str);
    let path = PathBuf::from(decoded);
    path.parent().map(|p| p.to_path_buf())
}

/// Percent-decode a percent-encoded path component (inverse of [percent_encode_path]).
fn percent_decode_path(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_digit(bytes[i + 1]);
            let lo = hex_digit(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Inject an external resolver function (e.g. `bun_resolver::Resolver` bridge).
/// Subsequent calls to `resolve_specifier` will delegate to this function first,
/// falling back to the built-in logic only if it returns `None`.
pub fn set_resolver(resolver: ResolverFn) {
    EXTERNAL_RESOLVER.with(|r| *r.borrow_mut() = Some(resolver));
}

/// Try the external resolver if one is installed.
/// Returns `None` if no external resolver is set or it returns `None`.
pub fn try_external_resolve(specifier: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    EXTERNAL_RESOLVER.with(|r| {
        r.borrow()
            .and_then(|resolver| resolver(specifier, base_dir))
    })
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
        let base_dir = abs_filename
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| ::std::env::current_dir().ok());

        CURRENT_DIR.with(|d| *d.borrow_mut() = base_dir.clone());
        // @trace REQ-ENG-006 [entity:JSContext] — entry module path is used to
        // set import.meta.main = true on this module (Bun semantics).
        ENTRY_MODULE.with(|e| *e.borrow_mut() = Some(abs_filename.clone()));

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

        install_console(realm_cx, global.handle());
        if let Some(setup) = global_setup {
            unsafe { setup(realm_cx, global.handle()) };
        }

        let c_filename =
            CString::new(filename).unwrap_or_else(|_| CString::new("<module>").unwrap());
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
            // SM leaves a pending exception describing the parse error; pull
            // it out so the user-facing JsError carries the actual line:col.
            let mut detail = extract_module_error(realm_cx);
            if detail.line == 0 && detail.column == 0 {
                detail.message = format!("Failed to compile module: {}", detail.message);
            } else {
                detail.message = format!(
                    "Failed to compile module: {} ({}:{}:{})",
                    detail.message, detail.filename, detail.line, detail.column
                );
            }
            return ::std::result::Result::Err(detail);
        }

        // BUG-ENG-365: attach module private value (file:// URL) BEFORE linking
        // so that host_resolve_imported_module receives a non-undefined
        // referencingPrivate and host_populate_import_meta can populate
        // import.meta.url correctly.
        let entry_url = path_to_file_url(&abs_filename);
        unsafe {
            set_module_private(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &entry_url,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        if !unsafe { ModuleLink(realm_cx, module_obj.handle()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        if !unsafe { ModuleEvaluate(realm_cx, module_obj.handle(), rval.handle_mut()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        drain_job_queue(realm_cx);

        // BCE (module timer keepalive): drive the hook UNBOUNDED, exactly
        // like the script path (`JsContext::eval`'s `loop {}`). The former
        // `for _ in 0..1000` cap killed `bao run app.mjs` after ~1000 ticks
        // while an active Bun.serve/timer still held the loop alive — with a
        // server registered, `drain_and_check` takes the non-blocking
        // `tick_without_idle` branch (~1ms/iteration), so the cap expired in
        // ~1.2s and the process exited mid-serve. Node semantics: an active
        // handle (server) or pending timer keeps the loop — and therefore the
        // process — alive; only the hook's own `false` (no pending work, or
        // process.exit) ends it.
        if let Some(hook) = post_eval_hook {
            loop {
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
                hook(realm_cx);
                drain_job_queue(realm_cx);
            }
        }

        // Surface a top-level throw / rejected top-level await to the caller
        // (Node semantics: `node foo.mjs` with a top-level throw exits 1).
        check_module_evaluation_promise(realm_cx, rval.handle())?;

        ::std::result::Result::Ok(unsafe { jsval_to_jsvalue(realm_cx.raw_cx_no_gc(), rval.get()) })
    }

    /// Evaluate `source` as an ES module under a fresh realm, then invoke
    /// `after_eval` while that realm is still alive. Used by `bao test` so the
    /// test runner (`globalThis.__run_bun_tests()`) executes against the same
    /// global object that registered the suites.
    ///
    /// `after_eval` receives the live `JSContext` (still inside `AutoRealm`),
    /// so it can read state installed by the module.
    pub fn eval_module_then<R>(
        cx: &mut mozjs::context::JSContext,
        source: &str,
        filename: &str,
        global_setup: Option<GlobalSetupFn>,
        post_eval_hook: Option<PostEvalHook>,
        after_eval: impl FnOnce(&mut mozjs::context::JSContext) -> R,
    ) -> ::std::result::Result<R, JsError> {
        let abs_filename = if Path::new(filename).is_absolute() {
            PathBuf::from(filename)
        } else {
            ::std::env::current_dir().unwrap_or_default().join(filename)
        };
        let base_dir = abs_filename
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| ::std::env::current_dir().ok());

        CURRENT_DIR.with(|d| *d.borrow_mut() = base_dir.clone());
        // @trace REQ-ENG-006 [entity:JSContext] — entry module path is used to
        // set import.meta.main = true on this module (Bun semantics).
        ENTRY_MODULE.with(|e| *e.borrow_mut() = Some(abs_filename.clone()));

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

        install_console(realm_cx, global.handle());
        if let Some(setup) = global_setup {
            unsafe { setup(realm_cx, global.handle()) };
        }

        let c_filename =
            CString::new(filename).unwrap_or_else(|_| CString::new("<module>").unwrap());
        let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

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
            let mut detail = extract_module_error(realm_cx);
            if detail.line == 0 && detail.column == 0 {
                detail.message = format!("Failed to compile module: {}", detail.message);
            } else {
                detail.message = format!(
                    "Failed to compile module: {} ({}:{}:{})",
                    detail.message, detail.filename, detail.line, detail.column
                );
            }
            return ::std::result::Result::Err(detail);
        }

        let entry_url = path_to_file_url(&abs_filename);
        unsafe {
            set_module_private(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &entry_url,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        if !unsafe { ModuleLink(realm_cx, module_obj.handle()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        if !unsafe { ModuleEvaluate(realm_cx, module_obj.handle(), rval.handle_mut()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        drain_job_queue(realm_cx);

        // `_then` variants (test-runner hand-off): the pre-callback pump
        // stays BOUNDED. The `after_eval` callback (bun:test runner) owns the
        // loop from here — suites commonly clear module-top-level timers
        // inside test bodies, so an unbounded pre-test pump would spin on
        // those timers forever and never reach the runner (regression caught
        // by `bao test` on a file with a top-level setInterval). The run
        // paths (`eval_module` / `eval_module_in_realm`) are unbounded —
        // Node process semantics; see their BCE notes.
        if let Some(hook) = post_eval_hook {
            for _ in 0..1000 {
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
                hook(realm_cx);
                drain_job_queue(realm_cx);
            }
        }

        // Surface a top-level throw / rejected top-level await BEFORE
        // `after_eval` runs: a failed module must not proceed to downstream
        // work (e.g. `bao test` must not run suites registered by a module
        // whose evaluation failed).
        check_module_evaluation_promise(realm_cx, rval.handle())?;

        ::std::result::Result::Ok(after_eval(realm_cx))
    }

    /// Evaluate `source` as an ES module INSIDE AN EXISTING realm's global.
    ///
    /// Realm-per-context variant (ECMA-262/Node semantics): the caller passes
    /// the persistent realm global — obtained from
    /// `JsContext::ensure_realm_global` / `bao_engine::context::thread_realm_global`
    /// — and this fn does NOT create a new global and does NOT apply
    /// `global_setup`. The realm already has its globals installed (by the
    /// first `eval` / `ensure_realm_global`); a module evaluated here shares
    /// `globalThis`, `require` singletons, and registered handlers with every
    /// script eval on the same context.
    ///
    /// Module compile/link/evaluate + microtask drain + post_eval_hook all run
    /// inside `AutoRealm(global)`, matching `eval_module` exactly minus the
    /// realm-creation block. Returns the module evaluation result.
    pub fn eval_module_in_realm(
        cx: &mut mozjs::context::JSContext,
        source: &str,
        filename: &str,
        post_eval_hook: Option<PostEvalHook>,
        global: mozjs::rust::Handle<*mut JSObject>,
    ) -> ::std::result::Result<JsValue, JsError> {
        let abs_filename = if Path::new(filename).is_absolute() {
            PathBuf::from(filename)
        } else {
            ::std::env::current_dir().unwrap_or_default().join(filename)
        };
        let base_dir = abs_filename
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| ::std::env::current_dir().ok());

        CURRENT_DIR.with(|d| *d.borrow_mut() = base_dir.clone());
        // @trace REQ-ENG-006 [entity:JSContext] — entry module path is used to
        // set import.meta.main = true on this module (Bun semantics).
        ENTRY_MODULE.with(|e| *e.borrow_mut() = Some(abs_filename.clone()));

        let mut realm = AutoRealm::new_from_handle(cx, global);
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        let c_filename =
            CString::new(filename).unwrap_or_else(|_| CString::new("<module>").unwrap());
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
            let mut detail = extract_module_error(realm_cx);
            if detail.line == 0 && detail.column == 0 {
                detail.message = format!("Failed to compile module: {}", detail.message);
            } else {
                detail.message = format!(
                    "Failed to compile module: {} ({}:{}:{})",
                    detail.message, detail.filename, detail.line, detail.column
                );
            }
            return ::std::result::Result::Err(detail);
        }

        // BUG-ENG-365: attach module private value (file:// URL) BEFORE linking.
        let entry_url = path_to_file_url(&abs_filename);
        unsafe {
            set_module_private(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &entry_url,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        if !unsafe { ModuleLink(realm_cx, module_obj.handle()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        if !unsafe { ModuleEvaluate(realm_cx, module_obj.handle(), rval.handle_mut()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        drain_job_queue(realm_cx);

        // BCE (module timer keepalive): drive the hook UNBOUNDED, exactly
        // like the script path (`JsContext::eval`'s `loop {}`). The former
        // `for _ in 0..1000` cap killed `bao run app.mjs` after ~1000 ticks
        // while an active Bun.serve/timer still held the loop alive — with a
        // server registered, `drain_and_check` takes the non-blocking
        // `tick_without_idle` branch (~1ms/iteration), so the cap expired in
        // ~1.2s and the process exited mid-serve. Node semantics: an active
        // handle (server) or pending timer keeps the loop — and therefore the
        // process — alive; only the hook's own `false` (no pending work, or
        // process.exit) ends it.
        if let Some(hook) = post_eval_hook {
            loop {
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
                hook(realm_cx);
                drain_job_queue(realm_cx);
            }
        }

        // Surface a top-level throw / rejected top-level await to the caller
        // (Node semantics: `node foo.mjs` with a top-level throw exits 1).
        check_module_evaluation_promise(realm_cx, rval.handle())?;

        ::std::result::Result::Ok(unsafe {
            jsval_to_jsvalue(realm_cx.raw_cx_no_gc(), rval.get())
        })
    }

    /// Realm-per-context variant of `eval_module_then`: evaluate `source` as
    /// an ES module inside the existing realm's `global`, then invoke
    /// `after_eval` while that realm is still entered. Used by `bao test` so
    /// `globalThis.__run_bun_tests()` executes against the same realm that
    /// registered the suites (which, in realm-per-context, is also the same
    /// realm as every prior script eval).
    ///
    /// See [`eval_module_in_realm`] for the realm contract (caller-supplied
    /// global; no JS_NewGlobalObject; no global_setup).
    pub fn eval_module_in_realm_then<R>(
        cx: &mut mozjs::context::JSContext,
        source: &str,
        filename: &str,
        post_eval_hook: Option<PostEvalHook>,
        global: mozjs::rust::Handle<*mut JSObject>,
        after_eval: impl FnOnce(&mut mozjs::context::JSContext) -> R,
    ) -> ::std::result::Result<R, JsError> {
        let abs_filename = if Path::new(filename).is_absolute() {
            PathBuf::from(filename)
        } else {
            ::std::env::current_dir().unwrap_or_default().join(filename)
        };
        let base_dir = abs_filename
            .parent()
            .map(|p| p.to_path_buf())
            .or_else(|| ::std::env::current_dir().ok());

        CURRENT_DIR.with(|d| *d.borrow_mut() = base_dir.clone());
        // @trace REQ-ENG-006 [entity:JSContext] — entry module path is used to
        // set import.meta.main = true on this module (Bun semantics).
        ENTRY_MODULE.with(|e| *e.borrow_mut() = Some(abs_filename.clone()));

        let mut realm = AutoRealm::new_from_handle(cx, global);
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        let c_filename =
            CString::new(filename).unwrap_or_else(|_| CString::new("<module>").unwrap());
        let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

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
            let mut detail = extract_module_error(realm_cx);
            if detail.line == 0 && detail.column == 0 {
                detail.message = format!("Failed to compile module: {}", detail.message);
            } else {
                detail.message = format!(
                    "Failed to compile module: {} ({}:{}:{})",
                    detail.message, detail.filename, detail.line, detail.column
                );
            }
            return ::std::result::Result::Err(detail);
        }

        let entry_url = path_to_file_url(&abs_filename);
        unsafe {
            set_module_private(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &entry_url,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        if !unsafe { ModuleLink(realm_cx, module_obj.handle()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        if !unsafe { ModuleEvaluate(realm_cx, module_obj.handle(), rval.handle_mut()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        drain_job_queue(realm_cx);

        // `_then` variants (test-runner hand-off): the pre-callback pump
        // stays BOUNDED. The `after_eval` callback (bun:test runner) owns the
        // loop from here — suites commonly clear module-top-level timers
        // inside test bodies, so an unbounded pre-test pump would spin on
        // those timers forever and never reach the runner (regression caught
        // by `bao test` on a file with a top-level setInterval). The run
        // paths (`eval_module` / `eval_module_in_realm`) are unbounded —
        // Node process semantics; see their BCE notes.
        if let Some(hook) = post_eval_hook {
            for _ in 0..1000 {
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
                hook(realm_cx);
                drain_job_queue(realm_cx);
            }
        }

        // Surface a top-level throw / rejected top-level await BEFORE
        // `after_eval` runs: a failed module must not proceed to downstream
        // work (e.g. `bao test` must not run suites registered by a module
        // whose evaluation failed).
        check_module_evaluation_promise(realm_cx, rval.handle())?;

        ::std::result::Result::Ok(after_eval(realm_cx))
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_resolve_imported_module(
    raw_cx: *mut JSContext,
    referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
) -> *mut JSObject {
    let specifier = unsafe { GetModuleRequestSpecifier(raw_cx, module_request) };
    if specifier.is_null() {
        return ::std::ptr::null_mut();
    }

    let specifier_str = mozjs::conversions::unsafe_jsstr_to_string(
        raw_cx,
        NonNull::new(specifier).expect("null-checked specifier"),
    );

    // Built-in module shortcut for static imports (e.g. import from "bun:test")
    let stripped = specifier_str
        .strip_prefix("node:")
        .unwrap_or(&specifier_str);

    let builtin_modules = [
        "fs",
        "path",
        "crypto",
        "os",
        "url",
        "events",
        "net",
        "http",
        "https",
        "child_process",
        "util",
        "assert",
        "stream",
        "zlib",
        "dns",
        "querystring",
        "buffer",
        "string_decoder",
        "timers",
        "readline",
        "perf_hooks",
        "tls",
        "bun:test",
        "harness",
        "test",
        // Stubbed builtins (registered by bao_runtime::node_stubs).
        "async_hooks",
        "cluster",
        "console",
        "constants",
        "dgram",
        "diagnostics_channel",
        "domain",
        "http2",
        "inspector",
        "punycode",
        "repl",
        "trace_events",
        "v8",
        "worker_threads",
        "sys",
        "vm",
        "tty",
        "module",
        "process",
        "_http_agent",
        "_http_client",
        "_http_common",
        "_http_incoming",
        "_http_outgoing",
        "_http_server",
        "_stream_duplex",
        "_stream_passthrough",
        "_stream_readable",
        "_stream_transform",
        "_stream_wrap",
        "_stream_writable",
        "_tls_common",
        "_tls_wrap",
        "assert/strict",
        "dns/promises",
        "fs/promises",
        "path/posix",
        "path/win32",
        "readline/promises",
        "stream/consumers",
        "stream/promises",
        "stream/web",
        "util/types",
        "inspector/promises",
        "timers/promises",
    ];

    // Synthetic ESM modules with explicit named exports for known builtins
    let synthetic_esm = match stripped {
        // @trace REQ-ENG-005 [module:node:test] — node:test runner shim.
        // Delegates to the CJS `node:test` builtin (bao_runtime::node_test),
        // the single gated source of truth: outside `bao test`
        // (process.argv[1] !== 'test') test/describe/it and the hooks throw
        // a pointer to `bao test <file>` instead of silently registering
        // suites that would never execute; under `bao test` the same calls
        // bridge to the real bun:test registration/execution.
        "test" => Some(NODE_TEST_ESM_BRIDGE),
        "bun:test" => Some(
            r#"var _m = require("bun:test");
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
"#,
        ),
        "harness" => Some(
            r#"var _m = require("harness");
export var gc = _m.gc;
export var bunExe = _m.bunExe;
export var bunEnv = _m.bunEnv;
export var bunRun = _m.bunRun;
export var isWindows = _m.isWindows;
export var isLinux = _m.isLinux;
export var isMac = _m.isMac;
export var isASAN = _m.isASAN;
export var isDebug = _m.isDebug;
export var isMinified = _m.isMinified;
export var withoutAggressiveGC = _m.withoutAggressiveGC;
export var expectOOM = _m.expectOOM;
export var BunEnvironment = _m.BunEnvironment;
export var tempDirWithFiles = _m.tempDirWithFiles;
export var joinP = _m.joinP;
export var gcTick = _m.gcTick;
export var invert = _m.invert;
export var stackTrace = _m.stackTrace;
export default _m;
"#,
        ),
        // Generic builtin modules: each builtin's `require("<name>")` returns a
        // cached module object exposing named properties (Buffer, createHash,
        // readFile, EventEmitter, ...). We emit explicit static `export var`
        // declarations per known builtin so SM's ESM linker can resolve
        // `import { Buffer } from "buffer"` / `import { createHash } from
        // "crypto"` / `import { readFile } from "fs"` at link time. Each export
        // binds lazily to the underlying property so it stays in sync with the
        // builtin module object after initialisation.
        _ if builtin_modules.contains(&stripped) => Some(builtin_esm_source(stripped)),
        _ => None,
    };

    if let Some(esm_src) = synthetic_esm {
        // Check cache first — synthetic modules must be returned as the same object
        let cache_key = format!("builtin:{}", stripped);
        let cached = module_cache_get(raw_cx, &cache_key);
        if let Some(existing) = cached
            && !existing.is_null()
        {
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
                // BUG-ENG-365: attach private value to synthetic builtin modules
                let priv_url = format!("builtin:{}", stripped);
                set_module_private(raw_cx, module, &priv_url);
                module_cache_insert(raw_cx, &cache_key, module);
            }
            return module;
        }
    }

    // BUG-ENG-365: derive base_dir from the referencing module's private URL
    // when available — this makes relative imports resolve against the
    // importing module's directory, per ECMAScript module semantics.
    let base_from_private = unsafe { base_dir_from_private_cx(raw_cx, referencing_private) };
    let base_dir = base_from_private.or_else(|| CURRENT_DIR.with(|d| d.borrow().clone()));

    // @trace REQ-ENG-005 — data: URL ESM modules (static import path).
    // Same loader as dynamic imports: parse + decode + compile. Static
    // imports return the module object directly; the cache key is the URL
    // itself so the same data: URL imported twice resolves to the same
    // module instance (SM requires referential identity for modules).
    if specifier_str.starts_with("data:") {
        if let ::std::result::Result::Ok(payload) = parse_data_url(&specifier_str) {
            let cache_key = format!("data-url:{}", specifier_str);
            let cached = module_cache_get(raw_cx, &cache_key);
            if let Some(existing) = cached
                && !existing.is_null()
            {
                return existing;
            }
            let c_filename = CString::new(specifier_str.to_string())
                .unwrap_or_else(|_| CString::new("<data-url>").unwrap());
            let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
            if !opts.is_null() {
                let mut src = transform_str_to_source_text(&payload);
                let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
                libc::free(opts as *mut _);
                if !module.is_null() {
                    set_module_private(raw_cx, module, &specifier_str);
                    module_cache_insert(raw_cx, &cache_key, module);
                }
                return module;
            }
            return ::std::ptr::null_mut();
        }
        // Malformed data: URL: surface as a compile-time failure.
        return ::std::ptr::null_mut();
    }

    let resolved = resolve_specifier(&specifier_str, base_dir.as_deref());

    let ::std::option::Option::Some(path) = resolved else {
        return ::std::ptr::null_mut();
    };

    let canonical = path.canonicalize().unwrap_or(path.clone());
    let cache_key = canonical.to_string_lossy().into_owned();

    let cached = module_cache_get(raw_cx, &cache_key);
    if let Some(existing) = cached
        && !existing.is_null()
    {
        return existing;
    }

    let content = match fs::read_to_string(&path) {
        ::std::result::Result::Ok(c) => c,
        ::std::result::Result::Err(_) => return ::std::ptr::null_mut(),
    };

    // BUG-ENG-365: CJS↔ESM interop.
    // When an ESM `import` targets a CJS module (`.cjs` or `.js` lacking
    // ESM syntax with a `module.exports` shape), wrap it as a synthetic ESM
    // whose default export is module.exports and named exports are exposed
    // via Object.keys re-exports. This matches Node.js ESM-CJS interop.
    if is_cjs_module(&canonical, &content) {
        let wrapper = cjs_compat_wrapper_source(&canonical, &content);
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<cjs-compat>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = transform_str_to_source_text(&wrapper);
            let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
            libc::free(opts as *mut _);
            if !module.is_null() {
                let priv_url = path_to_file_url(&canonical);
                set_module_private(raw_cx, module, &priv_url);
                module_cache_insert(raw_cx, &cache_key, module);
                return module;
            }
        }
        // Fallback: treat as ESM below if CJS wrap fails.
    }

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
            // BUG-ENG-365: SetModulePrivate on every compiled module so that
            // subsequent imports/evaluates can resolve relative specifiers and
            // populate import.meta.url correctly.
            let priv_url = path_to_file_url(&canonical);
            set_module_private(raw_cx, module, &priv_url);
            module_cache_insert(raw_cx, &cache_key, module);
        }
        module
    }
}

/// Heuristic: detect whether a file is a CJS module (vs ESM).
///
/// Rules:
/// - `.cjs` extension → CJS
/// - `.mjs` extension → ESM (never CJS)
/// - `.js` / `.ts` → CJS if source contains `module.exports`, `exports.`,
///   `require(`, or lacks `import`/`export` ESM keywords
fn is_cjs_module(path: &Path, content: &str) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("cjs") => return true,
        Some("mjs") => return false,
        _ => {}
    }
    // Strong CJS signal: assignment to module.exports or exports.x
    let has_cjs_marker = content.contains("module.exports")
        || content.contains("exports.")
        || content.contains("exports[");
    // Strong ESM signal: import/export statements
    let has_esm_marker = content.contains("import ")
        || content.contains("export ")
        || content.contains("export default")
        || content.contains("import *")
        || content.contains("import {");
    if has_esm_marker && !has_cjs_marker {
        return false;
    }
    has_cjs_marker
}

/// Render a Rust string as a JS double-quoted string literal with proper escapes.
fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Named ESM exports to surface for each Node-compatible builtin module.
///
/// The list mirrors the surface installed by `bao_runtime` (see
/// `node_buffer.rs`, `node_crypto.rs`, `node_fs.rs`, `node_events.rs`, ...).
/// Each name becomes a static `export var` declaration so SM's ESM linker can
/// resolve `import { Buffer } from "buffer"` at link time. Unknown names fall
/// back to a `default`-only module; builtins without an entry here are still
/// importable via default export.
fn builtin_named_exports(name: &str) -> &'static [&'static str] {
    match name {
        "buffer" => &[
            "Buffer",
            "SlowBuffer",
            "kMaxLength",
            "constants",
            "INSPECT_MAX_BYTES",
            // @trace REQ-ENG-005 — extras imported by upstream tests.
            // buffer.test.js imports isAscii, isUtf8; buffer-resolveObjectURL
            // imports resolveObjectURL. Blob/File live on globalThis per Web
            // IDL but buffer.test.js "File" drives
            //   const BufferModule = await import("buffer");
            //   expect(BufferModule.File).toBe(File);
            // so File MUST appear as a named export. transcode is the
            // masqueradesAsUndefined sentinel (typeof === "undefined" but
            // callable and throws "Not implemented" — see buffer module
            // installer in node_buffer.rs).
            "isAscii",
            "isUtf8",
            "resolveObjectURL",
            "Blob",
            "File",
            "transcode",
        ],
        "crypto" => &[
            "createHash",
            "createHmac",
            "createCipher",
            "createCipheriv",
            "createDecipher",
            "createDecipheriv",
            "randomBytes",
            "pseudoRandomBytes",
            "randomInt",
            "randomUUID",
            "pbkdf2",
            "pbkdf2Sync",
            "scrypt",
            "scryptSync",
            "hkdf",
            "hkdfSync",
            "digest",
            "hash",
            "createSign",
            "createVerify",
            "getHashes",
            "getCiphers",
            "timingSafeEqual",
            "timingSafeCompare",
            "DiffieHellmanGroup",
            "createDiffieHellman",
            "DiffieHellman",
            "createECDH",
            "generateKeyPair",
            "generateKeyPairSync",
            "createSecretKey",
            "createPublicKey",
            "createPrivateKey",
            "KeyObject",
            "webcrypto",
            "constants",
            "randomFill",
            "randomFillSync",
        ],
        "fs" => &[
            "readFile",
            "readFileSync",
            "writeFile",
            "writeFileSync",
            "appendFile",
            "appendFileSync",
            "readdir",
            "readdirSync",
            "stat",
            "statSync",
            "lstat",
            "lstatSync",
            "fstat",
            "fstatSync",
            "exists",
            "existsSync",
            "mkdir",
            "mkdirSync",
            "rmdir",
            "rmdirSync",
            "rm",
            "rmSync",
            "unlink",
            "unlinkSync",
            "rename",
            "renameSync",
            "copyFile",
            "copyFileSync",
            "open",
            "openSync",
            "close",
            "closeSync",
            "read",
            "readSync",
            "write",
            "writeSync",
            "realpath",
            "realpathSync",
            "createReadStream",
            "createWriteStream",
            "watch",
            "watchFile",
            "unwatchFile",
            "access",
            "accessSync",
            "chmod",
            "chmodSync",
            "chown",
            "chownSync",
            "utimes",
            "utimesSync",
            "lutimes",
            "lutimesSync",
            "link",
            "linkSync",
            "symlink",
            "symlinkSync",
            "readlink",
            "readlinkSync",
            "truncate",
            "truncateSync",
            "ftruncate",
            "ftruncateSync",
            "fchmod",
            "fchmodSync",
            "fchown",
            "fchownSync",
            "mkdirp",
            "mkdirpSync",
            "cp",
            "cpSync",
            "opendir",
            "opendirSync",
            "Dir",
            "promises",
            "constants",
            "F_OK",
            "R_OK",
            "W_OK",
            "X_OK",
            "Stats",
            "ReadStream",
            "WriteStream",
            "Dirent",
        ],
        "events" => &[
            "EventEmitter",
            "once",
            "on",
            "getEventListeners",
            "setMaxListeners",
            "getMaxListeners",
            "EventEmitterAsyncResource",
            "captureRejections",
            "captureRejectionSymbol",
            "defaultMaxListeners",
            "errorMonitor",
            "listenerCount",
        ],
        "os" => &[
            "platform",
            "arch",
            "type",
            "release",
            "hostname",
            "cpus",
            "totalmem",
            "freemem",
            "uptime",
            "loadavg",
            "networkInterfaces",
            "userInfo",
            "homedir",
            "tmpdir",
            "endianness",
            "EOL",
            "constants",
            "availableParallelism",
            "getPriority",
            "setPriority",
            "machine",
            "version",
            "devDir",
        ],
        "path" => &[
            "resolve",
            "normalize",
            "isAbsolute",
            "join",
            "relative",
            "dirname",
            "basename",
            "extname",
            "parse",
            "format",
            "sep",
            "delimiter",
            "win32",
            "posix",
            "toNamespacedPath",
            "matchesGlob",
        ],
        "url" => &[
            "parse",
            "resolve",
            "resolveObject",
            "format",
            "Url",
            "URL",
            "URLSearchParams",
            "domainToASCII",
            "domainToUnicode",
            "fileURLToPath",
            "pathToFileURL",
            "urlToHttpOptions",
        ],
        "util" => &[
            "format",
            "debug",
            "log",
            "inspect",
            "isArray",
            "isBoolean",
            "isNull",
            "isNullOrUndefined",
            "isNumber",
            "isString",
            "isSymbol",
            "isUndefined",
            "isRegExp",
            "isObject",
            "isDate",
            "isError",
            "isFunction",
            "isPrimitive",
            "isBuffer",
            "promisify",
            "callbackify",
            "inherits",
            "types",
            "TextEncoder",
            "TextDecoder",
            "_extend",
            "deprecate",
            "formatWithOptions",
            "styleText",
            "stripVTControlCharacters",
            "parseArgs",
            "MIMEType",
            "parseMIMEType",
            "aborted",
            "transferable",
            "deepEqual",
            "deepStrictEqual",
        ],
        "string_decoder" => &["StringDecoder"],
        // @trace REQ-ENG-005 — node:tty named exports. ReadStream, WriteStream,
        // and isatty are imported by tty.test.ts and nodettywrap.test.ts.
        "tty" => &["ReadStream", "WriteStream", "isatty"],
        "timers" => &[
            "setTimeout",
            "clearTimeout",
            "setInterval",
            "clearInterval",
            "setImmediate",
            "clearImmediate",
            "promises",
        ],
        "stream" => &[
            "Stream",
            "Readable",
            "Writable",
            "Duplex",
            "Transform",
            "PassThrough",
            "pipeline",
            "finished",
            "addAbortSignal",
            "promises",
            "ReadableStream",
            "WritableStream",
            "TransformStream",
            "getDefaultHighWaterMark",
            "setDefaultHighWaterMark",
            "isDisturbed",
        ],
        "assert" => &[
            "ok",
            "equal",
            "notEqual",
            "deepEqual",
            "notDeepEqual",
            "deepStrictEqual",
            "notDeepStrictEqual",
            "strict",
            "fail",
            "throws",
            "doesNotThrow",
            "ifError",
            "rejects",
            "doesNotReject",
            "match",
            "doesNotMatch",
            "CallTracker",
            "partialDeepStrictEqual",
        ],
        "querystring" => &[
            "escape",
            "unescape",
            "encode",
            "decode",
            "stringify",
            "parse",
        ],
        "net" => &[
            "createServer",
            "createConnection",
            "connect",
            "Server",
            "Socket",
            "isIP",
            "isIPv4",
            "isIPv6",
            "BlockList",
            "SocketAddress",
        ],
        "tls" => &[
            "createServer",
            "createSecureContext",
            "createSecureServer",
            "connect",
            "TLSSocket",
            "Server",
            "SecureContext",
            "checkServerIdentity",
            "getCiphers",
            "rootCertificates",
            "DEFAULT_ECDH_CURVE",
            "DEFAULT_MIN_VERSION",
            "DEFAULT_MAX_VERSION",
        ],
        "dns" => &[
            "lookup",
            "resolve",
            "resolve4",
            "resolve6",
            "resolveAny",
            "reverse",
            "Resolver",
            "getServers",
            "setServers",
            "lookupService",
            "promises",
            "defaultResolver",
            "setDefaultResultOrder",
            "getDefaultResultOrder",
            "lookupSync",
        ],
        "zlib" => &[
            "deflate",
            "deflateSync",
            "inflate",
            "inflateSync",
            "gzip",
            "gzipSync",
            "gunzip",
            "gunzipSync",
            "deflateRaw",
            "deflateRawSync",
            "inflateRaw",
            "inflateRawSync",
            "brotliCompress",
            "brotliCompressSync",
            "brotliDecompress",
            "brotliDecompressSync",
            "createDeflate",
            "createInflate",
            "createGzip",
            "createGunzip",
            "createDeflateRaw",
            "createInflateRaw",
            "createBrotliCompress",
            "createBrotliDecompress",
            "Deflate",
            "Inflate",
            "Gzip",
            "Gunzip",
            "DeflateRaw",
            "InflateRaw",
            "BrotliCompress",
            "BrotliDecompress",
            "constants",
            "crc32",
            "crc32Sync",
        ],
        "child_process" => &[
            "spawn",
            "exec",
            "execFile",
            "execFileSync",
            "spawnSync",
            "execSync",
            "fork",
            "ChildProcess",
        ],
        "readline" => &[
            "createInterface",
            "clearLine",
            "clearScreenDown",
            "cursorTo",
            "emitKeypressEvents",
            "moveCursor",
            "promises",
            "Interface",
            "question",
        ],
        "perf_hooks" => &[
            "PerformanceObserver",
            "PerformanceEntry",
            "PerformanceObserverEntryList",
            "Performance",
            "monitorEventLoopDelay",
            "constants",
        ],
        "http" => &[
            "request",
            "get",
            "ClientRequest",
            "IncomingMessage",
            "Server",
            "ServerResponse",
            "METHODS",
            "STATUS_CODES",
            "globalAgent",
            "Agent",
            "MaxRequestsPerServer",
            "createServer",
            "validateHeaderName",
            "validateHeaderValue",
            "setGlobalDispatcher",
            "getGlobalDispatcher",
        ],
        "https" => &["request", "get", "Server", "Agent", "globalAgent"],
        _ => &[],
    }
}

/// Build the ESM source for a builtin module. Emits `export var` declarations
/// for each named export (so SM's linker can resolve them statically) plus a
/// `default` export pointing at the full builtin module object. Unknown names
/// fall back to `default`-only (still `import X from "<builtin>"`-able).
/// ESM bridge source for `node:test` — shared by the static-import resolve
/// hook (`host_resolve_imported_module`) and dynamic `import("node:test")`
/// (`dynamic_import_builtin`) so both expose the identical surface.
///
/// Forwards every named export to the CJS `node:test` builtin installed by
/// `bao_runtime::node_test` — the gated single source of truth. The previous
/// version bound directly to `require("bun:test")`, which registers suites
/// with no runner gate: under `bao run` a file doing
/// `import { test } from "node:test"` silently registered nothing and faked
/// a pass. Routing through the CJS module keeps the ESM and CJS contracts
/// identical (throw outside `bao test`, real registration inside it).
const NODE_TEST_ESM_BRIDGE: &str = r#"var _m = require("node:test");
export var test = _m.test;
export var describe = _m.describe;
export var it = _m.it;
export var before = _m.before;
export var after = _m.after;
export var beforeAll = _m.beforeAll;
export var afterAll = _m.afterAll;
export var beforeEach = _m.beforeEach;
export var afterEach = _m.afterEach;
export var mock = _m.mock;
export var assert = _m.assert;
export var run = _m.run;
export default _m;
"#;

fn builtin_esm_source(name: &str) -> &'static str {
    // Static module source is required because SM's ESM linker reads exports
    // at compile time. We render this once per builtin and leak it so the
    // returned `&'static str` matches the other match arms.
    let spec = js_string_literal(name);
    let named = builtin_named_exports(name);
    let mut src = String::with_capacity(64 + named.len() * 32);
    src.push_str("var _m = require(");
    src.push_str(&spec);
    src.push_str(");\n");
    // Deduplicate: SM rejects duplicate export names at compile time. Iterate
    // and skip any name we've already emitted.
    let mut seen: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
    for &exp in named {
        if !seen.insert(exp) {
            continue;
        }
        // `export var <name> = _m.<name>;` — bind the export to the property
        // value at module evaluation time. Re-binds on each evaluation.
        src.push_str("export var ");
        src.push_str(exp);
        src.push_str(" = _m.");
        src.push_str(exp);
        src.push_str(";\n");
    }
    src.push_str("export default _m;\n");
    Box::leak(src.into_boxed_str())
}

/// Generate the CJS-compat ESM wrapper source that wraps a CJS module.exports
/// for ESM `import`. The wrapper:
///   1. Defines `module`, `exports` locals (CJS environment).
///   2. Evaluates the CJS source in a function scope.
///   3. Exports `default = module.exports`.
///   4. Re-exports all enumerable keys as named exports (live bindings).
fn cjs_compat_wrapper_source(canonical: &Path, cjs_source: &str) -> String {
    let escaped = cjs_source
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");
    let filename_str = js_string_literal(&canonical.to_string_lossy());
    let dirname_str = js_string_literal(
        &canonical
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    format!(
        r#"
var __cjs_module = {{ exports: {{}} }};
var module = __cjs_module;
var exports = __cjs_module.exports;
var __filename = {filename_str};
var __dirname = {dirname_str};
(function() {{
{src}
}}).call(exports);
var __cjs_default = (module.exports === exports) ? exports : module.exports;
export default __cjs_default;
var __cjs_keys = (typeof __cjs_default === 'object' && __cjs_default !== null)
    ? Object.keys(__cjs_default) : [];
for (var __i = 0; __i < __cjs_keys.length; __i++) {{
    var __k = __cjs_keys[__i];
    try {{
        Object.defineProperty(exports, __k, {{
            get: function() {{ return __cjs_default[__k]; }},
            enumerable: true,
            configurable: true,
        }});
    }} catch (e) {{}}
}}
"#,
        filename_str = filename_str,
        dirname_str = dirname_str,
        src = escaped,
    )
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_populate_import_meta(
    raw_cx: *mut JSContext,
    private_value: Handle<Value>,
    meta_object: Handle<*mut JSObject>,
) -> bool {
    unsafe {
        // @trace REQ-ENG-006 [entity:JSContext] — Bun's import.meta extensions.
        // Bun exposes dir/path/file/main on import.meta (in addition to url).
        // - url:   file:// URL of this module (always present)
        // - path:  absolute filesystem path (__filename equivalent)
        // - dir:   parent directory of path (__dirname equivalent)
        // - file:  just the file name component of path
        // - main:  true if this module is the entry point
        let specifier = if private_value.is_string() {
            mozjs::conversions::unsafe_jsstr_to_string(
                raw_cx,
                NonNull::new(private_value.to_string()).expect("valid private value"),
            )
        } else {
            String::new()
        };
        let (resolved_url, fs_path): (String, Option<PathBuf>) = if specifier.starts_with("file://")
        {
            // Already a file URL — derive fs path by stripping the scheme.
            let stripped = specifier
                .strip_prefix("file://")
                .unwrap_or(&specifier)
                .to_string();
            let p = PathBuf::from(&stripped);
            (specifier.clone(), Some(p))
        } else if !specifier.is_empty() {
            let base_dir = CURRENT_DIR.with(|d| d.borrow().clone());
            match resolve_specifier(&specifier, base_dir.as_deref()) {
                Some(p) => {
                    let url = format!("file://{}", p.to_string_lossy());
                    (url, Some(p))
                }
                None => (format!("file://{}", specifier), None),
            }
        } else {
            ("file://".to_string(), None)
        };

        // url — always defined.
        let Ok(c_url) = CString::new(resolved_url.as_str()) else {
            return false;
        };
        let url_js = JS_NewStringCopyZ(raw_cx, c_url.as_ptr());
        if url_js.is_null() {
            return false;
        }
        let url_val = mozjs::jsval::StringValue(&*url_js);
        // BCE-20260619-012: StringValue contains GC-managed JSString pointer; must be rooted.
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
        rooted!(&in(wrapped_cx) let url_val_root = url_val);
        if !JS_DefineProperty(
            raw_cx,
            meta_object,
            c"url".as_ptr(),
            url_val_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        ) {
            return false;
        }

        // Derive Bun-style path/dir/file from the filesystem path. If we could
        // not resolve a real path (synthetic/builtin module), leave them as
        // empty strings — Bun does the same for non-file modules.
        let (path_str, dir_str, file_str) = match &fs_path {
            Some(p) => {
                let path_s = p.to_string_lossy().into_owned();
                let dir_s = p
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let file_s = p
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (path_s, dir_s, file_s)
            }
            None => (String::new(), String::new(), String::new()),
        };

        let define_str_prop = |name: &::std::ffi::CStr, value: &str| -> bool {
            let cstr = ::std::ffi::CString::new(value)
                .unwrap_or_else(|_| ::std::ffi::CString::new("").unwrap());
            let js = JS_NewStringCopyZ(raw_cx, cstr.as_ptr());
            if js.is_null() {
                return false;
            }
            let v = mozjs::jsval::StringValue(&*js);
            // BCE-20260619-012: StringValue contains GC-managed JSString pointer; must be rooted.
            rooted!(&in(wrapped_cx) let v_root = v);
            JS_DefineProperty(
                raw_cx,
                meta_object,
                name.as_ptr(),
                v_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            )
        };

        if !define_str_prop(c"path".as_ref(), &path_str) {
            return false;
        }
        if !define_str_prop(c"dir".as_ref(), &dir_str) {
            return false;
        }
        if !define_str_prop(c"file".as_ref(), &file_str) {
            return false;
        }

        // main — true if this module's absolute path matches the entry path.
        // The entry is tracked in CURRENT_DIR-adjacent ENTRY_MODULE thread-local.
        let is_main = ENTRY_MODULE.with(|e| {
            e.borrow()
                .as_ref()
                .and_then(|entry| {
                    fs_path
                        .as_ref()
                        .map(|p| fs::canonicalize(p).ok() == fs::canonicalize(entry).ok())
                })
                .unwrap_or(false)
        });
        let main_val = mozjs::jsval::BooleanValue(is_main);
        let main_h = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &main_val,
        };
        if !JS_DefineProperty(
            raw_cx,
            meta_object,
            c"main".as_ptr(),
            main_h,
            JSPROP_ENUMERATE as u32,
        ) {
            return false;
        }

        // @trace REQ-ENG-006 [api:import.meta.require] — Bun/Node.js ESM-CJS
        // interop. `import.meta.require(specifier)` is the synchronous CJS
        // `require()` available inside ESM modules. We expose the global
        // `require` function (installed by bao_runtime) as a non-enumerable
        // property of `import.meta`, so ESM code can do
        // `import.meta.require("fs")` without going through dynamic import().
        let global_obj = unsafe { CurrentGlobalOrNull(raw_cx) };
        if !global_obj.is_null() {
            rooted!(in(raw_cx) let global_root = global_obj);
            let mut require_val = mozjs::jsval::UndefinedValue();
            let require_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut require_val,
            };
            let got = unsafe {
                mozjs_sys::jsapi::JS_GetProperty(
                    raw_cx,
                    global_root.handle().into(),
                    c"require".as_ptr(),
                    require_h,
                )
            };
            if got && require_val.is_object() {
                let require_obj_val = require_val;
                // BCE-20260619-012: require_obj_val may contain GC-managed object; must be rooted.
                rooted!(&in(wrapped_cx) let require_obj_root = require_obj_val);
                // Non-enumerable — `import.meta.require` is a function reference,
                // not a data property that should serialize.
                let _ = unsafe {
                    JS_DefineProperty(
                        raw_cx,
                        meta_object,
                        c"require".as_ptr(),
                        require_obj_root.handle().into(),
                        0,
                    )
                };
            }
        }
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_dynamic_import(
    raw_cx: *mut JSContext,
    referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
    promise: Handle<*mut JSObject>,
) -> bool {
    let specifier = unsafe { GetModuleRequestSpecifier(raw_cx, module_request) };
    if specifier.is_null() {
        return false;
    }
    let specifier_str = mozjs::conversions::unsafe_jsstr_to_string(
        raw_cx,
        NonNull::new(specifier).expect("null-checked specifier"),
    );

    // Built-in module shortcut: synthetic ESM modules backing Node.js builtins.
    //
    // The synthetic ESM source (see `builtin_esm_source`) declares named
    // exports plus `export default _m`, so the resulting module namespace
    // contains both named bindings AND a `default` key. This matches Node.js
    // ESM-CJS interop: `import zlib from "zlib"` and `import { gzipSync } from
    // "zlib"` both work, and `await import("zlib")` returns a namespace whose
    // `"default" in mod` is true (per stubs.test.js contract).
    //
    // Path:
    //   1. Find/build the synthetic module (cache key `builtin:{stripped}`).
    //   2. ModuleLink + ModuleEvaluate + drain job queue.
    //   3. FinishDynamicModuleImport — SM resolves the user-facing promise with
    //      the *module namespace* (which carries the `default` property).
    let builtin_modules = [
        "fs",
        "path",
        "crypto",
        "os",
        "url",
        "events",
        "net",
        "http",
        "https",
        "child_process",
        "util",
        "assert",
        "stream",
        "zlib",
        "dns",
        "querystring",
        "buffer",
        "string_decoder",
        "timers",
        "readline",
        "perf_hooks",
        "tls",
        "bun:test",
        "harness",
        "test",
        // Stubbed builtins (registered by bao_runtime::node_stubs).
        "async_hooks",
        "cluster",
        "console",
        "constants",
        "dgram",
        "diagnostics_channel",
        "domain",
        "http2",
        "inspector",
        "punycode",
        "repl",
        "trace_events",
        "v8",
        "worker_threads",
        "sys",
        "vm",
        "tty",
        "module",
        "process",
        "_http_agent",
        "_http_client",
        "_http_common",
        "_http_incoming",
        "_http_outgoing",
        "_http_server",
        "_stream_duplex",
        "_stream_passthrough",
        "_stream_readable",
        "_stream_transform",
        "_stream_wrap",
        "_stream_writable",
        "_tls_common",
        "_tls_wrap",
        "assert/strict",
        "dns/promises",
        "fs/promises",
        "path/posix",
        "path/win32",
        "readline/promises",
        "stream/consumers",
        "stream/promises",
        "stream/web",
        "util/types",
        "inspector/promises",
        "timers/promises",
    ];
    let stripped = specifier_str
        .strip_prefix("node:")
        .unwrap_or(&specifier_str);
    if builtin_modules.contains(&stripped) {
        return unsafe {
            dynamic_import_builtin(
                raw_cx,
                stripped,
                referencing_private,
                module_request,
                promise,
            )
        };
    }

    // @trace REQ-ENG-005 — data: URL ESM modules.
    //
    // WHATWG-fetch-style `data:text/javascript,...` and
    // `data:text/javascript;base64,...` URLs are loadable ESM sources
    // (string-module.test.js). They never hit the filesystem. Decode the
    // payload (URL-decode for inline, base64-decode for the ;base64 form)
    // and feed the bytes straight to JS::CompileModule1.
    //
    // string-module.test.js asserts that a malformed base64 payload throws
    // `Base64DecodeError`. We surface that via SM's pending-exception
    // mechanism and return false so the failure is observable.
    if specifier_str.starts_with("data:") {
        match parse_data_url(&specifier_str) {
            ::std::result::Result::Ok(payload) => {
                return unsafe {
                    dynamic_import_data_url(
                        raw_cx,
                        &specifier_str,
                        &payload,
                        referencing_private,
                        module_request,
                        promise,
                    )
                };
            }
            ::std::result::Result::Err(err) => {
                // Malformed data URL: throw + reject so both sync-throw and
                // await-based consumers observe the error.
                let c_msg = CString::new(err.as_str())
                    .unwrap_or_else(|_| CString::new("Module load error").unwrap());
                unsafe { mozjs::error::throw_type_error(raw_cx, c_msg.as_ref()) };
                let _ = unsafe { reject_dynamic_promise(raw_cx, promise, &err) };
                return false;
            }
        }
    }

    // BUG-ENG-365: derive base_dir from referencing module's private URL.
    let base_dir = unsafe { base_dir_from_private_cx(raw_cx, referencing_private) }
        .or_else(|| CURRENT_DIR.with(|d| d.borrow().clone()));
    let resolved = resolve_specifier(&specifier_str, base_dir.as_deref());

    let ::std::option::Option::Some(path) = resolved else {
        return unsafe {
            reject_dynamic_promise(
                raw_cx,
                promise,
                &format!("Cannot find module '{}'", specifier_str),
            )
        };
    };

    let canonical = path.canonicalize().unwrap_or(path.clone());
    let cache_key = canonical.to_string_lossy().into_owned();

    // BUG-ENG-365: For file modules we MUST use FinishDynamicModuleImport
    // per SM Module API spec. This drives the SM-side state machine and
    // resolves the user-facing promise with the module namespace.
    let content = match fs::read_to_string(&path) {
        ::std::result::Result::Ok(c) => c,
        ::std::result::Result::Err(e) => {
            return unsafe {
                reject_dynamic_promise(
                    raw_cx,
                    promise,
                    &format!("Cannot read module '{}': {}", specifier_str, e),
                )
            };
        }
    };

    // CJS target — wrap as ESM for interop; otherwise transpile TS/JSX.
    let effective_source = if is_cjs_module(&canonical, &content) {
        cjs_compat_wrapper_source(&canonical, &content)
    } else if needs_transpile(&path) {
        strip_typescript(&content, &path)
    } else {
        content
    };

    unsafe {
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return unsafe {
                reject_dynamic_promise(raw_cx, promise, "Internal: compile options alloc failed")
            };
        }
        let mut src = transform_str_to_source_text(&effective_source);
        let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        libc::free(opts as *mut _);
        if module.is_null() {
            return unsafe {
                reject_dynamic_promise(raw_cx, promise, "Internal: module compilation failed")
            };
        }

        // BUG-ENG-365: SetModulePrivate before linking.
        let priv_url = path_to_file_url(&canonical);
        set_module_private(raw_cx, module, &priv_url);

        module_cache_insert(raw_cx, &cache_key, module);

        rooted!(in(raw_cx) let module_root = module);
        if !mozjs_sys::jsapi::JS::ModuleLink(raw_cx, module_root.handle().into()) {
            // Link failed — complete via FinishDynamicModuleImport with null eval promise.
            rooted!(in(raw_cx) let null_root = ::std::ptr::null_mut::<JSObject>());
            return unsafe {
                mozjs_sys::jsapi::JS::FinishDynamicModuleImport(
                    raw_cx,
                    null_root.handle().into(),
                    referencing_private,
                    module_request,
                    promise,
                )
            };
        }

        let mut eval_rval = UndefinedValue();
        let eval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut eval_rval,
        };
        let eval_ok =
            mozjs_sys::jsapi::JS::ModuleEvaluate(raw_cx, module_root.handle().into(), eval_h);

        // Drain microtasks so synchronous module bodies complete.
        mozjs_sys::jsapi::js::RunJobs(raw_cx);

        // ModuleEvaluate returns the evaluation promise (object) on success,
        // or undefined/false on failure. Per SM spec we pass this evaluation
        // promise to FinishDynamicModuleImport.
        let evaluation_promise = if eval_ok && eval_rval.is_object() {
            eval_rval.to_object()
        } else {
            ::std::ptr::null_mut::<JSObject>()
        };
        rooted!(in(raw_cx) let eval_promise_root = evaluation_promise);

        // BUG-ENG-365: spec-mandated completion path.
        mozjs_sys::jsapi::JS::FinishDynamicModuleImport(
            raw_cx,
            eval_promise_root.handle().into(),
            referencing_private,
            module_request,
            promise,
        )
    }
}

/// Resolve the user-facing dynamic import promise directly with a JS value.
/// Used for built-in modules that have no SM module record.
///
/// # Safety
/// Caller must hold a valid `cx`.
unsafe fn resolve_dynamic_promise_with_value(
    raw_cx: *mut JSContext,
    promise: Handle<*mut JSObject>,
    val: Value,
) -> bool {
    // BCE-20260619-012: val may contain GC-managed pointer; must be rooted.
    let wrapped_cx =
        unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
    rooted!(&in(wrapped_cx) let val_root = val);
    unsafe { mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise, val_root.handle().into()) }
}

/// Reject the user-facing dynamic import promise with an Error object
/// carrying `msg`.
///
/// # Safety
/// Caller must hold a valid `cx`.
unsafe fn reject_dynamic_promise(
    raw_cx: *mut JSContext,
    promise: Handle<*mut JSObject>,
    msg: &str,
) -> bool {
    let Ok(c_msg) = CString::new(msg) else {
        return false;
    };
    let err_obj = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(raw_cx) };
    if !err_obj.is_null() {
        rooted!(in(raw_cx) let err_root = err_obj);
        let err_msg = unsafe { JS_NewStringCopyZ(raw_cx, c_msg.as_ptr()) };
        if !err_msg.is_null() {
            let msg_val = unsafe { mozjs::jsval::StringValue(&*err_msg) };
            rooted!(in(raw_cx) let msg_h = msg_val);
            unsafe {
                JS_SetProperty(
                    raw_cx,
                    err_root.handle().into(),
                    c"message".as_ptr(),
                    msg_h.handle().into(),
                )
            };
        }
        let err_val = mozjs::jsval::ObjectValue(err_obj);
        // BCE-20260619-012: ObjectValue contains GC-managed object; must be rooted.
        rooted!(in(raw_cx) let err_root_val = err_val);
        unsafe {
            mozjs_sys::jsapi::JS::RejectPromise(raw_cx, promise, err_root_val.handle().into())
        };
    }
    true
}

/// @trace REQ-ENG-005 [algorithm:data_url] — parse a `data:` URL into its
/// decoded ESM source string.
///
/// WHATWG RFC 2397 form: `data:[<mediatype>][;base64],<data>`. We accept
/// `text/javascript` (and the legacy `application/javascript`) media types.
/// For the `;base64` form, we decode the payload and surface a
/// `Base64DecodeError` when the bytes are invalid — matching the contract
/// expected by string-module.test.js.
///
/// Returns:
///   - `Ok(source)` — decoded ESM source string.
///   - `Err(msg)` — error name to attach to the rejected import promise.
fn parse_data_url(url: &str) -> ::std::result::Result<String, String> {
    let Some(rest) = url.strip_prefix("data:") else {
        return Err("Not a data URL".to_string());
    };
    let Some(comma) = rest.find(',') else {
        return Err("Data URL missing payload".to_string());
    };
    let meta = &rest[..comma];
    let payload = &rest[comma + 1..];

    let is_base64 = meta.split(';').any(|s| s.eq_ignore_ascii_case("base64"));
    // Media type sanity: reject anything that is not javascript-ish.
    let mediatype_ok = {
        let mt = meta
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        mt.is_empty()
            || mt == "text/javascript"
            || mt == "application/javascript"
            || mt == "text/ecmascript"
            || mt == "application/ecmascript"
            || mt == "module"
            || mt == "text/javascript,module"
    };
    if !mediatype_ok {
        return Err(format!("Unsupported data URL media type: {}", meta));
    }

    if is_base64 {
        // @trace REQ-ENG-005 [algorithm:base64]
        // Bun's contract (string-module.test.js) is that malformed base64
        // raises a `Base64DecodeError`. We implement a small RFC 4648
        // decoder here rather than pulling a new crate dep into bun_sm
        // (which is otherwise dependency-free).
        match base64_decode(payload) {
            ::std::result::Result::Ok(bytes) => match ::std::str::from_utf8(&bytes) {
                ::std::result::Result::Ok(s) => ::std::result::Result::Ok(s.to_owned()),
                ::std::result::Result::Err(_) => {
                    Err("Data URL payload is not valid UTF-8".to_string())
                }
            },
            ::std::result::Result::Err(_) => Err("Base64DecodeError".to_string()),
        }
    } else {
        // Percent-decode inline payload per RFC 3986 §2.1. Most JS module
        // data: URLs are sent un-encoded, but we must handle %20, %2C, %0A,
        // etc. so that quoted payloads still round-trip.
        let decoded = percent_decode(payload);
        Ok(decoded)
    }
}

/// Minimal percent-decoder. Accepts `%XX` hex escapes and passes other bytes
/// through verbatim (data URLs are conventionally ASCII; non-ASCII bytes get
/// passed through as UTF-8 which is what JS expects for source text).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_val(bytes[i + 1]);
            let lo = hex_val(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> ::std::option::Option<u8> {
    match b {
        b'0'..=b'9' => ::std::option::Option::Some(b - b'0'),
        b'a'..=b'f' => ::std::option::Option::Some(b - b'a' + 10),
        b'A'..=b'F' => ::std::option::Option::Some(b - b'A' + 10),
        _ => ::std::option::Option::None,
    }
}

/// RFC 4648 standard-alphabet base64 decoder. URL-safe `-_` are also
/// accepted. Whitespace is tolerated; malformed input yields `Err`.
/// Used only by `parse_data_url` so this is intentionally minimal.
fn base64_decode(s: &str) -> ::std::result::Result<Vec<u8>, ()> {
    fn lookup(b: u8) -> ::std::option::Option<u8> {
        match b {
            b'A'..=b'Z' => ::std::option::Option::Some(b - b'A'),
            b'a'..=b'z' => ::std::option::Option::Some(b - b'a' + 26),
            b'0'..=b'9' => ::std::option::Option::Some(b - b'0' + 52),
            b'+' | b'-' => ::std::option::Option::Some(62),
            b'/' | b'_' => ::std::option::Option::Some(63),
            _ => ::std::option::Option::None,
        }
    }
    // Strip ASCII whitespace + padding so the length checks are simple.
    let filtered: Vec<u8> = s
        .bytes()
        .filter(|&b| !b.is_ascii_whitespace() && b != b'=')
        .collect();
    if filtered.is_empty() {
        return ::std::result::Result::Ok(Vec::new());
    }
    // Reject trailing partial groups other than the canonical 2/3-char tails.
    let tail = filtered.len() % 4;
    if tail == 1 {
        return Err(());
    }
    let mut out: Vec<u8> = Vec::with_capacity(filtered.len() * 3 / 4);
    let main_len = filtered.len() - tail;
    let mut i = 0;
    while i < main_len {
        let b0 = lookup(filtered[i]).ok_or(())?;
        let b1 = lookup(filtered[i + 1]).ok_or(())?;
        let b2 = lookup(filtered[i + 2]).ok_or(())?;
        let b3 = lookup(filtered[i + 3]).ok_or(())?;
        out.push((b0 << 2) | (b1 >> 4));
        out.push((b1 << 4) | (b2 >> 2));
        out.push((b2 << 6) | b3);
        i += 4;
    }
    if tail == 2 {
        let b0 = lookup(filtered[i]).ok_or(())?;
        let b1 = lookup(filtered[i + 1]).ok_or(())?;
        out.push((b0 << 2) | (b1 >> 4));
    } else if tail == 3 {
        let b0 = lookup(filtered[i]).ok_or(())?;
        let b1 = lookup(filtered[i + 1]).ok_or(())?;
        let b2 = lookup(filtered[i + 2]).ok_or(())?;
        out.push((b0 << 2) | (b1 >> 4));
        out.push((b1 << 4) | (b2 >> 2));
    }
    ::std::result::Result::Ok(out)
}

/// Drive a dynamic `import()` of a `data:` URL to completion. Same module
/// lifecycle as a file module: CompileModule1 → ModuleLink → ModuleEvaluate
/// → FinishDynamicModuleImport.
///
/// # Safety
/// Caller must hold a valid `raw_cx` and valid handles.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dynamic_import_data_url(
    raw_cx: *mut JSContext,
    specifier_str: &str,
    payload: &str,
    referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
    promise: Handle<*mut JSObject>,
) -> bool {
    let cache_key = format!("data-url:{}", specifier_str);
    if let ::std::option::Option::Some(existing) = module_cache_get(raw_cx, &cache_key)
        && !existing.is_null()
    {
        // Already loaded — resolve immediately via the synthetic path.
        let mod_val = mozjs::jsval::ObjectValue(existing);
        return unsafe { resolve_dynamic_promise_with_value(raw_cx, promise, mod_val) };
    }

    let Ok(c_filename) = CString::new(specifier_str.to_string()) else {
        return unsafe { reject_dynamic_promise(raw_cx, promise, "Invalid data URL filename") };
    };
    let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
    if opts.is_null() {
        return unsafe {
            reject_dynamic_promise(raw_cx, promise, "Internal: compile options alloc failed")
        };
    }
    let mut src = transform_str_to_source_text(payload);
    let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
    libc::free(opts as *mut _);
    if module.is_null() {
        return unsafe {
            reject_dynamic_promise(raw_cx, promise, "Failed to compile data URL module")
        };
    }

    set_module_private(raw_cx, module, specifier_str);
    module_cache_insert(raw_cx, &cache_key, module);

    rooted!(in(raw_cx) let module_root = module);
    if !mozjs_sys::jsapi::JS::ModuleLink(raw_cx, module_root.handle().into()) {
        rooted!(in(raw_cx) let null_root = ::std::ptr::null_mut::<JSObject>());
        return unsafe {
            mozjs_sys::jsapi::JS::FinishDynamicModuleImport(
                raw_cx,
                null_root.handle().into(),
                referencing_private,
                module_request,
                promise,
            )
        };
    }

    let mut eval_rval = UndefinedValue();
    let eval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut eval_rval,
    };
    let eval_ok = mozjs_sys::jsapi::JS::ModuleEvaluate(raw_cx, module_root.handle().into(), eval_h);
    mozjs_sys::jsapi::js::RunJobs(raw_cx);

    let evaluation_promise = if eval_ok && eval_rval.is_object() {
        eval_rval.to_object()
    } else {
        ::std::ptr::null_mut::<JSObject>()
    };
    rooted!(in(raw_cx) let eval_promise_root = evaluation_promise);

    mozjs_sys::jsapi::JS::FinishDynamicModuleImport(
        raw_cx,
        eval_promise_root.handle().into(),
        referencing_private,
        module_request,
        promise,
    )
}

/// Drive a dynamic `import()` of a builtin module to completion.
///
/// Builds (or reuses) the synthetic ESM module for `stripped`, links and
/// evaluates it, then calls `FinishDynamicModuleImport`. SM's internal
/// `FinishDynamicModuleImport` resolves the user-facing promise with the
/// module namespace object — which carries the `default` property (and the
/// named bindings), so `await import("zlib")` returns an object satisfying
/// `"default" in mod` and exposing `mod.gzipSync` etc.
///
/// This mirrors the static-import path in `host_resolve_imported_module`
/// (same cache key, same synthetic source), keeping the two flows consistent.
///
/// # Safety
/// Caller must hold a valid `raw_cx` and valid handles.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dynamic_import_builtin(
    raw_cx: *mut JSContext,
    stripped: &str,
    referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
    promise: Handle<*mut JSObject>,
) -> bool {
    // Step 1: locate or build the synthetic module.
    let cache_key = format!("builtin:{}", stripped);
    let mut module = ::std::ptr::null_mut::<JSObject>();
    let mut already_evaluated = false;
    if let Some(existing) = module_cache_get(raw_cx, &cache_key)
        && !existing.is_null()
    {
        module = existing;
        // SM module objects remember their status. Once a module is in the
        // Evaluated state, re-running ModuleLink/ModuleEvaluate is illegal and
        // can crash SM. Track this so we skip the link/evaluate step below and
        // drive FinishDynamicModuleImport straight from the existing namespace.
        // We approximate "already evaluated" with the cache presence — the only
        // way a module enters MODULE_CACHE is via this function (after
        // successful evaluation) or via host_resolve_imported_module (which
        // also evaluates). Either way, the module is at least Linked.
        already_evaluated = true;
    }
    if module.is_null() {
        // Build the synthetic ESM source. `bun:test` and `harness` have
        // hand-written sources in the resolve hook; everything else uses
        // `builtin_esm_source` (which always emits `export default _m`).
        let esm_src: ::std::borrow::Cow<'static, str> = match stripped {
            // node:test — same gated CJS bridge as the static-import path
            // (see NODE_TEST_ESM_BRIDGE). The previous generic fallback only
            // exposed `default`, so `const { test } = await import("node:test")`
            // linked to a missing export.
            "test" => ::std::borrow::Cow::Borrowed(NODE_TEST_ESM_BRIDGE),
            "bun:test" => ::std::borrow::Cow::Borrowed(
                r#"var _m = require("bun:test");
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
"#,
            ),
            "harness" => ::std::borrow::Cow::Borrowed(
                r#"var _m = require("harness");
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
"#,
            ),
            _ => ::std::borrow::Cow::Borrowed(builtin_esm_source(stripped)),
        };

        let c_filename = CString::new(format!("<builtin:{}>", stripped))
            .unwrap_or_else(|_| CString::new("<builtin>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return unsafe {
                reject_dynamic_promise(raw_cx, promise, "Internal: compile options alloc failed")
            };
        }
        let mut src = transform_str_to_source_text(&esm_src);
        let compiled = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        libc::free(opts as *mut _);
        if compiled.is_null() {
            return unsafe {
                reject_dynamic_promise(
                    raw_cx,
                    promise,
                    "Internal: builtin module compilation failed",
                )
            };
        }
        let priv_url = format!("builtin:{}", stripped);
        unsafe { set_module_private(raw_cx, compiled, &priv_url) };
        module_cache_insert(raw_cx, &cache_key, compiled);
        module = compiled;
    }

    // Step 2: link + evaluate (only if the module hasn't been linked/eval'd
    // before). Re-entering ModuleLink/ModuleEvaluate on an evaluated module
    // is illegal in SM and crashes the host process.
    rooted!(in(raw_cx) let module_root = module);
    if !already_evaluated {
        if !unsafe { mozjs_sys::jsapi::JS::ModuleLink(raw_cx, module_root.handle().into()) } {
            // BCE-20260619-012: null_obj must be rooted before creating Handle
            rooted!(in(raw_cx) let null_root = ::std::ptr::null_mut::<JSObject>());
            return unsafe {
                mozjs_sys::jsapi::JS::FinishDynamicModuleImport(
                    raw_cx,
                    null_root.handle().into(),
                    referencing_private,
                    module_request,
                    promise,
                )
            };
        }

        let mut eval_rval = UndefinedValue();
        let eval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut eval_rval,
        };
        let eval_ok = unsafe {
            mozjs_sys::jsapi::JS::ModuleEvaluate(raw_cx, module_root.handle().into(), eval_h)
        };
        unsafe { mozjs_sys::jsapi::js::RunJobs(raw_cx) };

        // Capture the evaluation promise for FinishDynamicModuleImport. When
        // ModuleEvaluate succeeds synchronously the return is undefined (not a
        // promise) — that's still a success state.
        let evaluation_promise: *mut JSObject = if eval_ok && eval_rval.is_object() {
            eval_rval.to_object()
        } else {
            ::std::ptr::null_mut::<JSObject>()
        };
        rooted!(in(raw_cx) let eval_promise_root = evaluation_promise);
        return unsafe {
            mozjs_sys::jsapi::JS::FinishDynamicModuleImport(
                raw_cx,
                eval_promise_root.handle().into(),
                referencing_private,
                module_request,
                promise,
            )
        };
    }

    // Step 3 (already-evaluated path): the module was already linked and
    // evaluated. Calling ModuleLink/ModuleEvaluate again would crash SM. We
    // fetch the module namespace directly and resolve the user-facing
    // promise with it. The namespace object exposes the same shape (named
    // exports + `default`) that FinishDynamicModuleImport would resolve to.
    let ns =
        unsafe { mozjs_sys::jsapi::JS::GetModuleNamespace(raw_cx, module_root.handle().into()) };
    if ns.is_null() {
        return unsafe {
            reject_dynamic_promise(raw_cx, promise, "Internal: failed to get module namespace")
        };
    }
    let ns_val = mozjs::jsval::ObjectValue(ns);
    unsafe { resolve_dynamic_promise_with_value(raw_cx, promise, ns_val) }
}

fn resolve_specifier(specifier: &str, base_dir: Option<&Path>) -> ::std::option::Option<PathBuf> {
    // External resolver (bun_resolver) takes priority
    if let Some(result) = EXTERNAL_RESOLVER.with(|r| {
        r.borrow()
            .and_then(|resolver| resolver(specifier, base_dir))
    }) {
        return Some(result);
    }

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

fn resolve_node_modules(
    specifier: &str,
    base_dir: Option<&Path>,
) -> ::std::option::Option<PathBuf> {
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
        // @trace REQ-ENG-005 — prefer SWC's official TS→JS strip (full grammar
        // coverage: generics, type annotations, unions, interfaces, type
        // aliases, `import type`, `declare`, enums). Fall back to the legacy
        // hand-written stripper on SWC failure (defensive, never hard-fails).
        "ts" | "tsx" => {
            let fname = path.to_str().unwrap_or("");
            match bun_transpiler::transpile_ts(source, fname) {
                Ok(out) => out,
                Err(_) => strip_ts_impl(source),
            }
        }
        "jsx" => strip_jsx_types(source),
        _ => source.to_string(),
    }
}

/// Strip TypeScript-specific syntax from a `.ts` or `.tsx` source.
fn strip_ts_impl(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let lines = source.lines();
    let _in_interface = false;
    let _in_type_alias = false;
    let _in_enum = false;
    let _brace_depth: i32 = 0;
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
    let mut result = line.to_string();

    // Remove `as Type` assertions — handle `<expr> as <Type>`
    result = strip_as_assertions(&result);

    // Remove return type annotations: `): ReturnType {` → `) {`
    result = strip_return_types(&result);

    // Remove type annotations from parameters and variable declarations
    result = strip_param_types(&result);

    // Remove generic type parameters: `<T>`, `<T extends U>`, etc.
    result = strip_generics(&result);

    // Remove call-site generic type arguments: `id<number>(...)`,
    // `obj.method<T>(...)`, `Map<string, number>(...)`. Must run AFTER
    // strip_generics so function/class declarations are already stripped —
    // otherwise the definition's `<T>` would also match here.
    // @trace REQ-ENG-006 — TS generic call-site support.
    result = strip_call_site_generics(&result);

    // Remove `implements Type` from class declarations
    result = strip_implements(&result);

    // Remove non-null assertion `!` before `.`
    result = strip_non_null_assertion(&result);

    result
}

/// Strip call-site generic type arguments like `id<number>(args)`.
///
/// Scans for the pattern `<TypeList>(` where `<` is immediately preceded by
/// a JS identifier character (so we skip relational `<` like `a < b`) and
/// `>` is immediately followed by `(`. This avoids stripping comparison
/// operators (`a < b`, `if (x < 2)`) and JSX-like tags.
///
/// `<TypeList>` may contain nested `<>`, `,`, identifiers, `extends`, `=`,
/// string literals, and `?`/`&`/`|` (constraint & union syntax). We track
/// string state and bracket depth so those don't confuse the matcher.
fn strip_call_site_generics(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(s.len());
    let mut i = 0;

    while i < len {
        let c = chars[i];

        // Skip string literals (don't match inside them).
        if c == '\'' || c == '"' || c == '`' {
            let delim = c;
            result.push(c);
            i += 1;
            while i < len {
                let cc = chars[i];
                result.push(cc);
                i += 1;
                if cc == '\\' && i < len {
                    // Escape — copy next char verbatim.
                    result.push(chars[i]);
                    i += 1;
                    continue;
                }
                if cc == delim {
                    break;
                }
            }
            continue;
        }

        // Skip line comments (// ...).
        if c == '/' && i + 1 < len && chars[i + 1] == '/' {
            while i < len && chars[i] != '\n' {
                result.push(chars[i]);
                i += 1;
            }
            continue;
        }
        // Skip block comments (/* ... */).
        if c == '/' && i + 1 < len && chars[i + 1] == '*' {
            result.push(chars[i]);
            result.push(chars[i + 1]);
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                result.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                result.push(chars[i]);
                result.push(chars[i + 1]);
                i += 2;
            }
            continue;
        }

        // Candidate generic arg list: identifier-char before `<`, balanced
        // `<>`, and `(` immediately after closing `>`.
        if c == '<' && !result.is_empty() {
            let last = result.chars().last().unwrap();
            if last.is_ascii_alphanumeric()
                || last == '_'
                || last == '.'
                || last == ')'
                || last == ']'
            {
                // Try to match a balanced <...> followed by `(`.
                if let Some(close_rel) = find_matching_gt(&chars, i) {
                    let after_close = close_rel + 1;
                    // Skip whitespace between `>` and `(`.
                    let mut j = after_close;
                    while j < len && (chars[j] == ' ' || chars[j] == '\t') {
                        j += 1;
                    }
                    if j < len && chars[j] == '(' {
                        // Confirmed call-site generic. Drop the `<...>` segment
                        // (don't push it), advance past `>`.
                        i = after_close;
                        continue;
                    }
                }
            }
        }

        result.push(c);
        i += 1;
    }

    result
}

/// Given chars[start] == '<', return the relative index of the matching '>'
/// that closes the generic arg list, or None if unbalanced / contains a
/// character that proves this isn't a generic list (newline-terminated, etc.).
fn find_matching_gt(chars: &[char], start: usize) -> Option<usize> {
    let len = chars.len();
    let mut depth: i32 = 0;
    let mut i = start;
    let mut in_str: Option<char> = None;
    while i < len {
        let c = chars[i];
        if let Some(delim) = in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == delim {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                in_str = Some(c);
            }
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            // Newlines inside `<...>` almost never appear in call-site
            // generics (and never in single-line strip context). Bail out to
            // avoid eating multi-line comparisons.
            '\n' if depth > 0 => return None,
            '{' | ';' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Strip `as Type` assertions from a string.
fn strip_as_assertions(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if i + 4 < len
            && chars[i] == ' '
            && chars[i + 1] == 'a'
            && chars[i + 2] == 's'
            && chars[i + 3] == ' '
        {
            let type_start = i + 4;
            let mut type_end = type_start;
            while type_end < len {
                let c = chars[type_end];
                if c.is_alphanumeric()
                    || c == '_'
                    || c == '.'
                    || c == '<'
                    || c == '>'
                    || c == '['
                    || c == ']'
                    || c == '|'
                    || c == '&'
                    || c == ' '
                    || c == '-'
                    || c == '\''
                {
                    type_end += 1;
                } else {
                    break;
                }
            }
            if type_end >= len
                || chars[type_end] == ';'
                || chars[type_end] == ')'
                || chars[type_end] == ','
                || chars[type_end] == '}'
                || chars[type_end] == '\n'
                || chars[type_end] == ' '
                || chars[type_end] == '='
                || chars[type_end] == ')'
            {
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

    loop {
        let changed = false;
        if let Some(pos) = result.find("): ") {
            let after_paren = pos + 3;
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
                let trimmed_type = type_str.trim();
                if !trimmed_type.is_empty()
                    && !trimmed_type.starts_with("//")
                    && !trimmed_type.starts_with("/*")
                {
                    // Keep everything up to and including `)` (pos+1), drop the
                    // `: ReturnType` segment, resume at `end` (`{`/`;`/`=>`).
                    // Previously this used `pos+2` which leaked a stray `:`
                    // and produced `):{` instead of `){`.
                    result = format!("{}{}", &result[..pos + 1], &result[end..]);
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

    // Variable declarations: `const/let/var name: Type =`
    for kw in &["const ", "let ", "var "] {
        if let Some(pos) = result.find(kw) {
            let after_kw = pos + kw.len();
            if let Some(colon_pos) = result[after_kw..].find(':') {
                let abs_colon = after_kw + colon_pos;
                if let Some(eq_pos) = result[abs_colon..].find('=') {
                    let abs_eq = abs_colon + eq_pos;
                    if abs_eq + 1 < result.len() && result.as_bytes()[abs_eq + 1] != b'=' {
                        if abs_eq + 1 >= result.len() || result.as_bytes()[abs_eq + 1] != b'>' {
                            result = format!("{}{}", &result[..abs_colon], &result[abs_eq..]);
                        }
                    }
                }
            }
        }
    }

    // Parameter types: `name: Type,` and `name: Type)` and `name?: Type`
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
            let mut j = i + 1;
            while j < len && chars[j] == ' ' {
                j += 1;
            }
            let mut bracket_depth: usize = 0;
            while j < len {
                let tc = chars[j];
                if tc == '<' {
                    bracket_depth += 1;
                    j += 1;
                    continue;
                }
                if tc == '>' {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    j += 1;
                    continue;
                }
                if bracket_depth == 0
                    && (tc == ',' || tc == ')' || tc == '=' || tc == '{' || tc == '\n')
                {
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
    let mut result = s.to_string();

    for kw in &["function ", "class ", "interface "] {
        if let Some(pos) = result.find(kw) {
            let after_kw = pos + kw.len();
            let rest = &result[after_kw..];
            let mut name_end = 0;
            while name_end < rest.len()
                && (rest.as_bytes()[name_end].is_ascii_alphanumeric()
                    || rest.as_bytes()[name_end] == b'_')
            {
                name_end += 1;
            }
            if name_end < rest.len() && rest.as_bytes()[name_end] == b'<' {
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
        if let Some(_brace) = after.find('{') {
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

/// Surface a module evaluation failure hidden inside the evaluation promise.
///
/// SM contract (`js/src/vm/Modules.cpp`, `ModuleEvaluate`): the out-value is
/// ALWAYS the module's top-level capability promise. A top-level `throw` (or
/// a top-level-await rejection settling during the job-queue drain) makes
/// `ModuleEvaluate` return `true` with a REJECTED promise — the error is
/// captured into the module record and cleared from the context. Without this
/// check the error is silently swallowed and callers see a successful
/// evaluation (module_loader defect: `bao run foo.mjs` exit 0 on throw,
/// worker bootstrap failures invisible, test files passing vacuously).
///
/// Rejection is unwrapped via `ThrowOnModuleEvaluationFailure`
/// (`ThrowModuleErrorsSync`), which moves the rejection reason onto the
/// context as a pending exception and marks the settled promise handled (so
/// the rejection tracker does not double-report), then extracted through the
/// same path as compile/link errors. A still-pending promise (a top-level
/// await that outlives the drains) is NOT an error — the evaluation simply
/// outlives this call.
///
/// Must be called after the job queue has been drained (post-evaluate and
/// post post-eval-hook), while `rval` — the evaluation promise — is still
/// rooted and we are still inside the module's realm.
fn check_module_evaluation_promise(
    realm_cx: &mut mozjs::context::JSContext,
    rval: mozjs::rust::Handle<Value>,
) -> ::std::result::Result<(), JsError> {
    if !rval.get().is_object() {
        return ::std::result::Result::Ok(());
    }
    rooted!(&in(realm_cx) let promise = rval.get().to_object());
    unsafe {
        if !IsPromiseObject(promise.handle()) {
            return ::std::result::Result::Ok(());
        }
        match GetPromiseState(promise.handle()) {
            PromiseState::Rejected => {
                // ThrowModuleErrorsSync: rejected promise → the rejection
                // reason becomes the pending exception, return value false.
                ThrowOnModuleEvaluationFailure(
                    realm_cx,
                    promise.handle(),
                    ModuleErrorBehaviour::ThrowModuleErrorsSync,
                );
                ::std::result::Result::Err(extract_module_error(realm_cx))
            }
            _ => ::std::result::Result::Ok(()),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::env;
    use ::std::fs;

    #[test]
    fn extract_field_basic() {
        let json = r#"{"main": "index.js"}"#;
        assert_eq!(
            extract_json_string_field(json, "main"),
            Some("index.js".into())
        );
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
        assert_eq!(
            extract_json_string_field(json, "main"),
            Some("app.js".into())
        );
    }

    #[test]
    fn extract_field_module_fallback() {
        let json = r#"{"module": "esm/index.mjs"}"#;
        assert_eq!(
            extract_json_string_field(json, "module"),
            Some("esm/index.mjs".into())
        );
    }

    #[test]
    fn extract_field_multiple_fields() {
        let json = r#"{"name": "pkg", "main": "src/index.ts", "version": "1.0"}"#;
        assert_eq!(
            extract_json_string_field(json, "main"),
            Some("src/index.ts".into())
        );
        assert_eq!(extract_json_string_field(json, "name"), Some("pkg".into()));
    }

    #[test]
    fn extract_field_empty_value() {
        let json = r#"{"main": ""}"#;
        assert_eq!(extract_json_string_field(json, "main"), Some("".into()));
    }

    #[test]
    fn extract_field_nested_json() {
        let json = r#"{"name": "pkg", "exports": {"main": "dist/index.js"}}"#;
        let result = extract_json_string_field(json, "main");
        assert!(
            result.is_some(),
            "parser finds first occurrence of 'main' key"
        );
    }

    #[test]
    fn extract_field_value_with_escapes() {
        let json = r#"{"main": "path/with\"quote"}"#;
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
        assert_eq!(
            extract_json_string_field(json, "main"),
            Some("lib/index.js".into())
        );
    }

    #[test]
    fn extract_field_duplicate_keys() {
        let json = r#"{"main": "first.js", "main": "second.js"}"#;
        assert_eq!(
            extract_json_string_field(json, "main"),
            Some("first.js".into())
        );
    }

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
        let dir = env::temp_dir().join("bao_test_resolve_bare");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let result = resolve_specifier("nonexistent-pkg", Some(&dir));
        assert!(result.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

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
        assert!(
            output.contains("function identity"),
            "output was: {}",
            output
        );
        assert!(
            !output.contains("<T>"),
            "generic decl not stripped, output was: {}",
            output
        );
    }

    #[test]
    fn strip_generic_call_site() {
        // id<number>(42) — call-site type argument must be removed so SM does
        // not parse `<number>` as a JSX/cast.
        let input = "function id<T>(x: T): T { return x; }\nconst result = id<number>(42);";
        let output = strip_ts_impl(input);
        assert!(
            !output.contains("id<number>"),
            "call-site generic not stripped, output was: {}",
            output
        );
        assert!(
            output.contains("id(42)"),
            "call site mangled, output was: {}",
            output
        );
    }

    #[test]
    fn strip_generic_call_site_multi() {
        let input = "const m = map<string, number>(arr);";
        let output = strip_ts_impl(input);
        assert!(
            !output.contains("<string, number>"),
            "multi-arg generic not stripped, output was: {}",
            output
        );
    }

    #[test]
    fn strip_generic_call_site_preserves_comparison() {
        // `a < b` must NOT be stripped even though it matches `<...>` shape-ish.
        let input = "const ok = a < b && b > c;";
        let output = strip_ts_impl(input);
        assert!(
            output.contains("a < b"),
            "comparison stripped, output was: {}",
            output
        );
        assert!(
            output.contains("b > c"),
            "comparison stripped, output was: {}",
            output
        );
    }

    #[test]
    fn strip_generic_call_site_preserves_if_less_than() {
        let input = "if (x < 10) { return; }";
        let output = strip_ts_impl(input);
        assert!(
            output.contains("x < 10"),
            "if comparison stripped, output was: {}",
            output
        );
    }

    #[test]
    fn strip_generic_call_site_chained() {
        let input = "const r = obj.method<number>(42);";
        let output = strip_ts_impl(input);
        assert!(
            !output.contains("<number>"),
            "method generic not stripped, output was: {}",
            output
        );
        assert!(output.contains("obj.method(42)"), "output was: {}", output);
    }

    #[test]
    fn strip_generic_call_site_nested() {
        let input = "const r = foo<Array<number>>(x);";
        let output = strip_ts_impl(input);
        assert!(
            !output.contains("<Array<number>>"),
            "nested generic not stripped, output was: {}",
            output
        );
        assert!(output.contains("foo(x)"), "output was: {}", output);
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
        let input =
            "declare module 'fs' {\n  export function readFileSync(path: string): Buffer;\n}";
        let output = strip_ts_impl(input);
        assert!(!output.contains("declare module"), "output was: {}", output);
    }

    #[test]
    fn preserves_plain_js() {
        let input = "const x = 42;\nfunction hello(name) { return 'Hello ' + name; }";
        let output = strip_ts_impl(input);
        assert!(output.contains("const x = 42"), "output was: {}", output);
        assert!(
            output.contains("function hello(name)"),
            "output was: {}",
            output
        );
    }

    #[test]
    fn strip_typescript_routing_function() {
        let input = "const x: number = 42;\nexport default x;";
        let output = strip_ts_impl(input);
        assert!(output.contains("const x"), "output was: {}", output);
        assert!(output.contains("42"), "output was: {}", output);
        assert!(
            output.contains("export default x"),
            "output was: {}",
            output
        );
    }

    // @trace REQ-ENG-005 — ESM builtin named exports.
    // GAP-2: `import { Buffer } from "buffer"` must work. The builtin ESM
    // source is generated with static `export var` declarations so SM's
    // linker can resolve named imports at link time.
    #[test]
    fn builtin_esm_source_emits_named_exports_for_buffer() {
        let src = builtin_esm_source("buffer");
        assert!(
            src.contains("require(\"buffer\")"),
            "must require builtin: {}",
            src
        );
        assert!(
            src.contains("export var Buffer = _m.Buffer;"),
            "must export Buffer: {}",
            src
        );
        assert!(
            src.contains("export default _m;"),
            "must have default export: {}",
            src
        );
    }

    #[test]
    fn builtin_esm_source_emits_named_exports_for_crypto_fs_events() {
        for (name, sentinel) in [
            ("crypto", "createHash"),
            ("fs", "readFile"),
            ("events", "EventEmitter"),
        ] {
            let src = builtin_esm_source(name);
            assert!(
                src.contains(&format!("export var {} = _m.{};", sentinel, sentinel)),
                "builtin {} must export {}: {}",
                name,
                sentinel,
                src
            );
        }
    }

    #[test]
    fn builtin_esm_source_unknown_builtin_falls_back_to_default_only() {
        // 'net' is a known builtin; 'unknown-builtin-xyz' is not in
        // builtin_named_exports and yields no named exports, only default.
        let src = builtin_esm_source("net");
        assert!(src.contains("export default _m;"));
        // Unknown name returns empty named list — only default + require.
        let unknown = builtin_named_exports("does-not-exist-xyz");
        assert!(
            unknown.is_empty(),
            "unknown builtin should have no named exports"
        );
    }
}
