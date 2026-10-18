//! C15 coverage wave — `bun_runtime::dispatch` link-time faces + the
//! `interrupt_bridge` SIGINT interception contract (REQ-ENG-010 邻接面,
//! REQ-CLI-001 bridge 载体).
//!
//! `dispatch.rs` owns the `#[unsafe(no_mangle)] extern "Rust"` symbols
//! `bun_io`/`bun_event_loop` resolve at link time; these tests drive them
//! through the crate paths directly (`pub mod dispatch`). Covered: the
//! timer-dispatch null/tag/FIRED-guard arms, the epoch accessors, the
//! FilePoll null arm, both `__bun_get_vm_ctx` allocator arms, and every
//! `__bun_dns_prefetch` guard. `interrupt_bridge.rs` is exercised end to end
//! over real signals: armed-cancel consumption, disarmed swallow-then-reraise
//! into the restored handler, and the singleton double-install panic.
//!
//! Engine discipline: one NodeRuntime per #[test] (nextest runs each test in
//! its own process; JSEngine/JSContext are thread/process singletons).

use std::sync::atomic::{AtomicU32, Ordering};

use bun_event_loop::EventLoopTimer::{EventLoopTimer, State as TimerState, Tag};
use bun_runtime::dispatch;
use bun_runtime::timers::BaoTimeoutObject;

// ─── dispatch: timer faces ─────────────────────────────────────────────────

/// `__bun_fire_timer(null, …)` is an explicit no-op (the null guard).
#[test]
fn fire_timer_null_is_noop_c15() {
    // SAFETY: null pointer is the guarded arm; no dereference happens.
    unsafe {
        dispatch::__bun_fire_timer(
            std::ptr::null_mut(),
            &bun_core::Timespec::EPOCH,
            std::ptr::null_mut(),
        );
    }
}

/// A PENDING TimeoutObject timer fires exactly once through the dispatch:
/// state → FIRED and the epoch bumps for heap re-queue ordering; a second
/// dispatch of the same timer must hit the FIRED guard (no re-fire, no
/// double epoch bump). The test process registers no JSContext, so the
/// dispatch takes the state-only `fire` arm (no cx → no `fire_js`).
#[test]
fn fire_timer_fires_pending_once_and_guards_already_fired_c15() {
    let mut obj = BaoTimeoutObject::new_paused();
    assert!(matches!(obj.event_loop_timer.state, TimerState::PENDING));
    let t_ptr = &mut obj.event_loop_timer as *mut EventLoopTimer;

    // SAFETY: t_ptr points at a live BaoTimeoutObject's first field
    // (container_of contract); now is a valid Timespec.
    unsafe {
        dispatch::__bun_fire_timer(t_ptr, &bun_core::Timespec::EPOCH, std::ptr::null_mut());
    }
    assert!(matches!(obj.event_loop_timer.state, TimerState::FIRED));
    assert_eq!(obj.epoch, 1, "epoch bumps once on fire");

    // FIRED guard: the second dispatch must not re-fire.
    unsafe {
        dispatch::__bun_fire_timer(t_ptr, &bun_core::Timespec::EPOCH, std::ptr::null_mut());
    }
    assert_eq!(obj.epoch, 1, "already-fired timer must not re-fire");
    assert!(matches!(obj.event_loop_timer.state, TimerState::FIRED));
}

/// Non-timeout tags fall through the `_ => {}` arm untouched: a
/// `TimerCallback`-tagged timer dispatched through `__bun_fire_timer` keeps
/// its PENDING state (Bao's dispatch table only owns TimeoutObject and
/// ImmediateObject).
#[test]
fn fire_timer_non_timeout_tag_is_ignored_c15() {
    let mut t = EventLoopTimer::init_paused(Tag::TimerCallback);
    // SAFETY: live timer, ignored tag arm.
    unsafe {
        dispatch::__bun_fire_timer(&mut t, &bun_core::Timespec::EPOCH, std::ptr::null_mut());
    }
    assert!(
        matches!(t.state, TimerState::PENDING),
        "unowned tag must not be fired"
    );
}

