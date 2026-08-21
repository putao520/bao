//! Process-global liveness registry for cross-thread wakeups of per-thread
//! `MiniEventLoop`s.
//!
//! ## Root cause this module eradicates (BCE-20260814-TLS-DRIVER-UAF)
//!
//! A `MiniEventLoop` is `Box::into_raw`-leaked per thread (timers.rs
//! `BAO_RUNTIME_LOOP`), so a captured `*mut MiniEventLoop` stays valid
//! memory forever. Its `loop_` field, however, points at the C-owned uws
//! loop materialized by the C/C++ library's own thread-local (BUG-353
//! C-exclusive loop: `UwsLoop::get` inside `MiniEventLoop::init`) — whose
//! thread-exit destructor **frees that loop when the owning thread exits**.
//! Cross-thread callers that held only the `MiniEventLoop` pointer (the
//! `bao-tls-driver` thread's `tls_schedule_tasklet`, HTTPThread-side
//! `resolve_tasklet` scheduling) woke the uws loop via
//! `enqueue_task_concurrent` under the assumption "valid for the thread's
//! lifetime" — false after the owning JS thread exits: `us_wakeup_loop`
//! on freed loop memory → SIGSEGV in `us_poll_fd` (tls_sni_server_tests
//! under load: ~50% crash rate). The existing `is_null()` guards only
//! covered "loop never captured", never "captured, then thread exited"
//! (TOCTOU).
//!
//! ## Fix: registry + lock handshake
//!
//! Every thread registers its `MiniEventLoop` → uws-loop pair here at
//! materialization; a thread-exit guard deregisters it. Cross-thread
//! enqueue checks membership **and performs the wakeup while holding the
//! registry lock**; deregistration takes the same lock **before** the
//! loop memory is freed. This makes the two races mutually exclusive:
//!
//! - An in-flight wakeup (lock held) completes before deregistration can
//!   run, hence before the free.
//! - After deregistration returns, every future enqueue sees "not
//!   registered" and never touches the loop.
//!
//! Teardown ordering (why the free really happens after deregister):
//! glibc runs a thread's TLS destructors (Rust `thread_local!` guards and
//! C++ `thread_local` objects alike) in reverse registration order.
//! `with_event_loop` materializes the C-owned uws loop first (inside
//! `MiniEventLoop::init` → `UwsLoop::get` — the C++ library's thread-local
//! registers its destructor then) and registers here second, so this
//! module's guard drops (deregisters) *before* the C/C++ library's
//! destructor frees the uws loop. Production code never frees a thread's
//! uws loop mid-thread (`us_loop_free` is test-only), so no stale
//! re-registration window exists. The leaked `MiniEventLoop` box's address
//! is never reused, so the addr key has no ABA.
//!
//! Keyed by address (not a generation counter) because the MiniEventLoop
//! allocation is never freed — a stale entry can only mean "thread still
//! live", and a missing entry can only mean "thread exited" (or "never
//! registered"). Both are decided atomically under the lock, which is the
//! property a bare generation counter cannot provide (check-then-wakeup
//! would still race the free).

use ::std::collections::HashMap;
use ::std::sync::Mutex;

use bun_uws::Loop as UwsLoop;

use crate::AnyTaskWithExtraContext::AnyTaskWithExtraContext;
use crate::MiniEventLoop::MiniEventLoop;

/// mini addr → its C-owned uws loop addr. Membership means "the owning
/// thread is still running and its uws loop is live". Both sides are
/// stored as `usize` (addresses as registry keys); raw pointers would
/// make the `Mutex` `!Sync`. `Option` because `HashMap::new` is not a
/// const fn — the map materializes on first registration.
static LIVE_MINI_LOOPS: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

/// Thread-exit deregistration guard. Materialized on the owning thread at
/// registration; `Drop` runs at thread exit, *before* the C/C++ library's
/// uws-loop thread-local destructor (TLS destructors run in reverse
/// registration order — see module docs).
struct ThreadExitDeregister(::std::cell::Cell<usize>);

impl Drop for ThreadExitDeregister {
    fn drop(&mut self) {
        let addr = self.0.get();
        if addr != 0 {
            if let Ok(mut guard) = LIVE_MINI_LOOPS.lock() {
                if let Some(map) = guard.as_mut() {
                    map.remove(&addr);
                }
            }
        }
    }
}

::std::thread_local! {
    static THREAD_EXIT_DEREGISTER: ThreadExitDeregister =
        ThreadExitDeregister(::std::cell::Cell::new(0));
}

/// Register the current thread's `MiniEventLoop` as a live cross-thread
/// wakeup target. Called once, right after materialization (timers.rs
/// `with_event_loop`), on the owning thread only.
pub fn register_thread_loop(mini: *mut MiniEventLoop<'static>) {
    let addr = mini as usize;
    // SAFETY: `mini` was just materialized on this thread (the Box is
    // leaked, so the memory is stable and never reallocated); `loop_ptr()`
    // is the C-owned loop set by `MiniEventLoop::init` and never mutated
    // afterwards. Any handoff of `mini` to another thread (Arc/Mutex,
    // driver command queue) publishes both writes before the reader runs.
    let uws: *mut UwsLoop = unsafe { (*mini).loop_ptr() };
    LIVE_MINI_LOOPS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(addr, uws as usize);
    THREAD_EXIT_DEREGISTER.with(|g| g.0.set(addr));
}

/// Cross-thread enqueue of a concurrent task onto a (possibly dead)
/// thread's `MiniEventLoop`, waking its uws loop.
///
/// Returns `false` when the owning thread has exited (or the loop was
/// never registered): nothing was pushed and no wakeup was attempted —
/// the caller must handle it exactly like the "loop not captured" case.
///
/// `mini` must be a pointer previously handed out by the owning thread's
/// `with_event_loop` (leaked allocation; never freed, so passing a stale
/// pointer after thread exit is defined behavior — it only reads fields
/// frozen at init and pushes into a queue nobody drains).
pub fn enqueue_task_concurrent_cross_thread(
    mini: *mut MiniEventLoop<'static>,
    task: core::ptr::NonNull<AnyTaskWithExtraContext>,
) -> bool {
    let addr = mini as usize;
    let guard = LIVE_MINI_LOOPS.lock().unwrap();
    let Some(map) = guard.as_ref() else {
        return false;
    };
    let Some(&uws_addr) = map.get(&addr) else {
        return false;
    };
    // Keep the lock across push + wakeup: this is the handshake that makes
    // the call race-free against `ThreadExitDeregister::drop` (which takes
    // the same lock before the uws loop's memory is freed).
    //
    // SAFETY (push): the MiniEventLoop allocation is leaked (stable
    // memory); `concurrent_tasks` is a lock-free MPSC queue whose producer
    // side is designed for cross-thread callers (HTTPThread's
    // `resolve_tasklet` already pushes cross-thread). The single consumer
    // is the owning thread's tick.
    unsafe { (*mini).concurrent_tasks.push(task) };
    // SAFETY (wakeup): `uws_addr` is registered ⇒ its owning thread has
    // not run the deregister guard ⇒ the loop memory cannot have been
    // freed (the free happens strictly after deregistration, on the owning
    // thread, LIFO TLS order). Call the raw extern — not
    // `Loop::wakeup(&mut self)` — so no `&mut UwsLoop` is formed across
    // threads (noalias hazard, see the uws_sys Loop.rs re-export note).
    unsafe { bun_uws::us_wakeup_loop(uws_addr as *mut UwsLoop) };
    true
}
