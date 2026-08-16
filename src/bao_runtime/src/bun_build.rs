// @trace REQ-ENG-006 [api:Bun.build] [req:REQ-ENG-006] [level:library]
//! `Bun.build(config)` — JS face + native-bundler bridge (BuildTasklet).
//!
//! ## Architecture (FetchTasklet pattern, per the CYCLEBREAK layering)
//!
//! `bun_bundler` (the full BundleV2 pipeline) references SM-backed
//! CYCLEBREAK symbols that only `bao_bundler` defines, and `bao_bundler`
//! depends on `bun_runtime` — so `bun_api.rs` cannot name the bundler
//! directly without either a dependency cycle or undefined symbols in every
//! downstream test binary. Instead:
//!
//!   1. This module owns the **Rust-only contract** ([`NativeBuildConfig`] /
//!      [`NativeBuildResult`]) and a process-global registry
//!      ([`NATIVE_BUILD_IMPL`]) holding the native bundle driver.
//!   2. `bao_bundler::build_api::install()` (linked by `bao_cli` and by the
//!      e2e tests) registers the real driver: `bun_bundler`'s
//!      `BundleV2::generate_from_cli` on the calling thread. Unregistered →
//!      explicit degraded `success:false + logs` (fail-closed, never a fake
//!      success or a silent throw).
//!   3. [`start`] marshals the JS call the same way `fetch_async` does:
//!      pending `JS::NewPromiseObject` + `RawValueRootGuard` heap root, the
//!      bundle runs on a dedicated OS thread (upstream uses the dedicated
//!      `JSBundleThread`; the pipeline itself fans parse/generate work out
//!      to the shared `CountedTask` pool), completion crosses back via a
//!      `ConcurrentTask` on the JS thread's `MiniEventLoop`
//!      (`enqueue_task_concurrent_cross_thread` auto-wakes the JS thread).
//!
//! ## Upstream semantics preserved
//!
//!   * `Bun.build(config)` returns `Promise<BuildOutput>`; build *failures*
//!     (e.g. unresolvable entry) RESOLVE with `{ success:false, logs:[...] }`
//!     — they never reject. Only an invalid config object throws
//!     (TypeError-style), mirroring `JSBundler.zig build()` →
//!     `throwInvalidArguments`.
//!   * `BuildOutput = { success, outputs, logs }`; each output is a
//!     `BuildArtifact` (Blob subclass face: `await artifact.text()` /
//!     `.arrayBuffer()` / `.size` / `.type`) plus `path` / `kind` / `hash` /
//!     `loader` / `sourcemap`.

use ::std::cell::RefCell;
use ::std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use ::std::sync::{Arc, Mutex, OnceLock};

use bao_engine::context::RawValueRootGuard;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_NewPlainObject, NewArrayObject1};

// ──────────────────────────────────────────────────────────────────────────
// Rust-only native contract (crosses the thread boundary; no SM types)
// ──────────────────────────────────────────────────────────────────────────

/// Minify switches — `config.minify: boolean | { whitespace, syntax, identifiers }`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeMinify {
    pub whitespace: bool,
    pub syntax: bool,
    pub identifiers: bool,
}

impl NativeMinify {
    pub fn all() -> Self {
        Self { whitespace: true, syntax: true, identifiers: true }
    }
}

/// One build log entry (upstream `BuildMessage` subset: level + message).
#[derive(Clone, Debug)]
pub struct NativeBuildLog {
    /// "error" | "warn" | "info"
    pub level: String,
    pub message: String,
}

