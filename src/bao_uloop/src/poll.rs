// @trace REQ-ENG-008 [entity:BaoPoll]
//! Wave 74-LOOP-C.2: us_poll_* internal poll ABI + tagged ready-event dispatch.
//!
//! Upstream C `us_poll_t` is a 4-byte struct with bitfields `{fd:27, poll_type:5}`
//! plus `alignas(LIBUS_EXT_ALIGNMENT)` (16) padding. It is the common header for
//! all pollable entities: sockets, listen sockets, callbacks (timers/asyncs).
//!
//! ## Dispatch model
//!
//! `epoll_event.data.ptr` carries either:
//!   - A **tagged pointer** (bits 49..63 set) → FilePoll, dispatched via
//!     `Bun__internal_dispatch_ready_poll`
//!   - An **untagged `us_poll_t*`** → dispatched via
//!     `us_internal_dispatch_ready_poll(poll, error, eof, events)` which
//!     routes by `poll_type & KIND_MASK`
//!
//! This matches the upstream C `CLEAR_POINTER_TAG` pattern exactly.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_uint, c_void};
use core::ptr;

use bun_uws_sys::{Loop, PosixLoop};

// ────────────────────────── poll type constants ──────────────────────────
// Must match upstream internal.h enum exactly.

/// Three low bits: the kind of pollable entity.
pub const POLL_TYPE_SOCKET: c_int = 0;
pub const POLL_TYPE_SOCKET_SHUT_DOWN: c_int = 1;
pub const POLL_TYPE_SEMI_SOCKET: c_int = 2;
pub const POLL_TYPE_CALLBACK: c_int = 3;
pub const POLL_TYPE_UDP: c_int = 4;

/// Two high bits: what events are being polled for.
pub const POLL_TYPE_POLLING_OUT: c_int = 8;
pub const POLL_TYPE_POLLING_IN: c_int = 16;

/// Mask to extract the kind (low 3 bits).
pub const POLL_TYPE_KIND_MASK: c_int = 0b111;
/// Mask to extract the polling direction (bits 3..5).
pub const POLL_TYPE_POLLING_MASK: c_int = 0b11000;

// ────────────────────────── us_poll_t ──────────────────────────────────

/// Rust mirror of the C `us_poll_t`. Layout:
/// ```c
/// struct us_poll_t {
///     alignas(LIBUS_EXT_ALIGNMENT) struct {
///         signed int fd : 27;
///         unsigned int poll_type : 5;
///     } state;
/// };
/// ```
///
/// The C struct is 4 bytes of bitfields + 12 bytes alignment padding = 16 bytes
/// total (with `alignas(16)`). We represent the bitfield as a single `u32`
/// and provide accessors.
///
/// **Invariant**: `fd` fits in i27 (−2²⁶..2²⁶−1), `poll_type` fits in u5 (0..31).
#[repr(C, align(16))]
pub struct BaoPoll {
    /// Bitfield: bits 0..27 = fd (signed), bits 27..32 = poll_type (unsigned).
    /// Stored as a single u32 to match the C bitfield layout.
    state: u32,
}

// Bitfield layout constants
const FD_BITS: u32 = 27;
const FD_MASK: u32 = (1u32 << FD_BITS) - 1; // 0x07FF_FFFF
const POLL_TYPE_SHIFT: u32 = FD_BITS;
const POLL_TYPE_MASK: u32 = !FD_MASK; // 0xF800_0000 (top 5 bits)

impl BaoPoll {
    /// Read the fd field (signed 27-bit).
    #[inline]
    pub fn fd(&self) -> c_int {
        let raw = (self.state & FD_MASK) as i32;
        // Sign-extend from 27 bits
        let shift = (32 - FD_BITS) as i32;
        (raw << shift) >> shift
    }

    /// Write the fd field.
    #[inline]
    pub fn set_fd(&mut self, fd: c_int) {
        // Store as unsigned 27-bit value
        self.state = (self.state & POLL_TYPE_MASK) | ((fd as u32) & FD_MASK);
    }

    /// Read the poll_type field (unsigned 5-bit).
    #[inline]
    pub fn poll_type(&self) -> c_int {
        ((self.state >> POLL_TYPE_SHIFT) as c_int) & 0x1F
    }

    /// Write the poll_type field.
    #[inline]
    pub fn set_poll_type(&mut self, pt: c_int) {
        self.state = (self.state & FD_MASK) | (((pt as u32) & 0x1F) << POLL_TYPE_SHIFT);
    }

    /// Returns the kind: `poll_type & POLL_TYPE_KIND_MASK`.
    #[inline]
    pub fn kind(&self) -> c_int {
        self.poll_type() & POLL_TYPE_KIND_MASK
    }

    /// Decode the polling events to epoll flags.
    /// `(POLLING_IN ? EPOLLIN : 0) | (POLLING_OUT ? EPOLLOUT : 0)`
    #[inline]
    pub fn events(&self) -> c_int {
        let pt = self.poll_type();
        ((pt & POLL_TYPE_POLLING_IN != 0) as c_int) * libc::EPOLLIN
            | ((pt & POLL_TYPE_POLLING_OUT != 0) as c_int) * libc::EPOLLOUT
    }

