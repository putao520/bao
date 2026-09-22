// @trace REQ-ENG-004
use ::std::cell::RefCell;
use ::std::collections::VecDeque;
use ::std::ffi::CString;
use ::std::os::raw::c_void;
use ::std::ptr;
use ::std::sync::atomic::{AtomicUsize, Ordering};
use ::std::sync::OnceLock;

use mozjs::glue::{CreateJobQueue, DeleteJobQueue, JobQueueTraps};
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{RunJobs, SetJobQueue};

static JOB_COUNTER: AtomicUsize = AtomicUsize::new(0);

// ── Uncaught-exception / unhandled-rejection hooks ─────────────────────────
//
// bao_engine cannot depend on bao_runtime (dependency edge is the other way),
// so the runtime registers its exception router here after context init —
// same indirection pattern as `module_loader::set_job_queue_drain`.
//
// `uncaught`: invoked when a job's JS_CallFunctionValue failed — the pending
//             exception has been captured and cleared by the trap; the hook
//             routes it (process.on('uncaughtException') or print + exit 1).
// `flush`:    invoked at the run_jobs tail (job queue drained) so the runtime
//             can dispatch unhandled promise rejections on a clean stack.

pub type UncaughtExceptionHook = unsafe fn(cx: *mut JSContext, reason: JSVal);
pub type FlushRejectionsHook = unsafe fn(cx: *mut JSContext);

static UNCAUGHT_HOOK: OnceLock<UncaughtExceptionHook> = OnceLock::new();
static FLUSH_HOOK: OnceLock<FlushRejectionsHook> = OnceLock::new();

/// Register the runtime's exception router. Idempotent (first registration
/// wins — every bao_runtime context installs the same functions).
///
/// Contract (first-writer-wins by design):
/// - the first registration installs (release semantics unchanged);
/// - re-registration with the **same** fn pointers is an idempotent no-op
///   (the documented multi-context shape: every `bao_runtime` context
///   installs the same zero-capture router, the first one serves the whole
///   process);
/// - re-registration with a **different** fn pointer means the callers
///   genuinely diverged — surfaced fail-closed under `debug_assertions`;
///   the first writer stays installed, so release keeps plain
///   first-writer-wins.
pub fn set_uncaught_hooks(uncaught: UncaughtExceptionHook, flush: FlushRejectionsHook) {
    if let Err(incoming) = UNCAUGHT_HOOK.set(uncaught) {
        // `OnceLock::set` returns the REJECTED value in `Err` (the cell keeps
        // its first writer), so `incoming` is the late registration.
        let diverged = match UNCAUGHT_HOOK.get() {
            ::std::option::Option::Some(installed) => {
                !::std::ptr::fn_addr_eq(*installed, incoming)
            }
            // set() only fails when a value is installed; defensive default.
            ::std::option::Option::None => true,
        };
        debug_assert!(
            !diverged,
            "UNCAUGHT_HOOK re-registration diverged: contract expects every \
             bao_runtime context to install the SAME zero-capture exception \
             router (first-writer-wins by design); a differing fn pointer is \
             real semantic drift, not an idempotent re-register"
        );
    }
    if let Err(incoming) = FLUSH_HOOK.set(flush) {
        let diverged = match FLUSH_HOOK.get() {
            ::std::option::Option::Some(installed) => {
                !::std::ptr::fn_addr_eq(*installed, incoming)
            }
            // set() only fails when a value is installed; defensive default.
            ::std::option::Option::None => true,
        };
        debug_assert!(
            !diverged,
            "FLUSH_HOOK re-registration diverged: contract expects every \
             bao_runtime context to install the SAME zero-capture rejection \
             flusher (first-writer-wins by design); a differing fn pointer \
             is real semantic drift, not an idempotent re-register"
        );
    }
}

thread_local! {
    // Track job IDs in order — the actual JSObject* is stored as a global property
    // (keyed by the id) on the global that was current at enqueue time. The
    // global pointer is stored alongside the id because `run_jobs` may run
    // outside any realm (event-loop tick / ConcurrentTask dispatch), where
    // `CurrentGlobalOrNull(cx)` is NULL and the job's backing global cannot
    // be rediscovered (BCE-BUG-ENG-370 companion fix). A realm's global
    // outlives the realm's jobs and is kept alive by its realm (and every
    // live job object is itself rooted as a property of that global).
    static JOB_IDS: RefCell<VecDeque<(usize, *mut mozjs::jsapi::JSObject)>> =
        const { RefCell::new(VecDeque::new()) };
    static QUEUE_PTR: RefCell<*mut mozjs::jsapi::JobQueue> = const { RefCell::new(ptr::null_mut()) };
}