/// `Bun.build(config)` — parsed JS config (pure Rust, Send).
#[derive(Clone, Debug, Default)]
pub struct NativeBuildConfig {
    /// `entrypoints: string[]` (required, non-empty upstream).
    pub entrypoints: Vec<String>,
    /// `outdir: string | undefined` — when set, outputs are written to disk.
    pub outdir: Option<String>,
    /// `root: string | undefined` — the root dir for output path computation.
    pub root: Option<String>,
    /// `target: "browser" | "bun" | "node"` (upstream default "browser").
    pub target: String,
    /// `format: "esm" | "cjs" | "iife"` (upstream default "esm").
    pub format: String,
    /// `naming: string` — entry template when a plain string
    /// ("[dir]/[name].[ext]" placeholders).
    pub naming: Option<String>,
    pub naming_entry: Option<String>,
    pub naming_chunk: Option<String>,
    pub naming_asset: Option<String>,
    pub minify: NativeMinify,
    /// `sourcemap: "none" | "linked" | "inline" | "external"` (default "none").
    pub sourcemap: String,
    /// `external: string[]` — passthrough module specifiers.
    pub external: Vec<String>,
    /// `define: Record<string, string>`.
    pub define: Vec<(String, String)>,
    /// `splitting: boolean` (code splitting; default false).
    pub splitting: bool,
    /// `banner: string` prepended to each output.
    pub banner: Option<String>,
    /// `footer: string` appended to each output.
    pub footer: Option<String>,
    /// `publicPath: string`.
    pub public_path: Option<String>,
    /// `jsx.runtime`: "classic" | "automatic" (upstream default automatic).
    pub jsx_runtime: Option<String>,
    /// `jsx.factory` (dotted member list, e.g. "React.createElement").
    pub jsx_factory: Option<String>,
    /// `jsx.fragment` (dotted member list, e.g. "React.Fragment").
    pub jsx_fragment: Option<String>,
    /// `jsx.importSource` (e.g. "react" → react/jsx-runtime).
    pub jsx_import_source: Option<String>,
    /// `jsx.development`: development vs production automatic runtime.
    pub jsx_development: Option<bool>,
}

/// One build artifact (bytes already extracted from the linker OutputFile).
#[derive(Clone, Debug)]
pub struct NativeOutputFile {
    /// Destination path relative to the (virtual) output root, e.g. "index.js".
    pub path: String,
    /// "entry-point" | "chunk" | "asset" | "sourcemap" | "bytecode".
    pub kind: String,
    /// Loader name for the artifact ("js" | "json" | "file" | ...).
    pub loader: String,
    /// MIME type for the Blob face (e.g. "text/javascript").
    pub mime_type: String,
    pub hash: u64,
    pub bytes: Vec<u8>,
    /// Index of the paired sourcemap artifact in `NativeBuildResult.outputs`
    /// (sourcemaps are also emitted as standalone outputs, upstream shape).
    pub sourcemap_index: Option<usize>,
}

/// Result handed from the native bundler back to the JS thread.
#[derive(Clone, Debug, Default)]
pub struct NativeBuildResult {
    pub success: bool,
    pub outputs: Vec<NativeOutputFile>,
    pub logs: Vec<NativeBuildLog>,
}

/// The native bundle driver signature. Runs on the dedicated build thread;
/// must be pure Rust (no SM API) so it can cross the thread boundary.
pub type NativeBuildFn = fn(&NativeBuildConfig) -> NativeBuildResult;

static NATIVE_BUILD_IMPL: OnceLock<NativeBuildFn> = OnceLock::new();

/// Register the native bundle driver (idempotent; first registration wins).
/// Called by `bao_bundler::build_api::install()` at product bring-up
/// (`bao_cli::run`) and by the e2e tests.
pub fn install_native_build_impl(f: NativeBuildFn) {
    let _ = NATIVE_BUILD_IMPL.set(f);
}

/// Whether a native bundle driver is linked into this binary.
pub fn native_build_installed() -> bool {
    NATIVE_BUILD_IMPL.get().is_some()
}

// ──────────────────────────────────────────────────────────────────────────
// PendingBuild (BuildTasklet) — the async carrier
// ──────────────────────────────────────────────────────────────────────────

struct PendingBuild {
    /// SpiderMonkey context that owns the Promise. Only touched on the JS thread.
    cx: *mut JSContext,
    /// RAII heap root keeping the pending Promise alive across the async window.
    promise_root: Option<RawValueRootGuard>,
    /// Spawn-time snapshot; only used when rooting failed (degraded path).
    promise_val: JSVal,
    /// Build-thread result slot.
    outcome: Arc<Mutex<Option<NativeBuildResult>>>,
    /// Pointer to the JS thread's `MiniEventLoop<'static>` (captured at start).
    mini_loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
    /// ConcurrentTask carrier (dispatches `resolve_tasklet` on the JS thread).
    concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext,
    /// Prevents duplicate ConcurrentTask scheduling.
    has_schedule_callback: AtomicBool,
}