    /// Return a pointer to the trailing extension bytes.
    #[inline]
    pub fn ext(&self) -> *mut c_void {
        unsafe { (self as *const Self).add(1) as *mut c_void }
    }
}

// ────────────────── CLEAR_POINTER_TAG (upstream compat) ──────────────────

/// Mask to clear the tag bits (49..63) from a pointer.
/// Matches upstream `UNSET_BITS_49_UNTIL_64 = 0x0000FFFFFFFFFFFF`.
const UNSET_BITS_49_UNTIL_64: usize = 0x0000_FFFF_FFFF_FFFF;

/// Clear the tag bits from a pointer. If the result differs from the input,
/// the pointer is a tagged FilePoll pointer.
#[inline]
fn clear_pointer_tag(p: *mut c_void) -> *mut c_void {
    ((p as usize) & UNSET_BITS_49_UNTIL_64) as *mut c_void
}

/// Returns true if `p` is a tagged FilePoll pointer (high bits set).
#[inline]
fn is_tagged_pointer(p: *mut c_void) -> bool {
    clear_pointer_tag(p) != p
}

// ──────────────── FFI: FilePoll dispatch (from lib.rs) ────────────────

#[cfg(not(test))]
unsafe extern "C" {
    fn Bun__internal_dispatch_ready_poll(loop_: *mut Loop, tagged_pointer: *mut c_void);
}

#[cfg(test)]
#[allow(non_snake_case)]
unsafe extern "C" fn Bun__internal_dispatch_ready_poll(_loop_: *mut Loop, _tagged_pointer: *mut c_void) {}

// ──────────────── FFI: us_dispatch_* (from dispatch.zig) ────────────────
// These are the socket event dispatchers defined in the Zig/C++ layer.
// They route by `s->kind` to the appropriate handler (HTTP, WS, etc.).
// 74-C.3 wires BaoSocket to call these; for now we just declare the symbols.

#[allow(dead_code)] // Used by 74-C.3 socket dispatch
unsafe extern "C" {
    fn us_dispatch_open(s: *mut c_void, is_client: c_int, ip: *mut u8, ip_length: c_int) -> *mut c_void;
    fn us_dispatch_data(s: *mut c_void, data: *mut u8, length: c_int) -> *mut c_void;
    fn us_dispatch_writable(s: *mut c_void) -> *mut c_void;
    fn us_dispatch_close(s: *mut c_void, code: c_int, reason: *mut c_void) -> *mut c_void;
    fn us_dispatch_end(s: *mut c_void) -> *mut c_void;
    fn us_dispatch_connect_error(s: *mut c_void, code: c_int) -> *mut c_void;
}

// ──────────────── us_internal_callback_t (partial) ────────────────
// The C struct `us_internal_callback_t` extends `us_poll_t` with:
//   loop: *mut Loop
//   cb_expects_the_loop: c_int
//   leave_poll_ready: c_int
//   cb: fn pointer
//   has_added_timer_to_event_loop: c_uint (Linux)
//
// We only need to read `leave_poll_ready`, `cb_expects_the_loop`, `loop`, and `cb`
// from the dispatch path.

/// Callback function type for `us_internal_callback_t`.
pub type InternalCallbackFn = unsafe extern "C" fn(*mut BaoPoll);

/// Partial mirror of `us_internal_callback_t` — just enough to dispatch.
/// Layout: `BaoPoll` (16 bytes) + `loop` (8) + `cb_expects_the_loop` (4) +
/// `leave_poll_ready` (4) + `cb` (8) + `has_added_timer_to_event_loop` (4) + pad (4)
/// = 48 bytes on Linux.
#[repr(C)]
struct BaoInternalCallback {
    p: BaoPoll,
    loop_: *mut Loop,
    cb_expects_the_loop: c_int,
    leave_poll_ready: c_int,
    cb: Option<InternalCallbackFn>,
    has_added_timer_to_event_loop: c_uint,
}

// ──────────────── us_internal_dispatch_ready_poll ────────────────

/// Dispatch a ready `us_poll_t*` (untagged) by its poll type.
///
/// Matches upstream `loop.c:us_internal_dispatch_ready_poll`:
///   - CALLBACK: read eventfd if `!leave_poll_ready`, then call `cb`
///   - SEMI_SOCKET: listen socket accept loop or connect completion
///   - SOCKET / SOCKET_SHUT_DOWN: read/write/close dispatch
///
/// For now (74-C.2), CALLBACK is fully implemented. SEMI_SOCKET and SOCKET
/// are stubs that 74-C.3/74-C.4/74-C.5 will wire.
#[inline]
unsafe fn dispatch_ready_poll(poll: *mut BaoPoll, error: c_int, eof: c_int, events: c_int) {
    unsafe {
        let kind = (*poll).kind();

        match kind {
            POLL_TYPE_CALLBACK => {
                let cb_ptr = poll as *mut BaoInternalCallback;
                if (*cb_ptr).leave_poll_ready == 0 {
                    // Read the eventfd/timerfd to re-arm level-triggered.
                    accept_poll_event(poll);
                }
                if let Some(cb) = (*cb_ptr).cb {
                    if (*cb_ptr).cb_expects_the_loop != 0 {
                        cb((*cb_ptr).loop_ as *mut BaoPoll);
                    } else {
                        cb(poll);
                    }
                }
            }
            POLL_TYPE_SEMI_SOCKET => {
                // 74-C.4/74-C.5: listen socket accept loop / connect completion.
                // For now, no semi-socket registrations exist.
            }
            POLL_TYPE_SOCKET | POLL_TYPE_SOCKET_SHUT_DOWN => {
                // 74-C.3: socket read/write/close dispatch.
                // For now, no socket registrations exist.
                let _ = (error, eof, events);
            }
            POLL_TYPE_UDP => {
                // 74-C.x: UDP dispatch (deferred).
            }
            _ => {}
        }
    }
}

