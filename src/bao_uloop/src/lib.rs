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
#![allow(dead_code)]
// BUG-353 fix: loop entry points now extern "C" from C/C++ libs.
// Internal helpers retained for poll.rs (FilePoll graft).
#![cfg(target_os = "linux")] // 74-C.1: Linux epoll only; kqueue = 74-C.8

pub mod poll;

use core::ffi::{c_char, c_int, c_uint, c_void};
use core::ptr;

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
// The following types and helpers are retained for poll.rs (FilePoll graft)
// and future integration. The old loop entry points (us_create_loop, etc.)
// are now extern "C" imports from libusockets.a/libuwsockets.a (BUG-353 fix).

#[allow(dead_code)]
pub type LoopCb = unsafe extern "C" fn(*mut Loop);
#[allow(dead_code)]
pub type LoopCtxCb = unsafe extern "C" fn(*mut c_void, *mut Loop);
#[allow(dead_code)]
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

    /// Saved pointer to the `Box::into_raw`-ed `BaoWakeupAsync` so `Drop` can
    /// recover the correct Rust type. `InternalLoopData.wakeup_async` stores
    /// this same address cast to `*mut us_internal_async` (an opaque ZST), so
    /// we cannot reconstruct the `BaoWakeupAsync` from it — we must keep the
    /// original typed pointer.
    wakeup_async_ptr: *mut BaoWakeupAsync,

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

// SAFETY: BaoLoopState owns its raw pointers and only accesses them from the
// thread that created them. The `wakeup_async_ptr` field is a `Box::into_raw`
// pointer that we reconstruct in `Drop` — it is never shared across threads.
unsafe impl Send for BaoLoopState {}

impl Drop for BaoLoopState {
    fn drop(&mut self) {
        // Release order: child resources first, then parent struct, then fd.
        // This mirrors C's `us_loop_free` → `us_internal_loop_data_free` → `close(fd)` → `us_free(loop)`.

        // 1. Close wakeup eventfd and free BaoWakeupAsync (Box-allocated).
        if !self.wakeup_async_ptr.is_null() {
            let wakeup = unsafe { &*self.wakeup_async_ptr };
            if wakeup.fd >= 0 {
                unsafe {
                    libc::close(wakeup.fd);
                }
            }
            // Reconstruct the Box so Rust drops it.
            unsafe {
                drop(Box::from_raw(self.wakeup_async_ptr));
            }
            self.wakeup_async_ptr = ptr::null_mut();
        }

        // 2. Free recv_buf and send_buf (libc::malloc-allocated).
        if !self.loop_ptr.is_null() {
            let internal = unsafe { &(*self.loop_ptr).internal_loop_data };
            if !internal.recv_buf.is_null() {
                unsafe {
                    libc::free(internal.recv_buf as *mut c_void);
                }
            }
            if !internal.send_buf.is_null() {
                unsafe {
                    libc::free(internal.send_buf as *mut c_void);
                }
            }
            // 3. Free PosixLoop itself (Box::into_raw-allocated).
            unsafe {
                drop(Box::from_raw(self.loop_ptr));
            }
            self.loop_ptr = ptr::null_mut();
        }

        // 4. Close epoll fd (last — child resources may reference it during teardown).
        if self.epfd >= 0 {
            unsafe {
                libc::close(self.epfd);
            }
            self.epfd = -1;
        }
    }
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
        jsc_vm: ptr::null(), // In Bao, this holds a SpiderMonkey JSContext pointer
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
            wakeup_async_ptr: wakeup_async,
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
fn with_matching_state<R>(loop_: *mut Loop, f: impl FnOnce(&mut BaoLoopState) -> R) -> Option<R> {
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
            let ms = ts.sec * 1000 + ts.nsec / 1_000_000;
            ms.min(i32::MAX as i64) as c_int
        }
    };

    let epfd = BAO_LOOP.with(|cell| {
        let slot = cell.borrow();
        slot.as_ref()
            .filter(|s| ptr::eq(s.loop_ptr, loop_))
            .map(|s| s.epfd)
    });
    let Some(epfd) = epfd else { return };

    let loop_ptr: *mut PosixLoop = loop_;
    let nfds =
        unsafe { libc::epoll_wait(epfd, (*loop_ptr).ready_polls.as_mut_ptr(), 1024, timeout_ms) };

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
    let wakeup_async_raw =
        unsafe { (*loop_ptr).internal_loop_data.wakeup_async as *mut BaoWakeupAsync };

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
            unsafe {
                (*loop_ptr).ready_polls[i as usize].u64 = 0;
            }
        }
    }

    // Dispatch remaining events via the CLEAR_POINTER_TAG pattern.
    unsafe {
        poll::dispatch_ready_polls(loop_);
    }
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

// ─────────────────────── FFI entry points (BUG-353 fix) ────────────────────
//
// BUG-353 root cause (architect MCP analysis, session 2afeca83):
//   - bao_uloop defined these 11 symbols as #[no_mangle] extern "C" fn
//   - libusockets.a (C, loop.c) and libuwsockets.a (C++, libuwsockets.cpp)
//     ALSO define them
//   - Rust #[no_mangle] won link resolution over C/C++ static archives
//   - bao_uloop::us_create_loop allocated only sizeof(PosixLoop) with no
//     ext_size, so loop+1 (where C++ places LoopData) was uninitialized
//   - C++ uWS::TemplatedApp::listen read loop+1 → malloc corruption
//
// Fix (Solution A: C-exclusive): declare these symbols as `extern "C"`.
// The C/C++ library versions resolve at link time. bao_uloop's role is now:
//   - FilePoll graft (poll.rs - epoll fd sharing)
//   - us_dispatch_* (socket event routing)
//   - Bun__addrinfo_* (usockets DNS seam → bun_dns shared cache)
//
// CLAUDE.md L13/L26: "禁止手写 C 已实现符号的 Rust 翻译". The previous
// Rust implementations violated this rule. The fix restores compliance.

