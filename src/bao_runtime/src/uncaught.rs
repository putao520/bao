// @trace REQ-ENG-006 [api:process.on uncaughtException/unhandledRejection]
//
// Unified uncaught-exception / unhandled-rejection router — Node semantics.
//
// Every async JS entry point (timers, promise jobs, EventEmitter listener
// dispatch) funnels callback exceptions here instead of silently clearing
// them:
//
//   uncaught exception:
//     - `process.on('uncaughtException', h)` registered → h(err); the process
//       keeps running and the exit code is untouched (the handler decides).
//     - no handler → full stack printed to stderr + exit code 1 + exit
//       requested (the orderly-exit machinery then dispatches 'exit'
//       listeners with code 1, exactly like process.exit(1)).
//     - the handler itself throws → fatal: printed + exit code 1, no
//       re-dispatch (re-entrancy latch).
//
//   unhandled promise rejection (SpiderMonkey rejection tracker):
//     - `process.on('unhandledRejection', h)` registered → h(reason, promise);
//       exit code untouched.
//     - no handler → Node's default `--unhandled-rejections=throw` mode: the
//       rejection escalates to the uncaught-exception path above.
//
// Rejections are NOT dispatched from inside the tracker callback (that fires
// mid-Job from SpiderMonkey internals). The tracker only records the rejected
// promise (rooted via GcStore); the flush runs after the job queue drains
// (`job_queue::run_jobs` tail hook) so handlers see a clean JS stack, and a
// `.catch()` attached in a later microtask cancels the pending entry first
// (the tracker's Handled callback drops both the entry and its GcStore root)
// — same delayed-detection model as Node's per-tick unhandled-rejection scan.

use ::std::cell::{Cell, RefCell};
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;

use crate::gc_store::{gc_store_get_ns, gc_store_insert_ns, gc_store_remove_ns};

/// Monotonic id for GcStore keys of pending-unhandled promises.
static UNHANDLED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// GcStore namespace for pending-unhandled promise roots.
const NS: &str = "unhandled";

thread_local! {
    /// Re-entrancy latch: true while an uncaughtException/unhandledRejection
    /// handler is being dispatched. A throw observed while latched is the
    /// handler itself failing — fatal, never re-dispatched (Node semantics).
    static DISPATCHING: Cell<bool> = const { Cell::new(false) };

    /// Re-entrancy guard for flush_pending_rejections (route dispatch can run
    /// jobs that re-enter the trap's tail flush).
    static FLUSHING: Cell<bool> = const { Cell::new(false) };

    /// Rejected-without-handler promises awaiting the post-job-queue flush.
    /// The JSObject is rooted as a global property under `gc_key`.
    static PENDING_REJECTIONS: RefCell<Vec<PendingRejection>> =
        const { RefCell::new(Vec::new()) };

    /// When true, default reports append to CAPTURED instead of stderr —
    /// lets integration tests assert the printed stack without forking a
    /// process. Set by `begin_capture()` in tests.
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    static CAPTURED: RefCell<String> = const { RefCell::new(String::new()) };
}

struct PendingRejection {
    promise: *mut JSObject,
    gc_key: String,
}

/// RAII latch reset — survives early returns and handler throws.
struct LatchReset;
impl Drop for LatchReset {
    fn drop(&mut self) {
        DISPATCHING.with(|c| c.set(false));
    }
}

/// Install the router on the current thread's runtime:
/// - registers the SpiderMonkey promise-rejection tracker (idempotent — the
///   callback lives on the runtime; re-installs overwrite with the same fn);
/// - registers the bao_engine job-queue hooks (uncaught-exception routing +
///   pending-rejection flush) so `job_queue` can call back without a
///   bao_engine → bao_runtime dependency edge (same pattern as
///   `module_loader::set_job_queue_drain`).
pub fn install(cx: &mut mozjs::context::JSContext) {
    unsafe {
        mozjs_sys::jsapi::JS::SetPromiseRejectionTrackerCallback(
            cx.raw_cx(),
            Some(promise_rejection_tracker),
            ::std::ptr::null_mut(),
        );
    }
    bao_engine::job_queue::set_uncaught_hooks(
        uncaught_exception_hook,
        flush_rejections_hook,
    );
}