/// Read from the poll's fd (eventfd/timerfd) to re-arm level-triggered.
/// Matches upstream `us_internal_accept_poll_event`.
unsafe fn accept_poll_event(poll: *mut BaoPoll) -> u64 {
    let fd = unsafe { (*poll).fd() };
    let mut buf: u64 = 0;
    unsafe { libc::read(fd, &mut buf as *mut u64 as *mut c_void, 8); }
    buf
}

// ──────────────── FFI: us_poll_* ABI ────────────────

/// Allocate a new `us_poll_t` with trailing extension bytes.
/// `fallthrough=0` increments `loop->num_polls`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_create_poll(
    loop_: *mut Loop,
    fallthrough: c_int,
    ext_size: c_uint,
) -> *mut BaoPoll {
    let total = core::mem::size_of::<BaoPoll>() + ext_size as usize;
    let p = unsafe { libc::calloc(1, total) as *mut BaoPoll };
    if p.is_null() {
        return ptr::null_mut();
    }
    if fallthrough == 0 {
        unsafe {
            (*loop_).num_polls += 1;
        }
    }
    // Clear any tag bits that might be in the returned pointer
    // (matches upstream CLEAR_POINTER_TAG on the malloc result)
    clear_pointer_tag(p as *mut c_void) as *mut BaoPoll
}

/// Free a poll and decrement `loop->num_polls`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_free(p: *mut BaoPoll, loop_: *mut Loop) {
    if p.is_null() {
        return;
    }
    unsafe {
        (*loop_).num_polls -= 1;
        libc::free(p as *mut c_void);
    }
}

/// Initialize a poll's fd and poll_type fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_init(p: *mut BaoPoll, fd: c_int, poll_type: c_int) {
    unsafe {
        (*p).set_fd(fd);
        (*p).set_poll_type(poll_type);
    }
}

/// Return the fd.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_fd(p: *mut BaoPoll) -> c_int {
    unsafe { (*p).fd() }
}

/// Return the epoll events currently armed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_events(p: *mut BaoPoll) -> c_int {
    unsafe { (*p).events() }
}

/// Return the poll kind (low 3 bits of poll_type).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_internal_poll_type(p: *mut BaoPoll) -> c_int {
    unsafe { (*p).kind() }
}

/// Set the poll kind while preserving the polling direction bits.
/// Note: this is a *change* operation, not a *set* — the polling bits
/// are preserved from the existing value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_internal_poll_set_type(p: *mut BaoPoll, poll_type: c_int) {
    unsafe {
        let old = (*p).poll_type();
        (*p).set_poll_type(poll_type | (old & POLL_TYPE_POLLING_MASK));
    }
}

/// Return a pointer to the trailing extension bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_ext(p: *mut BaoPoll) -> *mut c_void {
    unsafe { p.add(1) as *mut c_void }
}

/// Register a poll into the epoll set with the given events.
/// Sets the polling bits in poll_type and calls `epoll_ctl(ADD)`.
///
/// `event.data.ptr = p` (untagged — the dispatch loop uses
/// `CLEAR_POINTER_TAG` to distinguish from FilePoll tagged pointers).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_start(p: *mut BaoPoll, loop_: *mut Loop, events: c_int) {
    unsafe { let _ = us_poll_start_rc(p, loop_, events); }
}

/// Same as `us_poll_start` but returns the epoll_ctl return code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_start_rc(p: *mut BaoPoll, loop_: *mut Loop, events: c_int) -> c_int {
    unsafe {
        // Set the polling direction bits in poll_type
        let kind = (*p).kind();
        let new_pt = kind
            | if events & libc::EPOLLIN != 0 { POLL_TYPE_POLLING_IN } else { 0 }
            | if events & libc::EPOLLOUT != 0 { POLL_TYPE_POLLING_OUT } else { 0 };
        (*p).set_poll_type(new_pt);

        let mut event: libc::epoll_event = core::mem::zeroed();
        let mut ev = events;
        // If neither readable nor writable, add error/hangup detection
        if ev & libc::EPOLLIN == 0 && ev & libc::EPOLLOUT == 0 {
            ev |= libc::EPOLLRDHUP | libc::EPOLLHUP | libc::EPOLLERR;
        }
        event.events = ev as u32;
        event.u64 = p as usize as u64;
        let epfd = (*loop_).fd;
        let mut ret: c_int;
        loop {
            ret = libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, (*p).fd(), &mut event);
            if ret != -1 || *libc::__errno_location() != libc::EINTR {
                break;
            }
        }
        ret
    }
}