fn job_prop_name(id: usize) -> CString {
    CString::new(format!("__job_{}", id)).unwrap_or_default()
}

/// ABI-safe [`JS::DequeueNextRegularMicroTask`] — root cause of the W8 深修②
/// teardown AV (`destroyRuntime → GC → RootingContext::traceStackRoots`,
/// dangling exact stack root on every promise-job context, Windows-only).
///
/// The C++ API returns `JS::Value` **by value**. `JS::Value` has
/// user-provided constructors, so under the MSVC x64 ABI (clang-cl follows
/// ms_abi) it is returned through a **hidden sret slot**: the callee expects
/// `(RCX = Value* slot, RDX = cx)`. The bindgen declaration reads as
/// `fn(cx) -> Value`, so Rust passes `cx` in RCX and interprets RAX as the
/// value. Net effect of every call:
///   1. the callee writes the dequeued value through `*RCX == *cx` —
///      `cx + 0` is `RootingContext::stackRoots_[0]`, so the first exact
///      stack-root list head is clobbered with GC-value bits;
///   2. Rust "reads" the value as the sret slot pointer (a dead stack
///      address), and that garbage sits in the `task` root for the drain
///      iteration.
/// The next GC walks the corrupted `stackRoots_` list → AV in
/// `traceStackRoots`. This is the W8 sret class (509761cc:
/// `already_AddRefed<Stencil>` / `JS::PropertyKey`); this family landed with
/// SM153 前移 after that sweep, so it was missed there.
///
/// Fix: re-declare the SAME mangled symbol with the ABI-correct sret shape
/// and route through it. Itanium returns 8-byte trivially-copyable
/// aggregates in RAX, so the plain bindgen declaration stays correct on
/// non-msvc targets — hence the cfg split. (`DequeueNextDebuggerMicroTask` /
/// `PeekNextMicroTask` share the by-value shape but have zero call sites in
/// the bao tree today; they stay on the plain declaration.)
#[cfg(target_env = "msvc")]
unsafe fn dequeue_next_regular_micro_task_abi_safe(cx: *mut JSContext) -> Value {
    unsafe extern "C" {
        #[link_name = "\u{1}?DequeueNextRegularMicroTask@JS@@YA?AVValue@1@PEAUJSContext@@@Z"]
        fn dequeue_next_regular_micro_task_sret(slot: *mut Value, cx: *mut JSContext);
    }
    let mut slot: Value = ::std::mem::zeroed();
    unsafe {
        dequeue_next_regular_micro_task_sret(&mut slot, cx);
    }
    slot
}

#[cfg(not(target_env = "msvc"))]
unsafe fn dequeue_next_regular_micro_task_abi_safe(cx: *mut JSContext) -> Value {
    unsafe { JS::DequeueNextRegularMicroTask(cx) }
}

pub struct JobQueue;

impl JobQueue {
    pub fn init(cx: &mozjs::context::JSContext) -> bool {
        // SM153: promise reaction jobs enqueue into the engine-owned regular
        // microtask queue (no enqueuePromiseJob trap); runJobs drains both
        // that queue and bao's stored jobs. The interrupt-queue traps must be
        // real functions now — RustJobQueue's destructor and SavedQueue
        // bookkeeping call them unconditionally.
        let traps = JobQueueTraps {
            getHostDefinedData: Some(get_host_defined_data),
            getHostDefinedGlobal: Some(get_host_defined_global),
            runJobs: Some(run_jobs),
            traceNonGCThingMicroTask: Some(trace_non_gc_thing_microtask),
            pushNewInterruptQueue: Some(push_new_interrupt_queue),
            popInterruptQueue: Some(pop_interrupt_queue),
            dropInterruptQueues: Some(drop_interrupt_queues),
        };

        let queue = unsafe { CreateJobQueue(&traps, ptr::null(), ptr::null_mut()) };
        if queue.is_null() {
            return false;
        }

        QUEUE_PTR.with(|p| {
            *p.borrow_mut() = queue;
        });

        unsafe { SetJobQueue(cx, queue) }
        true
    }