// SAFETY: `cx`/`promise_val` are only dereferenced on the JS thread that
// created them; the build thread only touches `outcome` and
// `has_schedule_callback` (pure Rust / atomic).
unsafe impl ::std::marker::Send for PendingBuild {}

thread_local! {
    static PENDING: RefCell<Vec<*mut PendingBuild>> = const { RefCell::new(Vec::new()) };
}

/// JS-thread poll: are there outstanding builds on this thread?
pub fn has_pending() -> bool {
    PENDING.with(|p| !p.borrow().is_empty())
}

fn resolve_tasklet_shim(ctx: *mut PendingBuild, _parent: *mut ()) {
    // SAFETY: ctx was set to the PendingBuild pointer in start(); valid heap
    // allocation not yet freed (freed by resolve_tasklet itself).
    unsafe { resolve_tasklet(ctx) };
}

/// Start an async `Bun.build`. The caller must have already created the
/// pending Promise via `JS::NewPromiseObject(cx, null)` and passes it as
/// `promise_val`; `args.rval()` should be set to the same value.
///
/// When no native driver is installed, the Promise resolves immediately with
/// an explicit degraded `success:false + logs` payload (fail-closed — the
/// default product binary installs the driver, so this is embedder error).
///
/// # Safety
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
pub unsafe fn start(cx: *mut JSContext, promise_val: JSVal, config: NativeBuildConfig) {
    // GUARD-A (GC root): heap-root the pending Promise across the async
    // window (same invariant as FetchTasklet.promise).
    let promise_root = unsafe {
        RawValueRootGuard::new(
            cx,
            ::std::slice::from_ref(&promise_val),
            c"BuildTasklet.promise",
        )
    };
    let rooted_val = promise_root.as_ref().map_or(promise_val, |g| g.get(0));

    let outcome: Arc<Mutex<Option<NativeBuildResult>>> = Arc::new(Mutex::new(None));

    let pending = Box::new(PendingBuild {
        cx,
        promise_root,
        promise_val: rooted_val,
        outcome: Arc::clone(&outcome),
        mini_loop_ptr: ::std::ptr::null(), // filled below
        concurrent_task:
            bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext::default(),
        has_schedule_callback: AtomicBool::new(false),
    });
    let pending_ptr = Box::into_raw(pending);

    // Capture the JS thread's MiniEventLoop for concurrent-task scheduling.
    // SAFETY: with_event_loop borrows the MiniEventLoop on the current thread;
    // the pointer remains valid for the thread's lifetime (leaked at exit).
    let loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
        crate::timers::with_event_loop(|loop_| loop_ as *const _);

    // SAFETY: pending_ptr is a live heap allocation we just created.
    unsafe {
        (*pending_ptr).mini_loop_ptr = loop_ptr;
        let _field_offset = ::std::mem::offset_of!(PendingBuild, concurrent_task);
        (*pending_ptr)
            .concurrent_task
            .from(pending_ptr, resolve_tasklet_shim);
    }

    PENDING.with(|p| p.borrow_mut().push(pending_ptr));

    let Some(driver) = NATIVE_BUILD_IMPL.get().copied() else {
        // Fail-closed explicit degraded: no native bundler linked in this
        // binary. Resolve (never reject — upstream build errors resolve).
        let degraded = NativeBuildResult {
            success: false,
            outputs: Vec::new(),
            logs: vec![NativeBuildLog {
                level: "error".into(),
                message: "Bun.build: native bundler is not installed in this binary \
                          (install via bao_bundler::build_api::install)"
                    .into(),
            }],
        };
        complete_build(pending_ptr, degraded);
        return;
    };

    // Dedicated build thread (upstream: JSBundleThread singleton). The
    // pipeline itself fans parse/generate work out to the shared thread
    // pool's CountedTask batches; this thread only drives
    // `generate_from_cli` (which pumps its own Mini AnyEventLoop).
    // The raw `*mut PendingBuild` crosses to the worker thread as an
    // integer token (raw pointers are not Send); reconstituted only inside
    // the worker closure. On the worker side the pointee is touched solely
    // via `complete_build`'s atomics/Mutex; all SM access stays on the JS
    // thread.
    let pending_token = pending_ptr as usize;
    ::std::thread::spawn(move || {
        let result = driver(&config);
        let pending_ptr = pending_token as *mut PendingBuild;
        complete_build(pending_ptr, result);
    });
}