/// Modify the events a poll is registered for.
/// Calls `epoll_ctl(MOD)` and updates pending ready polls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_change(p: *mut BaoPoll, loop_: *mut Loop, events: c_int) {
    unsafe {
        let old_events = (*p).events();
        if old_events == events {
            return;
        }

        // Update the polling direction bits
        let kind = (*p).kind();
        let new_pt = kind
            | if events & libc::EPOLLIN != 0 { POLL_TYPE_POLLING_IN } else { 0 }
            | if events & libc::EPOLLOUT != 0 { POLL_TYPE_POLLING_OUT } else { 0 };
        (*p).set_poll_type(new_pt);

        let mut event: libc::epoll_event = core::mem::zeroed();
        let mut ev = events;
        if ev & libc::EPOLLIN == 0 && ev & libc::EPOLLOUT == 0 {
            ev |= libc::EPOLLRDHUP | libc::EPOLLHUP | libc::EPOLLERR;
        }
        event.events = ev as u32;
        event.u64 = p as usize as u64;
        let epfd = (*loop_).fd;
        loop {
            let rc = libc::epoll_ctl(epfd, libc::EPOLL_CTL_MOD, (*p).fd(), &mut event);
            if rc != -1 || *libc::__errno_location() != libc::EINTR {
                break;
            }
        }

        // Update pending ready polls (null out removed events)
        update_pending_ready_polls(loop_, p, p, old_events, events);
    }
}

/// Remove a poll from the epoll set.
/// Calls `epoll_ctl(DEL)` and nulls the poll in pending ready events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_stop(p: *mut BaoPoll, loop_: *mut Loop) {
    unsafe {
        let old_events = (*p).events();
        let epfd = (*loop_).fd;
        let mut event: libc::epoll_event = core::mem::zeroed();
        loop {
            let rc = libc::epoll_ctl(epfd, libc::EPOLL_CTL_DEL, (*p).fd(), &mut event);
            if rc != -1 || *libc::__errno_location() != libc::EINTR {
                break;
            }
        }

        // Null out this poll in the pending ready poll list
        update_pending_ready_polls(loop_, p, ptr::null_mut(), old_events, 0);
    }
}

/// Resize a poll's extension area. If the new size is larger, realloc and
/// re-register with epoll. Returns the (possibly new) pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_poll_resize(
    p: *mut BaoPoll,
    loop_: *mut Loop,
    old_ext_size: c_uint,
    ext_size: c_uint,
) -> *mut BaoPoll {
    let old_size = core::mem::size_of::<BaoPoll>() + old_ext_size as usize;
    let new_size = core::mem::size_of::<BaoPoll>() + ext_size as usize;
    if new_size <= old_size {
        return p;
    }

    unsafe {
        let new_p = libc::calloc(1, new_size) as *mut BaoPoll;
        if new_p.is_null() {
            return p;
        }
        ptr::copy_nonoverlapping(p as *const u8, new_p as *mut u8, old_size);

        // Increment poll count for the new poll (the old one will be freed separately)
        (*loop_).num_polls += 1;

        let events = (*p).events();
        // Reset polling bits so us_poll_change re-registers cleanly
        let kind = (*new_p).kind();
        (*new_p).set_poll_type(kind);
        us_poll_change(new_p, loop_, events);

        // Update pending ready polls to point to new_p instead of p
        update_pending_ready_polls(loop_, p, new_p, events, events);

        new_p
    }
}

/// Read from the poll's fd (eventfd/timerfd) to re-arm level-triggered.
/// Matches upstream `us_internal_accept_poll_event`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn us_internal_accept_poll_event(p: *mut BaoPoll) -> usize {
    unsafe { accept_poll_event(p) as usize }
}

// ──────────────── pending ready poll update ────────────────

/// Update the `ready_polls` array when a poll is modified or removed.
/// Matches upstream `us_internal_loop_update_pending_ready_polls`.
///
/// On epoll, each poll appears at most once in `ready_polls`, so we only
/// need to scan from `current_ready_poll` onwards and update at most 1 entry.
unsafe fn update_pending_ready_polls(
    loop_: *mut Loop,
    old_poll: *mut BaoPoll,
    new_poll: *mut BaoPoll,
    _old_events: c_int,
    _new_events: c_int,
) {
    unsafe {
        let loop_ptr: *mut PosixLoop = loop_ as *mut PosixLoop;
        let num_ready = (*loop_ptr).num_ready_polls;
        let current = (*loop_ptr).current_ready_poll;

        // On epoll, at most 1 entry per poll
        let mut remaining = 1;
        let mut i = current;
        while i < num_ready && remaining > 0 {
            let event = (*loop_ptr).ready_polls[i as usize];
            let poll_ptr = event.u64 as usize as *mut BaoPoll;
            if poll_ptr == old_poll {
                (*loop_ptr).ready_polls[i as usize].u64 = new_poll as usize as u64;
                remaining -= 1;
            }
            i += 1;
        }
    }
}