    pub fn drain(cx: &mut mozjs::context::JSContext) {
        unsafe { RunJobs(cx) }
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        QUEUE_PTR.with(|p| {
            let ptr = *p.borrow();
            if !ptr.is_null() {
                unsafe { DeleteJobQueue(ptr) };
                *p.borrow_mut() = ptr::null_mut();
            }
        });
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn enqueue_job(
    _queue: *const c_void,
    cx: *mut JSContext,
    _promise: Handle<*mut JSObject>,
    job: Handle<*mut JSObject>,
    _allocation_site: Handle<*mut JSObject>,
    _host_defined_data: Handle<*mut JSObject>,
) -> bool {
    let job_obj = *job.ptr;
    if job_obj.is_null() {
        return true;
    }

    let id = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let global = unsafe { CurrentGlobalOrNull(cx) };
    if global.is_null() {
        return true;
    }

    // Store job as a property on the global object — GC-safe
    let prop = job_prop_name(id);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let job_root = mozjs::jsval::ObjectValue(job_obj));
    rooted!(&in(wrapped_cx) let global_root = global);
    unsafe {
        JS_DefineProperty(
            cx,
            global_root.handle().into(),
            prop.as_ptr(),
            job_root.handle().into(),
            0,
        );
    }

    JOB_IDS.with(|q| {
        q.borrow_mut().push_back((id, global));
    });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn run_jobs(_queue: *const c_void, cx: *mut JSContext) {
    // SM153: fixpoint-drain BOTH sources — the engine's regular microtask
    // queue (promise reactions, engine jobs) and bao's stored jobs — running
    // one kind can enqueue more of the other. Ordering parity with SM140:
    // jobs run FIFO per source, interleaved to fixpoint, then the rejection
    // flush fires on a clean stack.
    loop {
        let mut progress = false;

        // (a) engine regular microtasks — promise reactions etc.
        while JS::HasRegularMicroTasks(cx) {
            progress = true;
            rooted!(in(cx) let task = dequeue_next_regular_micro_task_abi_safe(cx));
            let task_val: Value = task.handle().get();
            let job = JS::ToMaybeWrappedJSMicroTask(&task_val);
            if job.is_null() {
                continue;
            }
            let global = JS::GetExecutionGlobalFromJSMicroTask(job);
            if global.is_null() {
                continue;
            }
            let mut wrapped_cx =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let mut realm = AutoRealm::new(
                &mut wrapped_cx,
                ::std::ptr::NonNull::new_unchecked(global),
            );
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            rooted!(&in(realm_cx) let job_root = job);
            unsafe {
                if !JS::RunJSMicroTask(cx, job_root.handle().into())
                    && JS_IsExceptionPending(cx)
                {
                    // A job threw. Capture the pending exception, clear it,
                    // and hand it to the runtime's uncaught-exception router
                    // (same contract as bao's stored-job throws below).
                    let mut exn = UndefinedValue();
                    JS_GetPendingException(
                        cx,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut exn,
                        },
                    );
                    JS_ClearPendingException(cx);
                    rooted!(&in(realm_cx) let reason_root = exn);
                    if !exn.is_undefined() {
                        if let Some(&hook) = UNCAUGHT_HOOK.get() {
                            hook(cx, exn);
                        }
                    }
                }
            }
        }

        // (b) one of bao's own stored jobs (queueMicrotask closures kept as
        // global properties).
        if run_one_bao_job(cx) {
            progress = true;
        }

        if !progress {
            break;
        }
    }

    // Job queue drained — dispatch unhandled promise rejections recorded by
    // the runtime's rejection tracker. Runs after every drain (all pump
    // paths funnel through this trap), on a clean JS stack.
    if let Some(&hook) = FLUSH_HOOK.get() {
        // SAFETY: cx is live (trap contract).
        unsafe { hook(cx) };
    }
}

/// Run a single job from bao's stored-job queue. Returns true when a job ran.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn run_one_bao_job(cx: *mut JSContext) -> bool {
    {
        let job_entry = JOB_IDS.with(|q| q.borrow_mut().pop_front());
        let Some((id, global)) = job_entry else {
            return false;
        };

        if global.is_null() {
            return true;
        }

        // `run_jobs` is invoked from js::RunJobs which may fire outside any
        // realm (event-loop tick, ConcurrentTask dispatch) — cx->realm_ is
        // NULL there, so property access on `global` requires entering its
        // realm first. AutoRealm restores the (possibly NULL) previous realm
        // on drop.
        let prop = job_prop_name(id);
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let mut realm = AutoRealm::new(
            &mut wrapped_cx,
            ::std::ptr::NonNull::new_unchecked(global),
        );
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        rooted!(&in(realm_cx) let global_root = global);
        let mut job_val = UndefinedValue();
        unsafe {
            // BCE (P0 browser startup panic, servo error.rs:74): the job pump
            // probes the per-thread global (servo Window in browser mode) for
            // the queued job closure. A failed JS_GetProperty (throwing
            // accessor / proxy hook) returns false WITH the exception
            // pending; the old code ignored the return, so the stale
            // exception leaked onto the ScriptThread context and detonated
            // servo's `assert!(!JS_IsExceptionPending)` in
            // `throw_dom_exception` on the next error path. Consume it — the
            // job reads as absent and is skipped.
            if !JS_GetProperty(
                cx,
                global_root.handle().into(),
                prop.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut job_val,
                },
            ) {
                JS_ClearPendingException(cx);
                return true;
            }
        }

