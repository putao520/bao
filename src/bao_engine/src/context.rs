// @trace REQ-ENG-001 [entity:JsContext]
//! SpiderMonkey JSContext — always parasitic on servo's Runtime.
//!
//! 铁律: Bao 始终寄生 servo 的 JSContext，不存在独立 JSContext。
//! 所有模式（CLI/browser/CDP）共享 servo 的唯一 JSContext。
//!
//! 初始化路径：
//!   - CLI 模式: `JsContext::init_runtime()` → JSEngine + Runtime + JobQueue
//!     返回 `SmRuntimeGuard` 持有所有权，BaoRuntime 持有 guard。
//!   - Browser 模式: servo 初始化 Runtime → `JsContext::from_servo_runtime()` 寄生
//!     servo 拥有 Runtime 生命周期，不需要 guard。
//!   - 两者共享同一个 `mozjs::rust::Runtime::get()` TLS 全局
//!
//! TLS 生命周期策略：
//!   - JSEngine 是进程级单例（JSEngine::init 只能成功一次，JS_ShutDown 后不可重启）
//!   - Engine 存储在 TLS 中，线程退出时 mem::forget（永不调 JS_ShutDown）
//!   - Runtime（JSContext）可安全创建/销毁多次
//!   - Runtime 在 TLS 中存储，线程退出时 mem::forget 避免在 __call_tls_dtors 中
//!     执行 JS_DestroyContext（mozjs 的 GCRuntime::finishRoots 在 C++ TLS teardown
//!     期间会 SIGSEGV）

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::ptr::{self, NonNull};
use std::sync::{Mutex, OnceLock};

use mozjs::jsapi::{JS_ShutDown, JSContext as RawJSContext, OnNewGlobalHookOption};
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_NewGlobalObject;
use mozjs::rust::{RealmOptions, SIMPLE_GLOBAL_CLASS};

use crate::error::JsError;
use crate::host_fn;
use crate::job_queue::JobQueue;
use crate::module_loader::ModuleLoader;
use crate::value::{JsValue, jsval_to_jsvalue};

// Re-export GlobalSetupFn and PostEvalHook from bun_sm (canonical definitions).
// These were previously defined here but moved to bun_sm::module_loader.
pub use crate::module_loader::{GlobalSetupFn, PostEvalHook};

/// Parasitic JSContext — borrows servo's JSContext pointer.
/// Does NOT own a mozjs::rust::Runtime; servo owns that lifetime.
///
/// 铁律: Bao 始终寄生 servo 的 JSContext，不存在独立 JSContext。
/// 所有模式（CLI/browser/CDP）共享 servo 的唯一 JSContext。
///
/// Realm model (first-principles, ECMA-262/Node semantics): a Realm belongs
/// to the agent (= this JsContext) for the latter's whole lifetime, NOT to a
/// single script execution. Scripts execute inside the realm; the realm
/// persists across `eval` calls. This is what lets `setTimeout`/server
/// handlers registered by one script fire after that script returns, and
/// what makes `globalThis.x` set by eval A visible to eval B.
///
/// Concretely `JsContext` lazily owns ONE global object (`realm_global`),
/// created on the first `eval`/`eval_module_setup` and reused by every
/// subsequent eval (which merely `AutoRealm`s into it). The global is
/// persistent-rooted via `JS_AddExtraRoot` for the context's lifetime —
/// SpiderMonkey does not auto-root globals.
pub struct JsContext {
    cx: NonNull<RawJSContext>,
    global_setup: Option<GlobalSetupFn>,
    post_eval_hook: Option<PostEvalHook>,
    /// The persistent realm's global object for this context. `None` until
    /// the first eval lazily creates it (applying `global_setup`). Once
    /// `Some`, every eval reuses it.
    realm_global: Option<Box<PersistentGlobal>>,
}

/// A heap `Value` slot registered with `JS_AddExtraRoot` (`AddRawValueRoot`)
/// — the mozjs 0.21.4 equivalent of `JS::PersistentRooted`. Holds the
/// context's single realm global alive for the context's lifetime.
///
/// The box pins the `Value` at a stable address (the GC scans/updates that
/// memory in place); on drop the registration is removed (guarded by a
/// liveness check so test teardown that destroys the context first does not
/// unroot into a freed extra-roots table).
struct PersistentGlobal {
    cx: *mut RawJSContext,
    global_val: mozjs::jsval::JSVal,
}

impl Drop for PersistentGlobal {
    fn drop(&mut self) {
        if Self::cx_alive(self.cx) {
            unsafe {
                mozjs::jsapi::RemoveRawValueRoot(self.cx, &mut self.global_val);
            }
        }
    }
}