unsafe extern "C" {
    /// Thread-local singleton loop accessor. Provided by libuwsockets.a (C++).
    /// Safe to call: returns a per-thread loop pointer, never null after init.
    pub safe fn uws_get_loop() -> *mut Loop;

    /// Loop construction. Provided by libusockets.a (C, loop.c).
    /// Allocates sizeof(us_loop_t) + ext_size and initialises wakeup eventfd.
    pub unsafe fn us_create_loop(
        hint: *mut c_void,
        wakeup_cb: Option<LoopCb>,
        pre_cb: Option<LoopCb>,
        post_cb: Option<LoopCb>,
        ext_size: c_uint,
    ) -> *mut Loop;

    /// Loop destruction. Provided by libusockets.a (C, loop.c).
    pub unsafe fn us_loop_free(loop_: *mut Loop);

    /// Cross-thread wake. Provided by libusockets.a (C, loop.c).
    pub unsafe fn us_wakeup_loop(loop_: *mut Loop);

    /// Run until active==0. Provided by libusockets.a (C, loop.c).
    pub unsafe fn us_loop_run(loop_: *mut Loop);

    /// Defer a callback to next tick. Provided by libuwsockets.a (C++).
    pub unsafe fn uws_loop_defer(loop_: *mut Loop, ctx: *mut c_void, cb: DeferCb);

    /// Register a pre-tick handler. Provided by libuwsockets.a (C++).
    pub unsafe fn uws_loop_addPreHandler(loop_: *mut Loop, ctx: *mut c_void, cb: LoopCtxCb);

    /// Remove a pre-tick handler. Provided by libuwsockets.a (C++).
    pub unsafe fn uws_loop_removePreHandler(loop_: *mut Loop, ctx: *mut c_void, cb: LoopCtxCb);

    /// Register a post-tick handler. Provided by libuwsockets.a (C++).
    pub unsafe fn uws_loop_addPostHandler(loop_: *mut Loop, ctx: *mut c_void, cb: LoopCtxCb);

    /// Remove a post-tick handler. Provided by libuwsockets.a (C++).
    pub unsafe fn uws_loop_removePostHandler(loop_: *mut Loop, ctx: *mut c_void, cb: LoopCtxCb);
}

// ──────────────── us_loop_run_bun_tick (Rust override of C version) ──────
//
// The C version (libusockets.a, loop.c) does its own `epoll_pwait2` which
// blocked indefinitely on NULL timeout — the root cause of BCE-007 fetch hang.
//
// We can't simply #[no_mangle] override it because the C symbol is `T`
// (exported), causing a duplicate-symbol error with the wild linker.
// Instead we provide a Rust tick entry point and call it from the sites
// that used to call the C version (Loop.rs::tick / tick_without_idle).
//
// The C version remains in libusockets.a but is no longer called from
// bao's Rust code — all tick paths go through `bao_uloop_tick`.

/// Single-iteration event loop tick — Rust implementation.
/// Does `epoll_wait` with controlled timeout (no NULL blocking) then
/// dispatches via `dispatch_ready_polls` → C `us_internal_dispatch_ready_poll`
/// for socket events.
///
/// # Safety
/// `loop_` must be a valid `*mut Loop` created by `us_create_loop`.
/// Single-iteration event loop tick — Rust implementation.
/// Does `epoll_wait` with controlled timeout (no NULL blocking) then
/// dispatches via `dispatch_ready_polls` → C `us_internal_dispatch_ready_poll`
/// for socket events.
///
/// Called from `bun_uws_sys::Loop::tick/tick_without_idle` via extern "C"
/// linkage (#[no_mangle] `bao_loop_tick`). This avoids a crate dependency
/// cycle (bun_uws_sys cannot depend on bao_uloop).
///
/// # Safety
/// `loop_` must be a valid `*mut Loop` created by `us_create_loop`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bao_loop_tick(loop_: *mut Loop, timeout: *const Timespec) {
    let loop_ptr: *mut PosixLoop = loop_;
    let pending = unsafe { (*loop_ptr).active };

    // Check if this thread has a BaoLoopState for this loop (JS thread does,
    // HTTPThread does not). If yes → Rust epoll_wait + dispatch. If no →
    // fall back to C us_loop_run_bun_tick which uses the loop's InternalLoopData.
    let has_rust_state = BAO_LOOP.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|s| ptr::eq(s.loop_ptr, loop_))
            .unwrap_or(false)
    });

    if has_rust_state {
        run_epoll(loop_, pending, timeout);
    } else {
        // HTTPThread or any thread without BaoLoopState: use C tick.
        // The C version has its own epoll_wait + dispatch for socket events.
        // BCE-007-R3/R3-ext: callers use tick_without_idle (zero timeout),
        // so the C epoll_wait won't block indefinitely.
        unsafe extern "C" {
            fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec);
        }
        unsafe { us_loop_run_bun_tick(loop_, timeout) };
    }
}

// ──────────────── us_dispatch_* kind→vtable routing ──────────────────
// These are the socket event dispatchers called by libusockets (loop.c,
// socket.c, context.c). In Bun upstream they're implemented in Zig
// (src/runtime/socket/uws_dispatch.zig) and route by `s->kind` to the
// appropriate vtable handler (HTTP, WS, etc.). Bao implements the same
// routing logic: read `s.kind()` → for Invalid, panic → get
// `s.raw_group().vtable` → call the callback if present, else return `s`.

use bun_uws_sys::socket_group::VTable;
use bun_uws_sys::{ConnectingSocket, SocketKind, us_bun_verify_error_t, us_socket_t};

/// Dispatch a socket event through its group's vtable. Returns the socket
/// unchanged if the group has no vtable or the callback slot is None.
///
/// # Safety
/// `s` must be a live `us_socket_t` per the caller contract.
#[inline]
unsafe fn dispatch_via_vtable<S, R>(
    s: *mut c_void,
    fallback: S,
    call: impl FnOnce(&'static VTable, *mut us_socket_t) -> R,
) -> R
where
    S: FnOnce() -> R,
{
    let sock = s as *mut us_socket_t;
    let sock_ref = unsafe { &mut *sock };
    let kind = sock_ref.kind();

    // Invalid kind = bug (socket not initialised or corrupted). Panic
    // mirrors upstream Zig's unreachable trap.
    if kind == SocketKind::Invalid {
        panic!("us_dispatch: socket kind is Invalid — uninitialized or corrupted socket");
    }

    // All kinds route through their group's vtable. If the group has
    // no vtable (or the specific callback slot is None), fall through
    // and return the socket unchanged.
    let group = sock_ref.raw_group();
    match group.vtable {
        Some(vtable) => call(vtable, sock),
        None => fallback(),
    }
}

/// Socket opened (accept or connect completion).
/// Routes to `group.vtable.on_open` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_open(
    s: *mut c_void,
    is_client: c_int,
    ip: *mut u8,
    ip_length: c_int,
) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_open {
                Some(cb) => cb(sock, is_client, ip, ip_length) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket received data.
/// Routes to `group.vtable.on_data` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_data(
    s: *mut c_void,
    data: *mut u8,
    length: c_int,
) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_data {
                Some(cb) => cb(sock, data, length) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket received fd (IPC).
/// Routes to `group.vtable.on_fd` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_fd(s: *mut c_void, fd: c_int) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_fd {
                Some(cb) => cb(sock, fd) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket became writable.
/// Routes to `group.vtable.on_writable` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_writable(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_writable {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket closed.
/// Routes to `group.vtable.on_close` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_close(
    s: *mut c_void,
    code: c_int,
    reason: *mut c_void,
) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_close {
                Some(cb) => cb(sock, code, reason) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket timed out.
/// Routes to `group.vtable.on_timeout` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_timeout(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_timeout {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket long-timeout.
/// Routes to `group.vtable.on_long_timeout` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_long_timeout(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_long_timeout {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Socket received FIN/EOF.
/// Routes to `group.vtable.on_end` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_end(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_end {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Established socket connect error.
/// Routes to `group.vtable.on_connect_error` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_connect_error(s: *mut c_void, code: c_int) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(
            s,
            || s,
            |vt, sock| match vt.on_connect_error {
                Some(cb) => cb(sock, code) as *mut c_void,
                None => s,
            },
        )
    }
}

