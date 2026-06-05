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
#![allow(dead_code)] // BUG-353 fix: loop entry points now extern "C" from C/C++ libs.
                     // Internal helpers retained for poll.rs (FilePoll graft).
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
            let ms = ts.sec * 1000 + ts.nsec / 1_000_000;
            ms.min(i32::MAX as i64) as c_int
        }
    };

    let epfd = BAO_LOOP.with(|cell| {
        let slot = cell.borrow();
        slot.as_ref().filter(|s| ptr::eq(s.loop_ptr, loop_)).map(|s| s.epfd)
    });
    let Some(epfd) = epfd else { return };

    let loop_ptr: *mut PosixLoop = loop_;
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
//   - Bun__addrinfo_* stubs (DNS no-op for plain TCP)
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

    /// Single-iteration tick. Provided by libusockets.a (C, loop.c).
    pub unsafe fn us_loop_run_bun_tick(loop_: *mut Loop, timeout: *const Timespec);

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

// ──────────────── us_dispatch_* kind→vtable routing ──────────────────
// These are the socket event dispatchers called by libusockets (loop.c,
// socket.c, context.c). In Bun upstream they're implemented in Zig
// (src/runtime/socket/uws_dispatch.zig) and route by `s->kind` to the
// appropriate vtable handler (HTTP, WS, etc.). Bao implements the same
// routing logic: read `s.kind()` → for Invalid, panic → get
// `s.raw_group().vtable` → call the callback if present, else return `s`.

use bun_uws_sys::{SocketKind, us_socket_t, ConnectingSocket, us_bun_verify_error_t};
use bun_uws_sys::socket_group::VTable;

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
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_open {
                Some(cb) => cb(sock, is_client, ip, ip_length) as *mut c_void,
                None => s,
            }
        })
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
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_data {
                Some(cb) => cb(sock, data, length) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Socket received fd (IPC).
/// Routes to `group.vtable.on_fd` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_fd(s: *mut c_void, fd: c_int) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_fd {
                Some(cb) => cb(sock, fd) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Socket became writable.
/// Routes to `group.vtable.on_writable` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_writable(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_writable {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            }
        })
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
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_close {
                Some(cb) => cb(sock, code, reason) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Socket timed out.
/// Routes to `group.vtable.on_timeout` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_timeout(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_timeout {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Socket long-timeout.
/// Routes to `group.vtable.on_long_timeout` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_long_timeout(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_long_timeout {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Socket received FIN/EOF.
/// Routes to `group.vtable.on_end` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_end(s: *mut c_void) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_end {
                Some(cb) => cb(sock) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Established socket connect error.
/// Routes to `group.vtable.on_connect_error` if available, else returns `s`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_connect_error(s: *mut c_void, code: c_int) -> *mut c_void {
    unsafe {
        dispatch_via_vtable(s, || s, |vt, sock| {
            match vt.on_connect_error {
                Some(cb) => cb(sock, code) as *mut c_void,
                None => s,
            }
        })
    }
}

/// Connecting socket error.
/// Routes to `group.vtable.on_connecting_error` if available, else returns `c`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_dispatch_connecting_error(
    c: *mut c_void,
    code: c_int,
) -> *mut c_void {
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
        dispatch_via_vtable(s, || {}, |vt, sock| {
            if let Some(cb) = vt.on_handshake {
                cb(sock, success, err, core::ptr::null_mut());
            }
        })
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

// ──────────────── Bun__addrinfo_* stubs ────────────────────────────
// Async DNS resolution API. In Bun upstream, these are implemented in
// src/runtime/dns_jsc/dns.rs and backed by c-ares. For Bao's current
// plain-TCP mode without async DNS, we provide no-op stubs that always
// report "cache miss" (Bun__addrinfo_get returns -1) so the caller
// falls back to synchronous getaddrinfo.

/// Query the DNS cache. Returns -1 (cache miss) so the caller uses
/// synchronous resolution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_get(
    _loop: *mut c_void,
    _host: *const c_char,
    _port: u16,
    _ptr: *mut *mut c_void,
) -> c_int {
    -1
}

/// Associate a connecting socket with a DNS request. No-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_set(
    _ptr: *mut c_void,
    _socket: *mut c_void,
) -> c_int {
    0
}

/// Cancel a DNS request association. No-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_cancel(
    _ptr: *mut c_void,
    _socket: *mut c_void,
) -> c_int {
    0
}

/// Free a DNS request. No-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_freeRequest(
    _req: *mut c_void,
    _error: c_int,
) {
}

/// Get the result of a DNS request. Returns NULL (no result).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_getRequestResult(
    _req: *mut c_void,
) -> *mut c_void {
    ptr::null_mut()
}

/// Register QUIC address info callback. No-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__addrinfo_registerQuic(_req: *mut c_void, _pc: *mut c_void) {}

/// Bun HTTP date header timer optimization. No-op in plain TCP mode.
/// Called from us_internal_enable_sweep_timer in loop.c.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Bun__internal_ensureDateHeaderTimerIsEnabled(_loop: *mut c_void) {}

/// Force the linker to keep bao_uloop's `#[no_mangle] extern "C"` symbols.
/// BUG-353 fix: loop symbols (us_create_loop, uws_get_loop, etc.) are now
/// extern "C" imports from libusockets.a/libuwsockets.a — no need to force-link
/// them here. Only dispatch stubs and Bun__ addrinfo stubs remain local.
#[inline(never)]
pub fn force_link() {
    // Loop symbols: now extern "C" from C/C++ libs — no force needed.

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

    // Addrinfo stubs
    let _ = Bun__addrinfo_get as unsafe extern "C" fn(_, _, _, _) -> c_int;
    let _ = Bun__addrinfo_set as unsafe extern "C" fn(_, _) -> c_int;
    let _ = Bun__addrinfo_cancel as unsafe extern "C" fn(_, _) -> c_int;
    let _ = Bun__addrinfo_freeRequest as unsafe extern "C" fn(_, _);
    let _ = Bun__addrinfo_getRequestResult as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = Bun__addrinfo_registerQuic as unsafe extern "C" fn(_, _);

    // Bun internal stubs
    let _ = Bun__internal_ensureDateHeaderTimerIsEnabled as unsafe extern "C" fn(_);
}
// Tests removed: they tested the old Rust loop implementation that caused BUG-353.
// The C/C++ loop implementation is now tested via bao_runtime integration tests
// (uws_link_verification_tests, bun_api_tests, realworld_http_service_tests).
