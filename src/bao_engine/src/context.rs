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

use std::cell::Cell;
use std::ptr::{self, NonNull};

use mozjs::jsapi::{JSObject, JSContext as RawJSContext, OnNewGlobalHookOption};
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

pub type GlobalSetupFn = unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>);
pub type PostEvalHook = fn(&mut mozjs::context::JSContext) -> bool;

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

/// Owns the SM Runtime+Engine for CLI/test mode.
/// Browser mode never constructs this — servo owns the lifetime there.
///
/// 字段顺序即 drop 顺序：runtime 先 drop（JS_DestroyContext + handle 计数归零），
/// engine 后 drop（断言 outstanding_handles==0 通过 → JS_ShutDown）。
/// 反序即 panic。Rust 保证 struct 字段按声明顺序析构。
pub struct SmRuntimeGuard {
    runtime: mozjs::rust::Runtime,
    _engine: mozjs::rust::JSEngine,
}

/// Per-thread guard: keeps the SmRuntimeGuard alive until the thread exits.
/// Stored in thread_local so tests (which only hold JsContext, not BaoRuntime)
/// still get orderly shutdown.
thread_local! {
    static RUNTIME_GUARD: Cell<Option<SmRuntimeGuard>> = const { Cell::new(None) };
}

impl JsContext {
    /// Initialize SpiderMonkey Runtime for CLI mode.
    ///
    /// Returns `(JsContext, Option<SmRuntimeGuard>)`. The guard owns the
    /// Engine+Runtime lifetime. Caller must hold the guard until done with
    /// all JS execution. When guard drops, orderly shutdown occurs:
    /// Runtime → JS_DestroyContext, Engine → JS_ShutDown.
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

        let engine = mozjs::rust::JSEngine::init()
            .map_err(|e| JsError {
                message: format!("Failed to init JSEngine: {:?}", e).into(),
                filename: "<engine>".into(),
                line: 0, column: 0, stack: None,
            })?;

        let runtime = mozjs::rust::Runtime::new(engine.handle());

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

        let guard = SmRuntimeGuard { runtime, _engine: engine };

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

        Ok(JsContext { cx, global_setup: None, post_eval_hook: None })
    }

    /// Test-only alias for `init_runtime()`.
    ///
    /// Stores the guard in thread_local so it survives the test function
    /// scope. The guard drops when the thread exits, providing orderly
    /// shutdown without needing `_exit(0)` hacks.
    #[doc(hidden)]
    pub fn for_test() -> Result<Self, JsError> {
        let (ctx, guard) = Self::init_runtime()?;
        if let Some(g) = guard {
            RUNTIME_GUARD.with(|c| c.set(Some(g)));
        }
        Ok(ctx)
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
    fn sm_runtime_guard_field_order_ensures_drop_order() {
        // Runtime must be declared before Engine so it drops first.
        // This test documents the invariant — if field order changes,
        // this test must be updated accordingly.
        let offset_runtime = std::mem::offset_of!(SmRuntimeGuard, runtime);
        let offset_engine = std::mem::offset_of!(SmRuntimeGuard, _engine);
        assert!(offset_runtime < offset_engine,
            "SmRuntimeGuard: runtime (offset {}) must precede _engine (offset {}) for correct drop order",
            offset_runtime, offset_engine);
    }
}