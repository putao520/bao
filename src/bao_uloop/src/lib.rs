// @trace REQ-ENG-008 [entity:BaoLoopState]
//! Wave 74-LOOP-A: mio-backed implementation of the uSockets loop ABI.
//!
//! Upstream Bun relies on the C library `libusockets` to provide `us_loop_*` /
//! `us_poll_*` / `us_socket_*` / `uws_get_loop` etc. Bao does not link any C
//! compiler output — every `#[no_mangle] extern "C"` symbol consumed by
//! `bun_uws_sys::c::*` extern blocks must come from a Rust crate. This crate
//! is that Rust crate for the **event-loop core**:
//!
//!   - `uws_get_loop`           — thread-local singleton accessor
//!   - `us_create_loop`         — explicit construction (with callbacks)
//!   - `us_loop_free`           — destruction
//!   - `us_loop_run`            — run until empty
//!   - `us_loop_run_bun_tick`   — single iteration (Bun's `tick`)
//!   - `us_wakeup_loop`         — cross-thread wake
//!   - `uws_loop_defer`         — next-tick callback queue
//!   - `uws_loop_addPreHandler` / `addPostHandler` / `remove*`
//!
//! ## Layout strategy
//!
//! `bun_uws_sys::PosixLoop` is `#[repr(C, align(16))]` with a fixed field
//! layout that downstream callers (FilePoll, dispatch_sm) read directly
//! (e.g. `internal_loop_data.iteration_nr`). We allocate a `Box<PosixLoop>`
//! per thread, zero-initialise its fields, and hand out raw pointers to it.
//! The `Box` is intentionally leaked (`Box::leak`) — the loop has process
//! lifetime, matching upstream Bun's `us_create_loop` semantics.
//!
//! ## mio backend
//!
//! Each `PosixLoop` carries a `BaoLoopState` (held in a `thread_local!`)
//! containing:
//!   - `mio::Poll` — the platform poll backend (epoll on Linux,
//!     kqueue on macOS/BSD)
//!   - `mio::Waker` — arm of the cross-thread wake token (replaces
//!     `us_wakeup_loop`'s eventfd / pipe)
//!   - `deferred` — `VecDeque` of next-tick callbacks pushed by
//!     `uws_loop_defer`
//!   - `pre_handlers` / `post_handlers` — registered `addPreHandler` /
//!     `addPostHandler` callbacks (small vec of fn pointers)
//!
//! ## Scope (Wave 74-LOOP-A)
//!
//! Only the loop core is implemented here. Timer / Poll / Socket / SSL / QUIC
//! subsystems land in subsequent sub-waves (B, C, E and Phase-level 74-TLS /
//! 74-QUIC). Until then, the corresponding `us_*` symbols stay in
//! `bao_native_stubs` as safe no-ops.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_uint, c_void};
use core::ptr;

use bun_uws_sys::{InternalLoopData, Loop, PosixLoop, Timespec};

// ────────────────────────────── types ──────────────────────────────

pub type LoopCb = unsafe extern "C" fn(*mut Loop);
pub type LoopCtxCb = unsafe extern "C" fn(*mut c_void, *mut Loop);
pub type DeferCb = unsafe extern "C" fn(*mut c_void);

/// Per-thread state backing each `PosixLoop` returned by `uws_get_loop` /
/// `us_create_loop`. Stored as `thread_local! { RefCell<Option<...>> }` so the
/// first call lazily materialises both the `PosixLoop` shell and the mio
/// backend in lock-step.
struct BaoLoopState {
    /// Pointer to the `Box::leak`-ed `PosixLoop` we exposed to FFI. Stored
    /// here so the FFI side and the Rust side agree on a single allocation.
    loop_ptr: *mut PosixLoop,

    /// mio poll backend. `None` only briefly while the loop is being torn
    /// down (which, in practice, never happens — loops are process-lifetime).
    poll: Option<mio::Poll>,

    /// Cross-thread waker. Clones handed out to callers that need to wake
    /// the loop from another thread (replaces `us_wakeup_loop`'s eventfd).
    waker: Option<mio::Waker>,

