// @trace REQ-ENG-001 [entity:BaoRuntime] — SpiderMonkey Interrupt.h 接入(engine-native
// execution control),SM-EVOLUTION #24 minimal closed loop(S0-A 同轮编码 slice)。
//!
//! # Interrupt / timeout / cancellation — minimal closed loop
//!
//! Engine facts (vendored SpiderMonkey, verified in `js/public/Interrupt.h` +
//! `js/src/vm/Runtime.cpp` `HandleInterrupt`):
//!
//! - `JS_AddInterruptCallback(cx, cb)` **appends** to a per-JSContext callback
//!   vector; callbacks run on the JS (owner) thread, some time after ANY
//!   thread calls the documented-thread-safe `JS_RequestInterruptCallback(cx)`.
//! - A callback returning `false` terminates the running script with an
//!   **uncatchable** exception: `HandleInterrupt` calls
//!   `cx->reportUncatchableException()`, which *clears* the pending exception
//!   — no exception state leaks onto the context (verified in
//!   `js/src/vm/JSContext.h:636`).
//! - Loop back-edges / JIT stack checks call `CheckForInterrupt`, so a plain
//!   `while(true){}` observes a requested interrupt within one back-edge —
//!   no polling, no cooperative script requirements.
//! - The engine ALSO invokes interrupt callbacks for its own reasons (GC
//!   slices, off-thread compile attach). A control callback that returned
//!   `false` with no armed control would terminate *unrelated* scripts on the
//!   shared thread context — therefore the callback continues (`true`)
//!   whenever no control is armed.
//!
//! # Ownership / threading model (CLAUDE.md JSContext 铁律)
//!
//! - All JS engine interaction happens on the owner thread (the armed-stack
//!   thread-local + the interrupt callback).
//! - External threads only get `ExecutionControl` (an `Arc` of atomics + a
//!   requester). `cancel()` submits an atomic flag and the documented
//!   thread-safe `JS_RequestInterruptCallback` — it never touches JSObject /
//!   GC cells / any other JSAPI entry.
//! - The deadline watcher thread is joined before `eval_with_control`
//!   returns (condvar-cancellable), so the raw context pointer it holds is
//!   provably alive for every request it can make.
//!
//! # Status: internal experimental surface
//!
//! `#[doc(hidden)]` + NOT a stable public API commitment (SM-EVOLUTION plan
//! Phase S1 will unify this with the runtime scheduler under issue #24/#25).

use std::cell::{Cell, RefCell};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use mozjs::jsapi::JSContext as RawJSContext;
use mozjs::jsapi::JS_RequestInterruptCallback;
use mozjs::rust::wrappers2::JS_AddInterruptCallback;

use crate::context::JsContext;
use crate::error::JsError;
use crate::value::JsValue;

// ── Terminal state ──────────────────────────────────────────────────────────

/// Terminal state of one controlled execution (#24: completed / errored /
/// cancelled / timed-out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalState {
    /// Armed but not finished (also the state after [`ExecutionControl::reset`]).
    Running,
    /// Script (incl. microtask drain) returned successfully.
    Completed,
    /// Script failed with a JS error (not a control termination).
    Errored,
    /// Cancelled via [`ExecutionControl::cancel`] before/at completion.
    Cancelled,
    /// Deadline exceeded; runaway script terminated by the interrupt callback.
    TimedOut,
}

const ST_RUNNING: u8 = 0;
const ST_COMPLETED: u8 = 1;
const ST_ERRORED: u8 = 2;
const ST_CANCELLED: u8 = 3;
const ST_TIMED_OUT: u8 = 4;

impl TerminalState {
    fn from_u8(v: u8) -> Self {
        match v {
            ST_COMPLETED => TerminalState::Completed,
            ST_ERRORED => TerminalState::Errored,
            ST_CANCELLED => TerminalState::Cancelled,
            ST_TIMED_OUT => TerminalState::TimedOut,
            _ => TerminalState::Running,
        }
    }
    fn to_u8(self) -> u8 {
        match self {
            TerminalState::Running => ST_RUNNING,
            TerminalState::Completed => ST_COMPLETED,
            TerminalState::Errored => ST_ERRORED,
            TerminalState::Cancelled => ST_CANCELLED,
            TerminalState::TimedOut => ST_TIMED_OUT,
        }
    }
}

// ── Thread-safe interrupt requester ─────────────────────────────────────────

/// The ONLY cross-thread operation this module performs on the JSContext:
/// `JS_RequestInterruptCallback` (Interrupt.h: "will be called from the JS
/// thread some time after any thread triggered the callback"). No JSObject /
/// GC pointer ever crosses a thread through this type.
struct InterruptRequester {
    cx: *mut RawJSContext,
}