/// Worker-thread completion: store the outcome and enqueue the JS-thread
/// tasklet. Pure Rust on the build thread (INV: no SM API off the JS thread).
fn complete_build(pending_ptr: *mut PendingBuild, result: NativeBuildResult) {
    // SAFETY: pending_ptr is a live PendingBuild; the outcome write is
    // ordered before the ConcurrentTask enqueue by the acquire/release
    // compare_exchange below, so the JS-thread read cannot miss it.
    unsafe {
        {
            let mut slot = (*pending_ptr).outcome.lock().unwrap();
            *slot = Some(result);
        }
        if (*pending_ptr)
            .has_schedule_callback
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_ok()
        {
            let loop_ptr = (*pending_ptr).mini_loop_ptr;
            if !loop_ptr.is_null() {
                let concurrent_task_ptr =
                    ::std::ptr::addr_of_mut!((*pending_ptr).concurrent_task);
                // SAFETY: concurrent_task_ptr is the embedded carrier; loop_ptr
                // was captured on the JS thread. Success pushes the task and
                // wakes the JS thread out of any blocking epoll_wait.
                let _ = bun_event_loop::ConcurrentWakeup::enqueue_task_concurrent_cross_thread(
                    loop_ptr as *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
                    ::std::ptr::NonNull::new_unchecked(concurrent_task_ptr),
                );
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// JS-thread resolve: build the BuildOutput JS object and settle the Promise
// ──────────────────────────────────────────────────────────────────────────

unsafe fn resolve_tasklet(this: *mut PendingBuild) {
    // 1. Reset scheduling flag + take the outcome.
    // SAFETY: this is the live PendingBuild; sole consumer.
    unsafe {
        (*this).has_schedule_callback.store(false, AtomicOrdering::Release);
    }
    let outcome = unsafe {
        (*this)
            .outcome
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or(NativeBuildResult {
                success: false,
                outputs: Vec::new(),
                logs: vec![NativeBuildLog {
                    level: "error".into(),
                    message: "Bun.build: result slot was empty".into(),
                }],
            })
    };

    let cx = unsafe { (*this).cx };
    let pending = unsafe { &*this };
    let promise_val = pending
        .promise_root
        .as_ref()
        .map_or(pending.promise_val, |g| g.get(0));

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise_obj = promise_val.to_object());
    let promise_h = promise_obj.handle().into();

    // BCE-BUG-ENG-370 class: tasklets run from the MiniEventLoop tick OUTSIDE
    // any JS activation — enter the Promise's realm for the whole window.
    // (fetch_async pattern: root through the realm guard's reborrow of cx.)
    {
        let mut realm = AutoRealm::new_from_handle(cx_ref, promise_obj.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        let output_obj = build_output_js(cx, &outcome);
        if !output_obj.is_null() {
            rooted!(&in(realm_cx) let out_val = ObjectValue(output_obj));
            // SAFETY: live JSContext; rooted handles.
            unsafe { JS::ResolvePromise(cx, promise_h, out_val.handle().into()) };
        } else {
            // SAFETY: live JSContext; reject_with_message roots its reason.
            unsafe {
                reject_with_message(cx, promise_h, "Bun.build: failed to build the output object")
            };
        }
    }

    // Remove from the PENDING registry.
    PENDING.with(|p| {
        let mut guard = p.borrow_mut();
        if let Some(pos) = guard.iter().position(|&ptr| ptr == this) {
            guard.swap_remove(pos);
        }
    });

    // Deallocate the PendingBuild Box (terminal unroot is RAII in its Drop).
    // SAFETY: allocated by Box::into_raw in start; sole consumer.
    unsafe { drop(Box::from_raw(this)) };

    // Flush microtasks queued by the settle.
    mozjs_sys::jsapi::js::RunJobs(cx);
}

/// Reject the promise with a plain message string.
unsafe fn reject_with_message(cx: *mut JSContext, promise_h: Handle<*mut JSObject>, msg: &str) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let c_msg = bun_core::ZBox::from_vec(msg.as_bytes().to_vec());
    let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
    rooted!(&in(cx_ref) let reason = if js_str.is_null() {
        UndefinedValue()
    } else {
        mozjs::jsval::StringValue(&*js_str)
    });
    // SAFETY: live JSContext; rooted reason handle.
    unsafe { JS::RejectPromise(cx, promise_h, reason.handle().into()) };
}

/// Build the `BuildOutput` JS object: `{ success, outputs: BuildArtifact[], logs }`.
unsafe fn build_output_js(cx: *mut JSContext, result: &NativeBuildResult) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let out = JS_NewPlainObject(cx_ref));
    if out.get().is_null() {
        return ::std::ptr::null_mut();
    }
    let out_h = out.handle().into();

    rooted!(&in(cx_ref) let ok_val = mozjs::jsval::BooleanValue(result.success));
    JS_DefineProperty(
        cx,
        out_h,
        c"success".as_ptr(),
        ok_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // outputs: BuildArtifact[]. First pass creates + inserts every artifact
    // (the array roots them); second pass wires `sourcemap` backrefs.
    rooted!(&in(cx_ref) let outputs_arr = NewArrayObject1(cx_ref, result.outputs.len()));
    for (idx, file) in result.outputs.iter().enumerate() {
        let obj = build_artifact_js(cx, file);
        if obj.is_null() {
            continue;
        }
        rooted!(&in(cx_ref) let obj_val = ObjectValue(obj));
        JS_SetElement(cx, outputs_arr.handle().into(), idx as u32, obj_val.handle().into());
    }
    for (idx, file) in result.outputs.iter().enumerate() {
        let Some(sm_idx) = file.sourcemap_index else { continue };
        if sm_idx >= result.outputs.len() || sm_idx == idx {
            continue;
        }
        rooted!(&in(cx_ref) let mut obj_val = UndefinedValue());
        JS_GetElement(
            cx,
            outputs_arr.handle().into(),
            idx as u32,
            obj_val.handle_mut().into(),
        );
        rooted!(&in(cx_ref) let mut sm_val = UndefinedValue());
        JS_GetElement(
            cx,
            outputs_arr.handle().into(),
            sm_idx as u32,
            sm_val.handle_mut().into(),
        );
        if !obj_val.get().is_object() || !sm_val.get().is_object() {
            continue;
        }
        rooted!(&in(cx_ref) let obj_r = obj_val.get().to_object());
        JS_DefineProperty(
            cx,
            obj_r.handle().into(),
            c"sourcemap".as_ptr(),
            sm_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    rooted!(&in(cx_ref) let outputs_val = ObjectValue(outputs_arr.get()));
    JS_DefineProperty(
        cx,
        out_h,
        c"outputs".as_ptr(),
        outputs_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // logs: [{ level, message }]
    rooted!(&in(cx_ref) let logs_arr = NewArrayObject1(cx_ref, result.logs.len()));
    for (idx, log) in result.logs.iter().enumerate() {
        rooted!(&in(cx_ref) let log_obj = JS_NewPlainObject(cx_ref));
        if log_obj.get().is_null() {
            continue;
        }
        let log_h = log_obj.handle().into();
        define_string_prop(cx, log_h, c"level", &log.level);
        define_string_prop(cx, log_h, c"message", &log.message);
        rooted!(&in(cx_ref) let lv = ObjectValue(log_obj.get()));
        JS_SetElement(cx, logs_arr.handle().into(), idx as u32, lv.handle().into());
    }
    rooted!(&in(cx_ref) let logs_val = ObjectValue(logs_arr.get()));
    JS_DefineProperty(
        cx,
        out_h,
        c"logs".as_ptr(),
        logs_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    out.get()
}

/// Define a UTF-8 string property (no-op on OOM, matching the surrounding
/// code's OOM posture).
unsafe fn define_string_prop(
    cx: *mut JSContext,
    obj: mozjs::jsapi::Handle<*mut JSObject>,
    name: &::std::ffi::CStr,
    value: &str,
) {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let c_val = bun_core::ZBox::from_vec(value.as_bytes().to_vec());
    let js_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
        JS_DefineProperty(cx, obj, name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
    }
}

/// Build one `BuildArtifact` JS object with the Blob face.
///
/// The runtime's Blob is the JS polyfill installed by `globals` (chunks +
/// `size` + prototype methods). We create the object with `Blob.prototype`
/// as its [[Prototype]] and populate the polyfill's internal fields
/// (`_chunks`, `size`, `type`) so `await artifact.text()` /
/// `.arrayBuffer()` run through the real Blob implementation, then add the
/// BuildArtifact fields (`path` / `kind` / `hash` / `loader`).
unsafe fn build_artifact_js(cx: *mut JSContext, file: &NativeOutputFile) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(cx_ref) let global_rooted = global);

    rooted!(&in(cx_ref) let artifact = JS_NewPlainObject(cx_ref));
    if artifact.get().is_null() {
        return ::std::ptr::null_mut();
    }
    let art_h = artifact.handle().into();

    // Blob payload: `_chunks = [Uint8Array(bytes)]` + `size` + `type` —
    // the polyfill's internal fields, populated exactly the way its own
    // constructor would (bytes copied before any GC point, view rooted).
    // `_chunks` is an ARRAY of chunks (the polyfill iterates + concats), not
    // the view itself.
    let len = file.bytes.len();
    let arr_obj = JS_NewUint8Array(cx, len);
    if !arr_obj.is_null() {
        rooted!(&in(cx_ref) let arr_root = arr_obj);
        if len > 0 {
            let mut is_shared = false;
            let data_ptr = JS_GetUint8ArrayData(arr_root.get(), &mut is_shared, ::std::ptr::null());
            if !data_ptr.is_null() {
                ::std::ptr::copy_nonoverlapping(file.bytes.as_ptr(), data_ptr, len);
            }
        }
        rooted!(&in(cx_ref) let chunks_arr = NewArrayObject1(cx_ref, 1));
        if !chunks_arr.get().is_null() {
            rooted!(&in(cx_ref) let arr_val = ObjectValue(arr_root.get()));
            JS_SetElement(
                cx,
                chunks_arr.handle().into(),
                0,
                arr_val.handle().into(),
            );
            rooted!(&in(cx_ref) let chunks_val = ObjectValue(chunks_arr.get()));
            JS_DefineProperty(
                cx,
                art_h,
                c"_chunks".as_ptr(),
                chunks_val.handle().into(),
                0, // internal field of the polyfill — non-enumerable
            );
        }
    }
    rooted!(&in(cx_ref) let size_val = mozjs::jsval::DoubleValue(len as f64));
    JS_DefineProperty(
        cx,
        art_h,
        c"size".as_ptr(),
        size_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    define_string_prop(cx, art_h, c"type", &file.mime_type);

    // BuildArtifact fields.
    define_string_prop(cx, art_h, c"path", &file.path);
    define_string_prop(cx, art_h, c"kind", &file.kind);
    define_string_prop(cx, art_h, c"loader", &file.loader);
    rooted!(&in(cx_ref) let hash_val = mozjs::jsval::DoubleValue(file.hash as f64));
    JS_DefineProperty(
        cx,
        art_h,
        c"hash".as_ptr(),
        hash_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // [[Prototype]] = Blob.prototype LAST: the polyfill's prototype methods
    // (`text`/`arrayBuffer`/`slice`/`stream`) resolve through the real Blob
    // implementation against the `_chunks`/`size` fields set above. When the
    // Blob constructor is genuinely absent (embedded realm), define local
    // `text()`/`arrayBuffer()` fallbacks so the artifact still carries its
    // bytes (explicit support, not a silent stub).
    rooted!(&in(cx_ref) let mut blob_val = UndefinedValue());
    JS_GetProperty(
        cx,
        global_rooted.handle().into(),
        c"Blob".as_ptr(),
        blob_val.handle_mut().into(),
    );
    let mut has_blob_proto = false;
    if blob_val.get().is_object() {
        rooted!(&in(cx_ref) let blob_ctor = blob_val.get().to_object());
        rooted!(&in(cx_ref) let mut proto_val = UndefinedValue());
        JS_GetProperty(
            cx,
            blob_ctor.handle().into(),
            c"prototype".as_ptr(),
            proto_val.handle_mut().into(),
        );
        if proto_val.get().is_object() {
            rooted!(&in(cx_ref) let proto = proto_val.get().to_object());
            if JS_SetPrototype(cx, art_h, proto.handle().into()) {
                has_blob_proto = true;
            }
        }
    }
    if !has_blob_proto {
        JS_DefineFunction(
            cx,
            art_h,
            c"text".as_ptr(),
            Some(artifact_text_fallback),
            0,
            0,
        );
        JS_DefineFunction(
            cx,
            art_h,
            c"arrayBuffer".as_ptr(),
            Some(artifact_arraybuffer_fallback),
            0,
            0,
        );
    }

    artifact.get()
}

/// `text()` fallback for Blob-less realms: Promise resolving to the UTF-8
/// decode of the artifact's `_chunks[0]` payload.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn artifact_text_fallback(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());

    rooted!(&in(cx_ref) let promise = JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let bytes = read_first_chunk(cx, obj.get());
    let text = String::from_utf8_lossy(&bytes);
    let c_text = bun_core::ZBox::from_vec(text.as_bytes().to_vec());
    let js_str = JS_NewStringCopyZ(cx, c_text.as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let str_val = mozjs::jsval::StringValue(&*js_str));
        JS::ResolvePromise(cx, promise.handle().into(), str_val.handle().into());
    }
    args.rval().set(ObjectValue(promise.get()));
    true
}

/// `arrayBuffer()` fallback for Blob-less realms: Promise resolving to a
/// fresh ArrayBuffer copy of the artifact's `_chunks[0]` payload.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn artifact_arraybuffer_fallback(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());

    rooted!(&in(cx_ref) let promise = JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let bytes = read_first_chunk(cx, obj.get());
    // Build a Uint8Array copy (rooted; bytes copied before any GC point) and
    // resolve with its `.buffer` so the payload is an ArrayBuffer.
    let arr_obj = JS_NewUint8Array(cx, bytes.len());
    if !arr_obj.is_null() {
        rooted!(&in(cx_ref) let arr_root = arr_obj);
        if !bytes.is_empty() {
            let mut is_shared = false;
            let data_ptr =
                JS_GetUint8ArrayData(arr_root.get(), &mut is_shared, ::std::ptr::null());
            if !data_ptr.is_null() {
                ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
            }
        }
        rooted!(&in(cx_ref) let mut buffer_val = UndefinedValue());
        JS_GetProperty(
            cx,
            arr_root.handle().into(),
            c"buffer".as_ptr(),
            buffer_val.handle_mut().into(),
        );
        if buffer_val.get().is_object() {
            rooted!(&in(cx_ref) let bv = buffer_val.get());
            JS::ResolvePromise(cx, promise.handle().into(), bv.handle().into());
        }
    }
    args.rval().set(ObjectValue(promise.get()));
    true
}

/// Read `_chunks[0]` (Uint8Array) off an artifact object into a Vec. Empty
/// when the field is missing (defensive — construction always sets it).
unsafe fn read_first_chunk(cx: *mut JSContext, obj: *mut JSObject) -> Vec<u8> {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    rooted!(&in(cx_ref) let mut chunks_val = UndefinedValue());
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_chunks".as_ptr(),
        chunks_val.handle_mut().into(),
    );
    if !chunks_val.get().is_object() {
        return Vec::new();
    }
    rooted!(&in(cx_ref) let chunks = chunks_val.get().to_object());
    rooted!(&in(cx_ref) let mut first_val = UndefinedValue());
    JS_GetElement(cx, chunks.handle().into(), 0, first_val.handle_mut().into());
    if !first_val.get().is_object() {
        return Vec::new();
    }
    rooted!(&in(cx_ref) let arr = first_val.get().to_object());
    // Length via the `length` property (works for any typed array shape).
    rooted!(&in(cx_ref) let mut len_val = UndefinedValue());
    JS_GetProperty(cx, arr.handle().into(), c"length".as_ptr(), len_val.handle_mut().into());
    if !len_val.get().is_number() {
        return Vec::new();
    }
    let len = len_val.get().to_number() as usize;
    if len == 0 {
        return Vec::new();
    }
    let mut is_shared = false;
    let data_ptr = JS_GetUint8ArrayData(arr.get(), &mut is_shared, ::std::ptr::null());
    if data_ptr.is_null() {
        return Vec::new();
    }
    // SAFETY: `arr` is rooted; no JS/GC runs before the copy completes.
    unsafe { ::std::slice::from_raw_parts(data_ptr, len) }.to_vec()
}