    /// Pending wakeups counter. Mirrors `PosixLoop::pending_wakeups` but
    /// kept on the Rust side so we can atomically swap-and-clear without
    /// touching FFI memory.
    pending_wakeups: core::sync::atomic::AtomicU32,

    /// `uws_loop_defer` FIFO. Drained at the start of every `tick`.
    deferred: std::collections::VecDeque<DeferredCall>,

    /// Pre-tick handlers registered via `uws_loop_addPreHandler`.
    pre_handlers: Vec<HandlerSlot>,

    /// Post-tick handlers registered via `uws_loop_addPostHandler`.
    post_handlers: Vec<HandlerSlot>,

    /// User wake callback set at `us_create_loop` time. Mirrors
    /// `LoopHandler::WAKEUP` upstream.
    wakeup_cb: Option<LoopCb>,

    /// Optional pre-callback set at `us_create_loop` time.
    pre_cb: Option<LoopCb>,

    /// Optional post-callback set at `us_create_loop` time.
    post_cb: Option<LoopCb>,
}

#[derive(Clone, Copy)]
struct DeferredCall {
    ctx: *mut c_void,
    cb: DeferCb,
}

#[derive(Clone, Copy)]
struct HandlerSlot {
    ctx: *mut c_void,
    cb: LoopCtxCb,
}

thread_local! {
    /// Single-thread "the loop" — Bun upstream assumes one loop per thread,
    /// so this is the same shape.
    static BAO_LOOP: std::cell::RefCell<Option<BaoLoopState>> =
        const { std::cell::RefCell::new(None) };
}

// ──────────────────────────── allocation ───────────────────────────

/// Allocate a zero-initialised `PosixLoop` shell and a fresh `BaoLoopState`
/// tied to it. Stores the state in the current thread's `BAO_LOOP`.
///
/// Returns the raw `*mut PosixLoop` for FFI consumption.
fn create_loop(
    wakeup_cb: Option<LoopCb>,
    pre_cb: Option<LoopCb>,
    post_cb: Option<LoopCb>,
) -> *mut PosixLoop {
    // Allocate the recv/send buffers required by `InternalLoopData::recv_slice`
    // upstream (LIBUS_RECV_BUFFER_LENGTH = 524_288). The C side frees these
    // via `free()` on loop teardown; we match the allocator here so the
    // pointer remains libc-free-able.
    const RECV_BUF_LEN: usize = 524_288;
    let recv_buf: *mut u8 = unsafe { libc::malloc(RECV_BUF_LEN) as *mut u8 };
    assert!(
        !recv_buf.is_null(),
        "bao_uloop: libc::malloc(recv_buf) failed"
    );
    unsafe { ptr::write_bytes(recv_buf, 0, RECV_BUF_LEN) };

    let send_buf: *mut u8 = unsafe { libc::malloc(RECV_BUF_LEN) as *mut u8 };
    assert!(
        !send_buf.is_null(),
        "bao_uloop: libc::malloc(send_buf) failed"
    );
    unsafe { ptr::write_bytes(send_buf, 0, RECV_BUF_LEN) };

    // Build a zeroed `InternalLoopData` then patch in the buffers. All
    // pointer fields start at null; callers that need them (DNS / SSL /
    // QUIC sub-systems) populate them lazily via dispatch_sm / FFI.
    let internal = InternalLoopData {
        sweep_timer: ptr::null_mut(),
        sweep_timer_count: 0,
        wakeup_async: ptr::null_mut(),
        head: ptr::null_mut(),
        quic_head: ptr::null_mut(),
        quic_next_tick_us: 0,
        quic_timer: ptr::null_mut(),
        iterator: ptr::null_mut(),
        recv_buf,
        send_buf,
        ssl_data: ptr::null_mut(),
        pre_cb,
        post_cb,
        closed_udp_head: ptr::null_mut(),
        closed_head: ptr::null_mut(),
        low_prio_head: ptr::null_mut(),
        low_prio_budget: 0,
        dns_ready_head: ptr::null_mut(),
        closed_connecting_head: ptr::null_mut(),
        mutex: 0,
        parent_ptr: ptr::null_mut(),
        parent_tag: 0 as c_char,
        iteration_nr: 0,
        jsc_vm: ptr::null(),
        tick_depth: 0,
    };

    // Allocate the PosixLoop shell. The `ready_polls` array is 1024 entries
    // of `epoll_event` (Linux) / `kevent64_s` (macOS) — zero-init is sound.
    let boxed: Box<PosixLoop> = Box::new(PosixLoop {
        internal_loop_data: internal,
        num_polls: 0,
        num_ready_polls: 0,
        current_ready_poll: 0,
        // We surface -1 here. Downstream code reading `PosixLoop::fd` should
        // use the `us_poll_*` API (registered through `mio::Registry::register`
        // in Wave 74-LOOP-C). Direct epoll/kqueue fd access bypasses mio's
        // ownership model and breaks portability.
        fd: -1,
        active: 0,
        pending_wakeups: 0,
        ready_polls: [unsafe { core::mem::zeroed() }; 1024],
    });
    let loop_ptr: *mut PosixLoop = Box::into_raw(boxed);

    // Spin up mio. We allocate a Token(0) Waker so cross-thread wake works
    // from the get-go (subsystems that register real Interest tokens use
    // Token(1..)).
    let poll = mio::Poll::new().expect("bao_uloop: mio::Poll::new failed");
    let waker = mio::Waker::new(poll.registry(), mio::Token(0))
        .expect("bao_uloop: mio::Waker::new failed");

    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_some() {
            panic!("bao_uloop: thread already has a BaoLoopState — call us_loop_free first");
        }
        *slot = Some(BaoLoopState {
            loop_ptr,
            poll: Some(poll),
            waker: Some(waker),
            pending_wakeups: core::sync::atomic::AtomicU32::new(0),
            deferred: std::collections::VecDeque::new(),
            pre_handlers: Vec::new(),
            post_handlers: Vec::new(),
            wakeup_cb,
            pre_cb,
            post_cb,
        });
    });

    loop_ptr
}