// SAFETY: the raw pointer is used exclusively for the documented thread-safe
// `JS_RequestInterruptCallback` (sets an atomic interrupt bit on the context —
// the same operation mozjs' own `ThreadSafeJSContext::request_interrupt_callback`
// performs). Lifetime: every site that can invoke [`Self::request`] is joined
// before the owning `eval_with_control` frame returns, so the context is
// provably alive (see `ArmedExecutionGuard`).
unsafe impl Send for InterruptRequester {}
unsafe impl Sync for InterruptRequester {}

impl InterruptRequester {
    fn request(&self) {
        // SAFETY: see the Send/Sync note — documented thread-safe API; the
        // context is alive for every reachable call (watcher joined by the
        // eval frame; `cancel` callers hold an `ExecutionControl` which is
        // only handed out alongside a live owner context).
        unsafe { JS_RequestInterruptCallback(self.cx) };
    }
}

// ── Shared control state ────────────────────────────────────────────────────

struct ControlShared {
    /// Cancellation flag — any thread may set it (atomic submission only).
    cancelled: AtomicBool,
    /// Terminal-state latch: `Running` until exactly one of
    /// Completed/Errored/Cancelled/TimedOut wins.
    terminal: AtomicU8,
    /// Thread-safe interrupt request for the owner JSContext.
    requester: InterruptRequester,
}

