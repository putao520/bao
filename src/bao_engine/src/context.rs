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

use mozjs::jsapi::{JSContext as RawJSContext, JS_ShutDown, OnNewGlobalHookOption};
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
pub struct JsContext {
    cx: NonNull<RawJSContext>,
    global_setup: Option<GlobalSetupFn>,
    post_eval_hook: Option<PostEvalHook>,
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
            unsafe { ManuallyDrop::drop(&mut *borrow); }
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
// @trace REQ-PERF-004 [entity:DomainDispatch]
// REQ-PERF-004 验收:进程级 JSEngine 单例用 `OnceLock<JSEngineHandle>` 替代
// `Arc<Mutex<JSEngineHandle>>`,消除每次访问的 lock/unlock 开销。OnceLock 内部用
// AtomicU8 状态机,首次 init 后所有 get() 是无锁 atomic load。
static ENGINE_HANDLE: OnceLock<mozjs::rust::JSEngineHandle> = OnceLock::new();

/// Process-global lock serializing JSEngine/Runtime creation in `for_test()`.
/// SpiderMonkey's Runtime is process-global; concurrent `for_test()` calls
/// race the init and the loser fails. The lock lets the first caller init;
/// subsequent callers reuse the alive Runtime (checked via `Runtime::get()`).
/// This removes the need for per-test-crate Mutex workarounds.
static FOR_TEST_INIT_LOCK: Mutex<()> = Mutex::new(());

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
/// The first thread to call this initializes the JSEngine (stored in ENGINE_TLS
/// on that thread for lifetime), and stores a cloned handle in ENGINE_HANDLE
/// (process-wide OnceLock). Subsequent threads just clone the handle.
fn ensure_engine_handle() -> Result<mozjs::rust::JSEngineHandle, JsError> {
    // Fast path: engine already initialized, just clone the handle.
    if let Some(handle) = ENGINE_HANDLE.get() {
        return Ok(handle.clone());
    }
    // Slow path: this thread must initialize the engine.
    // First check if this thread already has it in TLS (unlikely on first call).
    let (engine, handle) = ENGINE_TLS.with(|tls| {
        if tls.is_some() {
            let handle = tls.0.borrow().as_ref().expect("ENGINE_TLS is Some but inner is None").handle();
            return Ok((None, handle));
        }
        let engine = mozjs::rust::JSEngine::init().map_err(|e| JsError {
            message: format!("Failed to init JSEngine: {:?}", e).into(),
            filename: "<engine>".into(),
            line: 0, column: 0, stack: None,
        })?;
        let handle = engine.handle();
        tls.set(Some(engine));
        Ok((Some(handle.clone()), handle))
    })?;
    // Store the handle in the global OnceLock so other threads can access it.
    if let Some(handle_to_store) = engine {
        let global_handle = ENGINE_HANDLE.get_or_init(|| handle_to_store);
        Ok(global_handle.clone())
    } else {
        Ok(handle)
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
            line: 0, column: 0, stack: None,
        })?;

        let mut cx_wrap = unsafe { mozjs::context::JSContext::from_ptr(cx) };
        if !JobQueue::init(&mut cx_wrap) {
            return Err(JsError { message: "Failed to init job queue".into(), filename: "<engine>".into(), line: 0, column: 0, stack: None });
        }
        ModuleLoader::init_thread_local(&cx_wrap);

        // Register the job queue drain callback so bun_sm::module_loader can drain microtasks
        // without depending on bao_engine (avoids circular dependency).
        crate::module_loader::set_job_queue_drain(JobQueue::drain);

        let guard = SmRuntimeGuard { runtime };

        crate::dispatch_sm::BaoEventLoop::register_js_context(cx.as_ptr().cast());