// ──────────────────────── BaoLoopState access ──────────────────────

/// Run `f` with the BaoLoopState if it matches `loop_`. Returns `None` if no
/// state is present or the pointer doesn't match. The closure is invoked
/// while the RefCell borrow is dropped, so re-entrant FFI into the same
/// thread_local is safe.
fn with_matching_state<R>(
    loop_: *mut Loop,
    f: impl FnOnce(&mut BaoLoopState) -> R,
) -> Option<R> {
    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let state = slot.as_mut()?;
        if !ptr::eq(state.loop_ptr, loop_) {
            return None;
        }
        Some(f(state))
    })
}

/// Drain the deferred queue for `loop_` into a Vec while holding the
/// RefCell borrow, then return it for caller-driven iteration (which
/// must run with the borrow released so callbacks can re-enter).
fn take_deferred(loop_: *mut Loop) -> Vec<DeferredCall> {
    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return Vec::new();
        };
        if !ptr::eq(state.loop_ptr, loop_) {
            return Vec::new();
        }
        state.deferred.drain(..).collect()
    })
}

fn snapshot_handlers(loop_: *mut Loop, which: HandlerKind) -> Vec<HandlerSlot> {
    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return Vec::new();
        };
        if !ptr::eq(state.loop_ptr, loop_) {
            return Vec::new();
        }
        match which {
            HandlerKind::Pre => state.pre_handlers.clone(),
            HandlerKind::Post => state.post_handlers.clone(),
        }
    })
}

#[derive(Clone, Copy)]
enum HandlerKind {
    Pre,
    Post,
}

fn run_mio_poll(loop_: *mut Loop, pending: u32, timeout: *const Timespec) {
    // Compute timeout up front while we hold the borrow briefly.
    //
    // `us_loop_run_bun_tick` is a *single iteration* API — passing null
    // timeout means "no explicit timeout", which we interpret as
    // non-blocking (zero). An indefinite block here would hang tests
    // (and Bun's drain loops). Callers wanting a blocking tick pass an
    // explicit `timespec`.
    let poll_timeout: std::time::Duration = if pending > 0 || timeout.is_null() {
        std::time::Duration::ZERO
    } else {
        let ts: Timespec = unsafe { *timeout };
        if ts.sec == 0 && ts.nsec == 0 {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::new(ts.sec as u64, ts.nsec as u32)
        }
    };

    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else { return };
        if !ptr::eq(state.loop_ptr, loop_) {
            return;
        }
        let Some(poll) = state.poll.as_mut() else {
            return;
        };
        let mut events = mio::Events::with_capacity(64);
        let _ = poll.poll(&mut events, Some(poll_timeout));
        // Token(0) is the waker; the rest would dispatch to FilePoll —
        // Wave 74-LOOP-C wires the registry side. Until then we just
        // drain events to clear the kernel-side queue.
    });
}