        if !job_val.is_object() {
            return true;
        }

        let mut rval = UndefinedValue();
        rooted!(&in(realm_cx) let obj_root = global);
        rooted!(&in(realm_cx) let fval_root = job_val);
        let empty_args = HandleValueArray::empty();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };

        unsafe {
            let ok = JS_CallFunctionValue(
                cx,
                obj_root.handle().into(),
                fval_root.handle().into(),
                &empty_args,
                rval_handle,
            );
            if !ok {
                // The job threw. Capture the pending exception, clear it, and
                // hand it to the runtime's uncaught-exception router (Node:
                // a queueMicrotask/job throw is an uncaught exception — NOT
                // silently swallowed). `reason_root` keeps the value alive
                // across the hook's JS dispatch.
                let mut exn = UndefinedValue();
                JS_GetPendingException(
                    cx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut exn,
                    },
                );
                JS_ClearPendingException(cx);
                rooted!(&in(realm_cx) let reason_root = exn);
                if !exn.is_undefined() {
                    if let Some(&hook) = UNCAUGHT_HOOK.get() {
                        // SAFETY: cx is live (trap contract); hook roots its
                        // argument before running JS.
                        unsafe { hook(cx, exn) };
                    }
                }
            }
        }

        // Clean up the property after execution
        unsafe {
            JS_DeleteProperty1(cx, global_root.handle().into(), prop.as_ptr());
        }

        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn get_host_defined_data(
    _cx: *mut JSContext,
    incumbent_global: MutableHandle<*mut JSObject>,
    optional_host_defined_data: MutableHandle<*mut JSObject>,
) -> bool {
    incumbent_global.set(ptr::null_mut());
    optional_host_defined_data.set(ptr::null_mut());
    true
}

/// SM153 new trap: the host-defined global for the current execution.
/// bao mirrors its stored-job global semantics: the realm's own global.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn get_host_defined_global(
    cx: *mut JSContext,
    data: MutableHandle<*mut JSObject>,
) -> bool {
    data.set(unsafe { CurrentGlobalOrNull(cx) });
    true
}

/// SM153 new trap: GC tracing for non-GC-thing microtask values. bao's
/// microtask values are all GC-things (JS objects/closures), so there is
/// nothing non-GC to trace — the SM140 face had no counterpart at all.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn trace_non_gc_thing_microtask(
    _trc: *mut JSTracer,
    _value_ptr: *mut Value,
) {
}

// ── Debugger interrupt-queue stack (SM153 requires real traps) ──────────────
//
// SM140 let bao leave these traps as None; 153's RustJobQueue destructor and
// SavedQueue bookkeeping call them unconditionally. bao runs no debugger
// interrupt queues, so the stack hands out unique well-formed tokens and
// keeps the pop-matches-push contract the C++ SavedQueue asserts.
thread_local! {
    static INTERRUPT_QUEUES: ::std::cell::RefCell<Vec<*const c_void>> =
        const { ::std::cell::RefCell::new(::std::vec::Vec::new()) };
}
static INTERRUPT_TOKEN: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(1);

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn push_new_interrupt_queue(_a: *mut c_void) -> *const c_void {
    let token = INTERRUPT_TOKEN.fetch_add(1, Ordering::Relaxed) as *const c_void;
    INTERRUPT_QUEUES.with(|q| q.borrow_mut().push(token));
    token
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn pop_interrupt_queue(_a: *mut c_void) -> *const c_void {
    INTERRUPT_QUEUES.with(|q| q.borrow_mut().pop()).unwrap_or(ptr::null())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn drop_interrupt_queues(_a: *mut c_void) {
    INTERRUPT_QUEUES.with(|q| q.borrow_mut().clear());
}