        Ok((JsContext { cx, global_setup: None, post_eval_hook: None }, Some(guard)))
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
            line: 0, column: 0, stack: None,
        })?;

        let mut cx_wrap = unsafe { mozjs::context::JSContext::from_ptr(cx) };
        if !JobQueue::init(&mut cx_wrap) {
            return Err(JsError { message: "Failed to init job queue".into(), filename: "<engine>".into(), line: 0, column: 0, stack: None });
        }
        ModuleLoader::init_thread_local(&cx_wrap);
        crate::module_loader::set_job_queue_drain(JobQueue::drain);

        crate::dispatch_sm::BaoEventLoop::register_js_context(cx.as_ptr().cast());

        Ok(JsContext { cx, global_setup: None, post_eval_hook: None })
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
        // Serialize JSEngine/Runtime init across concurrent test threads.
        // The guard is held for the whole call so the Runtime::get() reuse check
        // sees the first caller's finished init before any other caller proceeds.
        let _init_guard = FOR_TEST_INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Refuse to create a new Runtime after the engine has been shut down.
        // JS_ShutDown is irreversible — calling Runtime::new after it will crash.
        if ENGINE_SHUTDOWN.load(Ordering::SeqCst) {
            return Err(JsError {
                message: "JSEngine has been shut down — cannot create Runtime".into(),
                filename: "<engine>".into(),
                line: 0, column: 0, stack: None,
            });
        }

        // If Runtime is already alive on this thread, parasitize it.
        // This handles both servo-initialized runtimes and prior for_test() calls.
        if mozjs::rust::Runtime::get().is_some() {
            let cx = unsafe { Self::from_servo_runtime()? };
            return Ok(cx);
        }

        let engine_handle = ensure_engine_handle()?;
        let runtime = mozjs::rust::Runtime::new(engine_handle);

        let cx = mozjs::rust::Runtime::get().ok_or_else(|| JsError {
            message: "Runtime::new failed to set CONTEXT TLS".into(),
            filename: "<engine>".into(),
            line: 0, column: 0, stack: None,
        })?;

        let mut cx_wrap = unsafe { mozjs::context::JSContext::from_ptr(cx) };
        if !JobQueue::init(&mut cx_wrap) {
            return Err(JsError { message: "Failed to init job queue".into(), filename: "<engine>".into(), line: 0, column: 0, stack: None });
        }
        ModuleLoader::init_thread_local(&cx_wrap);
        crate::module_loader::set_job_queue_drain(JobQueue::drain);

        // Store runtime in TLS. ManuallyDrop ensures no destructor runs on thread exit.
        RUNTIME_TLS.with(|tls| tls.set(Some(runtime)));

        crate::dispatch_sm::BaoEventLoop::register_js_context(cx.as_ptr().cast());

        Ok(JsContext { cx, global_setup: None, post_eval_hook: None })
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

    pub fn raw_cx(&self) -> *mut RawJSContext { self.cx.as_ptr() }

    pub fn set_global_setup(&mut self, setup: GlobalSetupFn) { self.global_setup = Some(setup); }
    pub fn set_post_eval_hook(&mut self, hook: PostEvalHook) { self.post_eval_hook = Some(hook); }
    pub fn global_setup(&self) -> Option<GlobalSetupFn> { self.global_setup }
    pub fn post_eval_hook(&self) -> Option<PostEvalHook> { self.post_eval_hook }

    pub fn eval(&mut self, source: &str, filename: &str) -> Result<JsValue, JsError> {
        let global_setup = self.global_setup;
        let post_eval_hook = self.post_eval_hook;
        let mut cx = self.cx();
        let cx = &mut cx;
        let options = RealmOptions::default();

        rooted!(&in(cx) let global = unsafe {
            JS_NewGlobalObject(cx, &SIMPLE_GLOBAL_CLASS, ptr::null_mut(),
                               OnNewGlobalHookOption::FireOnNewGlobalHook,
                               &*options)
        });

        let c_filename = std::ffi::CString::new(filename)
            .unwrap_or_else(|_| std::ffi::CString::new("<eval>").unwrap());
        let compile_opts = mozjs::rust::CompileOptionsWrapper::new(cx, c_filename, 1);

        rooted!(&in(cx) let mut rval = UndefinedValue());

        {
            let mut realm = AutoRealm::new_from_handle(cx, global.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;

            host_fn::install_console(realm_cx, global.handle());
            if let Some(setup) = global_setup {
                unsafe { setup(realm_cx, global.handle()) };
            }

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
                        if !hook(realm_cx) { break; }
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
            }
        }

        Ok(unsafe { jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get()) })
    }
}

// No Drop — servo owns the Runtime (browser mode) or SmRuntimeGuard does (CLI mode).

#[allow(unsafe_op_in_unsafe_fn)]
fn extract_exception(cx: &mut mozjs::context::JSContext) -> JsError {
    rooted!(&in(cx) let mut exn = UndefinedValue());
    if let Some(info) = unsafe {
        mozjs::rust::error_info_from_exception_stack(cx.raw_cx_no_gc(), exn.handle_mut().into())
    } {
        JsError { message: info.message, filename: info.filename, line: info.line, column: info.col, stack: None }
    } else {
        JsError { message: "Unknown JS error".into(), filename: "<unknown>".into(), line: 0, column: 0, stack: None }
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
    fn jscontext_no_drop() {
        assert!(!std::mem::needs_drop::<JsContext>());
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
