// @trace REQ-ENG-008 [entity:BaoLoopState]
//! Wave 74-LOOP-C.1: raw epoll + eventfd implementation of the uSockets loop ABI.
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
//! The `Box` is intentionally leaked (`Box::into_raw`) — the loop has process
//! lifetime, matching upstream Bun's `us_create_loop` semantics.
//!
//! ## Raw epoll backend (Wave 74-LOOP-C.1)
//!
//! Each `PosixLoop` carries a `BaoLoopState` (held in a `thread_local!`)
//! containing:
//!   - `epfd` — `epoll_create1(EPOLL_CLOEXEC)` fd, also stored in
//!     `(*loop_ptr).fd` so FilePoll's `register_with_fd_impl` works
//!   - `deferred` — `VecDeque` of next-tick callbacks pushed by `uws_loop_defer`
//!   - `pre_handlers` / `post_handlers` — registered `addPreHandler` /
//!     `addPostHandler` callbacks (small vec of fn pointers)
//!
//! Cross-thread wake uses a raw `eventfd` registered into `epfd` with
//! `WAKEUP_TAG = 0` in the `data.u64` high bits. The eventfd fd is stored
//! in a heap-allocated `BaoWakeupAsync` whose pointer is cast to
//! `*mut us_internal_async` and placed in
//! `(*loop_ptr).internal_loop_data.wakeup_async` — this makes it reachable
//! from any thread holding `*mut Loop`, fixing the `with_matching_state`
//! thread_local limitation.
//!
//! ## Tagged pointer dispatch
//!
//! `epoll_event.data.u64` carries a tagged pointer:
//!   - Bits 0..49  → pointer (same as FilePoll's `TaggedPtr`)
//!   - Bits 49..64 → tag (u15):
//!     - 0    = WAKEUP (eventfd sentinel)
//!     - 1024 = FilePoll (Pollable::FILE_POLL_TAG)
//!     - 1..4 = BaoPoll (Socket/ListenSocket/Shutdown/Callback — 74-C.2)
//!
//! This matches `TaggedPtr::init` in `ptr/tagged_pointer.rs` and
//! `Pollable::FILE_POLL_TAG` in `io/posix_event_loop.rs`.

#![allow(clippy::missing_safety_doc)]
#![cfg(target_os = "linux")] // 74-C.1: Linux epoll only; kqueue = 74-C.8

pub mod poll;

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::ptr;
use core::sync::atomic::Ordering;

use bun_uws_sys::{InternalLoopData, Loop, PosixLoop, Timespec};

// ────────────────────────────── constants ──────────────────────────────

/// Number of bits to shift the tag into the high position.
/// Matches `TaggedPtr::ADDR_BITS` in `ptr/tagged_pointer.rs`.
const ADDR_BITS: u32 = 49;

/// Tag value for the wakeup eventfd: 0 (tag 0 = null-tagged pointer).
const WAKEUP_TAG: u16 = 0;

/// Encode a tagged pointer: `(ptr as u64 & ADDR_MASK) | (tag as u64 << ADDR_BITS)`.
/// Used only for the wakeup eventfd registration. All other epoll events
/// use untagged `data.ptr` (the CLEAR_POINTER_TAG dispatch model).
#[inline]
fn encode_tagged_ptr(ptr: *mut c_void, tag: u16) -> u64 {
    let addr = ptr as usize as u64;
    let addr_mask: u64 = (1u64 << ADDR_BITS) - 1;
    (addr & addr_mask) | ((tag as u64) << ADDR_BITS)
}

// ────────────────────────────── types ──────────────────────────────

pub type LoopCb = unsafe extern "C" fn(*mut Loop);
pub type LoopCtxCb = unsafe extern "C" fn(*mut c_void, *mut Loop);
pub type DeferCb = unsafe extern "C" fn(*mut c_void);

/// Heap-allocated structure holding the wakeup eventfd. Stored in
/// `InternalLoopData.wakeup_async` (cast to `*mut us_internal_async`)
/// so it's reachable from any thread holding `*mut Loop`.
///
/// Upstream C uses `us_internal_callback_t` (which wraps `us_poll_t`);
/// we only need the fd and the callback.
#[repr(C)]
struct BaoWakeupAsync {
    fd: c_int,
    cb: Option<unsafe extern "C" fn(*mut BaoWakeupAsync)>,
}