// ──────────────────────────────────────────────────────────────────────────
// SpiderMonkey rejection tracker: record / cancel pending rejections
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn promise_rejection_tracker(
    cx: *mut JSContext,
    _muted_errors: bool,
    promise: mozjs_sys::jsapi::JS::HandleObject,
    state: mozjs_sys::jsapi::JS::PromiseRejectionHandlingState,
    _data: *mut ::std::os::raw::c_void,
) {
    let promise_ptr = *promise.ptr;
    if promise_ptr.is_null() {
        return;
    }
    match state {
        mozjs_sys::jsapi::JS::PromiseRejectionHandlingState::Unhandled => {
            // Re-record after a Handled→Unhandled cycle replaces the dropped
            // entry with a fresh GcStore root.
            let id = UNHANDLED_COUNTER.fetch_add(1, Ordering::Relaxed);
            let gc_key = format!("promise_{id}");
            // cx is the live context SpiderMonkey invoked us on; the tracker
            // fires with a realm active, so GcStore can root the promise as
            // a global property.
            gc_store_insert_ns(cx, NS, &gc_key, promise_ptr);
            PENDING_REJECTIONS.with(|p| {
                p.borrow_mut().push(PendingRejection {
                    promise: promise_ptr,
                    gc_key,
                })
            });
        }
        mozjs_sys::jsapi::JS::PromiseRejectionHandlingState::Handled => {
            // A handler arrived after the rejection was recorded (e.g.
            // `.catch` attached in a later microtask) — cancel every pending
            // entry for this promise, GcStore root included, so the flush
            // never dispatches it.
            let cancelled: Vec<String> = PENDING_REJECTIONS.with(|p| {
                let mut pending = p.borrow_mut();
                let mut keys = Vec::new();
                pending.retain(|e| {
                    if ::std::ptr::eq(e.promise, promise_ptr) {
                        keys.push(e.gc_key.clone());
                        false
                    } else {
                        true
                    }
                });
                keys
            });
            for key in cancelled {
                gc_store_remove_ns(cx, NS, &key);
            }
        }
    }
}

/// job_queue hook: route a job failure (pending exception captured by the
/// trap after JS_CallFunctionValue returned false).
unsafe fn uncaught_exception_hook(cx: *mut JSContext, reason: JSVal) {
    unsafe { route_uncaught_exception(cx, reason) };
}

/// job_queue hook: flush pending unhandled rejections after the job queue
/// drained (run_jobs tail).
unsafe fn flush_rejections_hook(cx: *mut JSContext) {
    flush_pending_rejections(cx);
}

// ──────────────────────────────────────────────────────────────────────────
// Routing
// ──────────────────────────────────────────────────────────────────────────

/// Route a synchronous-callback exception (timer fire, job failure, emitter
/// listener throw). `reason` is the captured pending-exception value.
///
/// # Safety
/// - `raw_cx` must be a live JSContext on this thread.
/// - `reason` must not be GC-relevant beyond this call (it is rooted for the
///   duration of the dispatch inside).
pub unsafe fn route_uncaught_exception(raw_cx: *mut JSContext, reason: JSVal) {
    if DISPATCHING.with(|c| c.get()) {
        // Throw inside the uncaughtException/unhandledRejection dispatch
        // itself — fatal, do not recurse (Node: an exception in the exception
        // handler terminates the process).
        report_default(raw_cx, "uncaught exception (inside exception handler)", reason);
        crate::request_exit(1);
        return;
    }
    DISPATCHING.with(|c| c.set(true));
    let _latch = LatchReset;

    match emit_process_event(raw_cx, c"uncaughtException", &[reason], 1) {
        EmitOutcome::Handled => {
            // Handler ran — Node semantics: the process keeps running and the
            // exit code is the handler's business.
        }
        EmitOutcome::NoListeners => {
            report_default(raw_cx, "uncaught exception", reason);
            crate::request_exit(1);
        }
        EmitOutcome::NoProcess => {
            // Browser page thread (REQ-SEC-003: no process on page globals) —
            // report only; the exit slots belong to the CLI runtime thread.
            report_default(raw_cx, "uncaught exception", reason);
        }
    }
}