/// `__bun_js_timer_epoch`: TimeoutObject/ImmediateObject answer with the
/// object's epoch, every other tag answers None (heap-ordering contract).
#[test]
fn js_timer_epoch_by_tag_c15() {
    let mut obj = BaoTimeoutObject::new_paused();
    obj.epoch = 77;
    let t_ptr = &mut obj.event_loop_timer as *mut EventLoopTimer;

    // SAFETY: t_ptr is the event_loop_timer field of a live BaoTimeoutObject.
    unsafe {
        assert_eq!(
            dispatch::__bun_js_timer_epoch(Tag::TimeoutObject, t_ptr),
            Some(77)
        );
        assert_eq!(
            dispatch::__bun_js_timer_epoch(Tag::ImmediateObject, t_ptr),
            Some(77)
        );
        assert_eq!(dispatch::__bun_js_timer_epoch(Tag::TimerCallback, t_ptr), None);
    }
}

// ─── dispatch: file-poll / vm-ctx / dns faces ──────────────────────────────

/// `__bun_run_file_poll(null, …)` is an explicit no-op (the null guard).
#[cfg(not(windows))]
#[test]
fn run_file_poll_null_is_noop_c15() {
    // SAFETY: null pointer is the guarded arm.
    unsafe {
        dispatch::__bun_run_file_poll(std::ptr::null_mut(), 0);
    }
}

/// Both allocator arms of the process/thread event-loop selector: Mini
/// lazily materializes this thread's MiniEventLoop (loop pointer becomes
/// non-null), Js resolves through the SpiderMonkey BaoEventLoop thread-local.
#[test]
fn get_vm_ctx_mini_and_js_arms_initialize_c15() {
    // Before the Mini call the thread's loop slot may be null; after it, the
    // thread-local singleton must be live.
    let _mini = dispatch::__bun_get_vm_ctx(bun_io::AllocatorType::Mini);
    let loop_ptr = bun_event_loop::MiniEventLoop::GLOBAL.with(|g| g.get());
    assert!(!loop_ptr.is_null(), "Mini arm must materialize the thread loop");

    // SAFETY: the Js arm only reads the thread-local cell; the returned ctx
    // is dropped here without dereferencing its raw owner.
    let _js = dispatch::__bun_get_vm_ctx(bun_io::AllocatorType::Js);
}

/// Every `__bun_dns_prefetch` guard arm: null hostname, zero length, invalid
/// UTF-8, embedded NUL — all early-returns; the valid arm spawns the
/// fire-and-forget resolver thread (localhost resolves offline).
#[test]
fn dns_prefetch_guard_arms_and_valid_path_c15() {
    // SAFETY: each call is the documented guard contract; hostname slices
    // outlive the call (the valid arm clones before spawning).
    unsafe {
        dispatch::__bun_dns_prefetch(std::ptr::null_mut(), b"localhost".as_ptr(), 9, 9);
        dispatch::__bun_dns_prefetch(std::ptr::null_mut(), std::ptr::null(), 9, 9);
        dispatch::__bun_dns_prefetch(std::ptr::null_mut(), b"localhost".as_ptr(), 0, 9);
        dispatch::__bun_dns_prefetch(std::ptr::null_mut(), b"\xff\xfe\xfd".as_ptr(), 3, 9);
        dispatch::__bun_dns_prefetch(std::ptr::null_mut(), b"lo\0cal".as_ptr(), 6, 9);
        dispatch::__bun_dns_prefetch(std::ptr::null_mut(), b"localhost".as_ptr(), 9, 9);
    }
    // The valid arm's resolver thread is fire-and-forget; give it a beat and
    // require a live process (no panic from the spawned thread's resolve).
    std::thread::sleep(std::time::Duration::from_millis(100));
}

// ─── interrupt_bridge ──────────────────────────────────────────────────────

/// Counting SIGINT handler — async-signal-safe (lock-free fetch_add). Used as
/// the PRE-bridge disposition so the swallow-then-reraise contract is
/// observable in-process: the re-raise lands HERE, not at the default kill.
extern "C" fn c15_sigint_handler(_sig: libc::c_int) {
    C15_SIGINT_HITS.fetch_add(1, Ordering::Relaxed);
}