/// Per-thread state backing each `PosixLoop` returned by `uws_get_loop` /
/// `us_create_loop`. Stored as `thread_local! { RefCell<Option<...>> }` so the
/// first call lazily materialises both the `PosixLoop` shell and the epoll
/// backend in lock-step.
struct BaoLoopState {
    /// Pointer to the `Box::into_raw`-ed `PosixLoop` we exposed to FFI.
    loop_ptr: *mut PosixLoop,

    /// epoll fd from `epoll_create1(EPOLL_CLOEXEC)`. Also stored in
    /// `(*loop_ptr).fd` so FilePoll can `epoll_ctl(loop_.fd, ...)`.
    epfd: c_int,

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

    /// User wake callback set at `us_create_loop` time.
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
    assert!(!recv_buf.is_null(), "bao_uloop: libc::malloc(recv_buf) failed");
    unsafe { ptr::write_bytes(recv_buf, 0, RECV_BUF_LEN) };

    let send_buf: *mut u8 = unsafe { libc::malloc(RECV_BUF_LEN) as *mut u8 };
    assert!(!send_buf.is_null(), "bao_uloop: libc::malloc(send_buf) failed");
    unsafe { ptr::write_bytes(send_buf, 0, RECV_BUF_LEN) };

    // Create the epoll fd. This is the single poll set shared by FilePoll
    // and BaoPoll — FilePoll reads `loop_.fd` directly and does raw
    // `epoll_ctl` (see `posix_event_loop.rs:register_with_fd_impl`).
    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    assert!(epfd >= 0, "bao_uloop: epoll_create1 failed");

    // Create the wakeup eventfd. Registered into epfd with WAKEUP_TAG so
    // `epoll_wait` returns it as a ready event. Stored in a heap-allocated
    // `BaoWakeupAsync` whose pointer goes into `wakeup_async` (cross-thread
    // reachable from `*mut Loop`).
    let wakeup_fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    assert!(wakeup_fd >= 0, "bao_uloop: eventfd failed");

    let wakeup_async = Box::into_raw(Box::new(BaoWakeupAsync {
        fd: wakeup_fd,
        cb: None,
    }));