/// Route an unhandled promise rejection: `unhandledRejection` handlers first;
/// without one, escalate as an uncaught exception (Node default `throw` mode).
///
/// # Safety
/// - `raw_cx` must be a live JSContext on this thread.
/// - `promise` must be a live JSObject for the duration of the call.
pub unsafe fn route_unhandled_rejection(
    raw_cx: *mut JSContext,
    reason: JSVal,
    promise: *mut JSObject,
) {
    match emit_process_event(
        raw_cx,
        c"unhandledRejection",
        &[reason, ObjectValue(promise)],
        2,
    ) {
        EmitOutcome::Handled => return,
        EmitOutcome::NoProcess => {
            // Page thread: report only (no exit slots to steer).
            report_default(raw_cx, "unhandled promise rejection", reason);
            return;
        }
        // No unhandledRejection handler — Node default `throw` mode:
        // escalate the rejection as an uncaught exception.
        EmitOutcome::NoListeners => {}
    }
    unsafe { route_uncaught_exception(raw_cx, reason) };
}

/// Outcome of a `process.emit(eventName, ...)` dispatch.
enum EmitOutcome {
    /// At least one listener ran (emit returned true).
    Handled,
    /// `process` exists but no listener was registered for the event.
    NoListeners,
    /// No `process` object on this global (browser page thread) or no usable
    /// global at all.
    NoProcess,
}