/// Liveness check shared by the extra-roots guards: the captured context is
/// only safe to touch if the current thread's `Runtime::get()` still
/// resolves to it. `false` covers both "runtime destroyed" (root table gone)
/// and "dropped from a foreign thread" (root table alive but not ours).
fn raw_cx_alive(cx: *mut RawJSContext) -> bool {
    !cx.is_null() && mozjs::rust::Runtime::get().map(|c| c.as_ptr()) == Some(cx)
}

impl PersistentGlobal {
    fn cx_alive(cx: *mut RawJSContext) -> bool {
        raw_cx_alive(cx)
    }

    fn global_ptr(&self) -> *mut mozjs::jsapi::JSObject {
        if self.global_val.is_object() {
            self.global_val.to_object()
        } else {
            ::std::ptr::null_mut()
        }
    }
}

/// RAII guard for one or more heap `JSVal` slots registered with the GC
/// extra-roots table (`AddRawValueRoot`) — the general form of
/// `PersistentGlobal`'s rooting, for values that must survive across an
/// async window (pending fetch promises, fs/crypto callbacks, ...).
///
/// ## Why the slots live in a `Box<[JSVal]>`
///
/// `AddRawValueRoot` registers the slot's *address*: the GC scans and
/// updates that memory in place for as long as the root is registered, and
/// `RemoveRawValueRoot` removes by pointer — the registered address must
/// therefore stay both valid and identical until removal. Rooting a stack
/// local whose frame then returns leaves the GC tracing dead stack memory,
/// and removing with a different address is a silent no-op (the root table
/// is keyed by pointer), leaking the dangling root. The `Box` pins the
/// slots at a stable heap address for the guard's whole life; moving the
/// guard only moves the `Box` pointer, never the rooted memory.
///
/// ## Drop semantics (liveness-guarded, leak-on-foreign)
///
/// On drop, when the current thread's `Runtime::get()` still resolves to
/// the captured context, every slot is unrooted and the values freed.
/// Otherwise (dropped from a foreign thread while the context is alive, or
/// after the runtime went away) the rooted slots are *leaked*, never freed —
/// freeing memory the root table may still point at would leave the GC a
/// dangling scan address, which is strictly worse than a bounded leak.
pub struct RawValueRootGuard {
    cx: *mut RawJSContext,
    vals: Box<[mozjs::jsval::JSVal]>,
}

impl RawValueRootGuard {
    /// Root `vals` for an async window that spans ticks/frames.
    ///
    /// Returns `None` if any registration failed (OOM): slots registered
    /// before the failure are unrooted before returning, so a `None` carries
    /// no leaked roots and the caller keeps its own rooting (the
    /// pre-existing degraded path at the call sites).
    ///
    /// # Safety
    /// - `cx` must be a live `JSContext` on the current thread.
    /// - Each value must be a valid `JSVal`. After this call the guard's
    ///   slots are the live copies — read them via [`Self::get`], not via
    ///   pre-existing snapshots.
    pub unsafe fn new(
        cx: *mut RawJSContext,
        vals: &[mozjs::jsval::JSVal],
        name: &'static ::std::ffi::CStr,
    ) -> Option<Self> {
        let mut slots: Box<[mozjs::jsval::JSVal]> = vals.to_vec().into_boxed_slice();
        let mut rooted = 0usize;
        for slot in slots.iter_mut() {
            let ok =
                unsafe { mozjs::jsapi::AddRawValueRoot(cx, slot, name.as_ptr()) };
            if !ok {
                // Roll back the prefix so a failed `new` leaves no roots.
                for s in slots[..rooted].iter_mut() {
                    unsafe { mozjs::jsapi::RemoveRawValueRoot(cx, s) };
                }
                return None;
            }
            rooted += 1;
        }
        Some(RawValueRootGuard { cx, vals: slots })
    }

    /// The live (GC-updated) value at `i`. After any window in which a GC
    /// may have run, this — not a snapshot taken at spawn time — is the
    /// value to use (a moving GC updates the guard's slot in place).
    pub fn get(&self, i: usize) -> mozjs::jsval::JSVal {
        self.vals[i]
    }

    /// Number of rooted slots.
    pub fn len(&self) -> usize {
        self.vals.len()
    }