    // Register wakeup_fd into epfd with WAKEUP_TAG.
    let mut wakeup_event: libc::epoll_event = unsafe { core::mem::zeroed() };
    wakeup_event.events = libc::EPOLLIN as u32;
    wakeup_event.u64 = encode_tagged_ptr(wakeup_async as *mut c_void, WAKEUP_TAG);
    let ret = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, wakeup_fd, &mut wakeup_event) };
    assert!(ret == 0, "bao_uloop: epoll_ctl ADD wakeup_fd failed");

    // Build a zeroed `InternalLoopData` then patch in the buffers and wakeup.
    let internal = InternalLoopData {
        sweep_timer: ptr::null_mut(),
        sweep_timer_count: 0,
        wakeup_async: wakeup_async as *mut bun_uws_sys::internal_loop_data::us_internal_async,
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

    // Allocate the PosixLoop shell. Store the real epoll fd in `fd` — this
    // is critical: FilePoll reads `loop_.fd` and does `epoll_ctl(loop_.fd, ...)`.
    let boxed: Box<PosixLoop> = Box::new(PosixLoop {
        internal_loop_data: internal,
        num_polls: 0,
        num_ready_polls: 0,
        current_ready_poll: 0,
        fd: epfd,
        active: 0,
        pending_wakeups: 0,
        ready_polls: [unsafe { core::mem::zeroed() }; 1024],
    });
    let loop_ptr: *mut PosixLoop = Box::into_raw(boxed);

    BAO_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_some() {
            panic!("bao_uloop: thread already has a BaoLoopState — call us_loop_free first");
        }
        *slot = Some(BaoLoopState {
            loop_ptr,
            epfd,
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
/// state is present or the pointer doesn't match.
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

// ──────────────────────── epoll tick ────────────────────────────────

/// Single `epoll_wait` + dispatch. Replaces the old `run_mio_poll`.
///
/// Reads ready events into `(*loop_).ready_polls`, sets
/// `num_ready_polls` / `current_ready_poll`, then:
///   1. Drain the wakeup eventfd (if present in ready events)
///   2. Delegate all other events to `poll::dispatch_ready_polls`
///      which uses the `CLEAR_POINTER_TAG` pattern: tagged → FilePoll,
///      untagged → `us_internal_dispatch_ready_poll`.
fn run_epoll(loop_: *mut Loop, pending: u32, timeout: *const Timespec) {
    let timeout_ms: c_int = if pending > 0 || timeout.is_null() {
        0
    } else {
        let ts: Timespec = unsafe { *timeout };
        if ts.sec == 0 && ts.nsec == 0 {
            0
        } else {
            let ms = ts.sec as i64 * 1000 + ts.nsec as i64 / 1_000_000;
            ms.min(i32::MAX as i64) as c_int
        }
    };

    let epfd = BAO_LOOP.with(|cell| {
        let slot = cell.borrow();
        slot.as_ref().filter(|s| ptr::eq(s.loop_ptr, loop_)).map(|s| s.epfd)
    });
    let Some(epfd) = epfd else { return };

    let loop_ptr: *mut PosixLoop = loop_ as *mut PosixLoop;
    let nfds = unsafe {
        libc::epoll_wait(
            epfd,
            (*loop_ptr).ready_polls.as_mut_ptr(),
            1024,
            timeout_ms,
        )
    };

    if nfds <= 0 {
        return;
    }

    unsafe {
        (*loop_ptr).num_ready_polls = nfds;
        (*loop_ptr).current_ready_poll = 0;
    }

    // Drain the wakeup eventfd first (if it's in the ready set).
    // The wakeup is registered with WAKEUP_TAG in data.u64, so we identify
    // it by checking against InternalLoopData.wakeup_async.
    let wakeup_async_raw = unsafe {
        (*loop_ptr).internal_loop_data.wakeup_async as *mut BaoWakeupAsync
    };

    for i in 0..nfds {
        let event = unsafe { (*loop_ptr).ready_polls[i as usize] };
        if event.u64 == encode_tagged_ptr(wakeup_async_raw as *mut c_void, WAKEUP_TAG) {
            if !wakeup_async_raw.is_null() {
                let fd = unsafe { (*wakeup_async_raw).fd };
                let mut buf: u64 = 0;
                unsafe {
                    libc::read(fd, &mut buf as *mut u64 as *mut c_void, 8);
                }
                if let Some(cb) = unsafe { (*wakeup_async_raw).cb } {
                    unsafe { cb(wakeup_async_raw) };
                }
            }
            // Null this event so the dispatch loop skips it
            unsafe { (*loop_ptr).ready_polls[i as usize].u64 = 0; }
        }
    }

    // Dispatch remaining events via the CLEAR_POINTER_TAG pattern.
    unsafe { poll::dispatch_ready_polls(loop_); }
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
            let loop_ptr: *mut PosixLoop = loop_ as *mut PosixLoop;

            // Close the wakeup eventfd and free BaoWakeupAsync.
            let wakeup_async_ptr = unsafe { (*loop_ptr).internal_loop_data.wakeup_async }
                as *mut BaoWakeupAsync;
            if !wakeup_async_ptr.is_null() {
                unsafe {
                    libc::close((*wakeup_async_ptr).fd);
                    let _ = Box::from_raw(wakeup_async_ptr);
                }
            }

            // Close the epoll fd.
            unsafe { libc::close(state.epfd); }

            // Free recv/send buffers (libc-allocated in `create_loop`).
            unsafe {
                if !(*loop_ptr).internal_loop_data.recv_buf.is_null() {
                    libc::free((*loop_ptr).internal_loop_data.recv_buf as *mut c_void);
                }
                if !(*loop_ptr).internal_loop_data.send_buf.is_null() {
                    libc::free((*loop_ptr).internal_loop_data.send_buf as *mut c_void);
                }
                // Drop the Box<PosixLoop>.
                let _ = Box::from_raw(loop_ptr);
            }
            *slot = None;
        }
    });
}