/// Dispatch `process.emit(eventName, args[0..argc])` and report whether a
/// listener ran.
///
/// Uses the realm-per-context convention: falls back to
/// `bao_engine::context::thread_realm_global()` when dispatch runs outside
/// any entered realm (drain-time timer/job dispatch) and enters that realm
/// for the call.
///
/// # Safety
/// - `raw_cx` must be a live JSContext on this thread.
/// - `args` slice must have at least `argc` values; they are rooted here
///   before any JS runs (fixed two-slot rooting — call sites pass at most 2).
unsafe fn emit_process_event(
    raw_cx: *mut JSContext,
    event_name: &::std::ffi::CStr,
    args: &[JSVal],
    argc: usize,
) -> EmitOutcome {
    debug_assert!(argc <= 2 && args.len() >= argc, "emit_process_event: max 2 args");

    let global = unsafe { CurrentGlobalOrNull(raw_cx) };
    let global = if global.is_null() {
        match bao_engine::context::thread_realm_global() {
            Some(g) if !g.is_null() => g,
            _ => return EmitOutcome::NoProcess,
        }
    } else {
        global
    };

    let mut cx_ref =
        unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)) };
    // Enter the thread realm for property access + call (job_queue run_jobs
    // convention). AutoRealm restores the previous realm on drop.
    let mut realm = AutoRealm::new(&mut cx_ref, NonNull::new_unchecked(global));
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;

    rooted!(&in(cx_ref) let global_root = global);

    let mut proc_val = UndefinedValue();
    unsafe {
        JS_GetProperty(
            raw_cx,
            global_root.handle().into(),
            c"process".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut proc_val,
            },
        );
    }
    if !proc_val.is_object() {
        return EmitOutcome::NoProcess;
    }
    rooted!(&in(cx_ref) let proc_obj = proc_val.to_object());

    // Root every arg (they may be caller-stack JSVals) BEFORE the string
    // allocation below — JS_NewStringCopyZ can trigger GC.
    rooted!(&in(cx_ref) let arg0 = args.first().copied().unwrap_or_else(UndefinedValue));
    rooted!(&in(cx_ref) let arg1 = args.get(1).copied().unwrap_or_else(UndefinedValue));
    let event_str = unsafe { JS_NewStringCopyZ(raw_cx, event_name.as_ptr()) };
    if event_str.is_null() {
        return EmitOutcome::NoListeners;
    }
    rooted!(&in(cx_ref) let event_str_val = unsafe { StringValue(&*event_str) });

    // [eventName, args[0..argc]] — rooted handles dereferenced into the
    // stack array; the rooted! guards above outlive the call.
    let call_vals = [
        *event_str_val.handle(),
        *arg0.handle(),
        *arg1.handle(),
    ];
    let call_args = HandleValueArray {
        length_: 1 + argc,
        elements_: call_vals.as_ptr(),
    };

    let mut rval = UndefinedValue();
    let ok = unsafe {
        JS_CallFunctionName(
            raw_cx,
            proc_obj.handle().into(),
            c"emit".as_ptr(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        )
    };
    if !ok {
        // `emit` machinery failed (listener throws are handled inside
        // ee_emit's own routing). Clear and treat as not-dispatched.
        unsafe { JS_ClearPendingException(raw_cx) };
        return EmitOutcome::NoListeners;
    }
    if rval.is_boolean() && rval.to_boolean() {
        EmitOutcome::Handled
    } else {
        EmitOutcome::NoListeners
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Pending-rejection flush
// ──────────────────────────────────────────────────────────────────────────

/// Dispatch every recorded unhandled rejection. Called at the tail of
/// `job_queue::run_jobs` (all drains funnel there). Snapshot-and-drain: a
/// handler that rejects further promises records new entries which flush on
/// the next round — no unbounded recursion. Entries cancelled by a late
/// `.catch` (tracker Handled) lose their GcStore root before the flush and
/// are skipped via the `None` lookup.
///
/// # Safety
/// - `raw_cx` must be a live JSContext on this thread.
pub unsafe fn flush_pending_rejections(raw_cx: *mut JSContext) {
    if FLUSHING.with(|c| c.get()) {
        return;
    }
    FLUSHING.with(|c| c.set(true));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FLUSHING.with(|c| c.set(false));
        }
    }
    let _reset = Reset;

    let entries: Vec<PendingRejection> =
        PENDING_REJECTIONS.with(|p| ::std::mem::take(&mut *p.borrow_mut()));
    for entry in entries {
        // Take the root: None = cancelled by a late Handled before flush.
        let promise = gc_store_get_ns(raw_cx, NS, &entry.gc_key);
        gc_store_remove_ns(raw_cx, NS, &entry.gc_key);
        let Some(promise) = promise.filter(|p| !p.is_null()) else {
            continue;
        };

        let mut cx_ref =
            unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)) };
        rooted!(&in(cx_ref) let promise_root = promise);

        // Read the rejection reason. GetPromiseResult unwraps the promise
        // object; enter its realm first (AutoRealm) — the flush may run in
        // the thread's realm while the promise belongs to another.
        let mut reason = UndefinedValue();
        {
            let mut promise_realm = AutoRealm::new(&mut cx_ref, NonNull::new_unchecked(promise));
            let promise_cx: &mut mozjs::context::JSContext = &mut promise_realm;
            rooted!(&in(promise_cx) let p_root = promise);
            unsafe {
                mozjs_sys::glue::JS_GetPromiseResult(
                    p_root.handle().into(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut reason,
                    },
                );
            }
        }

        unsafe { route_unhandled_rejection(raw_cx, reason, promise) };
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Default report (no handler)
// ──────────────────────────────────────────────────────────────────────────

/// Print the uncaught value with its full stack to stderr (or the test
/// capture sink). Node prints `Error: msg\n    at ...` — the `.stack`
/// property of SM Error objects already carries both.
fn report_default(cx: *mut JSContext, label: &str, reason: JSVal) {
    let text = value_display(cx, reason);
    let report = format!("bao: {label}:\n{text}\n");
    if CAPTURING.with(|c| c.get()) {
        CAPTURED.with(|c| c.borrow_mut().push_str(&report));
    } else {
        eprint!("{report}");
    }
}