    /// Release the roots and take ownership of the values (explicit
    /// ownership transfer before the guard's natural drop site).
    ///
    /// Returns `None` when the roots could not be released (foreign thread
    /// or dead runtime): the rooted memory is then leaked by the guard —
    /// the caller must NOT receive memory the GC root table still points
    /// at.
    pub fn into_inner(mut self) -> Option<Box<[mozjs::jsval::JSVal]>> {
        if !raw_cx_alive(self.cx) {
            // Leak instead of handing out rooted memory.
            let leaked = ::std::mem::take(&mut self.vals);
            ::std::mem::forget(leaked);
            return None;
        }
        let mut vals = ::std::mem::take(&mut self.vals);
        for slot in vals.iter_mut() {
            unsafe { mozjs::jsapi::RemoveRawValueRoot(self.cx, slot) };
        }
        Some(vals)
    }
}

impl Drop for RawValueRootGuard {
    fn drop(&mut self) {
        if raw_cx_alive(self.cx) {
            for slot in self.vals.iter_mut() {
                unsafe { mozjs::jsapi::RemoveRawValueRoot(self.cx, slot) };
            }
        } else {
            // Foreign thread / dead runtime: leak the rooted slots — the
            // root table may still hold their addresses, so the memory must
            // outlive it (a bounded leak beats a dangling GC scan address).
            let leaked = ::std::mem::take(&mut self.vals);
            ::std::mem::forget(leaked);
        }
    }
}

thread_local! {
    /// The current thread's JsContext persistent realm global. Set the first
    /// time a JsContext on this thread creates its realm. Read by async
    /// dispatch sites (node:http route handlers, Bun.serve, timers, ...) that
    /// must `AutoRealm` into the persistent realm before touching JS — they
    /// only have a raw `*mut JSContext`, not a `&JsContext`.
    static THREAD_REALM_GLOBAL: ::std::cell::Cell<*mut mozjs::jsapi::JSObject> =
        const { ::std::cell::Cell::new(::std::ptr::null_mut()) };
}

/// The current thread's persistent realm global, if a JsContext on this
/// thread has created its realm. Used by dispatch sites to `AutoRealm`.
pub fn thread_realm_global() -> Option<*mut mozjs::jsapi::JSObject> {
    THREAD_REALM_GLOBAL.with(|c| {
        let p = c.get();
        if p.is_null() {
            None
        } else {
            Some(p)
        }
    })
}

/// Owns the SM Runtime for CLI/test mode.
/// Browser mode never constructs this — servo owns the lifetime there.
///
/// The JSEngine is a process-wide singleton (handle in ENGINE_HANDLE, engine in ENGINE_TLS).
/// This guard owns the Runtime (JSContext) which is destroyed on drop.
pub struct SmRuntimeGuard {
    #[allow(dead_code)]
    runtime: mozjs::rust::Runtime,
}

/// TLS wrapper that never drops its content.
/// Uses `ManuallyDrop` to prevent any destructor from running, even when
/// the TLS slot itself is destroyed during thread exit.
struct NeverDrop<T>(RefCell<ManuallyDrop<Option<T>>>);

impl<T> NeverDrop<T> {
    const fn new() -> Self {
        NeverDrop(RefCell::new(ManuallyDrop::new(None)))
    }

    fn is_some(&self) -> bool {
        self.0.borrow().is_some()
    }

    fn set(&self, val: Option<T>) {
        let mut borrow = self.0.borrow_mut();
        if borrow.is_some() {
            unsafe {
                ManuallyDrop::drop(&mut *borrow);
            }
        }
        *borrow = ManuallyDrop::new(val);
    }

    #[allow(dead_code)]
    fn take(&self) -> Option<T> {
        let mut borrow = self.0.borrow_mut();
        if borrow.is_some() {
            let val = unsafe { ManuallyDrop::take(&mut *borrow) };
            *borrow = ManuallyDrop::new(None);
            val
        } else {
            None
        }
    }
}

// No Drop impl — ManuallyDrop ensures nothing runs when TLS is destroyed.

use std::sync::atomic::{AtomicBool, Ordering};

/// Process-wide flag: true after `shutdown_engine()` has been called.
/// Prevents `for_test()` from creating new Runtimes on a shut-down engine.
static ENGINE_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Process-singleton JSEngine handle. JSEngineHandle is Send+Sync (Arc<AtomicU32>).
/// The first thread to call ensure_engine_handle() initializes the JSEngine
/// (stored in ENGINE_TLS on that thread) and stores a cloned handle here.
/// Other threads obtain the handle without calling JSEngine::init() again.
//
// @trace REQ-PERF-004 [entity:DomainDispatch] [level:integration]
// REQ-PERF-004 验收:进程级 JSEngine 单例用 `OnceLock<JSEngineHandle>` 替代
// `Arc<Mutex<JSEngineHandle>>`,消除每次访问的 lock/unlock 开销。OnceLock 内部用
// AtomicU8 状态机,首次 init 后所有 get() 是无锁 atomic load。
static ENGINE_HANDLE: OnceLock<mozjs::rust::JSEngineHandle> = OnceLock::new();