/// Cross-thread wake. Writes to the wakeup eventfd via
/// `InternalLoopData.wakeup_async` (loop-reachable, no thread_local match
/// needed). This fixes the old `with_matching_state` limitation where
/// wakeups from another thread silently failed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_wakeup_loop(loop_: *mut Loop) {
    if loop_.is_null() {
        return;
    }

    // Bump the pending-wakeups counter so the next tick returns immediately.
    let _ = with_matching_state(loop_, |state| {
        state.pending_wakeups.fetch_add(1, Ordering::SeqCst);
    });
    unsafe {
        (*loop_).pending_wakeups = (*loop_).pending_wakeups.wrapping_add(1);
    }

    // Write to the wakeup eventfd — this is the actual cross-thread wake.
    // The eventfd fd is stored in InternalLoopData.wakeup_async which is
    // reachable from any thread holding *mut Loop.
    let wakeup_async_ptr = unsafe { (*loop_).internal_loop_data.wakeup_async }
        as *mut BaoWakeupAsync;
    if !wakeup_async_ptr.is_null() {
        let fd = unsafe { (*wakeup_async_ptr).fd };
        let val: u64 = 1;
        unsafe {
            libc::write(fd, &val as *const u64 as *const c_void, 8);
        }
    }

    // Fire the user wakeup hook (mirrors upstream LoopHandler::WAKEUP).
    let cb = with_matching_state(loop_, |state| state.wakeup_cb).flatten();
    if let Some(cb) = cb {
        unsafe { (cb)(loop_) };
    }
}

/// Single-iteration tick. Drains deferred queue, invokes pre-handlers, polls
/// epoll with the supplied timeout, invokes post-handlers, increments
/// `iteration_nr`. This is the meat of `PosixLoop::tick()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec) {
    if loop_.is_null() {
        return;
    }

    // Drain pending wakeups — if any, the poll timeout collapses to zero.
    let pending = with_matching_state(loop_, |state| {
        state.pending_wakeups.swap(0, Ordering::SeqCst)
    })
    .unwrap_or(0);
    if pending > 0 {
        unsafe { (*loop_).pending_wakeups = 0; }
    }

    // Phase 1: drain deferred callbacks.
    let deferred = take_deferred(loop_);
    for call in deferred {
        unsafe { (call.cb)(call.ctx) };
    }

    // Phase 2: pre-callback (loop-level LoopHandler::PRE).
    if let Some(cb) = with_matching_state(loop_, |state| state.pre_cb).flatten() {
        unsafe { (cb)(loop_) };
    }

    // Phase 3: pre-handlers.
    let pre = snapshot_handlers(loop_, HandlerKind::Pre);
    for slot in &pre {
        unsafe { (slot.cb)(slot.ctx, loop_) };
    }

    // Phase 4: epoll_wait + tagged dispatch.
    run_epoll(loop_, pending, timeout);

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

        let p2 = uws_get_loop();
        assert_eq!(p1, p2, "uws_get_loop must be stable on the same thread");

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
            assert_eq!(counter.load(Ordering::SeqCst), 11);
        }
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

    // ──────── 74-C.1 new tests: epoll fd + eventfd ────────

    #[test]
    fn loop_fd_is_real_epoll_fd() {
        let p = uws_get_loop();
        unsafe {
            let fd = (*p).fd;
            assert!(fd >= 0, "loop_.fd must be a valid epoll fd, got {fd}");
        }
    }

    #[test]
    fn wakeup_async_is_non_null() {
        let p = uws_get_loop();
        unsafe {
            let wakeup_async = (*p).internal_loop_data.wakeup_async;
            assert!(
                !wakeup_async.is_null(),
                "wakeup_async must be non-null after loop creation"
            );
        }
    }

    #[test]
    fn cross_thread_wakeup_writes_eventfd() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let p = uws_get_loop() as usize; // *mut Loop is not Send; pass as usize
        let observed = Arc::new(AtomicU32::new(0));
        let observed_clone = observed.clone();

        // Spawn a thread that calls us_wakeup_loop on the same *mut Loop.
        // The eventfd write must succeed even though the other thread
        // doesn't own the thread_local BaoLoopState.
        std::thread::spawn(move || {
            unsafe { us_wakeup_loop(p as *mut Loop) };
            observed_clone.store(1, Ordering::SeqCst);
        })
        .join()
        .unwrap();

        assert_eq!(observed.load(Ordering::SeqCst), 1, "cross-thread wakeup must not panic");
        // The pending_wakeups should have been bumped.
        unsafe {
            assert!((*uws_get_loop()).pending_wakeups >= 1);
        }
    }
}