/// Connecting socket error.
/// Routes to `group.vtable.on_connecting_error` if available, else returns `c`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_connecting_error(c: *mut c_void, code: c_int) -> *mut c_void {
    let conn = c as *mut ConnectingSocket;
    let conn_ref = unsafe { &mut *conn };
    // ConnectingSocket dispatch also goes through its group's vtable,
    // but uses `us_connecting_socket_group` instead of `us_socket_group`.
    let group_ptr = conn_ref.raw_group();
    if group_ptr.is_null() {
        return c;
    }
    let group = unsafe { &*group_ptr };
    match group.vtable {
        Some(vtable) => match vtable.on_connecting_error {
            Some(cb) => unsafe { cb(conn, code) as *mut c_void },
            None => c,
        },
        None => c,
    }
}

/// SSL handshake completion. Calls `group.vtable.on_handshake` if available.
/// C signature: `void us_dispatch_handshake(s, int success, us_bun_verify_error_t err)`.
/// VTable callback signature: `fn(s, int success, us_bun_verify_error_t, *mut c_void)`.
/// The 4th argument (custom_data) is passed as null, matching Zig upstream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_handshake(
    s: *mut c_void,
    success: c_int,
    err: us_bun_verify_error_t,
) {
    unsafe {
        dispatch_via_vtable(
            s,
            || {},
            |vt, sock| {
                if let Some(cb) = vt.on_handshake {
                    cb(sock, success, err, core::ptr::null_mut());
                }
            },
        )
    }
}

/// SSL raw ciphertext tap. Returns `s` unchanged — no vtable hook for this.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_ssl_raw_tap(
    s: *mut c_void,
    _data: *mut u8,
    _length: c_int,
) -> *mut c_void {
    s
}

// ──────────────── Bun__addrinfo_* (usockets DNS seam) ────────────────────
// Real implementation (shared bun_dns cache + blocking getaddrinfo worker);
// contract notes and reference-counting rules live in the module. Replaces
// the former no-op stubs, whose `Bun__addrinfo_get → -1` left context.c's
// `ai_req` uninitialized on every hostname connect (UB).

mod addrinfo;

pub use addrinfo::Bun__addrinfo_cancel;
pub use addrinfo::Bun__addrinfo_freeRequest;
pub use addrinfo::Bun__addrinfo_get;
pub use addrinfo::Bun__addrinfo_getRequestResult;
pub use addrinfo::Bun__addrinfo_registerQuic;
pub use addrinfo::Bun__addrinfo_registerQuic2;
pub use addrinfo::Bun__addrinfo_set;

/// Bun HTTP date header timer optimization. No-op in plain TCP mode.
/// Called from us_internal_enable_sweep_timer in loop.c.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__internal_ensureDateHeaderTimerIsEnabled(_loop: *mut c_void) {}

// ──────────────── TLS C→Rust hooks (root_certs.cpp / openssl.c) ──────────
// Callbacks libusockets_tls.a calls back into once its C socket symbols are
// referenced. bao_uloop is chained into every link scope that pulls those
// archives (via bao_native_stubs or bun_runtime), so the single `#[no_mangle]`
// definitions live here (STUB-INVENTORY dual-def iron rule; former def sites
// bao_native_stubs/c_lib_stubs.rs and bun_runtime::product_native_symbols are
// deleted — do NOT reintroduce them).

/// Whether to load system CA certificates for TLS verification. Set by
/// `--use-system-ca` / `NODE_USE_SYSTEM_CA=1` upstream; default: load.
#[unsafe(no_mangle)]
pub static mut Bun__Node__UseSystemCA: bool = true;

/// Warning callback: `root_certs.cpp` calls this when a certificate file in
/// the system CA directory cannot be parsed (the certs are skipped).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn BUN__warn__extra_ca_load_failed(
    filename: *const c_char,
    error_msg: *const c_char,
) {
    let filename_str = if filename.is_null() {
        "(unknown)".to_string()
    } else {
        // SAFETY: caller contract (root_certs.cpp) provides NUL-terminated C strings.
        unsafe { std::ffi::CStr::from_ptr(filename) }
            .to_string_lossy()
            .into_owned()
    };
    let error_str = if error_msg.is_null() {
        "(unknown)".to_string()
    } else {
        // SAFETY: see above.
        unsafe { std::ffi::CStr::from_ptr(error_msg) }
            .to_string_lossy()
            .into_owned()
    };
    eprintln!("warn: ignoring extra certs from {filename_str}, load failed: {error_str}");
}

/// BoringSSL CRYPTO_EX_free callback (openssl.c `us_ctx_cache_ex_idx`).
/// Tombstones the SSLContextCache entry when the last SSL_CTX ref drops;
/// safe no-op while no SSL_CTX cache is wired (every handshake gets a fresh
/// SSL_CTX). Same shape the former stub/product defs had.
#[unsafe(no_mangle)]
pub extern "C" fn bun_ssl_ctx_cache_on_free(
    _parent: *mut c_void,
    _ptr: *mut c_void,
    _ad: *mut c_void,
    _index: c_int,
    _argl: i64,
    _argp: *mut c_void,
) {
}