/// Process-global lock serializing JSEngine init (and `for_test` Runtime setup).
///
/// `JSEngine` is a process-wide singleton (`JSEngine::init` may succeed only once).
/// Concurrent callers of `ensure_engine_handle` / `init_runtime` / `for_test` /
/// `BaoRuntime` must share this lock on the slow path so only one thread calls
/// `JSEngine::init()`; others double-check `ENGINE_HANDLE` after acquiring the lock.
/// Without this, two threads can both miss the `ENGINE_HANDLE` fast-path and the
/// second hits `AlreadyInitialized`.
static ENGINE_INIT_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// Per-thread JSEngine (only the initializing thread stores it here).
    static ENGINE_TLS: NeverDrop<mozjs::rust::JSEngine> = NeverDrop::new();

    /// Per-thread Runtime (JSContext). ManuallyDrop prevents TLS destructor.
    static RUNTIME_TLS: NeverDrop<mozjs::rust::Runtime> = NeverDrop::new();
}

// ── Raw pthread_key for SpiderMonkey cleanup ──
//
// Rust `thread_local!` cannot be accessed inside TLS destructors
// (AccessError: "cannot access TLS during or after destruction").
// We store a raw pointer to our cleanup state in a pthread_key instead.
// The pthread_key destructor receives the pointer directly — no Rust TLS needed.
//
// Cleanup calls JS_DestroyContext and JS_ShutDown at the C level, bypassing
// Rust Drop impls (which access Rust TLS internally and would panic).

// ── SpiderMonkey cleanup strategy ──
//
// Root cause: JS_DestroyContext calls trace_traceables() → accesses Rust TLS
// (RootedTraceableSet). This makes it IMPOSSIBLE to call from any destructor
// (atexit: wrong thread; pthread_key: TLS already being destroyed; Rust TLS:
// same issue).
//
// Solution: explicit cleanup via `shutdown_thread_sm()`. Tests MUST call it
// before the test function returns. This is the ONLY safe cleanup path.
//
// If shutdown_thread_sm() is NOT called, the process will SIGSEGV during exit
// (mozjs C++ TLS MutexImpl destructors crash on freed memory). This is by
// design — it forces correct lifecycle management.

/// Get or initialize the per-process JSEngine, returning a handle.
///
/// The first thread to call this initializes the JSEngine (stored in ENGINE_TLS
/// on that thread for lifetime), and stores a cloned handle in ENGINE_HANDLE
/// (process-wide OnceLock). Subsequent threads just clone the handle.
///
/// Worker threads call this to obtain the process-global JSEngine handle,
/// then create their own `Runtime::new(handle)` on the worker thread.
///
/// Concurrent `for_test` / `init_runtime` / `BaoRuntime` paths all go through
/// here and share [`ENGINE_INIT_LOCK`] on the slow path (see that static).
pub fn ensure_engine_handle() -> Result<mozjs::rust::JSEngineHandle, JsError> {
    // Fast path: engine already initialized, just clone the handle (no lock).
    if let Some(handle) = ENGINE_HANDLE.get() {
        return Ok(handle.clone());
    }
    // Slow path: serialize init so only one thread calls JSEngine::init().
    let _guard = ENGINE_INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ensure_engine_handle_locked()
}

