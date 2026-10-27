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
    CompileModule1, GetPromiseState, IsPromiseObject, JS_GetModulePrivate, JS_GetRuntime,
    JS_GetScriptPrivate, ModuleEvaluate, ModuleLink, ThrowOnModuleEvaluationFailure,
};
use mozjs::rust::{
    CompileOptionsWrapper, Runtime, SIMPLE_GLOBAL_CLASS, transform_str_to_source_text,
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

/// Single source of the module-cache key form: the CANONICALIZED filesystem
/// path as a string. Canonicalization resolves symlinks and `.`/`..`
/// components, so `./x`, `sub/../x` and `/abs/x` spellings of one file
/// collapse to one key. Idempotent on an already-canonical path. Used by
/// both the import hook (`load_module_record_sync`) and the eval entry
/// registration below — the two sides MUST derive identical keys or the
/// single-instance contract breaks (Node/spec: same URL ⇒ same module
/// record).
fn module_cache_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// e136 (REQ-ENG-001): register a directly-compiled ENTRY module in the
/// module cache. The eval_module* entries compile the entry from the
/// caller's source; without this registration the entry is invisible to the
/// import hook, so any by-path import of the entry (a dynamic
/// `import("./main.mjs")` from its own body, or a dependent's static
/// back-import during graph load) misses the cache and compiles a SECOND
/// instance from disk — the entry body evaluates twice and namespace
/// identity diverges (an import-observable fingerprint vs Node). Must run
/// BEFORE graph loading (`load_requested_modules_sync`): the back-import
/// fires during that pass. Per-realm semantics come free from the cache's
/// global-property store (`module_cache_insert` targets the current global).
///
/// e138: also the require(esm) entry registration (require.rs
/// `load_esm_module`) — same form, same single-source key; require and
/// import share one module registry (Node ≥22 require(esm) semantics).
pub fn register_entry_module_cache(raw_cx: *mut JSContext, module: *mut JSObject, abs_filename: &Path) {
    let cache_key = module_cache_key(abs_filename);
    module_cache_insert(raw_cx, &cache_key, module);
}

/// e138 (REQ-ENG-001): require(esm) bridge — look up the module cache by
/// absolute path under the SAME single-source key form the import hook
/// (`load_module_record_sync`) uses. A hit means the path was already
/// compiled into a module record (by the import hook, or an earlier
/// eval/require entry); the caller must serve that instance instead of
/// compiling a second one (Node ≥22 require(esm): require and import share
/// one module registry; same URL ⇒ one module record). The path may be any
/// spelling — `module_cache_key` canonicalizes (idempotent on canonical).
pub fn lookup_entry_module_cache(raw_cx: *mut JSContext, abs_filename: &Path) -> Option<*mut JSObject> {
    let cache_key = module_cache_key(abs_filename);
    module_cache_get(raw_cx, &cache_key).filter(|m| !m.is_null())
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
    let url_jsstr = referencing_private.to_string();
    let Some(jsstr) = NonNull::new(url_jsstr) else {
        return None;
    };
    let url = unsafe { mozjs::conversions::jsstr_to_string(&mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)), jsstr) };
    let path_str = url.strip_prefix("file://")?;
    // WHATWG pass-through percent-decode (inverse of [percent_encode_path]):
    // malformed escapes and `+` survive verbatim; the canonical byte-level
    // decoder lives in `bun_url::whatwg`.
    let decoded = String::from_utf8_lossy(&bun_url::whatwg::percent_decode(path_str.as_bytes()))
        .into_owned();
    let path = PathBuf::from(decoded);
    path.parent().map(|p| p.to_path_buf())
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
            // SM153 moduleloading: the SM140 three-hook face (Resolve for
            // static imports + DynamicImport for `import()`) collapsed into
            // the spec HostLoadImportedModule hook — the payload routes
            // completion (GraphLoadingStateRecordObject = static graph,
            // PromiseObject = dynamic import()), and the ENGINE performs
            // link+evaluate+resolve via ContinueDynamicImport.
            SetModuleLoadHook(rt, Some(host_load_imported_module));
            SetModuleMetadataHook(rt, Some(host_populate_import_meta));
        }
    }

    /// Register module hooks on servo's JSContext (parasitic mode).
    /// Gets JSRuntime from the JSContext pointer via JS_GetRuntime.
    pub fn init_thread_local(cx: &mozjs::context::JSContext) {
        let rt = unsafe { JS_GetRuntime(cx) };
        unsafe {
            // SM153 moduleloading: the SM140 three-hook face (Resolve for
            // static imports + DynamicImport for `import()`) collapsed into
            // the spec HostLoadImportedModule hook — the payload routes
            // completion (GraphLoadingStateRecordObject = static graph,
            // PromiseObject = dynamic import()), and the ENGINE performs
            // link+evaluate+resolve via ContinueDynamicImport.
            SetModuleLoadHook(rt, Some(host_load_imported_module));
            SetModuleMetadataHook(rt, Some(host_populate_import_meta));
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

        let options = crate::node_realm_options();

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

        // e136 (REQ-ENG-001): register the entry under the same
        // canonicalized-path key the import hook uses, so a self/back-import
        // during graph load resolves to THIS instance instead of compiling a
        // second record from disk (single-instance semantics).
        unsafe {
            register_entry_module_cache(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &abs_filename,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        // SM153: graph-load first (New -> Unlinked), then link.
        if !unsafe { load_requested_modules_sync(realm_cx.raw_cx(), module_obj.handle().into()) } {
            return Err(graph_load_error(realm_cx));
        }
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
        //
        // BCE (process 'exit' double dispatch): this loop used to call the
        // hook TWICE per iteration (condition + a bare call after sleep whose
        // return value was discarded). When the bare call returned false —
        // after dispatching 'exit' listeners — the loop fell back to the
        // condition, called the hook again, and dispatched a SECOND time.
        // Script parity (`JsContext::eval`): exactly one hook call per
        // iteration; jobs drain at the head so pump-resolved continuations
        // run before the next loop-alive verdict.
        if let Some(hook) = post_eval_hook {
            loop {
                drain_job_queue(realm_cx);
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
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

        let options = crate::node_realm_options();

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

        // e136 (REQ-ENG-001): register the entry under the same
        // canonicalized-path key the import hook uses, so a self/back-import
        // during graph load resolves to THIS instance instead of compiling a
        // second record from disk (single-instance semantics).
        unsafe {
            register_entry_module_cache(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &abs_filename,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        // SM153: graph-load first (New -> Unlinked), then link.
        if !unsafe { load_requested_modules_sync(realm_cx.raw_cx(), module_obj.handle().into()) } {
            return Err(graph_load_error(realm_cx));
        }
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
        // BCE (process 'exit' double dispatch): single hook call per
        // iteration — the former bare post-sleep call dispatched 'exit'
        // and then let the condition dispatch a second time. Jobs drain at
        // the head, mirroring the unbounded run paths above.
        if let Some(hook) = post_eval_hook {
            for _ in 0..1000 {
                drain_job_queue(realm_cx);
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
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

        // e136 (REQ-ENG-001): register the entry under the same
        // canonicalized-path key the import hook uses, so a self/back-import
        // during graph load resolves to THIS instance instead of compiling a
        // second record from disk (single-instance semantics).
        unsafe {
            register_entry_module_cache(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &abs_filename,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        // SM153: graph-load first (New -> Unlinked), then link.
        if !unsafe { load_requested_modules_sync(realm_cx.raw_cx(), module_obj.handle().into()) } {
            return Err(graph_load_error(realm_cx));
        }
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
        //
        // BCE (process 'exit' double dispatch): this loop used to call the
        // hook TWICE per iteration (condition + a bare call after sleep whose
        // return value was discarded). When the bare call returned false —
        // after dispatching 'exit' listeners — the loop fell back to the
        // condition, called the hook again, and dispatched a SECOND time.
        // Script parity (`JsContext::eval`): exactly one hook call per
        // iteration; jobs drain at the head so pump-resolved continuations
        // run before the next loop-alive verdict.
        if let Some(hook) = post_eval_hook {
            loop {
                drain_job_queue(realm_cx);
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
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

        // e136 (REQ-ENG-001): register the entry under the same
        // canonicalized-path key the import hook uses, so a self/back-import
        // during graph load resolves to THIS instance instead of compiling a
        // second record from disk (single-instance semantics).
        unsafe {
            register_entry_module_cache(
                realm_cx.raw_cx_no_gc(),
                module_obj.handle().get(),
                &abs_filename,
            )
        };

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        // SM153: graph-load first (New -> Unlinked), then link.
        if !unsafe { load_requested_modules_sync(realm_cx.raw_cx(), module_obj.handle().into()) } {
            return Err(graph_load_error(realm_cx));
        }
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
        // BCE (process 'exit' double dispatch): single hook call per
        // iteration — the former bare post-sleep call dispatched 'exit'
        // and then let the condition dispatch a second time. Jobs drain at
        // the head, mirroring the unbounded run paths above.
        if let Some(hook) = post_eval_hook {
            for _ in 0..1000 {
                drain_job_queue(realm_cx);
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
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
/// SM153: ModuleLink rejects status New — the New->Unlinked transition only
/// happens inside the engine's graph loading, so a directly-compiled module
/// must run JS::LoadRequestedModules (sync callbacks) before linking. bao's
/// ModuleLoadHook resolves synchronously, so the resolved callback fires
/// inline; a rejected load propagates the error as the pending exception.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn load_resolved_cb(_cx: *mut JSContext, _host_defined: Handle<Value>) -> bool {
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn load_rejected_cb(
    cx: *mut JSContext,
    _host_defined: Handle<Value>,
    error: Handle<Value>,
) -> bool {
    JS_SetPendingException(cx, error, JS::ExceptionStackBehavior::Capture);
    false
}

/// SM153: run the engine graph-loading pass for a directly-compiled module.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn load_requested_modules_sync(cx: *mut JSContext, module: Handle<*mut JSObject>) -> bool {
    rooted!(in(cx) let host_defined = UndefinedValue());
    JS::LoadRequestedModules(
        cx,
        module,
        host_defined.handle().into(),
        Some(load_resolved_cb),
        Some(load_rejected_cb),
    )
}

/// SM153 moduleloading face: the single HostLoadImportedModule hook.
///
/// SM 153 collapsed the SM140 three-hook face (ModuleResolveHook for static
/// imports + ModuleDynamicImportHook for `import()`) into this one hook
/// (js/public/Modules.h `ModuleLoadHook`). bao's loader still resolves
/// synchronously (builtin synthetic ESM / data: URL / file path — the exact
/// SM140 logic), then hands the module to `FinishLoadingImportedModule`,
/// whose `payload` argument routes completion:
///   - GraphLoadingStateRecordObject → ContinueModuleLoading (static graph)
///   - PromiseObject → ContinueDynamicImport (dynamic `import()`): the
///     ENGINE performs link + evaluate + promise resolution — but ONLY
///     that (Modules.cpp ContinueDynamicImport "Step 3" comment: "The
///     module dependencies has been loaded in the host layer, so we only
///     need to do _linkAndEvaluate_ part defined in the spec"). A fresh
///     record (status New) handed straight through makes
///     LinkAndEvaluateDynamicImport's JS::ModuleLink throw
///     JSMSG_BAD_MODULE_STATUS and the import() promise reject forever
///     (e139), so the dynamic payload drives the same synchronous graph
///     load the eval entry paths use before handing the record back; on an
///     already-loaded record InnerModuleLoading's status gate (New-only
///     entry) makes that drive a counter-decrement no-op.
/// `usePromise=true` preserves the SM140 job-based chaining (TLA-aware),
/// drained by bao's existing RunJobs points — behavior parity with the
/// removed SM140 flow where this file drove ModuleLink / ModuleEvaluate /
/// FinishDynamicModuleImport by hand (the BUG-ENG-365 machinery is now
/// engine-owned; re-evaluation safety included).
/// The referrer's private value (the file:// URL / `builtin:` / `data-url:`
/// key this loader sets via set_module_private) drives relative-specifier
/// base_dir derivation, exactly like SM140's `referencing_private` param.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_load_imported_module(
    raw_cx: *mut JSContext,
    referrer: Handle<*mut JSScript>,
    module_request: Handle<*mut JSObject>,
    _host_defined: Handle<Value>,
    payload: Handle<Value>,
    _line_number: u32,
    _column: ColumnNumberOneOrigin,
) -> bool {
    rooted!(in(raw_cx) let mut referencing_private = UndefinedValue());
    JS_GetScriptPrivate(referrer.get(), referencing_private.handle_mut().into());

    let module =
        load_module_record_sync(raw_cx, referencing_private.handle().into(), module_request);
    if module.is_null() {
        // Engine contract (js::HostLoadImportedModule): false + pending
        // exception routes the error through the payload — promise rejection
        // for dynamic import(), graph-loading failure for static imports.
        // Preserve the SM140 bao wording when the loader set no exception.
        if !JS_IsExceptionPending(raw_cx) {
            let specifier = GetModuleRequestSpecifier(raw_cx, module_request);
            let msg = if !specifier.is_null() {
                let s = mozjs::conversions::jsstr_to_string(
                    &mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)),
                    NonNull::new(specifier).expect("null-checked specifier"),
                );
                format!("Cannot find module '{}'", s)
            } else {
                "Module load error".to_string()
            };
            let mut cx_s =
                mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
            let c_msg = CString::new(msg)
                .unwrap_or_else(|_| CString::new("Cannot load module").unwrap());
            mozjs::error::throw_type_error(&mut cx_s, c_msg.as_ref());
        }
        return false;
    }

    rooted!(in(raw_cx) let module_root = module);

    // e139 (REQ-ENG-001): for a dynamic `import()` payload (a promise —
    // distinct from the GraphLoadingState payload of static graph loads,
    // where the ENGINE keeps driving InnerModuleLoading), SM153's
    // ContinueDynamicImport only performs link + evaluate: the host must
    // deliver the record with its graph load ALREADY complete. A fresh
    // record (status New) passed straight through fails
    // LinkAndEvaluateDynamicImport's JS::ModuleLink with
    // JSMSG_BAD_MODULE_STATUS ("module record has unexpected status: New")
    // and the import() promise rejects forever. Drive the same synchronous
    // graph load the eval entry paths use; on an already-registered record
    // (cache hit, Unlinked/Evaluated) InnerModuleLoading's status gate —
    // only status-New modules enter the loading pass — makes this a pure
    // counter-decrement no-op.
    let dynamic_payload = payload.get().is_object() && {
        rooted!(in(raw_cx) let payload_obj = payload.get().to_object());
        IsPromiseObject(payload_obj.handle())
    };
    if dynamic_payload && !load_requested_modules_sync(raw_cx, module_root.handle().into()) {
        // load_rejected_cb left the pending exception; returning false
        // routes the failure through the payload (promise rejection for
        // import(), graph-loading failure otherwise) via
        // js::HostLoadImportedModule's error path.
        return false;
    }

    mozjs_sys::jsapi::JS::FinishLoadingImportedModule(
        raw_cx,
        referrer,
        module_request,
        payload,
        module_root.handle().into(),
        true,
    )
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn load_module_record_sync(
    raw_cx: *mut JSContext,
    referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
) -> *mut JSObject {
    let specifier = unsafe { GetModuleRequestSpecifier(raw_cx, module_request) };
    if specifier.is_null() {
        return ::std::ptr::null_mut();
    }

    let specifier_str = mozjs::conversions::jsstr_to_string(&mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)),
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
        // Generic builtin modules: bridged via LIVE enumeration — the key set
        // is `require("<name>")`'s own string keys (incl. non-enumerable) at
        // bridge generation time (see builtin_bridge_source_live). No curated table:
        // keys the object exposes are exactly the keys the bridge declares,
        // so `import { open } from "fs/promises"` and every future builtin
        // surface resolves without per-builtin maintenance.
        _ if builtin_modules.contains(&stripped) => None,
        _ => None,
    };

    // Bridge candidates: the three static bridges above, plus every known
    // builtin (live-enumeration path). Cache check runs BEFORE the live
    // enumeration so a cache hit never pays the require/enumeration cost.
    let is_bridge_candidate = synthetic_esm.is_some() || builtin_modules.contains(&stripped);
    if is_bridge_candidate {
        // Check cache first — synthetic modules must be returned as the same object
        let cache_key = format!("builtin:{}", stripped);
        let cached = module_cache_get(raw_cx, &cache_key);
        if let Some(existing) = cached
            && !existing.is_null()
        {
            return existing;
        }

        let esm_src: String = match synthetic_esm {
            ::std::option::Option::Some(static_src) => static_src.to_string(),
            // Live-enumeration bridge. Fail-closed: Err means the pending
            // exception (missing require / throwing require / failed
            // enumeration) makes this import fail loudly — NO silent
            // default-only degradation.
            ::std::option::Option::None => {
                match builtin_bridge_source_live(raw_cx, stripped) {
                    ::std::result::Result::Ok(src) => src,
                    ::std::result::Result::Err(()) => return ::std::ptr::null_mut(),
                }
            }
        };

        let c_filename = CString::new(format!("<builtin:{}>", stripped))
            .unwrap_or_else(|_| CString::new("<builtin>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            // Fail-closed (same class as the CJS arm): a bridge candidate
            // without compile options must fail loudly, never silently fall
            // through to file-path resolution ("Cannot find module" for a
            // builtin = misleading diagnosis).
            let mut cx_s =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
            let msg =
                format!("Cannot compile builtin bridge '{stripped}' (no compile options)");
            let c_msg = CString::new(msg)
                .unwrap_or_else(|_| CString::new("Cannot compile builtin bridge").unwrap());
            mozjs::error::throw_type_error(&mut cx_s, c_msg.as_ref());
            return ::std::ptr::null_mut();
        }
        let mut src = transform_str_to_source_text(&esm_src);
        let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        mozjs::glue::DeleteCompileOptions(opts);
        if !module.is_null() {
            // BUG-ENG-365: attach private value to synthetic builtin modules
            let priv_url = format!("builtin:{}", stripped);
            set_module_private(raw_cx, module, &priv_url);
            module_cache_insert(raw_cx, &cache_key, module);
        }
        return module;
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
                mozjs::glue::DeleteCompileOptions(opts);
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
    // e136: key derivation is single-sourced in module_cache_key (the entry
    // registration below must produce the identical key form).
    let cache_key = module_cache_key(&canonical);

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
    // When an ESM `import` targets a CJS module (`.cjs` or `.js` lacking ESM
    // syntax with a `module.exports` shape), wrap it as a synthetic ESM whose
    // default export is `module.exports` and whose named exports are static
    // `export` declarations extracted from the CJS source via OXC AST
    // (`cjs_exports::extract_cjs_named_exports` — see cjs_compat_wrapper_source).
    // SM's ESM linker resolves named imports statically at ModuleLink time.
    if is_cjs_module(&canonical, &content) {
        let wrapper = cjs_compat_wrapper_source(&canonical, &content);
        let c_filename = CString::new(canonical.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<cjs-compat>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            // Fail-closed: cannot build compile options for an
            // authoritatively-CJS module. NEVER re-interpret the source as
            // native ESM below — that silently republishes a CJS module with
            // wrong flavor and swallows the real failure as spurious
            // missing-export link errors. Node parity: a CJS syntax error is
            // a loud SyntaxError, never a silent ESM recompile.
            let mut cx_s =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
            let msg = format!("Cannot compile CJS module {} (no compile options)", c_filename.to_string_lossy());
            let c_msg = CString::new(msg)
                .unwrap_or_else(|_| CString::new("Cannot compile CJS module").unwrap());
            mozjs::error::throw_type_error(&mut cx_s, c_msg.as_ref());
            return ::std::ptr::null_mut();
        }
        let mut src = transform_str_to_source_text(&wrapper);
        let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        mozjs::glue::DeleteCompileOptions(opts);
        if !module.is_null() {
            let priv_url = path_to_file_url(&canonical);
            set_module_private(raw_cx, module, &priv_url);
            module_cache_insert(raw_cx, &cache_key, module);
            return module;
        }
        // Fail-closed (BCE module-flavor misinterpretation class): the
        // wrapper failed to compile (e.g. the CJS source itself has a syntax
        // error). CompileModule1 left SM's pending exception (real
        // SyntaxError with line:col) — return null and let the hook's
        // `JS_IsExceptionPending` check surface it. The former
        // "treat as ESM below" fallback is DELETED: it recompiled the CJS
        // body as native ESM, silently producing a wrong-flavored module
        // whose import failures masked the true compile error.
        return ::std::ptr::null_mut();
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
        mozjs::glue::DeleteCompileOptions(opts);
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
export var suite = _m.suite;
export var before = _m.before;
export var after = _m.after;
export var beforeAll = _m.beforeAll;
export var afterAll = _m.afterAll;
export var beforeEach = _m.beforeEach;
export var afterEach = _m.afterEach;
export var mock = _m.mock;
export var assert = _m.assert;
export var run = _m.run;
// node v24 oracle (2026-10-20): the module's full named-export key set is
// [after, afterEach, assert, before, beforeEach, describe, expectFailure,
// getTestContext, it, mock, only, run, skip, snapshot, suite, test, todo].
// only/skip/todo are the top-level test-variant aliases, snapshot is the
// config-object face, expectFailure is the expected-failure registration,
// getTestContext returns the running body's TestContext. The former bridge
// exported only 14 of the 17 keys — the missing three broke every
// destructuring import of them at LINK time (static export check).
export var only = _m.only;
export var skip = _m.skip;
export var todo = _m.todo;
export var snapshot = _m.snapshot;
export var expectFailure = _m.expectFailure;
export var getTestContext = _m.getTestContext;
export default _m;
"#;

// @trace REQ-ENG-005 — bridge-reserved local names in the generated builtin
// ESM source. An export key colliding with one of these routes through the
// numbered-local alias form (a direct `export var _m;` would redeclare the
// bridge's own `var _m = require(...)` — SyntaxError).
const BRIDGE_RESERVED_IDENTS: &[&str] = &["_m"];

// @trace REQ-ENG-005 — Build the builtin-bridge ESM source from a key set.
// Pure core (unit-testable): `keys` are the live builtin object's own
// enumerable string keys — the single source of truth, enumerated at bridge
// generation time (no curated table to chase; keys the object does not have
// are never invented). Form (two-zone, mirroring cjs_compat_wrapper_source):
//
/// ```text
/// var _m = require("<name>");          // live object
/// // ── declaration zone ──
/// export var <k>;                      // identifier keys
/// let __bi_N;                          // string / reserved / colliding keys
/// export { __bi_N as "<k>" };          //   (ES2022 arbitrary namespace names)
/// export default _m;
/// // ── binding zone (evaluation time) ──
/// <k> = _m["<k>"];
/// __bi_N = _m["<k>"];
/// ```
///
/// `default` and `__esModule` are filtered; duplicates de-duplicated
/// (first-seen order kept). Identifier keys route to `export var`; keys that
/// are not valid JS identifiers, are reserved words, or collide with the
/// bridge's own locals route through the alias form.
fn builtin_esm_source_from_keys(name: &str, keys: &[String]) -> String {
    let spec = js_string_literal(name);
    let mut decls = String::new();
    let mut bindings = String::new();
    let mut seen: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
    let mut alias_idx = 0usize;
    for key in keys {
        if key == "default" || key == "__esModule" {
            continue;
        }
        if !seen.insert(key.as_str()) {
            continue;
        }
        if is_valid_js_identifier(key)
            && !is_reserved_word(key)
            && !BRIDGE_RESERVED_IDENTS.contains(&key.as_str())
            // A key literally shaped like our alias locals (`__bi_N`) must
            // also route through the alias form — via the identifier route it
            // would redeclare an alias local (`let __bi_0;` for some other
            // key + `export var __bi_0;` = SyntaxError). Through the alias
            // form it is collision-free: alias locals are indexed uniquely
            // per key, and a local whose text equals its own key still binds
            // correctly (`let __bi_3; __bi_3 = _m["__bi_3"]`).
            && !key.starts_with("__bi_")
        {
            decls.push_str(&format!("export var {key};\n"));
            bindings.push_str(&format!("{key} = _m[{}];\n", js_string_literal(key)));
        } else {
            let local = format!("__bi_{alias_idx}");
            alias_idx += 1;
            decls.push_str(&format!("let {local};\n"));
            decls.push_str(&format!(
                "export {{ {local} as {} }};\n",
                js_string_literal(key)
            ));
            bindings.push_str(&format!("{local} = _m[{}];\n", js_string_literal(key)));
        }
    }
    let mut src = String::with_capacity(96 + decls.len() + bindings.len());
    src.push_str(&format!("var _m = require({spec});\n"));
    src.push_str("// Static named exports — keys enumerated from the LIVE builtin object\n");
    src.push_str("// (own string keys incl. non-enumerable via GetPropertyKeys at bridge\n");
    src.push_str("// generation time); SM's linker resolves named imports statically.\n");
    src.push_str(&decls);
    src.push_str("export default _m;\n");
    src.push_str("// Bind named exports to the live object's values at evaluation time.\n");
    src.push_str(&bindings);
    src
}

// @trace REQ-ENG-005 — Thin cx wrapper: enumerate the LIVE builtin object's
// named-export keys and build the bridge source (see
// [`builtin_esm_source_from_keys`] for the pure core).
//
// Fail-closed contract: a missing/non-callable `globalThis.require`, a
// throwing `require` call, or a failed enumeration leaves SM's pending
// exception in place and returns `Err(())` — the caller must fail this
// import loudly (NO silent default-only degradation: that would republish
// "compatible" against a broken engine contract). `Ok` with an EMPTY key set
// is the honest empty-object case: the require call succeeded and the real
// object genuinely has no own string keys — default-only over a
// truth, not a degradation.
//
// Enumeration timing: this runs at bridge-generation time = SM ModuleLink of
// the importing module = user context with the realm fully initialised, so
// the require-driven lazy builtin materialization lands inside the designed
// M2 laziness window (link phase, engine thread, post realm init — safe by
// construction).
//
// Keys = own STRING keys INCLUDING non-enumerable (`GetPropertyKeys` with
// `JSITER_OWNONLY | JSITER_HIDDEN`; absence of `JSITER_SYMBOLS` ⇒ symbol-
// free). Beyond the `Object.keys` face on purpose: node builtin APIs are
// installed non-enumerable (farm diag3), and a named import must still be
// able to name them.
fn builtin_bridge_source_live(
    raw_cx: *mut JSContext,
    name: &str,
) -> ::std::result::Result<String, ()> {
    // Failure helper: throw a type error (engine-contract violation message)
    // and propagate Err(()) — the pending exception makes the import fail.
    let fail = |msg: String| -> ::std::result::Result<String, ()> {
        let mut cx_t = unsafe {
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx))
        };
        let c_msg = CString::new(msg)
            .unwrap_or_else(|_| CString::new("builtin bridge failure").unwrap());
        // throw_type_error is a SAFE wrapper (mozjs/src/error.rs) — no unsafe
        // shell.
        mozjs::error::throw_type_error(&mut cx_t, c_msg.as_ref());
        ::std::result::Result::Err(())
    };

    let global = unsafe { CurrentGlobalOrNull(raw_cx) };
    if global.is_null() {
        return fail(format!("builtin bridge: no current global while bridging '{name}'"));
    }

    let mut cx_s =
        unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
    rooted!(&in(cx_s) let global_root = global);

    // globalThis.require — must exist and be callable (engine contract).
    rooted!(&in(cx_s) let mut require_val = UndefinedValue());
    let got = unsafe {
        JS_GetProperty(
            raw_cx,
            global_root.handle().into(),
            c"require".as_ptr(),
            // `.into()`: the rooted! guard's lifetime-form rust::MutableHandle
            // adopts into the non-lifetime jsapi MutableHandle<Value> these
            // JSAPI wrappers expect (same form as the JS_GetModulePrivate
            // call above in this file).
            require_val.handle_mut().into(),
        )
    };
    if !got || !require_val.is_object() {
        return fail(format!(
            "builtin bridge: globalThis.require is not available while bridging '{name}'"
        ));
    }

    // require("<name>") — one string argument; live builtin object returned.
    let c_name = CString::new(name).unwrap_or_else(|_| CString::new("<builtin>").unwrap());
    let name_js = unsafe { JS_NewStringCopyZ(raw_cx, c_name.as_ptr()) };
    if name_js.is_null() {
        return fail(format!("builtin bridge: cannot allocate name string for '{name}'"));
    }
    // StringValue itself is safe (&JSString); the unsafe is precisely the
    // raw-pointer deref (name_js: *mut JSString, allocated above, alive).
    let name_val = mozjs::jsval::StringValue(unsafe { &*name_js });
    rooted!(&in(cx_s) let name_val_root = name_val);
    let args = [*name_val_root.handle()];
    let call_args = mozjs::jsapi::HandleValueArray {
        length_: 1,
        elements_: args.as_ptr(),
    };
    rooted!(&in(cx_s) let mut rval = UndefinedValue());
    let called = unsafe {
        JS_CallFunctionValue(
            raw_cx,
            global_root.handle().into(),
            require_val.handle().into(),
            &call_args,
            rval.handle_mut().into(),
        )
    };
    if !called {
        // require threw — its exception is pending; surface it verbatim.
        return ::std::result::Result::Err(());
    }
    if !rval.is_object() {
        return fail(format!(
            "builtin bridge: require('{name}') did not return an object (engine contract break)"
        ));
    }

    // Own string keys of the live object INCLUDING non-enumerable ones.
    // Diagnostic evidence (farm diag3, CJS require direct probe): the API
    // surface of node builtins is installed non-enumerable —
    // timers/promises' setTimeout/setImmediate/setInterval/scheduler and
    // module's createRequire/_resolveFilename are all own-but-NON-enumerable,
    // so plain JSITER_OWNONLY (= own + enumerable-only) saw none of them.
    // JSITER_HIDDEN (jsfriendapi.h: "Include nonenumerable properties") adds
    // them back; still no JSITER_SYMBOLS ⇒ string-only keys, matching what a
    // named import can name.
    rooted!(&in(cx_s) let builtin_obj = rval.to_object());
    let mut ids = mozjs::rust::IdVector::new(&mut cx_s);
    let enumerated = unsafe {
        mozjs::rust::wrappers2::GetPropertyKeys(
            &mut cx_s,
            builtin_obj.handle().into(),
            mozjs::jsapi::JSITER_OWNONLY | mozjs::jsapi::JSITER_HIDDEN,
            ids.handle_mut(),
        )
    };
    if !enumerated {
        // SM left a pending exception; ids must drop before returning so the
        // rooted vector is released on the same context.
        drop(ids);
        return ::std::result::Result::Err(());
    }

    // IdVector is the documented root for its contents ("Use
    // JS::Rooted<JS::IdVector>"); conversions to Rust strings run with no
    // intervening JS, so per-id extra rooting is unnecessary.
    let mut keys: Vec<String> = Vec::with_capacity(ids.len());
    for i in 0..ids.len() {
        let id = ids[i];
        if !id.is_string() {
            // Our flags exclude symbols; a non-string id here is impossible —
            // skip honestly rather than invent.
            continue;
        }
        let s = id.to_string();
        match NonNull::new(s) {
            Some(nn) => {
                let k = unsafe { mozjs::conversions::jsstr_to_string(&cx_s, nn) };
                keys.push(k);
            }
            None => {
                drop(ids);
                return fail(format!("builtin bridge: null key string while bridging '{name}'"));
            }
        }
    }
    drop(ids);

    ::std::result::Result::Ok(builtin_esm_source_from_keys(name, &keys))
}

/// Generate the CJS-compat ESM wrapper source that wraps a CJS module.exports
/// for ESM `import`. The wrapper:
///   1. Defines `module`, `exports` locals (CJS environment).
///   2. Evaluates the CJS source in a function scope.
///   3. Exports `default = module.exports`.
///   4. Statically re-exports named keys extracted from the AST via OXC
///      (`cjs_exports::extract_cjs_named_exports`) so SM's ESM linker can
///      resolve `import { foo } from './lib.cjs'` at ModuleLink time.
fn cjs_compat_wrapper_source(canonical: &Path, cjs_source: &str) -> String {
    // BCE (wrapper-escape pollution, GOLDEN-attributed 2026-10-26): the CJS
    // body is embedded at a NORMAL CODE POSITION of the wrapper — inside the
    // `(function() { ... }).call(exports);` IIFE — NOT inside a template
    // literal. The former `\` → `\\`, backtick → `\``, `${` → `\${` escape
    // set is the TEMPLATE-LITERAL escape set and is pure pollution at a code
    // position: every backtick in the CJS source (fs.js has 5, index.js 49 —
    // 69/96 GOLDEN fixtures import these) became `\`` = SyntaxError, and
    // `"a\nb"` string escapes were re-interpreted as literal backslash-n.
    // Node's own CJS wrapper `(function(exports, require, module,
    // __filename, __dirname) { <src> })` embeds the source with ZERO
    // escaping — an unbalanced-brace source is a syntax error in Node too,
    // so there is no injection-face difference. Consumer chain audited (all
    // call sites of this fn): the product goes straight through
    // `transform_str_to_source_text` (byte-faithful UTF-8 → JS source buffer)
    // into `JS::CompileModule1` — no layer ever re-embeds it in a template
    // literal or string literal, so no consumer gives these escapes meaning.
    let cjs_body = cjs_source;
    let filename_str = js_string_literal(&canonical.to_string_lossy());
    let dirname_str = js_string_literal(
        &canonical
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );

    // Extract named-export keys from the CJS source via OXC AST. These become
    // static `export let <key>;` declarations so SM's ModuleLink can resolve
    // named imports at link time (runtime Object.defineProperty on the
    // namespace object is invisible to the linker).
    let named_keys = crate::cjs_exports::extract_cjs_named_exports(cjs_source);
    let mut named_exports_src = String::new();
    let mut named_bindings_src = String::new();
    for (idx, key) in named_keys.iter().enumerate() {
        // Keys colliding with wrapper locals (module / exports / require /
        // __filename / __dirname / __cjs_*) cannot use `export let <key>;`
        // directly — that redeclares the wrapper's `var` and is a
        // SyntaxError. Same for non-identifier string keys AND for JS
        // reserved words (`export let class;` is a SyntaxError). All three
        // route through a collision-free numbered local +
        // `export { <local> as <name-or-"string"> }` (arbitrary module
        // namespace names, ES2022 — supported by SM153).
        if is_valid_js_identifier(key)
            && !WRAPPER_RESERVED_IDENTS.contains(&key.as_str())
            && !is_reserved_word(key)
        {
            named_exports_src.push_str(&format!("export let {key};\n"));
            named_bindings_src
                .push_str(&format!("{key} = __cjs_default[{}];\n", js_string_literal(key)));
        } else {
            // Numbered local: unique per wrapper, cannot collide with any
            // export key or wrapper local.
            let local = format!("__cjs_exp_{idx}");
            named_exports_src.push_str(&format!("let {local};\n"));
            named_exports_src.push_str(&format!(
                "export {{ {local} as {} }};\n",
                js_string_literal(key)
            ));
            named_bindings_src
                .push_str(&format!("{local} = __cjs_default[{}];\n", js_string_literal(key)));
        }
    }

    format!(
        r#"
var __cjs_module = {{ exports: {{}} }};
var module = __cjs_module;
var exports = __cjs_module.exports;
var __filename = {filename_str};
var __dirname = {dirname_str};
// BCE (ESM-imported CJS relative require base, GOLDEN-attributed 2026-10-26):
// the wrapped CJS body calls the GLOBAL require, whose thread-local
// REQUIRE_DIR is not set for this module (it was compiled as an ESM module
// via the import hook, not loaded through load_cjs_module). A relative
// `require('./sibling')` inside the body then resolved against the wrong
// base (or none) and threw MODULE_NOT_FOUND (GOLDEN A: 94/96 .mjs failed at
// common/index.js's require('./tmpdir')). Shadow the global with a
// module-local require that resolves relative to THIS module's own directory
// — created via module.createRequire(__filename) so builtin caching,
// extension probing and the CJS wrapper pipeline stay identical.
var require = (typeof globalThis.require === 'function' && globalThis.require.resolve)
    ? globalThis.require('module').createRequire(__filename)
    : globalThis.require;
(function() {{
{src}
}}).call(exports);
var __cjs_default = (module.exports === exports) ? exports : module.exports;
export default __cjs_default;
// Static named exports — keys extracted from the CJS source via OXC AST.
// Each is a live binding: `export let <key>` declares the export and the
// assignment below populates it from __cjs_default at evaluation time.
{named_exports_src}
// Bind named exports to the runtime values of __cjs_default.
{named_bindings_src}
"#,
        filename_str = filename_str,
        dirname_str = dirname_str,
        src = cjs_body,
        named_exports_src = named_exports_src,
        named_bindings_src = named_bindings_src,
    )
}

/// Check whether a string is a valid JS identifier (ECMAScript IdentifierName).
///
/// Used to decide whether a CJS export key can appear as a plain
/// `export let <ident>;` declaration or needs the
/// `export { <local> as "<string>" }` form.
fn is_valid_js_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Wrapper-local identifiers that a CJS export key must not collide with when
/// emitted as a plain `export let <key>;` — colliding keys route through the
/// numbered-local + `export { local as name }` alias form instead.
const WRAPPER_RESERVED_IDENTS: &[&str] = &[
    "module",
    "exports",
    "require",
    "__filename",
    "__dirname",
    "__cjs_module",
    "__cjs_default",
];

/// Reserved words that cannot appear in `export let <word>;` position.
/// Keys matching these route through the alias form
/// (`export { local as "<string>" }`) regardless of identifier validity.
fn is_reserved_word(s: &str) -> bool {
    matches!(
        s,
        "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "let"
            | "static"
            | "await"
            // Strict-mode reserved words — ES modules are always strict, so
            // `export let implements;` and friends are SyntaxErrors too.
            | "implements"
            | "interface"
            | "package"
            | "private"
            | "protected"
            | "public"
    )
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_populate_import_meta(
    raw_cx: *mut JSContext,
    module_record: Handle<*mut JSObject>,
    meta_object: Handle<*mut JSObject>,
) -> bool {
    // SM153: the hook receives the module record; the private value this
    // loader sets via set_module_private (file:// URL / builtin: / data-url:)
    // is fetched from it (SM140 passed the private value directly).
    rooted!(in(raw_cx) let mut private_root = UndefinedValue());
    JS_GetModulePrivate(module_record.get(), private_root.handle_mut().into());
    let private_value = private_root.handle();
    unsafe {
        // @trace REQ-ENG-006 [entity:JSContext] — Bun's import.meta extensions.
        // Bun exposes dir/path/file/main on import.meta (in addition to url).
        // - url:   file:// URL of this module (always present)
        // - path:  absolute filesystem path (__filename equivalent)
        // - dir:   parent directory of path (__dirname equivalent)
        // - file:  just the file name component of path
        // - main:  true if this module is the entry point
        let specifier = if private_value.is_string() {
            mozjs::conversions::jsstr_to_string(&mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)),
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
        let global_obj = CurrentGlobalOrNull(raw_cx);
        if !global_obj.is_null() {
            rooted!(in(raw_cx) let global_root = global_obj);
            let mut require_val = mozjs::jsval::UndefinedValue();
            let require_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut require_val,
            };
            let got = mozjs_sys::jsapi::JS_GetProperty(
                raw_cx,
                global_root.handle().into(),
                c"require".as_ptr(),
                require_h,
            );
            if got && require_val.is_object() {
                let require_obj_val = require_val;
                // BCE-20260619-012: require_obj_val may contain GC-managed object; must be rooted.
                rooted!(&in(wrapped_cx) let require_obj_root = require_obj_val);
                // Non-enumerable — `import.meta.require` is a function reference,
                // not a data property that should serialize.
                let _ = JS_DefineProperty(
                    raw_cx,
                    meta_object,
                    c"require".as_ptr(),
                    require_obj_root.handle().into(),
                    0,
                );
            }
        }
        true
    }
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
        // raises a `Base64DecodeError`. We implement a small RFC 4648 decoder
        // here rather than pulling in a base64 crate (percent-decoding, by
        // contrast, delegates to `bun_url::whatwg::percent_decode`).
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
        // Percent-decode inline payload per RFC 3986 §2.1 (pass-through form:
        // malformed escapes survive verbatim). Most JS module data: URLs are
        // sent un-encoded, but we must handle %20, %2C, %0A, etc. so that
        // quoted payloads still round-trip. Canonical decoder:
        // `bun_url::whatwg::percent_decode` — lossily finalized for `String`,
        // which is what JS expects for source text.
        let decoded =
            String::from_utf8_lossy(&bun_url::whatwg::percent_decode(payload.as_bytes()))
                .into_owned();
        Ok(decoded)
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
            skip_depth = scan_skip_depth(line, skip_depth);
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
            skip_depth = block_skip_depth(trimmed);
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
            skip_depth = block_skip_depth(trimmed);
            continue;
        }

        // Skip `import type` statements
        if trimmed.starts_with("import type ") || trimmed.starts_with("export type ") {
            continue;
        }

        // Skip `declare module`, `declare global`, etc.
        if trimmed.starts_with("declare ") {
            skip_depth = block_skip_depth(trimmed);
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

/// Advance a multi-line skip brace depth across `line`: every `{` re-deepens,
/// the `}` that re-balances to zero stops the scan (chars after it are
/// irrelevant — the skip ends either way).
fn scan_skip_depth(line: &str, mut skip_depth: i32) -> i32 {
    for ch in line.chars() {
        match ch {
            '{' => skip_depth += 1,
            '}' => {
                skip_depth -= 1;
                if skip_depth == 0 {
                    return skip_depth;
                }
            }
            _ => {}
        }
    }
    skip_depth
}

/// Skip depth after entering a multi-line block whose first line is
/// `trimmed`: the positive brace delta when the line opens more than it
/// closes, `1` when the block opens on a later line, `0` when the line is
/// already balanced (nothing to skip past).
fn block_skip_depth(trimmed: &str) -> i32 {
    if !trimmed.contains('{') {
        return 1;
    }
    let open = trimmed.matches('{').count() as i32;
    let close = trimmed.matches('}').count() as i32;
    if open > close {
        return open - close;
    }
    0
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
        result = strip_var_decl_type(&result, kw);
    }

    // Parameter types: `name: Type,` and `name: Type)` and `name?: Type`
    result = strip_paren_type_annotations(&result);

    result
}

/// Drop the `: Type` in a single `kw name: Type =` declaration (`kw` is
/// `const `/`let `/`var `). Returns `s` unchanged when no such annotated
/// declaration is present; the two `=`-lookalike guards mirror the original
/// nested form exactly (`==`, `=>`).
fn strip_var_decl_type(s: &str, kw: &str) -> String {
    let Some(pos) = s.find(kw) else {
        return s.to_string();
    };
    let after_kw = pos + kw.len();
    let Some(colon_pos) = s[after_kw..].find(':') else {
        return s.to_string();
    };
    let abs_colon = after_kw + colon_pos;
    let Some(eq_pos) = s[abs_colon..].find('=') else {
        return s.to_string();
    };
    let abs_eq = abs_colon + eq_pos;
    if abs_eq + 1 < s.len() && s.as_bytes()[abs_eq + 1] != b'=' {
        if abs_eq + 1 >= s.len() || s.as_bytes()[abs_eq + 1] != b'>' {
            return format!("{}{}", &s[..abs_colon], &s[abs_eq..]);
        }
    }
    s.to_string()
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
        result = strip_generics_kw(&result, kw);
    }
    result
}

/// Strip the `<...>` parameter list following `kw Name` in a single
/// function/class/interface declaration. Returns `s` unchanged when the
/// declaration has no generic list (or its `<` never closes).
fn strip_generics_kw(s: &str, kw: &str) -> String {
    let Some(pos) = s.find(kw) else {
        return s.to_string();
    };
    let after_kw = pos + kw.len();
    let rest = &s[after_kw..];
    let mut name_end = 0;
    while name_end < rest.len()
        && (rest.as_bytes()[name_end].is_ascii_alphanumeric()
            || rest.as_bytes()[name_end] == b'_')
    {
        name_end += 1;
    }
    if name_end >= rest.len() || rest.as_bytes()[name_end] != b'<' {
        return s.to_string();
    }
    if let Some(gt_pos) = angle_list_close_pos(rest, name_end) {
        let abs_gt = after_kw + gt_pos;
        return format!("{}{}", &s[..after_kw + name_end], &s[abs_gt..]);
    }
    s.to_string()
}

/// Index just past the `>` matching the `<` at `open_pos` in `s` (nesting
/// aware), or None when it never closes.
fn angle_list_close_pos(s: &str, open_pos: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, b) in s[open_pos..].bytes().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open_pos + idx + 1);
                }
            }
            _ => {}
        }
    }
    None
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

/// Graph-load failure error surface (BCE generic-error-masks-pending-
/// exception class): when a dependency module fails to load, the loader hook
/// leaves SM's real error (e.g. the SyntaxError of a broken CJS source, with
/// its own file:line:column) pending on the context. Surface THAT — the
/// generic "module graph loading failed" placeholder is the fallback ONLY
/// for the no-pending-exception case (hook returned null without an
/// exception, e.g. resolution failure paths that already reported).
fn graph_load_error(realm_cx: &mut mozjs::context::JSContext) -> JsError {
    if unsafe { JS_IsExceptionPending(realm_cx.raw_cx()) } {
        extract_module_error(realm_cx)
    } else {
        JsError {
            message: "module graph loading failed".into(),
            filename: String::new(),
            line: 0,
            column: 0,
            stack: None,
        }
    }
}

fn extract_module_error(cx: &mut mozjs::context::JSContext) -> JsError {
    rooted!(&in(cx) let mut exn = UndefinedValue());
    if let ::std::option::Option::Some(info) =
        mozjs::rust::error_info_from_exception_stack(cx, exn.handle_mut().into())
    {
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

    // @trace REQ-ENG-005 — ESM builtin named exports bridge (live-enumerated
    // keys form). GAP-2: `import { Buffer } from "buffer"` must work. The
    // bridge source is generated with static `export var` declarations so
    // SM's linker can resolve named imports at link time. The pure core
    // (builtin_esm_source_from_keys) is tested here against representative
    // key sets; the live-enumeration face (builtin_bridge_source_live) is
    // farm-validated (needs a real SM context + installed builtins).
    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn builtin_esm_source_from_keys_emits_named_exports_for_buffer() {
        let src = builtin_esm_source_from_keys("buffer", &[s("Buffer"), s("constants")]);
        assert!(
            src.contains("require(\"buffer\")"),
            "must require builtin: {}",
            src
        );
        assert!(
            src.contains("export var Buffer;"),
            "must declare Buffer export: {}",
            src
        );
        assert!(
            src.contains("Buffer = _m[\"Buffer\"];"),
            "must bind Buffer in the evaluation zone: {}",
            src
        );
        assert!(
            src.contains("export default _m;"),
            "must have default export: {}",
            src
        );
    }

    #[test]
    fn builtin_esm_source_from_keys_two_zone_ordering() {
        // Declaration zone (export var / export {}) must come BEFORE
        // `export default`, and the binding zone AFTER it — the cjs-wrapper
        // contract shape.
        let src = builtin_esm_source_from_keys("fs", &[s("readFile"), s("str-key")]);
        let decl_pos = src.find("export var readFile;").expect("decl zone");
        let alias_pos = src.find("export { __bi_0 as \"str-key\" };").expect("alias decl");
        let default_pos = src.find("export default _m;").expect("default");
        let bind_pos = src.find("readFile = _m[\"readFile\"];").expect("binding zone");
        assert!(decl_pos < default_pos && alias_pos < default_pos && default_pos < bind_pos);
    }

    #[test]
    fn builtin_esm_source_from_keys_filters_default_and_esmodule_and_dedups() {
        let src = builtin_esm_source_from_keys(
            "x",
            &[
                s("readFile"),
                s("default"),
                s("__esModule"),
                s("readFile"),
                s("open"),
            ],
        );
        assert!(src.contains("export var readFile;"));
        assert!(src.contains("export var open;"));
        assert_eq!(
            src.matches("export var readFile;").count(),
            1,
            "duplicate keys must dedup: {}",
            src
        );
        assert!(
            !src.contains("export var default;") && !src.contains("export var __esModule;"),
            "default/__esModule must be filtered: {}",
            src
        );
    }

    #[test]
    fn builtin_esm_source_from_keys_reserved_and_collision_route_alias() {
        // Reserved word (`class`) and bridge-local collision (`_m`) must both
        // route through the numbered-local alias form — a direct
        // `export var class;` / `export var _m;` is a SyntaxError.
        let src = builtin_esm_source_from_keys("x", &[s("class"), s("_m")]);
        assert!(src.contains("export { __bi_0 as \"class\" };"), "{}", src);
        assert!(src.contains("export { __bi_1 as \"_m\" };"), "{}", src);
        assert!(!src.contains("export var class;"));
        assert!(!src.contains("export var _m;"));
    }

    #[test]
    fn builtin_esm_source_from_keys_empty_keys_is_honest_default_only() {
        // Empty key set from a REAL object (enumeration succeeded, object has
        // no own string keys) → default-only bridge. This is the
        // honest empty set — distinct from the fail-closed Err path.
        let src = builtin_esm_source_from_keys("mystery", &[]);
        assert!(src.contains("require(\"mystery\")"));
        assert!(src.contains("export default _m;"));
        assert!(!src.contains("export var"));
        assert!(!src.contains("export {"));
    }
}