/// Force the linker to keep bao_uloop's `#[no_mangle] extern "C"` symbols.
/// BUG-353 fix: loop symbols (us_create_loop, uws_get_loop, etc.) are now
/// extern "C" imports from libusockets.a/libuwsockets.a — no need to force-link
/// them here. Only dispatch stubs and Bun__ addrinfo stubs remain local.
#[inline(never)]
pub fn force_link() {
    // Loop + poll ABI: now extern "C" from C/C++ libs — no force needed.
    poll::force_link_poll();

    // Dispatch stubs (still local #[no_mangle])
    let _ = us_dispatch_open as unsafe extern "C" fn(_, _, _, _) -> *mut c_void;
    let _ = us_dispatch_data as unsafe extern "C" fn(_, _, _) -> *mut c_void;
    let _ = us_dispatch_fd as unsafe extern "C" fn(_, _) -> *mut c_void;
    let _ = us_dispatch_writable as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = us_dispatch_close as unsafe extern "C" fn(_, _, _) -> *mut c_void;
    let _ = us_dispatch_timeout as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = us_dispatch_long_timeout as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = us_dispatch_end as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = us_dispatch_connect_error as unsafe extern "C" fn(_, _) -> *mut c_void;
    let _ = us_dispatch_connecting_error as unsafe extern "C" fn(_, _) -> *mut c_void;
    let _ = us_dispatch_handshake as unsafe extern "C" fn(_, _, _);
    let _ = us_dispatch_ssl_raw_tap as unsafe extern "C" fn(_, _, _) -> *mut c_void;

    // Addrinfo (DNS seam)
    let _ = Bun__addrinfo_get as unsafe extern "C" fn(_, _, _, _) -> c_int;
    let _ = Bun__addrinfo_set as unsafe extern "C" fn(_, _) -> c_int;
    let _ = Bun__addrinfo_cancel as unsafe extern "C" fn(_, _) -> c_int;
    let _ = Bun__addrinfo_freeRequest as unsafe extern "C" fn(_, _);
    let _ = Bun__addrinfo_getRequestResult as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = Bun__addrinfo_registerQuic as unsafe extern "C" fn(_, _);
    let _ = Bun__addrinfo_registerQuic2
        as unsafe extern "C" fn(_, _, Option<unsafe extern "C" fn(_)>) -> ();

    // Bun internal stubs
    let _ = Bun__internal_ensureDateHeaderTimerIsEnabled as unsafe extern "C" fn(_);
}
// Tests removed: they tested the old Rust loop implementation that caused BUG-353.
// The C/C++ loop implementation is now tested via bao_runtime integration tests
// (uws_link_verification_tests, bun_api_tests, realworld_http_service_tests).

#[cfg(test)]
mod hangup_tests {
    //! Behavioral test for the upstream e5a3fe6dc EPOLLHUP fix: an
    //! `allow_half_open` AF_UNIX socket whose peer closes first must get
    //! `on_end` + `on_close` exactly once — pre-fix, the level-triggered
    //! EPOLLHUP re-fired `on_end` on every tick (one full core per socket).
    //!
    //! Drives the C path end-to-end (`us_loop_run_bun_tick` → C
    //! `us_internal_dispatch_ready_polls` → loop.c hangup semantics). The
    //! Rust dispatch mapping in `poll::dispatch_ready_polls` is covered by
    //! the mapping unit tests in poll.rs.