impl ControlShared {
    /// Latch `state` only if still `Running` (first writer wins; a later
    /// deadline does not overwrite an earlier cancel, etc.).
    fn latch(&self, state: TerminalState) {
        let _ = self.terminal.compare_exchange(
            ST_RUNNING,
            state.to_u8(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

// ── Owner-thread armed-execution stack ──────────────────────────────────────

/// One armed controlled execution on the owner thread. The deadline is plain
/// owner-thread data (read only by the interrupt callback on this thread);
/// cross-thread state lives in [`ControlShared`].
struct ArmedExecution {
    control: Arc<ControlShared>,
    deadline: Option<Instant>,
}

thread_local! {
    /// Stack of armed controlled executions. The interrupt callback inspects
    /// only the TOP entry — the innermost armed eval — so nested controlled
    /// evals terminate innermost-first and an empty stack means "no control
    /// active → continue" (critical: engine-internal GC interrupts on the
    /// shared thread context must never terminate unrelated scripts).
    static ARMED: RefCell<Vec<ArmedExecution>> = const { RefCell::new(Vec::new()) };

    /// Raw JSContext address this thread's interrupt callback is installed on.
    /// `JS_AddInterruptCallback` APPENDS — installing per-eval would stack
    /// duplicate callbacks, so installation is once-per-JSContext. Cleared
    /// whenever the thread's context is destroyed or a fresh Runtime is
    /// created (see `on_context_destroyed` / `on_runtime_created`) so a
    /// recycled address never skips a needed install.
    static CALLBACK_INSTALLED_ON: Cell<*mut RawJSContext> = const { Cell::new(ptr::null_mut()) };
}

/// Install the engine interrupt callback on `cx` (once per JSContext).
pub(crate) fn ensure_callback_installed(cx: *mut RawJSContext) {
    CALLBACK_INSTALLED_ON.with(|c| {
        if c.get() == cx {
            return;
        }
        let cx_wrap = unsafe {
            mozjs::context::JSContext::from_ptr(ptr::NonNull::new_unchecked(cx))
        };
        // SAFETY: cx is the live owner-thread context; the callback is
        // 'static and reads only thread-local + atomic state.
        let ok = unsafe { JS_AddInterruptCallback(&cx_wrap, Some(bao_interrupt_callback)) };
        // The append only fails on OOM (jsapi.cpp: interruptCallbacks().append).
        assert!(ok, "JS_AddInterruptCallback failed (OOM appending callback)");
        c.set(cx);
    });
}

/// Clear the install-tracking TLS — called when this thread's JSContext is
/// destroyed (`JsContext::shutdown_thread_sm`), so a later context (possibly
/// at a recycled address) installs a fresh callback.
pub(crate) fn on_context_destroyed() {
    CALLBACK_INSTALLED_ON.with(|c| c.set(ptr::null_mut()));
}

/// Clear the install-tracking TLS — called right after bao_engine creates a
/// fresh Runtime (`init_runtime` / `for_test`), covering the CLI path where
/// the old context is destroyed by `SmRuntimeGuard` drop without
/// `shutdown_thread_sm`.
pub(crate) fn on_runtime_created() {
    CALLBACK_INSTALLED_ON.with(|c| c.set(ptr::null_mut()));
}

/// The SpiderMonkey interrupt callback — runs on the owner JS thread.
///
/// Decision table:
/// - no armed control → `true` (continue — engine-internal interrupts such as
///   GC slices must never terminate unrelated scripts);
/// - cancel flag set → latch `Cancelled`, terminate;
/// - deadline reached → latch `TimedOut`, terminate;
/// - otherwise → `true` (continue).
///
/// Never re-enters the engine, allocates, or touches the exception state
/// (Interrupt.h re-entrancy note; `CheckForInterrupt` MOZ_ASSERTs no pending
/// exception on entry).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bao_interrupt_callback(_cx: *mut RawJSContext) -> bool {
    let mut decision = true;
    ARMED.with(|stack| {
        let mut s = stack.borrow_mut();
        let Some(armed) = s.last_mut() else {
            return; // nothing armed → continue
        };
        if armed.control.cancelled.load(Ordering::Acquire) {
            armed.control.latch(TerminalState::Cancelled);
            decision = false;
        } else if armed
            .deadline
            .map_or(false, |d| Instant::now() >= d)
        {
            armed.control.latch(TerminalState::TimedOut);
            decision = false;
        }
        // Deadline not reached, not cancelled → continue (`true`).
    });
    decision
}

// ── Deadline watcher (condvar-cancellable, joined by the eval frame) ────────

struct WatcherSignal {
    done: Mutex<bool>,
    fire: Condvar,
}

/// Owns the deadline watcher thread. Drop pops the armed execution and joins
/// the watcher, therefore every `JS_RequestInterruptCallback` the watcher can
/// issue happens strictly inside the `eval_with_control` frame (context alive)
/// and a fast eval never blocks until its (unused) deadline.
struct ArmedExecutionGuard {
    watcher: Option<std::thread::JoinHandle<()>>,
    signal: Arc<WatcherSignal>,
}

impl ArmedExecutionGuard {
    fn new(control: Arc<ControlShared>, deadline: Option<Instant>) -> Self {
        ARMED.with(|s| s.borrow_mut().push(ArmedExecution {
            control: control.clone(),
            deadline,
        }));

        let signal = Arc::new(WatcherSignal {
            done: Mutex::new(false),
            fire: Condvar::new(),
        });
        let watcher = deadline.map(|d| {
            let sig = Arc::clone(&signal);
            // Move the Send-safe shared control (atomics + the Send
            // InterruptRequester) into the watcher — no bare raw pointer is
            // ever captured by the closure.
            let shared = Arc::clone(&control);
            std::thread::spawn(move || {
                let mut done = sig.done.lock().unwrap();
                while !*done {
                    let now = Instant::now();
                    if now >= d {
                        // Deadline reached: request the interrupt on the
                        // owner context. Thread-safe per Interrupt.h; the
                        // context is alive because the owning eval frame
                        // joins this watcher before it returns.
                        shared.requester.request();
                        return;
                    }
                    let (guard, _) = sig
                        .fire
                        .wait_timeout(done, d - now)
                        .unwrap_or_else(|e| e.into_inner());
                    done = guard;
                }
            })
        });
        ArmedExecutionGuard { watcher, signal }
    }
}

impl Drop for ArmedExecutionGuard {
    fn drop(&mut self) {
        // 1. Signal + join the watcher FIRST — while the armed entry is still
        //    on the stack the context is definitionally alive.
        *self.signal.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.signal.fire.notify_all();
        if let Some(handle) = self.watcher.take() {
            let _ = handle.join();
        }
        // 2. Pop this armed execution (innermost-first under nesting; also
        //    runs on panic unwind, so a poisoned eval cannot leak control
        //    state onto the next one).
        ARMED.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

// ── Public handle ───────────────────────────────────────────────────────────

/// Handle to one controlled execution. Cloneable and shareable to any thread;
/// external threads may only submit cancellation (atomic flag + the
/// documented thread-safe interrupt request) and poll the terminal state.
///
/// Must be created on the owner JSContext thread and must not outlive its
/// JSContext. Internal experimental surface — NOT a stable API commitment.
#[derive(Clone)]
pub struct ExecutionControl {
    shared: Arc<ControlShared>,
}

#[doc(hidden)]
impl ExecutionControl {
    /// Create a control bound to the CURRENT thread's JSContext. Must be
    /// called on the owner thread with the runtime alive.
    ///
    /// # Panics
    /// If no `Runtime` is alive on this thread (fail-closed: a control with
    /// no owner context cannot request interrupts).
    pub fn new() -> Self {
        let cx = mozjs::rust::Runtime::get()
            .expect("ExecutionControl::new must run on the owner JSContext thread (Runtime alive)")
            .as_ptr();
        ExecutionControl {
            shared: Arc::new(ControlShared {
                cancelled: AtomicBool::new(false),
                terminal: AtomicU8::new(ST_RUNNING),
                requester: InterruptRequester { cx },
            }),
        }
    }

    /// Request cancellation from any thread. Atomic flag submission + the
    /// documented thread-safe `JS_RequestInterruptCallback`; never touches a
    /// JSObject / GC pointer. The owner thread's interrupt callback observes
    /// the flag at the next engine interrupt check and terminates the script.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
        self.shared.requester.request();
    }

    /// Current terminal state (pollable from any thread).
    pub fn terminal_state(&self) -> TerminalState {
        TerminalState::from_u8(self.shared.terminal.load(Ordering::Acquire))
    }

    /// Clear stale control state (cancelled flag / terminal latch) so a
    /// previous timed-out or cancelled execution cannot terminate the next
    /// eval run with the same control. Owner-thread use, between evals.
    pub fn reset(&self) {
        self.shared.cancelled.store(false, Ordering::Release);
        self.shared
            .terminal
            .store(ST_RUNNING, Ordering::Release);
    }

    /// Stable error for a control-terminated execution (the engine clears the
    /// pending exception itself — `reportUncatchableException` — so the
    /// generic "Unknown JS error" fallback must be replaced with this).
    fn termination_error(&self, state: TerminalState) -> JsError {
        let message = match state {
            TerminalState::TimedOut => "Script terminated: deadline exceeded (timeout)".to_string(),
            TerminalState::Cancelled => "Script terminated: execution cancelled".to_string(),
            other => format!("Script terminated: {:?}", other),
        };
        JsError {
            message,
            filename: "<execution-control>".to_string(),
            line: 0,
            column: 0,
            stack: None,
        }
    }
}

// ── Controlled eval entry ───────────────────────────────────────────────────

impl JsContext {
    /// `eval` with engine-native timeout/cancellation (#24 minimal closed
    /// loop). Runs on the same persistent-realm path as [`JsContext::eval`];
    /// when the deadline passes or [`ExecutionControl::cancel`] is invoked,
    /// the owner-thread interrupt callback terminates the script (uncatchable
    /// by JS `try/catch`) and this returns a stable termination error.
    ///
    /// Internal experimental surface — NOT a stable API commitment
    /// (SM-EVOLUTION S1 unifies it with the runtime scheduler).
    #[doc(hidden)]
    pub fn eval_with_control(
        &mut self,
        control: &ExecutionControl,
        source: &str,
        filename: &str,
        timeout: Option<Duration>,
    ) -> Result<JsValue, JsError> {
        // Fail-closed misuse guard: the control's requester must point at
        // THIS context (created on this thread, same live runtime).
        assert_eq!(
            control.shared.requester.cx,
            self.raw_cx(),
            "ExecutionControl is bound to a different JSContext than the eval target"
        );

        // 1. Install the engine callback (once per JSContext).
        ensure_callback_installed(self.raw_cx());

        // 2. Pristine state for THIS execution — a stale Cancelled/TimedOut
        //    latch from a previous eval must not terminate this one (the
        //    pollution `reset` + this re-arm jointly lock out).
        control.reset();

        // 3. Arm (push onto the owner stack) + spawn the condvar-cancellable
        //    deadline watcher; Drop pops + joins on EVERY exit path.
        let _armed = ArmedExecutionGuard::new(
            Arc::clone(&control.shared),
            timeout.map(|t| Instant::now() + t),
        );

        // 4. Run the standard eval path (persistent realm, AutoRealm,
        //    evaluate_script, microtask drain) — the interrupt callback fires
        //    inside evaluate_script on loop back-edges.
        let result = self.eval(source, filename);

        // 5. Map to the terminal state: a control termination already latched
        //    by the callback wins over Completed/Errored; otherwise a JS
        //    error is Errored and success is Completed.
        match &result {
            Ok(_) => control.shared.latch(TerminalState::Completed),
            Err(_) => {
                control.shared.latch(TerminalState::Errored);
            }
        }

        // 6. A control-terminated eval reports the stable termination error
        //    (the engine cleared its pending exception; the generic fallback
        //    would surface as "Unknown JS error" — replaced here).
        match control.terminal_state() {
            TerminalState::TimedOut => Err(control.termination_error(TerminalState::TimedOut)),
            TerminalState::Cancelled => Err(control.termination_error(TerminalState::Cancelled)),
            _ => result,
        }
        // `_armed` drops here: watcher joined, armed entry popped.
    }
}