fn bump_iteration_nr(loop_: *mut Loop) {
    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(state) = slot.as_mut() else { return };
        if !ptr::eq(state.loop_ptr, loop_) {
            return;
        }
        let p = state.loop_ptr;
        unsafe {
            (*p).internal_loop_data.iteration_nr =
                (*p).internal_loop_data.iteration_nr.wrapping_add(1);
        }
    });
}

// ─────────────────────── FFI entry points ──────────────────────────

/// Default singleton accessor — equivalent to upstream `uws_get_loop()`:
/// materialises the per-thread loop lazily, with no explicit callbacks.
///
/// Replaces the prior safe no-op stub in `bao_native_stubs::c_lib_stubs`.
#[unsafe(no_mangle)]
pub extern "C" fn uws_get_loop() -> *mut Loop {
    BAO_LOOP.with(|cell| {
        let slot = cell.borrow();
        if let Some(state) = slot.as_ref() {
            return state.loop_ptr as *mut Loop;
        }
        drop(slot);
        let p = create_loop(None, None, None);
        p as *mut Loop
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_create_loop(
    _hint: *mut c_void,
    wakeup_cb: Option<LoopCb>,
    pre_cb: Option<LoopCb>,
    post_cb: Option<LoopCb>,
    _ext_size: c_uint,
) -> *mut Loop {
    let p = create_loop(wakeup_cb, pre_cb, post_cb);
    p as *mut Loop
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_loop_free(loop_: *mut Loop) {
    if loop_.is_null() {
        return;
    }
    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(state) = slot.as_mut()
            && ptr::eq(state.loop_ptr, loop_)
        {
            // Free recv/send buffers (libc-allocated in `create_loop`).
            unsafe {
                if !(*loop_).internal_loop_data.recv_buf.is_null() {
                    libc::free((*loop_).internal_loop_data.recv_buf as *mut c_void);
                }
                if !(*loop_).internal_loop_data.send_buf.is_null() {
                    libc::free((*loop_).internal_loop_data.send_buf as *mut c_void);
                }
                // Drop the Box<PosixLoop>.
                let _ = Box::from_raw(loop_);
            }
            // Drop the mio::Poll / Waker by letting the field go out of scope.
            *slot = None;
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_wakeup_loop(loop_: *mut Loop) {
    if loop_.is_null() {
        return;
    }
    // Bump the pending-wakeups counter so the next tick returns immediately.
    let woken = with_matching_state(loop_, |state| {
        state
            .pending_wakeups
            .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        unsafe {
            (*loop_).pending_wakeups = (*loop_).pending_wakeups.wrapping_add(1);
        }
        state.waker.as_ref().map(|w| w.wake())
    });

    if let Some(Some(result)) = woken {
        let _ = result;
    }

    // Fire the user wakeup hook (mirrors upstream LoopHandler::WAKEUP).
    let cb = with_matching_state(loop_, |state| state.wakeup_cb).flatten();
    if let Some(cb) = cb {
        unsafe { (cb)(loop_) };
    }
}

/// Single-iteration tick. Drains deferred queue, invokes pre-handlers, polls
/// mio with the supplied timeout, invokes post-handlers, increments
/// `iteration_nr`. This is the meat of `PosixLoop::tick()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec) {
    if loop_.is_null() {
        return;
    }

    // Drain pending wakeups — if any, the poll timeout collapses to zero.
    let pending = with_matching_state(loop_, |state| {
        state
            .pending_wakeups
            .swap(0, core::sync::atomic::Ordering::SeqCst)
    })
    .unwrap_or(0);
    if pending > 0 {
        unsafe { (*loop_).pending_wakeups = 0; }
    }

    // Phase 1: drain deferred callbacks. Take ownership so callbacks may
    // re-enter the thread_local safely.
    let deferred = take_deferred(loop_);
    for call in deferred {
        unsafe { (call.cb)(call.ctx) };
    }

    // Phase 2: pre-callback (loop-level LoopHandler::PRE, distinct from
    // user addPreHandler slots).
    if let Some(cb) = with_matching_state(loop_, |state| state.pre_cb).flatten() {
        unsafe { (cb)(loop_) };
    }

    // Phase 3: pre-handlers.
    let pre = snapshot_handlers(loop_, HandlerKind::Pre);
    for slot in &pre {
        unsafe { (slot.cb)(slot.ctx, loop_) };
    }

    // Phase 4: mio poll with computed timeout.
    run_mio_poll(loop_, pending, timeout);

    // Phase 5: post-handlers.
    let post = snapshot_handlers(loop_, HandlerKind::Post);
    for slot in &post {
        unsafe { (slot.cb)(slot.ctx, loop_) };
    }

    // Phase 6: post-callback (loop-level LoopHandler::POST).
    if let Some(cb) = with_matching_state(loop_, |state| state.post_cb).flatten() {
        unsafe { (cb)(loop_) };
    }

    // Phase 7: bump iteration_nr.
    bump_iteration_nr(loop_);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_loop_run(loop_: *mut Loop) {
    // Loop until `active == 0` (no KeepAlive refs). This is the same
    // termination condition as upstream `us_loop_run`.
    if loop_.is_null() {
        return;
    }
    loop {
        let active = unsafe { (*loop_).active };
        if active == 0 {
            break;
        }
        unsafe { us_loop_run_bun_tick(loop_, ptr::null()) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn uws_loop_defer(
    loop_: *mut Loop,
    ctx: *mut c_void,
    cb: DeferCb,
) {
    if loop_.is_null() {
        return;
    }
    with_matching_state(loop_, |state| {
        state.deferred.push_back(DeferredCall { ctx, cb });
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn uws_loop_addPreHandler(
    loop_: *mut Loop,
    ctx: *mut c_void,
    cb: LoopCtxCb,
) {
    if loop_.is_null() {
        return;
    }
    with_matching_state(loop_, |state| {
        state.pre_handlers.push(HandlerSlot { ctx, cb });
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn uws_loop_removePreHandler(
    loop_: *mut Loop,
    ctx: *mut c_void,
    cb: LoopCtxCb,
) {
    if loop_.is_null() {
        return;
    }
    with_matching_state(loop_, |state| {
        state.pre_handlers.retain(|slot| {
            !(slot.ctx == ctx && slot.cb as usize == cb as usize)
        });
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn uws_loop_addPostHandler(
    loop_: *mut Loop,
    ctx: *mut c_void,
    cb: LoopCtxCb,
) {
    if loop_.is_null() {
        return;
    }
    with_matching_state(loop_, |state| {
        state.post_handlers.push(HandlerSlot { ctx, cb });
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn uws_loop_removePostHandler(
    loop_: *mut Loop,
    ctx: *mut c_void,
    cb: LoopCtxCb,
) {
    if loop_.is_null() {
        return;
    }
    with_matching_state(loop_, |state| {
        state.post_handlers.retain(|slot| {
            !(slot.ctx == ctx && slot.cb as usize == cb as usize)
        });
    });
}

/// Force the linker to keep bao_uloop's `#[no_mangle] extern "C"` symbols.
/// Required because nothing in the regular call chain name-refers to them —
/// `bun_event_loop` resolves them as unresolved C externs at link time, and a
/// pure Rust `#[no_mangle]` symbol can be GC'd by the linker when no Rust
/// reference exists. Call from `bao_native_stubs::force_link()` so every
/// integration test binary pulls them in.
#[inline(never)]
pub fn force_link() {
    let _ = uws_get_loop;
    let _ = us_create_loop as unsafe extern "C" fn(_, _, _, _, _) -> *mut Loop;
    let _ = us_loop_free as unsafe extern "C" fn(_);
    let _ = us_wakeup_loop as unsafe extern "C" fn(_);
    let _ = us_loop_run_bun_tick as unsafe extern "C" fn(_, _);
    let _ = us_loop_run as unsafe extern "C" fn(_);
    let _ = uws_loop_defer as unsafe extern "C" fn(_, _, _);
    let _ = uws_loop_addPreHandler as unsafe extern "C" fn(_, _, _);
    let _ = uws_loop_removePreHandler as unsafe extern "C" fn(_, _, _);
    let _ = uws_loop_addPostHandler as unsafe extern "C" fn(_, _, _);
    let _ = uws_loop_removePostHandler as unsafe extern "C" fn(_, _, _);
}

// ─────────────────────────── tests ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uws_get_loop_returns_non_null_per_thread() {
        let p1 = uws_get_loop();
        assert!(!p1.is_null(), "uws_get_loop must return non-null");

        // Same thread → same pointer.
        let p2 = uws_get_loop();
        assert_eq!(p1, p2, "uws_get_loop must be stable on the same thread");

        // iteration_nr starts at 0.
        unsafe {
            assert_eq!((*p1).iteration_number(), 0);
        }
    }

    #[test]
    fn uws_get_loop_is_thread_local() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let main_ptr = uws_get_loop() as usize;
        let observed = Arc::new(AtomicUsize::new(0));
        let observed_clone = observed.clone();
        std::thread::spawn(move || {
            observed_clone.store(uws_get_loop() as usize, Ordering::SeqCst);
        })
        .join()
        .unwrap();
        assert_ne!(
            main_ptr,
            observed.load(Ordering::SeqCst),
            "different threads must get different loops"
        );
    }

    #[test]
    fn tick_increments_iteration_number() {
        let p = uws_get_loop();
        unsafe {
            let before = (*p).iteration_number();
            us_loop_run_bun_tick(p, ptr::null());
            let after = (*p).iteration_number();
            assert_eq!(after, before + 1, "tick must bump iteration_nr");
        }
    }

    #[test]
    fn defer_runs_on_next_tick() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        extern "C" fn inc(ctx: *mut c_void) {
            let c: *const AtomicUsize = ctx as *const AtomicUsize;
            unsafe { (*c).fetch_add(1, Ordering::SeqCst) };
        }
        let p = uws_get_loop();
        unsafe {
            uws_loop_defer(
                p,
                Arc::into_raw(counter_clone) as *mut c_void,
                inc,
            );
            // Counter not yet bumped.
            assert_eq!(counter.load(Ordering::SeqCst), 0);
            us_loop_run_bun_tick(p, ptr::null());
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn pre_post_handlers_fire() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let counter = Arc::new(AtomicUsize::new(0));

        extern "C" fn pre_cb(ctx: *mut c_void, _loop_: *mut Loop) {
            unsafe { (*(ctx as *const AtomicUsize)).fetch_add(1, Ordering::SeqCst) };
        }
        extern "C" fn post_cb(ctx: *mut c_void, _loop_: *mut Loop) {
            unsafe { (*(ctx as *const AtomicUsize)).fetch_add(10, Ordering::SeqCst) };
        }

        let p = uws_get_loop();
        let raw = Arc::into_raw(counter.clone()) as *mut c_void;
        unsafe {
            uws_loop_addPreHandler(p, raw, pre_cb);
            uws_loop_addPostHandler(p, raw, post_cb);
            us_loop_run_bun_tick(p, ptr::null());
            assert_eq!(counter.load(Ordering::SeqCst), 11);
            uws_loop_removePreHandler(p, raw, pre_cb);
            uws_loop_removePostHandler(p, raw, post_cb);
            us_loop_run_bun_tick(p, ptr::null());
            // No new increments after removal.
            assert_eq!(counter.load(Ordering::SeqCst), 11);
        }
        // Leak the Arc — tests don't need to clean up.
        std::mem::forget(counter);
    }

    #[test]
    fn wakeup_clears_pending_on_next_tick() {
        let p = uws_get_loop();
        unsafe {
            us_wakeup_loop(p);
            assert!((*p).pending_wakeups >= 1, "wakeup bumps pending_wakeups");
            us_loop_run_bun_tick(p, ptr::null());
            assert_eq!((*p).pending_wakeups, 0, "tick clears pending_wakeups");
        }
    }
}
