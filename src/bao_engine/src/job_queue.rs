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
pub fn set_uncaught_hooks(uncaught: UncaughtExceptionHook, flush: FlushRejectionsHook) {
    let _ = UNCAUGHT_HOOK.set(uncaught);
    let _ = FLUSH_HOOK.set(flush);
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

pub struct JobQueue;

impl JobQueue {
    pub fn init(cx: &mozjs::context::JSContext) -> bool {
        let traps = JobQueueTraps {
            getHostDefinedData: Some(get_host_defined_data),
            enqueuePromiseJob: Some(enqueue_job),
            runJobs: Some(run_jobs),
            empty: Some(is_empty),
            pushNewInterruptQueue: None,
            popInterruptQueue: None,
            dropInterruptQueues: None,
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
    loop {
        let job_entry = JOB_IDS.with(|q| q.borrow_mut().pop_front());
        let Some((id, global)) = job_entry else {
            break;
        };

        if global.is_null() {
            continue;
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
                continue;
            }
        }

        if !job_val.is_object() {
            continue;
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
    }

    // Job queue drained — dispatch unhandled promise rejections recorded by
    // the runtime's rejection tracker. Runs after every drain (all pump
    // paths funnel through this trap), on a clean JS stack.
    if let Some(&hook) = FLUSH_HOOK.get() {
        // SAFETY: cx is live (trap contract).
        unsafe { hook(cx) };
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn get_host_defined_data(
    _queue: *const c_void,
    _cx: *mut JSContext,
    data: MutableHandle<*mut JSObject>,
) -> bool {
    data.set(ptr::null_mut());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn is_empty(_queue: *const c_void) -> bool {
    JOB_IDS.with(|q| q.borrow().is_empty())
}