    use super::*;
    use bun_uws_sys::socket_group::VTable;
    use bun_uws_sys::{
        LIBUS_SOCKET_ALLOW_HALF_OPEN, ListenSocket, SocketGroup, SocketKind, us_socket_t,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static DATA_COUNT: AtomicUsize = AtomicUsize::new(0);
    static DATA_BYTES: AtomicUsize = AtomicUsize::new(0);
    static END_COUNT: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn t_open(
        s: *mut us_socket_t,
        _is_client: c_int,
        _ip: *mut u8,
        _ip_len: c_int,
    ) -> *mut us_socket_t {
        OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_data(
        s: *mut us_socket_t,
        _data: *mut u8,
        length: c_int,
    ) -> *mut us_socket_t {
        DATA_COUNT.fetch_add(1, Ordering::SeqCst);
        DATA_BYTES.fetch_add(length as usize, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_end(s: *mut us_socket_t) -> *mut us_socket_t {
        END_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_close(
        s: *mut us_socket_t,
        _code: c_int,
        _reason: *mut c_void,
    ) -> *mut us_socket_t {
        CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    static VTABLE: VTable = VTable {
        on_open: Some(t_open),
        on_data: Some(t_data),
        on_fd: None,
        on_writable: None,
        on_close: Some(t_close),
        on_timeout: None,
        on_long_timeout: None,
        on_end: Some(t_end),
        on_connect_error: None,
        on_connecting_error: None,
        on_handshake: None,
    };

    // root_certs.cpp / openssl.c C→Rust hooks (BUN__warn__extra_ca_load_failed,
    // Bun__Node__UseSystemCA, bun_ssl_ctx_cache_on_free) are now defined at lib
    // level in this crate (see the "TLS C→Rust hooks" section) — the former
    // test-local copies were a workaround for the dev-dep cycle and would
    // dual-define the lib symbols now that ownership lives here.

    /// FilePoll tagged-pointer dispatch (epoll_kqueue.c). The real one lives
    /// in bun_io; this test registers no FilePolls, so it is never called —
    /// the symbol only needs to exist for the link.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn Bun__internal_dispatch_ready_poll(
        _loop: *mut Loop,
        _tagged_pointer: *mut c_void,
    ) {
    }

    // loop.c calls data.pre_cb / post_cb / wakeup_cb unconditionally on every
    // tick — us_create_loop must be handed real (no-op) callbacks.
    unsafe extern "C" fn noop_cb(_loop: *mut Loop) {}

    #[test]
    fn unix_half_open_peer_close_ends_once_and_closes() {
        unsafe extern "C" {
            fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec);
        }

        // Propagate liblsquic.a into the test link: loop.c references quic.c,
        // which references lsquic symbols (same force-link pattern as
        // bao_native_stubs / bao_runtime).
        let _ = bun_lsquic_sys::force_link as *const () as usize;
        let _ = bun_lsquic_sys::force_link_lshpack as *const () as usize;
        // Pull bun_threading's rlib in so its #[no_mangle] Bun__lock family
        // (referenced by loop.c) resolves.
        let _ = bun_threading::Mutex::new as *const () as usize;
        // Same for bun_analytics' #[no_mangle] epoll_pwait2 kernel probe
        // (referenced by epoll_kqueue.c).
        let _ = bun_analytics::is_enabled as *const () as usize;

        let path = format!("/tmp/bao-uloop-hangup-test-{}.sock", std::process::id());
        let mut path_bytes = path.clone().into_bytes();
        path_bytes.push(0);
        let _ = std::fs::remove_file(&path);

        let loop_ = unsafe {
            us_create_loop(
                ptr::null_mut(),
                Some(noop_cb),
                Some(noop_cb),
                Some(noop_cb),
                0,
            )
        };
        assert!(!loop_.is_null(), "us_create_loop failed");

        let group: &'static mut SocketGroup = Box::leak(Box::new(SocketGroup::default()));
        group.init(loop_, Some(&VTABLE), ptr::null_mut());

        let mut err: c_int = 0;
        let ls: *mut ListenSocket = group.listen_unix(
            SocketKind::Dynamic,
            None,
            &path_bytes,
            LIBUS_SOCKET_ALLOW_HALF_OPEN,
            0,
            &mut err,
        );
        assert!(!ls.is_null(), "listen_unix failed, err = {err}");

        for c in [
            &OPEN_COUNT,
            &DATA_COUNT,
            &DATA_BYTES,
            &END_COUNT,
            &CLOSE_COUNT,
        ] {
            c.store(0, Ordering::SeqCst);
        }

        // Peer goes away first: connect, write a payload, then close. On
        // AF_UNIX the server side sees EPOLLHUP (not a plain FIN).
        {
            let mut client = std::os::unix::net::UnixStream::connect(&path).expect("connect");
            std::io::Write::write_all(&mut client, b"hello").expect("write");
            std::io::Write::flush(&mut client).expect("flush");
        } // drop → close(2)

        let zero = Timespec { sec: 0, nsec: 0 };
        // Pump the C tick (non-blocking epoll_wait + dispatch) until close.
        for _ in 0..200 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if CLOSE_COUNT.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(OPEN_COUNT.load(Ordering::SeqCst), 1, "one accepted socket");
        assert_eq!(DATA_COUNT.load(Ordering::SeqCst), 1, "payload drained once");
        assert_eq!(
            DATA_BYTES.load(Ordering::SeqCst),
            5,
            "all 5 bytes delivered"
        );
        assert_eq!(
            END_COUNT.load(Ordering::SeqCst),
            1,
            "on_end must fire exactly once"
        );
        assert_eq!(
            CLOSE_COUNT.load(Ordering::SeqCst),
            1,
            "hangup must close the half-open socket (pre-fix it stayed open)"
        );

        // Idle window: EPOLLHUP is level-triggered and unmaskable. Pre-fix,
        // every tick re-delivered it and re-fired on_end (the spin). Post-fix
        // the fd is closed, so nothing may fire here.
        for _ in 0..50 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            END_COUNT.load(Ordering::SeqCst),
            1,
            "EPOLLHUP re-fired after close — spin regression"
        );
        assert_eq!(CLOSE_COUNT.load(Ordering::SeqCst), 1);

        // Teardown.
        unsafe {
            (*ls).close();
            us_loop_run_bun_tick(loop_, &zero);
            SocketGroup::destroy(group);
            us_loop_free(loop_);
        }
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod paused_eof_tests {
    //! Behavioral test for upstream 088da62b6: a socket that already sent its
    //! FIN (`end()`), then pauses under backpressure, must defer the eof hint
    //! until resume — pre-fix, the `is_shut_down` exemption in loop.c closed
    //! the socket while the tail of the peer's stream was still queued in the
    //! kernel (truncated stream). Drives the C path end-to-end
    //! (`us_loop_run_bun_tick` → C `us_internal_dispatch_ready_poll`).

    use super::*;
    use bun_uws_sys::socket_group::VTable;
    use bun_uws_sys::{ListenSocket, SocketGroup, SocketKind, us_socket_t};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static DATA_BYTES: AtomicUsize = AtomicUsize::new(0);
    static END_COUNT: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
    /// Raw pointer stash of the accepted socket (test thread is the only loop
    /// thread; touched only between ticks).
    static SOCKET_PTR: std::sync::atomic::AtomicPtr<us_socket_t> =
        std::sync::atomic::AtomicPtr::new(ptr::null_mut());

    unsafe extern "C" {
        fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec);
        fn us_internal_socket_raw_shutdown(s: *mut us_socket_t);
        fn us_socket_get_fd(s: *mut us_socket_t) -> c_int;
        fn us_socket_pause(s: *mut us_socket_t);
        fn us_socket_resume(s: *mut us_socket_t);
    }

    unsafe extern "C" fn t_open(
        s: *mut us_socket_t,
        _is_client: c_int,
        _ip: *mut u8,
        _ip_len: c_int,
    ) -> *mut us_socket_t {
        OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
        SOCKET_PTR.store(s, Ordering::SeqCst);
        // Enlarge the receive buffer so the client's 1 MiB write completes
        // without blocking (a blocked writer never drops → no FIN → the
        // eof-while-paused moment this test needs would never happen).
        unsafe {
            let fd = us_socket_get_fd(s);
            let sz: c_int = 4 * 1024 * 1024;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                &sz as *const _ as *const c_void,
                core::mem::size_of::<c_int>() as u32,
            );
        }
        s
    }

    unsafe extern "C" fn t_data(
        s: *mut us_socket_t,
        _data: *mut u8,
        length: c_int,
    ) -> *mut us_socket_t {
        let first = DATA_BYTES.fetch_add(length as usize, Ordering::SeqCst) == 0;
        if first {
            // Backpressure: stop reading after the first chunk. us_socket_pause
            // drops READABLE interest; the unread tail stays in the kernel.
            // Only the first burst pauses — post-resume on_data must let the
            // read loop drain to recv()==0 so the eof path can close.
            unsafe { us_socket_pause(s) };
        }
        s
    }

    unsafe extern "C" fn t_end(s: *mut us_socket_t) -> *mut us_socket_t {
        END_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_close(
        s: *mut us_socket_t,
        _code: c_int,
        _reason: *mut c_void,
    ) -> *mut us_socket_t {
        CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    static VTABLE: VTable = VTable {
        on_open: Some(t_open),
        on_data: Some(t_data),
        on_fd: None,
        on_writable: None,
        on_close: Some(t_close),
        on_timeout: None,
        on_long_timeout: None,
        on_end: Some(t_end),
        on_connect_error: None,
        on_connecting_error: None,
        on_handshake: None,
    };

    unsafe extern "C" fn noop_cb(_loop: *mut Loop) {}

    #[test]
    fn shutdown_paused_socket_defers_eof_and_keeps_tail() {
        // Same link pulls as the hangup test (C archive deps).
        let _ = bun_lsquic_sys::force_link as *const () as usize;
        let _ = bun_lsquic_sys::force_link_lshpack as *const () as usize;
        let _ = bun_threading::Mutex::new as *const () as usize;
        let _ = bun_analytics::is_enabled as *const () as usize;

        const TOTAL: usize = 1024 * 1024;

        let path = format!("/tmp/bao-uloop-paused-eof-test-{}.sock", std::process::id());
        let mut path_bytes = path.clone().into_bytes();
        path_bytes.push(0);
        let _ = std::fs::remove_file(&path);

        let loop_ = unsafe {
            us_create_loop(
                ptr::null_mut(),
                Some(noop_cb),
                Some(noop_cb),
                Some(noop_cb),
                0,
            )
        };
        assert!(!loop_.is_null(), "us_create_loop failed");

        let group: &'static mut SocketGroup = Box::leak(Box::new(SocketGroup::default()));
        group.init(loop_, Some(&VTABLE), ptr::null_mut());

        let mut err: c_int = 0;
        let ls: *mut ListenSocket = group.listen_unix(
            SocketKind::Dynamic,
            None,
            &path_bytes,
            0,
            0,
            &mut err,
        );
        assert!(!ls.is_null(), "listen_unix failed, err = {err}");

        for c in [&OPEN_COUNT, &DATA_BYTES, &END_COUNT, &CLOSE_COUNT] {
            c.store(0, Ordering::SeqCst);
        }

        let zero = Timespec { sec: 0, nsec: 0 };
        let mut client = std::os::unix::net::UnixStream::connect(&path).expect("connect");
        // Pump until the server side accepted (on_open ran, socket known).
        for _ in 0..200 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if OPEN_COUNT.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(OPEN_COUNT.load(Ordering::SeqCst), 1, "accept never ran");

        let sock = SOCKET_PTR.load(Ordering::SeqCst);
        assert!(!sock.is_null());

        // Send our FIN first: the truncated case is a socket that already
        // end()ed and then pauses (`!us_socket_is_shut_down` exemption).
        unsafe { us_internal_socket_raw_shutdown(sock) };

        // Peer writes 1 MiB (fits in the enlarged buffers, so the write and
        // the subsequent close → FIN both complete) then goes away.
        let writer = std::thread::spawn(move || {
            let sz: c_int = 4 * 1024 * 1024;
            unsafe {
                libc::setsockopt(
                    client.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &sz as *const _ as *const c_void,
                    core::mem::size_of::<c_int>() as u32,
                );
            }
            let payload = vec![b'x'; TOTAL];
            let _ = std::io::Write::write_all(&mut client, &payload);
            drop(client); // close → FIN behind the data
        });

        // First readable dispatch: one on_data (≤ LIBUS_RECV_BUFFER_LENGTH),
        // which pauses. The rest of the payload stays queued.
        for _ in 0..200 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if DATA_BYTES.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            DATA_BYTES.load(Ordering::SeqCst) > 0,
            "first data chunk never arrived"
        );

        // Parked phase: the peer's FIN (EPOLLHUP, both directions down) must
        // be deferred — no close, no end, tail undelivered. Pre-fix the
        // is_shut_down exemption closed here and lost the tail.
        for _ in 0..50 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            CLOSE_COUNT.load(Ordering::SeqCst),
            0,
            "eof closed a paused shut-down socket instead of deferring (tail lost)"
        );
        assert_eq!(END_COUNT.load(Ordering::SeqCst), 0);
        assert!(
            DATA_BYTES.load(Ordering::SeqCst) < TOTAL,
            "paused socket must not have drained the whole stream"
        );

        // Resume: the poll is re-registered (the parked fd was DELed), the
        // read loop drains the tail to recv()==0, and only then does the
        // shut-down socket close — clean, with every byte delivered.
        unsafe { us_socket_resume(sock) };
        for _ in 0..400 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if CLOSE_COUNT.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        writer.join().expect("writer thread");

        assert_eq!(
            DATA_BYTES.load(Ordering::SeqCst),
            TOTAL,
            "stream truncated: paused shut-down socket lost part of the peer's data"
        );
        assert_eq!(
            END_COUNT.load(Ordering::SeqCst),
            0,
            "shut-down sockets close without on_end"
        );
        assert_eq!(
            CLOSE_COUNT.load(Ordering::SeqCst),
            1,
            "socket must close after the drain (recv()==0 → eof → clean close)"
        );

        // Teardown.
        unsafe {
            (*ls).close();
            us_loop_run_bun_tick(loop_, &zero);
            SocketGroup::destroy(group);
            us_loop_free(loop_);
        }
        let _ = std::fs::remove_file(&path);
    }

    use std::os::fd::AsRawFd;
}

#[cfg(test)]
mod write_rearm_paused_tests {
    //! Behavioral test for the `us_internal_rearm_writable` fix: a write that
    //! fails to send everything on a read-paused socket must re-arm WRITABLE
    //! only. The old unconditional `READABLE | WRITABLE` re-arm in
    //! us_socket_write/raw_write/write2/ipc_write_fd silently undid
    //! us_socket_pause mid-backpressure and delivered inbound data the caller
    //! asked to defer. Drives the C path end-to-end.

    use super::*;
    use bun_uws_sys::socket_group::VTable;
    use bun_uws_sys::{ListenSocket, SocketGroup, SocketKind, us_socket_t};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static DATA_BYTES: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);
    /// Raw pointer stash of the accepted socket (test thread is the only loop
    /// thread; touched only between ticks).
    static SOCKET_PTR: std::sync::atomic::AtomicPtr<us_socket_t> =
        std::sync::atomic::AtomicPtr::new(ptr::null_mut());

    unsafe extern "C" {
        fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec);
        fn us_socket_get_fd(s: *mut us_socket_t) -> c_int;
        fn us_socket_write(s: *mut us_socket_t, data: *const u8, length: c_int) -> c_int;
        fn us_socket_pause(s: *mut us_socket_t);
        fn us_socket_resume(s: *mut us_socket_t);
    }

    unsafe extern "C" fn t_open(
        s: *mut us_socket_t,
        _is_client: c_int,
        _ip: *mut u8,
        _ip_len: c_int,
    ) -> *mut us_socket_t {
        OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
        SOCKET_PTR.store(s, Ordering::SeqCst);
        // Shrink the send buffer so outbound backpressure (write < length)
        // hits after a few hundred KiB instead of filling defaults.
        unsafe {
            let fd = us_socket_get_fd(s);
            let sz: c_int = 8 * 1024;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &sz as *const _ as *const c_void,
                core::mem::size_of::<c_int>() as u32,
            );
        }
        s
    }