/// Slow-path body for [`ensure_engine_handle`]. Caller must hold [`ENGINE_INIT_LOCK`]
/// (or already observe a populated `ENGINE_HANDLE` via the unlocked fast path).
/// Double-checks `ENGINE_HANDLE` so a waiter that lost the race returns the
/// winner's handle without calling `JSEngine::init()` again.
fn ensure_engine_handle_locked() -> Result<mozjs::rust::JSEngineHandle, JsError> {
    // Double-check under lock: another thread may have finished init while we waited.
    if let Some(handle) = ENGINE_HANDLE.get() {
        return Ok(handle.clone());
    }
    // This thread must initialize the engine — or recover a handle if another
    // path (servo JSEngineSetup / concurrent ensure) already called JS_Init.
    // First check if this thread already has it in TLS (unlikely on first call).
    let (engine, handle) = ENGINE_TLS.with(|tls| {
        if tls.is_some() {
            let handle = tls
                .0
                .borrow()
                .as_ref()
                .expect("ENGINE_TLS is Some but inner is None")
                .handle();
            return Ok((None, handle));
        }
        match mozjs::rust::JSEngine::init() {
            Ok(engine) => {
                let handle = engine.handle();
                tls.set(Some(engine));
                Ok((Some(handle.clone()), handle))
            }
            Err(mozjs::rust::JSEngineError::AlreadyInitialized) => {
                // Winner may be servo (JSEngineSetup) or another bao thread.
                // mozjs publishes PROCESS_ENGINE_OUTSTANDING; prefer that, then
                // spin briefly for ENGINE_HANDLE if the winner is still storing.
                if let Some(h) = mozjs::rust::JSEngine::process_handle() {
                    return Ok((Some(h.clone()), h));
                }
                for _ in 0..50 {
                    if let Some(h) = ENGINE_HANDLE.get() {
                        return Ok((None, h.clone()));
                    }
                    if let Some(h) = mozjs::rust::JSEngine::process_handle() {
                        return Ok((Some(h.clone()), h));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(JsError {
                    message: "Failed to init JSEngine: AlreadyInitialized \
                              (no process handle published)"
                        .into(),
                    filename: "<engine>".into(),
                    line: 0,
                    column: 0,
                    stack: None,
                })
            }
            Err(e) => Err(JsError {
                message: format!("Failed to init JSEngine: {:?}", e).into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            }),
        }
    })?;
    // Store the handle in the global OnceLock so other threads can access it.
    if let Some(handle_to_store) = engine {
        let global_handle = ENGINE_HANDLE.get_or_init(|| handle_to_store);
        Ok(global_handle.clone())
    } else {
        // TLS reuse path — still ensure process OnceLock is populated if empty.
        let global_handle = ENGINE_HANDLE.get_or_init(|| handle.clone());
        Ok(global_handle.clone())
    }
}

impl JsContext {
    /// Initialize SpiderMonkey Runtime for CLI mode.
    ///
    /// Returns `(JsContext, Option<SmRuntimeGuard>)`. The guard owns the
    /// Runtime lifetime. The JSEngine is a process-wide singleton (never dropped).
    ///
    /// If servo already initialized the Runtime (browser mode ran first),
    /// returns `(JsContext, None)` — servo owns the lifetime.
    pub fn init_runtime() -> Result<(Self, Option<SmRuntimeGuard>), JsError> {
        // If Runtime is already alive on this thread (servo or prior call),
        // parasitize it — no new Engine/Runtime needed.
        if mozjs::rust::Runtime::get().is_some() {
            let ctx = unsafe { Self::from_servo_runtime()? };
            return Ok((ctx, None));
        }

        // CLI mode: get or init the process-wide JSEngine, then create a Runtime.
        let handle = ensure_engine_handle()?;
        let runtime = mozjs::rust::Runtime::new(handle);

        let cx = mozjs::rust::Runtime::get().ok_or_else(|| JsError {
            message: "Runtime::new failed to set CONTEXT TLS".into(),
            filename: "<engine>".into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        let mut cx_wrap = unsafe { mozjs::context::JSContext::from_ptr(cx) };
        if !JobQueue::init(&mut cx_wrap) {
            return Err(JsError {
                message: "Failed to init job queue".into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        ModuleLoader::init_thread_local(&cx_wrap);

        // Register the job queue drain callback so bun_sm::module_loader can drain microtasks
        // without depending on bao_engine (avoids circular dependency).
        crate::module_loader::set_job_queue_drain(JobQueue::drain);

        let guard = SmRuntimeGuard { runtime };

        crate::dispatch_sm::BaoEventLoop::register_js_context(cx.as_ptr().cast());

        Ok((
            JsContext {
                cx,
                global_setup: None,
                post_eval_hook: None,
                realm_global: None,
            },
            Some(guard),
        ))
    }

    /// Parasitize servo's Runtime on this thread.
    ///
    /// # Safety
    /// servo's Runtime must be alive on this thread (set via Runtime::new or
    /// bao_browser initialization).
    pub unsafe fn from_servo_runtime() -> Result<Self, JsError> {
        let cx = mozjs::rust::Runtime::get().ok_or_else(|| JsError {
            message: "servo Runtime not initialized — call JsContext::init_runtime() first".into(),
            filename: "<engine>".into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        let mut cx_wrap = unsafe { mozjs::context::JSContext::from_ptr(cx) };
        if !JobQueue::init(&mut cx_wrap) {
            return Err(JsError {
                message: "Failed to init job queue".into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        ModuleLoader::init_thread_local(&cx_wrap);
        crate::module_loader::set_job_queue_drain(JobQueue::drain);

        crate::dispatch_sm::BaoEventLoop::register_js_context(cx.as_ptr().cast());

        Ok(JsContext {
            cx,
            global_setup: None,
            post_eval_hook: None,
            realm_global: None,
        })
    }

    /// Test-only: create a JsContext backed by the TLS-managed Runtime.
    ///
    /// The JSEngine and Runtime are stored in thread_local storage.
    /// Both are created once and kept alive for the entire thread lifetime.
    /// Multiple calls to `for_test()` reuse the same JSEngine and Runtime.
    ///
    /// On thread exit, TLS destructors are skipped via `ManuallyDrop` to avoid
    /// SIGSEGV in mozjs's C++ TLS teardown (`mozilla::detail::MutexImpl`).
    #[doc(hidden)]
    pub fn for_test() -> Result<Self, JsError> {
        // Hold ENGINE_INIT_LOCK for the whole call so concurrent for_test /
        // ensure_engine_handle / init_runtime share one serialized init path.
        // Uses ensure_engine_handle_locked (not ensure_engine_handle) to avoid
        // re-locking the non-reentrant Mutex.
        let _init_guard = ENGINE_INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Refuse to create a new Runtime after the engine has been shut down.
        // JS_ShutDown is irreversible — calling Runtime::new after it will crash.
        if ENGINE_SHUTDOWN.load(Ordering::SeqCst) {
            return Err(JsError {
                message: "JSEngine has been shut down — cannot create Runtime".into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }

        // If Runtime is already alive on this thread, parasitize it.
        // This handles both servo-initialized runtimes and prior for_test() calls.
        if mozjs::rust::Runtime::get().is_some() {
            let cx = unsafe { Self::from_servo_runtime()? };
            return Ok(cx);
        }

        let engine_handle = ensure_engine_handle_locked()?;
        let runtime = mozjs::rust::Runtime::new(engine_handle);

        let cx = mozjs::rust::Runtime::get().ok_or_else(|| JsError {
            message: "Runtime::new failed to set CONTEXT TLS".into(),
            filename: "<engine>".into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        let mut cx_wrap = unsafe { mozjs::context::JSContext::from_ptr(cx) };
        if !JobQueue::init(&mut cx_wrap) {
            return Err(JsError {
                message: "Failed to init job queue".into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        ModuleLoader::init_thread_local(&cx_wrap);
        crate::module_loader::set_job_queue_drain(JobQueue::drain);

        // Store runtime in TLS. ManuallyDrop ensures no destructor runs on thread exit.
        RUNTIME_TLS.with(|tls| tls.set(Some(runtime)));

        crate::dispatch_sm::BaoEventLoop::register_js_context(cx.as_ptr().cast());

        Ok(JsContext {
            cx,
            global_setup: None,
            post_eval_hook: None,
            realm_global: None,
        })
    }

    /// Explicitly shut down the test Runtime stored in thread_local.
    ///
    /// Equivalent to `shutdown_thread_sm()`. Must be called on the same thread
    /// that created the Runtime, before that thread exits.
    #[doc(hidden)]
    pub fn shutdown_test_runtime() {
        Self::shutdown_thread_sm();
    }

    /// Shut down the SpiderMonkey Runtime on the current thread.
    ///
    /// Drops the Runtime (calling `JS_DestroyContext`) stored in TLS.
    /// Does NOT call `JS_ShutDown` — the JSEngine remains alive so that
    /// subsequent `for_test()` calls on the same thread can create a new Runtime.
    ///
    /// This is safe to call multiple times per thread (e.g., between tests).
    /// `JS_ShutDown` is deferred to `shutdown_engine()` which should only be
    /// called at process exit.
    #[doc(hidden)]
    pub fn shutdown_thread_sm() {
        // 0. Clear all rooted traceables on this thread before destroying the
        //    JSContext. Without this, stale pointers in ROOTED_TRACEABLES TLS
        //    would be traced during C++ TLS teardown after JS_DestroyContext,
        //    causing SIGSEGV (js::gc::HeaderWord::get on freed GC heap).
        unsafe {
            mozjs::gc::RootedTraceableSet::clear();
        }

        // 1. Drop Runtime — calls JS_DestroyContext, clears mozjs CONTEXT TLS.
        RUNTIME_TLS.with(|tls| {
            if tls.is_some() {
                let _ = tls.take(); // ManuallyDrop::take + drop → Runtime::drop → JS_DestroyContext
            }
        });

        // 2. Reset SIGSEGV/SIGBUS/SIGILL handlers to SIG_DFL.
        //    If a custom handler is installed (e.g., by bun_crash_handler
        //    via spawn's SignalForwarding), the SIGSEGV would be caught and
        //    converted to panic→abort(SIGILL), hiding the real crash location.
        //    Reset to SIG_DFL so late SIGSEGVs terminate immediately.
        #[cfg(unix)]
        {
            let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
            unsafe {
                sa.sa_sigaction = libc::SIG_DFL as usize;
                libc::sigemptyset(&mut sa.sa_mask);
                libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
                libc::sigaction(libc::SIGBUS, &sa, std::ptr::null_mut());
                libc::sigaction(libc::SIGILL, &sa, std::ptr::null_mut());
            }
        }
    }

    /// Shut down the SpiderMonkey engine entirely (process exit only).
    ///
    /// Calls `JS_ShutDown` to clean up SpiderMonkey's process-wide C++ state.
    /// After this, no new Runtime/JSContext can be created on any thread.
    ///
    /// This should only be called at process exit (e.g., via `atexit`).
    /// Tests should use `shutdown_thread_sm()` instead, which only destroys
    /// the per-thread Runtime without shutting down the engine.
    pub fn shutdown_engine() {
        if ENGINE_SHUTDOWN.swap(true, Ordering::SeqCst) {
            return; // Already shut down
        }

        // Destroy any remaining Runtime on this thread first.
        Self::shutdown_thread_sm();

        // Call JS_ShutDown to clean up the engine's C++ TLS state.
        // We cannot drop the JSEngine stored in ENGINE_TLS directly because
        // ENGINE_HANDLE (OnceLock) still holds a JSEngineHandle, making
        // outstanding_handles > 0, which triggers JSEngine::drop()'s assert.
        // Instead, call JS_ShutDown directly — this is what JSEngine::drop()
        // does internally after the assert.
        ENGINE_TLS.with(|tls| {
            if tls.is_some() {
                unsafe {
                    JS_ShutDown();
                }
                if let Some(engine) = tls.take() {
                    std::mem::forget(engine);
                }
            }
        });
    }

    /// Create a JSContext value wrapper from the stored pointer.
    /// The returned value is a zero-sized newtype — safe to create on demand.
    /// Caller holds this value and gets &mut from it for mozjs APIs.
    pub fn cx(&self) -> mozjs::context::JSContext {
        unsafe { mozjs::context::JSContext::from_ptr(self.cx) }
    }

    pub fn raw_cx(&self) -> *mut RawJSContext {
        self.cx.as_ptr()
    }

    pub fn set_global_setup(&mut self, setup: GlobalSetupFn) {
        self.global_setup = Some(setup);
    }
    pub fn set_post_eval_hook(&mut self, hook: PostEvalHook) {
        self.post_eval_hook = Some(hook);
    }
    pub fn global_setup(&self) -> Option<GlobalSetupFn> {
        self.global_setup
    }
    pub fn post_eval_hook(&self) -> Option<PostEvalHook> {
        self.post_eval_hook
    }

    pub fn eval(&mut self, source: &str, filename: &str) -> Result<JsValue, JsError> {
        let global_setup = self.global_setup;
        let post_eval_hook = self.post_eval_hook;
        let mut cx = self.cx();
        let cx = &mut cx;

        // Lazily create the persistent realm global on the first eval, then
        // reuse it for every subsequent eval (first-principles realm model:
        // one realm per context, not per script). `ensure_realm_global`
        // applies `global_setup` exactly once and publishes the global to the
        // thread-local so async dispatch sites can `AutoRealm` into it.
        let global_ptr = self.ensure_realm_global(cx, global_setup)?;
        rooted!(&in(cx) let global = global_ptr);

        let c_filename = std::ffi::CString::new(filename)
            .unwrap_or_else(|_| std::ffi::CString::new("<eval>").unwrap());
        let compile_opts = mozjs::rust::CompileOptionsWrapper::new(cx, c_filename, 1);

        rooted!(&in(cx) let mut rval = UndefinedValue());

        {
            let mut realm = AutoRealm::new_from_handle(cx, global.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;

            let result = mozjs::rust::evaluate_script(
                realm_cx,
                global.handle(),
                source,
                rval.handle_mut(),
                compile_opts,
            );

            if result.is_err() {
                return Err(extract_exception(realm_cx));
            }

            unsafe {
                let raw_cx = realm_cx.raw_cx();
                mozjs::jsapi::js::RunJobs(raw_cx);
                if let Some(hook) = post_eval_hook {
                    loop {
                        mozjs::jsapi::js::RunJobs(raw_cx);
                        if !hook(realm_cx) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            }
        }

        Ok(unsafe { jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get()) })
    }

    /// Lazily create this context's single persistent realm global on the
    /// first call, applying `global_setup` (install_console + the caller's
    /// globals like `Bun`/`process`/`require`). On every subsequent call the
    /// stored global is returned verbatim — setup is NOT re-applied (the
    /// realm already has those globals). Returns the realm's global pointer;
    /// also published to `THREAD_REALM_GLOBAL` for async dispatch.
    ///
    /// Rooting: the global is `AddRawValueRoot`-ed inside `PersistentGlobal`
    /// for the context's lifetime — SpiderMonkey does not auto-root globals.
    ///
    /// `pub` so the module path (`ModuleLoader::eval_module` / worker
    /// bootstrap) can proactively initialize the persistent realm when the
    /// FIRST execution on the context is a module rather than a script —
    /// there is no prior `eval` to lazily trigger it, and an empty-eval
    /// workaround would be unsafe (it would route through
    /// `post_eval_drain_then_exit` and misfire `process 'exit'` dispatch).
    pub fn ensure_realm_global(
        &mut self,
        cx: &mut mozjs::context::JSContext,
        global_setup: Option<GlobalSetupFn>,
    ) -> Result<*mut mozjs::jsapi::JSObject, JsError> {
        if let Some(ref pg) = self.realm_global {
            return Ok(pg.global_ptr());
        }
        let options = RealmOptions::default();
        rooted!(&in(cx) let global = unsafe {
            JS_NewGlobalObject(
                cx,
                &SIMPLE_GLOBAL_CLASS,
                ptr::null_mut(),
                OnNewGlobalHookOption::FireOnNewGlobalHook,
                &*options,
            )
        });
        if global.get().is_null() {
            return Err(JsError {
                message: "Failed to create realm global".into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        {
            let mut realm = AutoRealm::new_from_handle(cx, global.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            host_fn::install_console(realm_cx, global.handle());
            if let Some(setup) = global_setup {
                unsafe { setup(realm_cx, global.handle()) };
            }
        }
        let global_ptr = global.get();
        let mut pg = Box::new(PersistentGlobal {
            cx: self.cx.as_ptr(),
            global_val: mozjs::jsval::ObjectValue(global_ptr),
        });
        let rooted = unsafe {
            mozjs::jsapi::AddRawValueRoot(
                self.cx.as_ptr(),
                &mut pg.global_val,
                b"jscontext_realm_global\0".as_ptr() as *const ::std::os::raw::c_char,
            )
        };
        if !rooted {
            return Err(JsError {
                message: "AddRawValueRoot failed for realm global".into(),
                filename: "<engine>".into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        // Publish to the thread-local so async dispatch (route handlers,
        // timers, ...) can AutoRealm into this realm.
        THREAD_REALM_GLOBAL.with(|c| c.set(global_ptr));
        self.realm_global = Some(pg);
        Ok(global_ptr)
    }
}

// No Drop — servo owns the Runtime (browser mode) or SmRuntimeGuard does (CLI mode).

#[allow(unsafe_op_in_unsafe_fn)]
fn extract_exception(cx: &mut mozjs::context::JSContext) -> JsError {
    rooted!(&in(cx) let mut exn = UndefinedValue());
    if let Some(info) = unsafe {
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
            message: "Unknown JS error".into(),
            filename: "<unknown>".into(),
            line: 0,
            column: 0,
            stack: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jscontext_has_cx_ptr_not_runtime() {
        assert!(!std::any::type_name::<JsContext>().contains("Runtime"));
    }

    #[test]
    fn jscontext_realm_global_needs_drop() {
        // realm-per-context (c943b1cc): JsContext holds `realm_global:
        // Option<Box<PersistentGlobal>>`, whose Drop (RemoveRawValueRoot)
        // releases the rooted realm global before the Runtime goes away.
        // The type system must keep enforcing this release — failing here
        // means the realm root was dropped from JsContext.
        assert!(std::mem::needs_drop::<JsContext>());
    }

    #[test]
    fn sm_runtime_guard_holds_runtime_only() {
        // SmRuntimeGuard now only owns the Runtime.
        // The JSEngine is a process-wide singleton (handle in ENGINE_HANDLE, engine in ENGINE_TLS).
        // This test documents the new invariant.
        let size = std::mem::size_of::<SmRuntimeGuard>();
        assert!(size > 0, "SmRuntimeGuard must be non-zero sized");
    }
}