// ──────────────── dispatch entry point ────────────────

/// Dispatch all ready polls from the `ready_polls` array.
/// Called from `run_epoll` after `epoll_wait` returns.
///
/// For each ready event:
///   1. Get `data.ptr` as `*mut BaoPoll`
///   2. If it's a tagged pointer (high bits set) → FilePoll dispatch
///   3. Otherwise → `us_internal_dispatch_ready_poll(poll, error, eof, events)`
///
/// This matches the upstream `us_internal_dispatch_ready_polls` function.
pub(crate) unsafe fn dispatch_ready_polls(loop_: *mut Loop) {
    let loop_ptr: *mut PosixLoop = loop_ as *mut PosixLoop;
    let num_ready = unsafe { (*loop_ptr).num_ready_polls };

    for i in 0..num_ready {
        unsafe {
            (*loop_ptr).current_ready_poll = i;
        }
        let event = unsafe { (*loop_ptr).ready_polls[i as usize] };
        let poll_ptr = event.u64 as usize as *mut c_void;

        if poll_ptr.is_null() {
            continue;
        }

        // Tagged pointer → FilePoll (Bun's own dispatch)
        if is_tagged_pointer(poll_ptr) {
            unsafe { Bun__internal_dispatch_ready_poll(loop_, poll_ptr); }
            continue;
        }

        // Untagged → us_poll_t dispatch
        let poll = poll_ptr as *mut BaoPoll;
        let events = event.events as c_int;
        let error = events & libc::EPOLLERR;
        let eof = events & libc::EPOLLHUP;
        let filtered_events = events & unsafe { (*poll).events() };

        if filtered_events != 0 || error != 0 || eof != 0 {
            unsafe { dispatch_ready_poll(poll, error, eof, filtered_events); }
        }
    }
}

// ──────────────── force_link ────────────────

/// Force the linker to keep bao_uloop's poll ABI symbols.
#[inline(never)]
pub fn force_link_poll() {
    let _ = us_create_poll as unsafe extern "C" fn(_, _, _) -> *mut BaoPoll;
    let _ = us_poll_free as unsafe extern "C" fn(_, _);
    let _ = us_poll_init as unsafe extern "C" fn(_, _, _);
    let _ = us_poll_fd as unsafe extern "C" fn(_) -> c_int;
    let _ = us_poll_events as unsafe extern "C" fn(_) -> c_int;
    let _ = us_internal_poll_type as unsafe extern "C" fn(_) -> c_int;
    let _ = us_internal_poll_set_type as unsafe extern "C" fn(_, _);
    let _ = us_poll_ext as unsafe extern "C" fn(_) -> *mut c_void;
    let _ = us_poll_start as unsafe extern "C" fn(_, _, _);
    let _ = us_poll_start_rc as unsafe extern "C" fn(_, _, _) -> c_int;
    let _ = us_poll_change as unsafe extern "C" fn(_, _, _);
    let _ = us_poll_stop as unsafe extern "C" fn(_, _);
    let _ = us_poll_resize as unsafe extern "C" fn(_, _, _, _) -> *mut BaoPoll;
    let _ = us_internal_accept_poll_event as unsafe extern "C" fn(_) -> usize;
}