    unsafe extern "C" fn t_data(
        s: *mut us_socket_t,
        _data: *mut u8,
        length: c_int,
    ) -> *mut us_socket_t {
        let first = DATA_BYTES.fetch_add(length as usize, Ordering::SeqCst) == 0;
        if first {
            // Read pause mid-stream: the rest of the peer's writes stay in
            // the kernel until resume.
            unsafe { us_socket_pause(s) };
        }
        s
    }

    unsafe extern "C" fn t_close(
        s: *mut us_socket_t,
        _code: c_int,
        _reason: *mut c_void,
    ) -> *mut us_socket_t {
        CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    static VTABLE: VTable = VTable {
        on_open: Some(t_open),
        on_data: Some(t_data),
        on_fd: None,
        on_writable: None,
        on_close: Some(t_close),
        on_timeout: None,
        on_long_timeout: None,
        on_end: None,
        on_connect_error: None,
        on_connecting_error: None,
        on_handshake: None,
    };

    unsafe extern "C" fn noop_cb(_loop: *mut Loop) {}

    #[test]
    fn failed_write_does_not_resume_paused_socket() {
        // Same link pulls as the hangup test (C archive deps).
        let _ = bun_lsquic_sys::force_link as *const () as usize;
        let _ = bun_lsquic_sys::force_link_lshpack as *const () as usize;
        let _ = bun_threading::Mutex::new as *const () as usize;
        let _ = bun_analytics::is_enabled as *const () as usize;

        let path = format!("/tmp/bao-uloop-write-rearm-test-{}.sock", std::process::id());
        let mut path_bytes = path.clone().into_bytes();
        path_bytes.push(0);
        let _ = std::fs::remove_file(&path);

        let loop_ = unsafe {
            us_create_loop(
                ptr::null_mut(),
                Some(noop_cb),
                Some(noop_cb),
                Some(noop_cb),
                0,
            )
        };
        assert!(!loop_.is_null(), "us_create_loop failed");

        let group: &'static mut SocketGroup = Box::leak(Box::new(SocketGroup::default()));
        group.init(loop_, Some(&VTABLE), ptr::null_mut());

        let mut err: c_int = 0;
        let ls: *mut ListenSocket = group.listen_unix(
            SocketKind::Dynamic,
            None,
            &path_bytes,
            0,
            0,
            &mut err,
        );
        assert!(!ls.is_null(), "listen_unix failed, err = {err}");

        for c in [&OPEN_COUNT, &DATA_BYTES, &CLOSE_COUNT] {
            c.store(0, Ordering::SeqCst);
        }

        let zero = Timespec { sec: 0, nsec: 0 };
        let mut client = std::os::unix::net::UnixStream::connect(&path).expect("connect");
        // Pump until the server side accepted (on_open ran, socket known).
        for _ in 0..200 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if OPEN_COUNT.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(OPEN_COUNT.load(Ordering::SeqCst), 1, "accept never ran");

        let sock = SOCKET_PTR.load(Ordering::SeqCst);
        assert!(!sock.is_null());

        // Inbound: first chunk delivers one on_data and pauses the read side.
        let first_chunk = vec![b'a'; 64 * 1024];
        std::io::Write::write_all(&mut client, &first_chunk).expect("first write");
        for _ in 0..200 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if DATA_BYTES.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let paused_bytes = DATA_BYTES.load(Ordering::SeqCst);
        assert!(paused_bytes > 0, "first data chunk never arrived");

        // Queue an inbound tail behind the pause (enlarged client SNDBUF so
        // the write completes into the kernel without blocking the test).
        let tail = vec![b'b'; 256 * 1024];
        unsafe {
            let sz: c_int = 4 * 1024 * 1024;
            libc::setsockopt(
                client.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &sz as *const _ as *const c_void,
                core::mem::size_of::<c_int>() as u32,
            );
        }
        std::io::Write::write_all(&mut client, &tail).expect("tail write");

        // Outbound backpressure on the paused socket: keep writing until a
        // write fails to send everything — this is the rearm path under test.
        let out = vec![b'x'; 64 * 1024];
        let mut backpressured = false;
        for _ in 0..128 {
            let written =
                unsafe { us_socket_write(sock, out.as_ptr(), out.len() as c_int) };
            if written < out.len() as c_int {
                backpressured = true;
                break;
            }
        }
        assert!(
            backpressured,
            "never hit a partial write: backpressure path not exercised"
        );

        // Discriminator: pump the loop. Pre-fix, the write-failure re-arm was
        // READABLE | WRITABLE, which undid the pause and delivered the queued
        // inbound tail (DATA_BYTES grows). Post-fix only WRITABLE is armed
        // and the paused read side stays deferred.
        for _ in 0..100 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            DATA_BYTES.load(Ordering::SeqCst),
            paused_bytes,
            "failed write re-armed READABLE and undid the read pause"
        );
        assert_eq!(CLOSE_COUNT.load(Ordering::SeqCst), 0);

        // Resume: READABLE comes back and the queued tail drains.
        unsafe { us_socket_resume(sock) };
        for _ in 0..400 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if DATA_BYTES.load(Ordering::SeqCst) >= first_chunk.len() + tail.len() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            DATA_BYTES.load(Ordering::SeqCst),
            first_chunk.len() + tail.len(),
            "resume did not deliver the inbound tail"
        );