/// Human-readable form of a thrown/rejected value: Error message + stack
/// when available (SpiderMonkey's `.stack` carries location lines only —
/// the message line is prepended here so the report always names the error),
/// raw string for string throws, primitive rendering otherwise.
fn value_display(cx: *mut JSContext, val: JSVal) -> String {
    if val.is_object() {
        // SAFETY: cx is a live context on this thread (callers route from
        // live dispatch sites; the latched-fatal branch passes a live cx too).
        let cx_ref = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
        rooted!(&in(cx_ref) let obj = val.to_object());
        let read_prop = |name: &::std::ffi::CStr| -> Option<String> {
            let mut v = UndefinedValue();
            unsafe {
                // BCE (P0 browser startup panic, servo error.rs:74): the
                // thrown value is a caller-supplied object; a hostile
                // `stack`/`message` getter makes JS_GetProperty fail WITH
                // the exception pending. This runs on the servo
                // ScriptThread context mid-routing — a leaked second
                // exception detonates servo's
                // `assert!(!JS_IsExceptionPending)` on the next error path.
                // Clearing probe: failed read = "absent" diagnostic.
                bao_stealth::engine_props::get_property_clearing(
                    cx,
                    obj.handle().into(),
                    name,
                    &mut v,
                );
            }
            if v.is_string() {
                Some(unsafe { crate::js_to_rust_string(cx, v) })
            } else {
                None
            }
        };
        let stack = read_prop(c"stack");
        let msg = read_prop(c"message");
        return match (msg, stack) {
            (Some(m), Some(s)) if s.contains(&m) => s,
            (Some(m), Some(s)) => format!("Error: {m}\n{s}"),
            (Some(m), None) => format!("Error: {m}"),
            (None, Some(s)) => s,
            (None, None) => "<non-error object>".to_string(),
        };
    }
    if val.is_string() {
        return unsafe { crate::js_to_rust_string(cx, val) };
    }
    if val.is_int32() {
        return val.to_int32().to_string();
    }
    if val.is_double() {
        return val.to_double().to_string();
    }
    if val.is_boolean() {
        return val.to_boolean().to_string();
    }
    if val.is_null() {
        return "null".to_string();
    }
    if val.is_undefined() {
        return "undefined".to_string();
    }
    "<unprintable value>".to_string()
}

// ──────────────────────────────────────────────────────────────────────────
// Test capture
// ──────────────────────────────────────────────────────────────────────────

/// Begin capturing default reports (tests). While active, reports append to
/// the capture buffer instead of writing to stderr.
pub fn begin_capture() {
    CAPTURED.with(|c| c.borrow_mut().clear());
    CAPTURING.with(|c| c.set(true));
}

/// End capturing and return everything reported since `begin_capture`.
pub fn take_capture() -> String {
    CAPTURING.with(|c| c.set(false));
    CAPTURED.with(|c| ::std::mem::take(&mut *c.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_roundtrip() {
        begin_capture();
        report_default(::std::ptr::null_mut(), "uncaught exception", UndefinedValue());
        let out = take_capture();
        assert!(out.contains("uncaught exception"), "capture holds report: {out}");
        assert!(out.contains("undefined"), "undefined throw value rendered: {out}");
        assert!(out.ends_with('\n'), "report newline-terminated");
    }

    #[test]
    fn value_display_primitives_without_cx() {
        // Non-object, non-string primitives never touch cx.
        assert_eq!(
            value_display(::std::ptr::null_mut(), mozjs::jsval::Int32Value(7)),
            "7"
        );
        assert_eq!(
            value_display(::std::ptr::null_mut(), mozjs::jsval::BooleanValue(true)),
            "true"
        );
        assert_eq!(
            value_display(::std::ptr::null_mut(), mozjs::jsval::NullValue()),
            "null"
        );
        assert_eq!(
            value_display(::std::ptr::null_mut(), mozjs::jsval::DoubleValue(1.5)),
            "1.5"
        );
    }

    #[test]
    fn latched_route_reports_fatal_and_exits_1() {
        // The DISPATCHING branch never touches cx (undefined reason renders
        // without JSAPI), so a null cx exercises the latch semantics safely.
        crate::clear_exit();
        begin_capture();
        unsafe {
            DISPATCHING.with(|c| c.set(true));
            route_uncaught_exception(::std::ptr::null_mut(), UndefinedValue());
            DISPATCHING.with(|c| c.set(false));
        }
        let out = take_capture();
        assert!(
            out.contains("inside exception handler"),
            "latched route reports fatal handler failure: {out}"
        );
        assert!(crate::should_exit(), "fatal handler failure requests exit");
        assert_eq!(crate::exit_code(), 1, "fatal handler failure exits 1");
        crate::clear_exit();
    }
}