// ──────────────── tests ────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ──── BaoPoll layout ────

    #[test]
    fn bao_poll_layout_is_16_bytes_aligned() {
        assert_eq!(core::mem::size_of::<BaoPoll>(), 16);
        assert_eq!(core::mem::align_of::<BaoPoll>(), 16);
    }

    #[test]
    fn bao_poll_fd_read_write() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_fd(42);
        assert_eq!(p.fd(), 42);
        p.set_fd(0);
        assert_eq!(p.fd(), 0);
        p.set_fd(-1);
        assert_eq!(p.fd(), -1);
    }

    #[test]
    fn bao_poll_fd_max_values() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        // Max positive 27-bit value: 2^26 - 1 = 67108863
        p.set_fd(67108863);
        assert_eq!(p.fd(), 67108863);
        // Min negative 27-bit value: -2^26 = -67108864
        p.set_fd(-67108864);
        assert_eq!(p.fd(), -67108864);
    }

    #[test]
    fn bao_poll_type_read_write() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_poll_type(POLL_TYPE_SOCKET);
        assert_eq!(p.poll_type(), POLL_TYPE_SOCKET);
        assert_eq!(p.kind(), POLL_TYPE_SOCKET);

        p.set_poll_type(POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
        assert_eq!(p.poll_type(), POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
        assert_eq!(p.kind(), POLL_TYPE_CALLBACK);
    }

    #[test]
    fn bao_poll_all_kinds() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        for &kind in &[POLL_TYPE_SOCKET, POLL_TYPE_SOCKET_SHUT_DOWN,
                       POLL_TYPE_SEMI_SOCKET, POLL_TYPE_CALLBACK, POLL_TYPE_UDP] {
            p.set_poll_type(kind);
            assert_eq!(p.kind(), kind, "kind must match for {kind}");
        }
    }

    #[test]
    fn bao_poll_events_decoding() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_IN);
        assert_eq!(p.events(), libc::EPOLLIN);

        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_OUT);
        assert_eq!(p.events(), libc::EPOLLOUT);

        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_IN | POLL_TYPE_POLLING_OUT);
        assert_eq!(p.events(), libc::EPOLLIN | libc::EPOLLOUT);

        p.set_poll_type(POLL_TYPE_SOCKET);
        assert_eq!(p.events(), 0);
    }

    // ──── tagged pointer ────

    #[test]
    fn clear_pointer_tag_identifies_tagged() {
        // A pointer with high bits set (simulating FilePoll tagged pointer)
        let tagged: *mut c_void = ((1usize << 49) | 0x1000) as *mut c_void;
        assert!(is_tagged_pointer(tagged));
        assert_eq!(clear_pointer_tag(tagged), 0x1000 as *mut c_void);

        // A normal pointer (no high bits set)
        let normal: *mut c_void = 0x1000 as *mut c_void;
        assert!(!is_tagged_pointer(normal));
        assert_eq!(clear_pointer_tag(normal), normal);
    }

    #[test]
    fn null_pointer_is_not_tagged() {
        assert!(!is_tagged_pointer(ptr::null_mut()));
    }

    #[test]
    fn clear_pointer_tag_is_idempotent() {
        let tagged: *mut c_void = ((1usize << 49) | 0x1000) as *mut c_void;
        let once = clear_pointer_tag(tagged);
        let twice = clear_pointer_tag(once);
        assert_eq!(once, twice);
    }

    // ──── us_internal_poll_set_type ────

    #[test]
    fn us_internal_poll_set_type_preserves_polling_bits() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_IN);
        // Change kind to SHUT_DOWN, should keep POLLING_IN
        unsafe { us_internal_poll_set_type(&mut p, POLL_TYPE_SOCKET_SHUT_DOWN); }
        assert_eq!(p.kind(), POLL_TYPE_SOCKET_SHUT_DOWN);
        assert_eq!(p.poll_type() & POLL_TYPE_POLLING_MASK, POLL_TYPE_POLLING_IN);
    }

    #[test]
    fn us_internal_poll_set_type_preserves_polling_out() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_poll_type(POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_OUT);
        unsafe { us_internal_poll_set_type(&mut p, POLL_TYPE_SOCKET); }
        assert_eq!(p.kind(), POLL_TYPE_SOCKET);
        assert_eq!(p.poll_type() & POLL_TYPE_POLLING_MASK, POLL_TYPE_POLLING_OUT);
    }

    // ──── us_create_poll / us_poll_free ────

    #[test]
    fn us_create_poll_returns_aligned_non_null() {
        let loop_ = super::super::uws_get_loop();
        let poll = unsafe { us_create_poll(loop_, 0, 0) };
        assert!(!poll.is_null());
        // Must be 16-byte aligned
        assert_eq!((poll as usize) % 16, 0, "poll must be 16-byte aligned");
        unsafe { us_poll_free(poll, loop_); }
    }

    #[test]
    fn us_create_poll_with_ext_size() {
        let loop_ = super::super::uws_get_loop();
        let ext = 64;
        let poll = unsafe { us_create_poll(loop_, 0, ext) };
        assert!(!poll.is_null());
        let ext_ptr = unsafe { us_poll_ext(poll) };
        assert!(!ext_ptr.is_null());
        // ext must be after the 16-byte BaoPoll header
        assert_eq!(ext_ptr as usize, poll as usize + 16);
        unsafe { us_poll_free(poll, loop_); }
    }

    #[test]
    fn us_create_poll_increments_num_polls() {
        let loop_ = super::super::uws_get_loop();
        let before = unsafe { (*loop_).num_polls };
        let poll = unsafe { us_create_poll(loop_, 0, 0) };
        let after = unsafe { (*loop_).num_polls };
        assert_eq!(after, before + 1, "fallthrough=0 must increment num_polls");
        unsafe { us_poll_free(poll, loop_); }
    }

    #[test]
    fn us_create_poll_fallthrough_skips_increment() {
        let loop_ = super::super::uws_get_loop();
        let before = unsafe { (*loop_).num_polls };
        let poll = unsafe { us_create_poll(loop_, 1, 0) };
        let after = unsafe { (*loop_).num_polls };
        assert_eq!(after, before, "fallthrough=1 must not increment num_polls");
        // Still need to free — but us_poll_free always decrements
        unsafe { us_poll_free(poll, loop_); }
    }

    // ──── us_poll_init ────

    #[test]
    fn us_poll_init_sets_fd_and_type() {
        let loop_ = super::super::uws_get_loop();
        let poll = unsafe { us_create_poll(loop_, 0, 0) };
        unsafe { us_poll_init(poll, 42, POLL_TYPE_CALLBACK); }
        assert_eq!(unsafe { us_poll_fd(poll) }, 42);
        assert_eq!(unsafe { us_internal_poll_type(poll) }, POLL_TYPE_CALLBACK);
        assert_eq!(unsafe { us_poll_events(poll) }, 0, "no polling direction set");
        unsafe { us_poll_free(poll, loop_); }
    }

    // ──── poll start/stop/change ────

    #[test]
    fn poll_start_stop_cycle() {
        let loop_ = super::super::uws_get_loop();
        let epfd = unsafe { (*loop_).fd };

        // Create a pipe — fd[0] is readable, fd[1] is writable
        let mut fds: [c_int; 2] = [-1; 2];
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(ret, 0, "pipe() failed");
        let rfd = fds[0];

        // Create and init a poll
        let poll = unsafe { us_create_poll(loop_, 0, 0) };
        assert!(!poll.is_null());
        unsafe { us_poll_init(poll, rfd, POLL_TYPE_CALLBACK); }
        assert_eq!(unsafe { us_poll_fd(poll) }, rfd);
        assert_eq!(unsafe { us_internal_poll_type(poll) }, POLL_TYPE_CALLBACK);

        // Start polling for readable
        unsafe { us_poll_start(poll, loop_, libc::EPOLLIN); }
        assert_eq!(unsafe { us_poll_events(poll) }, libc::EPOLLIN);

        // Verify it's registered in epoll by checking with epoll_ctl MOD (should succeed)
        let mut ev: libc::epoll_event = unsafe { core::mem::zeroed() };
        ev.events = libc::EPOLLIN as u32;
        ev.u64 = 999; // sentinel
        let rc = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_MOD, rfd, &mut ev) };
        assert_eq!(rc, 0, "epoll_ctl MOD should succeed after poll_start");

        // Stop polling
        unsafe { us_poll_stop(poll, loop_); }

        // Verify it's removed from epoll
        let rc2 = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_MOD, rfd, &mut ev) };
        assert!(rc2 != 0, "epoll_ctl MOD should fail after poll_stop");

        // Clean up
        unsafe { us_poll_free(poll, loop_); }
        unsafe { libc::close(fds[0]); }
        unsafe { libc::close(fds[1]); }
    }

    #[test]
    fn poll_change_updates_events() {
        let loop_ = super::super::uws_get_loop();

        let mut fds: [c_int; 2] = [-1; 2];
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(ret, 0);
        let rfd = fds[0];

        let poll = unsafe { us_create_poll(loop_, 0, 0) };
        assert!(!poll.is_null());
        unsafe { us_poll_init(poll, rfd, POLL_TYPE_CALLBACK); }
        unsafe { us_poll_start(poll, loop_, libc::EPOLLIN); }
        assert_eq!(unsafe { us_poll_events(poll) }, libc::EPOLLIN);

        // Change to poll for writable
        unsafe { us_poll_change(poll, loop_, libc::EPOLLOUT); }
        assert_eq!(unsafe { us_poll_events(poll) }, libc::EPOLLOUT);

        // Change to poll for both
        unsafe { us_poll_change(poll, loop_, libc::EPOLLIN | libc::EPOLLOUT); }
        assert_eq!(unsafe { us_poll_events(poll) }, libc::EPOLLIN | libc::EPOLLOUT);

        unsafe { us_poll_stop(poll, loop_); }
        unsafe { us_poll_free(poll, loop_); }
        unsafe { libc::close(fds[0]); }
        unsafe { libc::close(fds[1]); }
    }

    #[test]
    fn poll_start_rc_returns_zero_on_success() {
        let loop_ = super::super::uws_get_loop();
        let mut fds: [c_int; 2] = [-1; 2];
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(ret, 0);
        let rfd = fds[0];

        let poll = unsafe { us_create_poll(loop_, 0, 0) };
        unsafe { us_poll_init(poll, rfd, POLL_TYPE_CALLBACK); }
        let rc = unsafe { us_poll_start_rc(poll, loop_, libc::EPOLLIN) };
        assert_eq!(rc, 0, "us_poll_start_rc must return 0 on success");

        unsafe { us_poll_stop(poll, loop_); }
        unsafe { us_poll_free(poll, loop_); }
        unsafe { libc::close(fds[0]); }
        unsafe { libc::close(fds[1]); }
    }

    #[test]
    fn poll_free_with_null_is_no_op() {
        let loop_ = super::super::uws_get_loop();
        unsafe { us_poll_free(ptr::null_mut(), loop_); }
    }

    #[test]
    fn poll_resize_same_size_returns_same_pointer() {
        let loop_ = super::super::uws_get_loop();
        let poll = unsafe { us_create_poll(loop_, 0, 32) };
        unsafe { us_poll_init(poll, -1, POLL_TYPE_CALLBACK); }
        // Resize to same size → should return same pointer
        let new_p = unsafe { us_poll_resize(poll, loop_, 32, 32) };
        assert_eq!(new_p, poll, "resize to same size must return same pointer");
        unsafe { us_poll_free(poll, loop_); }
    }

    #[test]
    fn poll_resize_larger_returns_new_pointer() {
        let loop_ = super::super::uws_get_loop();
        let poll = unsafe { us_create_poll(loop_, 0, 16) };
        unsafe { us_poll_init(poll, -1, POLL_TYPE_CALLBACK); }
        // Resize to larger → should return different pointer
        let new_p = unsafe { us_poll_resize(poll, loop_, 16, 64) };
        // new_p may or may not differ (depends on allocator), but must be non-null
        assert!(!new_p.is_null(), "resize must return non-null");
        // Don't free old poll — resize already accounted for it
        unsafe { us_poll_free(new_p, loop_); }
    }

    // ──── BaoPoll edge cases ────────────────────────────────────────
    // @trace REQ-ENG-008 [req:REQ-ENG-008] [level:unit]

    #[test]
    fn bao_poll_fd_and_poll_type_independent() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_fd(100);
        p.set_poll_type(POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
        assert_eq!(p.fd(), 100);
        assert_eq!(p.poll_type(), POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
        // Change fd, poll_type should be preserved
        p.set_fd(200);
        assert_eq!(p.fd(), 200);
        assert_eq!(p.poll_type(), POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
        // Change poll_type, fd should be preserved
        p.set_poll_type(POLL_TYPE_SOCKET);
        assert_eq!(p.fd(), 200);
        assert_eq!(p.poll_type(), POLL_TYPE_SOCKET);
    }

    #[test]
    fn bao_poll_fd_negative_values() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        p.set_fd(-1);
        assert_eq!(p.fd(), -1);
        p.set_fd(-100);
        assert_eq!(p.fd(), -100);
        p.set_fd(-67108864); // min i27
        assert_eq!(p.fd(), -67108864);
    }

    #[test]
    fn bao_poll_poll_type_max_5bit() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        // Max 5-bit value: 31
        p.set_poll_type(31);
        assert_eq!(p.poll_type(), 31);
        // Values > 31 are masked to 5 bits
        p.set_poll_type(32);
        assert_eq!(p.poll_type(), 0, "32 & 0x1F = 0");
        p.set_poll_type(33);
        assert_eq!(p.poll_type(), 1, "33 & 0x1F = 1");
    }

    #[test]
    fn bao_poll_kind_masks_low_3_bits() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        // poll_type = 7 (0b111) → kind = 7 & 0b111 = 7
        p.set_poll_type(7);
        assert_eq!(p.kind(), 7);
        // poll_type = POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN = 3 | 16 = 19
        p.set_poll_type(POLL_TYPE_CALLBACK | POLL_TYPE_POLLING_IN);
        assert_eq!(p.kind(), POLL_TYPE_CALLBACK);
    }

    #[test]
    fn bao_poll_events_all_combinations() {
        let mut p: BaoPoll = unsafe { core::mem::zeroed() };
        // No polling bits
        p.set_poll_type(POLL_TYPE_SOCKET);
        assert_eq!(p.events(), 0);
        // POLLING_IN only
        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_IN);
        assert_eq!(p.events(), libc::EPOLLIN);
        // POLLING_OUT only
        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_OUT);
        assert_eq!(p.events(), libc::EPOLLOUT);
        // Both
        p.set_poll_type(POLL_TYPE_SOCKET | POLL_TYPE_POLLING_IN | POLL_TYPE_POLLING_OUT);
        assert_eq!(p.events(), libc::EPOLLIN | libc::EPOLLOUT);
    }

    #[test]
    fn bao_poll_ext_returns_pointer_after_header() {
        let loop_ = super::super::uws_get_loop();
        let poll = unsafe { us_create_poll(loop_, 0, 32) };
        let ext = unsafe { us_poll_ext(poll) };
        assert!(!ext.is_null());
        assert_eq!(ext as usize, poll as usize + 16, "ext must be 16 bytes after poll");
        unsafe { us_poll_free(poll, loop_); }
    }

    // ──── tagged pointer edge cases ─────────────────────────────────
    // @trace REQ-ENG-008 [req:REQ-ENG-008] [level:unit]

    #[test]
    fn clear_pointer_tag_preserves_low_49_bits() {
        // A pointer with all bits set
        let all_bits: *mut c_void = usize::MAX as *mut c_void;
        let cleared = clear_pointer_tag(all_bits);
        // UNSET_BITS_49_UNTIL_64 = 0x0000_FFFF_FFFF_FFFF
        // This clears bits 49..63 (15 high bits)
        let expected: usize = 0x0000_FFFF_FFFF_FFFF;
        assert_eq!(cleared as usize, expected);
    }

    #[test]
    fn is_tagged_pointer_various_tags() {
        let base: usize = 0x1000;
        // Tag in bit 49 → tagged
        let tagged: *mut c_void = (base | (1usize << 49)) as *mut c_void;
        assert!(is_tagged_pointer(tagged));
        // Tag in bit 63 → tagged
        let high_tag: *mut c_void = (base | (1usize << 63)) as *mut c_void;
        assert!(is_tagged_pointer(high_tag));
        // No high bits → not tagged
        let plain: *mut c_void = base as *mut c_void;
        assert!(!is_tagged_pointer(plain));
    }

    #[test]
    fn clear_pointer_tag_roundtrip_with_re_encode() {
        let ptr = 0x5000 as *mut c_void;
        let tagged: *mut c_void = ((1usize << 49) | (ptr as usize)) as *mut c_void;
        let cleared = clear_pointer_tag(tagged);
        assert_eq!(cleared, ptr);
    }
}