        // Teardown.
        drop(client);
        for _ in 0..100 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if CLOSE_COUNT.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        unsafe {
            (*ls).close();
            us_loop_run_bun_tick(loop_, &zero);
            SocketGroup::destroy(group);
            us_loop_free(loop_);
        }
        let _ = std::fs::remove_file(&path);
    }

    use std::os::fd::AsRawFd;
}

#[cfg(test)]
mod ipc_recvmsg_tests {
    //! Behavioral test for upstream 3753c8bfc (the usockets part): the
    //! recvmsg control buffer must be sized with CMSG_SPACE (full aligned
    //! buffer — FreeBSD truncates and drops the fd with CMSG_LEN), received
    //! descriptors must be CLOEXEC (MSG_CMSG_CLOEXEC), and any extra
    //! descriptors a peer packed into one message must be closed, not leaked.
    //! On Linux the CMSG_LEN short buffer was tolerated, so the red/green
    //! discriminators here are CLOEXEC and extra-fd closing.

    use super::*;
    use bun_uws_sys::socket_group::VTable;
    use bun_uws_sys::{SocketGroup, SocketKind, us_socket_t};
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

    static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);
    static FD_COUNT: AtomicUsize = AtomicUsize::new(0);
    static RECEIVED_FD: AtomicI32 = AtomicI32::new(-1);
    static DATA_BYTES: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" {
        fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec);
        fn us_socket_from_fd(
            group: *mut SocketGroup,
            kind: u8,
            ssl_ctx: *mut c_void,
            socket_ext_size: c_int,
            fd: c_int,
            ipc: c_int,
        ) -> *mut us_socket_t;
    }

    unsafe extern "C" fn t_open(
        s: *mut us_socket_t,
        _is_client: c_int,
        _ip: *mut u8,
        _ip_len: c_int,
    ) -> *mut us_socket_t {
        OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_data(
        s: *mut us_socket_t,
        _data: *mut u8,
        length: c_int,
    ) -> *mut us_socket_t {
        DATA_BYTES.fetch_add(length as usize, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_fd(s: *mut us_socket_t, fd: c_int) -> *mut us_socket_t {
        FD_COUNT.fetch_add(1, Ordering::SeqCst);
        RECEIVED_FD.store(fd, Ordering::SeqCst);
        s
    }

    unsafe extern "C" fn t_close(
        s: *mut us_socket_t,
        _code: c_int,
        _reason: *mut c_void,
    ) -> *mut us_socket_t {
        CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
        s
    }

    static VTABLE: VTable = VTable {
        on_open: Some(t_open),
        on_data: Some(t_data),
        on_fd: Some(t_fd),
        on_writable: None,
        on_close: Some(t_close),
        on_timeout: None,
        on_long_timeout: None,
        on_end: None,
        on_connect_error: None,
        on_connecting_error: None,
        on_handshake: None,
    };

    unsafe extern "C" fn noop_cb(_loop: *mut Loop) {}

    /// Count open fds in /proc/self/fd referring to `inode` (0 or more).
    /// Inode-keyed so other test threads opening/closing their own fds
    /// cannot perturb the count. Pipes have per-fd unique inodes (unlike
    /// anon_inode-backed fds such as eventfd/epoll, which all share one).
    fn count_fds_for_inode(inode: u64) -> usize {
        let mut n = 0;
        let dir = std::fs::read_dir("/proc/self/fd").expect("read /proc/self/fd");
        for entry in dir.flatten() {
            let path = entry.path();
            if let Ok(fd_num) = path.file_name().unwrap().to_string_lossy().parse::<i32>() {
                let mut st: libc::stat = unsafe { core::mem::zeroed() };
                if unsafe { libc::fstat(fd_num, &mut st) } == 0 && st.st_ino == inode {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn ipc_recvmsg_delivers_fd_cloexec_and_closes_extras() {
        let _ = bun_lsquic_sys::force_link as *const () as usize;
        let _ = bun_lsquic_sys::force_link_lshpack as *const () as usize;
        let _ = bun_threading::Mutex::new as *const () as usize;
        let _ = bun_analytics::is_enabled as *const () as usize;

        let mut sv: [c_int; 2] = [-1, -1];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sv.as_mut_ptr()) },
            0,
            "socketpair failed"
        );

        let loop_ = unsafe {
            us_create_loop(
                ptr::null_mut(),
                Some(noop_cb),
                Some(noop_cb),
                Some(noop_cb),
                0,
            )
        };
        assert!(!loop_.is_null(), "us_create_loop failed");

        let group: &'static mut SocketGroup = Box::leak(Box::new(SocketGroup::default()));
        group.init(loop_, Some(&VTABLE), ptr::null_mut());

        // Adopt sv[0] as an IPC socket (is_ipc → loop.c recvmsg path).
        let sock = unsafe {
            us_socket_from_fd(
                group,
                SocketKind::Dynamic as u8,
                ptr::null_mut(),
                0,
                sv[0],
                1,
            )
        };
        assert!(!sock.is_null(), "us_socket_from_fd failed");

        for c in [&OPEN_COUNT, &FD_COUNT, &DATA_BYTES, &CLOSE_COUNT] {
            c.store(0, Ordering::SeqCst);
        }
        RECEIVED_FD.store(-1, Ordering::SeqCst);

        // First passed fd: a pipe read end carrying one known byte.
        let mut pipe_fds: [c_int; 2] = [-1, -1];
        assert_eq!(
            unsafe { libc::pipe(pipe_fds.as_mut_ptr()) },
            0,
            "pipe failed"
        );
        let z = b'Z';
        assert_eq!(
            unsafe { libc::write(pipe_fds[1], &z as *const u8 as *const c_void, 1) },
            1
        );
        // Extra fd packed into the same message: a second pipe's read end
        // (pipes have unique per-fifo inodes, unlike anon_inode-backed fds).
        let mut extra_pipe: [c_int; 2] = [-1, -1];
        assert_eq!(
            unsafe { libc::pipe(extra_pipe.as_mut_ptr()) },
            0,
            "extra pipe failed"
        );
        let extra_fd = extra_pipe[0];
        let mut extra_st: libc::stat = unsafe { core::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(extra_fd, &mut extra_st) }, 0);
        let extra_ino = extra_st.st_ino;

        // Send "x" + SCM_RIGHTS carrying [pipe_read, extra_fd] in ONE cmsghdr.
        #[repr(C, align(8))]
        struct Control([u8; 64]);
        let mut control = Control([0u8; 64]);
        let nfds: usize = 2;
        let sent = unsafe {
            let cm = control.0.as_mut_ptr() as *mut libc::cmsghdr;
            (*cm).cmsg_level = libc::SOL_SOCKET;
            (*cm).cmsg_type = libc::SCM_RIGHTS;
            (*cm).cmsg_len = libc::CMSG_LEN((nfds * core::mem::size_of::<c_int>()) as u32) as usize;
            let data = libc::CMSG_DATA(cm) as *mut c_int;
            *data = pipe_fds[0];
            *data.add(1) = extra_fd;

            let mut iov = libc::iovec {
                iov_base: b"x".as_ptr() as *mut c_void,
                iov_len: 1,
            };
            let mut msg: libc::msghdr = core::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.0.as_mut_ptr() as *mut c_void;
            msg.msg_controllen =
                libc::CMSG_SPACE((nfds * core::mem::size_of::<c_int>()) as u32) as usize;
            libc::sendmsg(sv[1], &msg, 0)
        };
        assert_eq!(sent, 1, "sendmsg failed");

        let zero = Timespec { sec: 0, nsec: 0 };
        for _ in 0..200 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if FD_COUNT.load(Ordering::SeqCst) == 1 && DATA_BYTES.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert_eq!(FD_COUNT.load(Ordering::SeqCst), 1, "on_fd never fired");
        assert_eq!(DATA_BYTES.load(Ordering::SeqCst), 1, "payload byte lost");
        let received = RECEIVED_FD.load(Ordering::SeqCst);
        assert!(received >= 0, "no fd delivered");

        // The delivered fd is the first passed one and carries the byte.
        let mut buf: [u8; 1] = [0];
        assert_eq!(
            unsafe { libc::read(received, buf.as_mut_ptr() as *mut c_void, 1) },
            1,
            "delivered fd is not the pipe read end"
        );
        assert_eq!(buf[0], b'Z');

        // MSG_CMSG_CLOEXEC: received descriptors must not leak into children
        // (pre-fix: plain recvmsg, no CLOEXEC → red).
        let flags = unsafe { libc::fcntl(received, libc::F_GETFD) };
        assert!(
            flags & libc::FD_CLOEXEC != 0,
            "received fd lacks FD_CLOEXEC (MSG_CMSG_CLOEXEC not applied)"
        );

        // The extra descriptor packed into the same message must have been
        // closed by the receiver, not leaked. We hold no reference to the
        // received dup ourselves: close our originals first, then no open fd
        // in the process may reference the extra pipe's inode.
        unsafe {
            libc::close(extra_fd);
            libc::close(extra_pipe[1]);
        }
        assert_eq!(
            count_fds_for_inode(extra_ino),
            0,
            "extra SCM_RIGHTS descriptor leaked (receiver must close beyond the first)"
        );

        // Teardown: close the peer so the IPC socket gets eof → close, then
        // free the loop and group (owner of the adopted socket).
        unsafe {
            libc::close(sv[1]);
        }
        for _ in 0..100 {
            unsafe { us_loop_run_bun_tick(loop_, &zero) };
            if CLOSE_COUNT.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Teardown: close the peer so the IPC socket gets eof → close, then
        // free the loop and group (owner of the adopted socket). extra_fd and
        // extra_pipe[1] were already closed above, before the inode scan.
        unsafe {
            libc::close(received);
            libc::close(pipe_fds[0]);
            libc::close(pipe_fds[1]);
            SocketGroup::destroy(group);
            us_loop_free(loop_);
        }
    }
}