static C15_SIGINT_HITS: AtomicU32 = AtomicU32::new(0);

/// Install the counting handler as the disposition the bridge will capture
/// and later restore. SAFETY: valid sigaction wiring; SA_RESTART mirrors the
/// bridge's own flag choice.
#[cfg(not(windows))]
unsafe fn install_counting_sigint_handler() {
    // SAFETY: valid sigaction wiring; SA_RESTART mirrors the bridge's own
    // flag choice. Edition-2024: unsafe ops inside the unsafe fn are still
    // block-scoped.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = c15_sigint_handler as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        let mut prev: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGINT, &sa, &mut prev),
            0,
            "sigaction(counting) failed"
        );
    }
}

#[cfg(not(windows))]
fn hits() -> u32 {
    C15_SIGINT_HITS.load(Ordering::Relaxed)
}

#[cfg(not(windows))]
fn raise_sigint() {
    // SAFETY: raise(2) targets the calling process.
    unsafe {
        assert_eq!(libc::raise(libc::SIGINT), 0, "raise(SIGINT) failed");
    }
}

/// Armed bridge: SIGINT while a control is armed must be CONSUMED by
/// `control.cancel()` — never marked unhandled, so Drop performs no re-raise
/// (the counting handler's delta must stay 0; a reraise would increment it).
/// The drop's watcher-join is the synchronization point: by the time it
/// returns, the signal was either cancelled (delta 0) or re-raised (delta 1).
#[test]
#[cfg(not(windows))]
fn armed_sigint_cancels_control_without_unhandled_reraise() {
    use bao_engine::execution_control::TerminalState;

    // Real engine: ExecutionControl binds to this thread's live JSContext.
    let rt = bun_runtime::NodeRuntime::new().expect("NodeRuntime");
    let ctrl = rt.execution_control();
    assert_eq!(ctrl.terminal_state(), TerminalState::Running);

    // SAFETY: test-process signal wiring.
    unsafe { install_counting_sigint_handler() };
    let start_hits = hits();

    let bridge = bun_runtime::interrupt_bridge::InterruptBridge::install();
    bridge.arm(&ctrl);
    raise_sigint();
    drop(bridge); // restores the counting handler, joins the watcher

    assert_eq!(
        hits(),
        start_hits,
        "armed SIGINT must be consumed by cancel(), not re-raised"
    );
    // cancel() leaves terminal RUNNING (the terminal latch belongs to the
    // eval machinery) — but the bridge must not have eaten it either.
    assert_eq!(ctrl.terminal_state(), TerminalState::Running);
}

/// Disarmed bridge: SIGINT with no armed control must be swallowed, recorded,
/// and RE-RAISED on Drop into the restored disposition — the faithful
/// default (Ctrl-C is never silently eaten). The re-raise lands in the
/// counting handler installed before the bridge, so the process survives and
/// the delta proves the re-raise happened.
#[test]
#[cfg(not(windows))]
fn disarmed_sigint_is_reraised_into_restored_handler() {
    // SAFETY: test-process signal wiring.
    unsafe { install_counting_sigint_handler() };
    let start_hits = hits();

    let bridge = bun_runtime::interrupt_bridge::InterruptBridge::install();
    // Deliberately NOT armed.
    raise_sigint();
    drop(bridge); // Drop observes eaten_unhandled and re-raises into ours

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while hits() == start_hits && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        hits(),
        start_hits + 1,
        "disarmed SIGINT must be re-raised exactly once on Drop"
    );
}

/// The bridge is a process singleton: a second install while one is live
/// must panic (fail-closed), and after the live bridge drops, install must
/// succeed again (the handshake resets on Drop).
#[test]
#[cfg(not(windows))]
fn double_install_panics_and_singleton_recovers_after_drop() {
    let bridge = bun_runtime::interrupt_bridge::InterruptBridge::install();

    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = bun_runtime::interrupt_bridge::InterruptBridge::install();
    }));
    assert!(second.is_err(), "second install must panic");

    drop(bridge);
    let third = bun_runtime::interrupt_bridge::InterruptBridge::install();
    drop(third);
}
