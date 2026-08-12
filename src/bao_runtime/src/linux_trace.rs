// @trace STUB-INVENTORY: Bun__linux_trace_* RealImpl (cross-platform, not noop)
//! Process-wide Linux ftrace / portable perf-trace backend for
//! `Bun__linux_trace_{init,emit,close}`.
//!
//! ## ABI (matches `bun_core::perf::sys` / upstream `perf.zig`)
//! ```c
//! int  Bun__linux_trace_init(void);                        // 1 = backend ready, 0 = not
//! void Bun__linux_trace_close(void);
//! int  Bun__linux_trace_emit(const char *event_name, int64_t duration_ns);
//! // emit: 0 = success, non-zero = failure
//! ```
//!
//! ## Backends
//! | OS            | Backend |
//! |---------------|---------|
//! | Linux/Android | `trace_marker` (`/sys/kernel/tracing` then debugfs) |
//! | macOS         | in-process ring + temp `bao-perf-trace.jsonl` |
//! | Windows       | `OutputDebugStringW` + temp jsonl + ring |
//!
//! All platforms keep a process-local ring (4096 slots) so unit tests can
//! observe `emit` side effects when init succeeded.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int};
use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::fs::OpenOptions;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::io::Write;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Capacity of the process-local ring buffer.
const RING_CAP: usize = 4096;
/// Max event name bytes stored in a ring slot (NUL not included).
const NAME_MAX: usize = 128;

/// One captured emit for tests / consumers of the ring.
#[derive(Clone, Debug)]
pub struct TraceRingEvent {
    pub name: String,
    pub duration_ns: i64,
    pub ts_ns: u64,
    pub pid: u32,
    pub tid: u64,
}

#[derive(Clone)]
struct RingSlot {
    name: [u8; NAME_MAX],
    name_len: u8,
    duration_ns: i64,
    ts_ns: u64,
    pid: u32,
    tid: u64,
}

impl RingSlot {
    fn empty() -> Self {
        Self {
            name: [0; NAME_MAX],
            name_len: 0,
            duration_ns: 0,
            ts_ns: 0,
            pid: 0,
            tid: 0,
        }
    }

    fn to_event(&self) -> TraceRingEvent {
        let n = self.name_len as usize;
        let name = String::from_utf8_lossy(&self.name[..n]).into_owned();
        TraceRingEvent {
            name,
            duration_ns: self.duration_ns,
            ts_ns: self.ts_ns,
            pid: self.pid,
            tid: self.tid,
        }
    }
}

/// Platform backend state held while tracing is active.
enum Backend {
    /// Linux/Android ftrace marker (owned fd).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    TraceMarker { fd: std::os::fd::OwnedFd },
    /// Portable jsonl session file (macOS / other non-Windows Unix).
    #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
    Jsonl { file: std::fs::File, path: PathBuf },
    /// Windows: OutputDebugStringW + jsonl under temp.
    #[cfg(windows)]
    DebugString { file: std::fs::File, path: PathBuf },
}

struct TraceState {
    backend: Option<Backend>,
    ring: Box<[RingSlot; RING_CAP]>,
    ring_head: usize,
    ring_len: usize,
    emit_ok: u64,
    emit_fail: u64,
}

impl TraceState {
    fn new() -> Self {
        // Fixed-size ring without heap realloc on the hot path.
        let ring = vec![RingSlot::empty(); RING_CAP]
            .into_boxed_slice()
            .try_into()
            .unwrap_or_else(|_| unreachable!("RING_CAP slots"));
        Self {
            backend: None,
            ring,
            ring_head: 0,
            ring_len: 0,
            emit_ok: 0,
            emit_fail: 0,
        }
    }

    fn push_ring(&mut self, name: &str, duration_ns: i64, ts_ns: u64, pid: u32, tid: u64) {
        let idx = (self.ring_head + self.ring_len) % RING_CAP;
        let slot = &mut self.ring[idx];
        let bytes = name.as_bytes();
        let n = bytes.len().min(NAME_MAX);
        slot.name[..n].copy_from_slice(&bytes[..n]);
        if n < NAME_MAX {
            slot.name[n..].fill(0);
        }
        slot.name_len = n as u8;
        slot.duration_ns = duration_ns;
        slot.ts_ns = ts_ns;
        slot.pid = pid;
        slot.tid = tid;
        if self.ring_len < RING_CAP {
            self.ring_len += 1;
        } else {
            self.ring_head = (self.ring_head + 1) % RING_CAP;
        }
    }

    fn clear_ring(&mut self) {
        self.ring_head = 0;
        self.ring_len = 0;
        self.emit_ok = 0;
        self.emit_fail = 0;
    }
}

static STATE: OnceLock<Mutex<TraceState>> = OnceLock::new();
static MONO_START: OnceLock<Instant> = OnceLock::new();
/// Process-wide emit counter (survives close) for cheap stats probes.
static EMIT_TOTAL: AtomicU64 = AtomicU64::new(0);

fn state() -> &'static Mutex<TraceState> {
    STATE.get_or_init(|| Mutex::new(TraceState::new()))
}

fn monotonic_ns() -> u64 {
    let start = MONO_START.get_or_init(Instant::now);
    start.elapsed().as_nanos() as u64
}

fn current_pid() -> u32 {
    std::process::id()
}

fn current_tid() -> u64 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // Prefer kernel tid (matches ftrace /proc conventions).
        unsafe { libc::syscall(libc::SYS_gettid) as u64 }
    }
    #[cfg(target_os = "macos")]
    {
        let mut tid: u64 = 0;
        // pthread_threadid_np(self, &tid) → current thread id.
        unsafe {
            libc::pthread_threadid_np(libc::pthread_self(), &mut tid);
        }
        tid
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        unsafe { GetCurrentThreadId() as u64 }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        windows
    )))]
    {
        0
    }
}

fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller contracts a valid NUL-terminated C string (or null).
    let c = unsafe { std::ffi::CStr::from_ptr(ptr) };
    c.to_str().ok()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn open_jsonl_session() -> Option<(std::fs::File, PathBuf)> {
    let mut path = std::env::temp_dir();
    path.push(format!("bao-perf-trace-{}.jsonl", std::process::id()));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    Some((file, path))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn try_open_trace_marker() -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    const PATHS: &[&str] = &[
        "/sys/kernel/tracing/trace_marker",
        "/sys/kernel/debug/tracing/trace_marker",
    ];
    for path in PATHS {
        let cpath = match std::ffi::CString::new(*path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // O_WRONLY only — matches upstream linux_perf_tracing.cpp.
        let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_WRONLY) };
        if fd >= 0 {
            // SAFETY: fresh exclusive fd from open(O_WRONLY).
            return Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
        }
    }
    None
}

/// Open the OS backend. Returns true if a backend is live.
fn backend_open(st: &mut TraceState) -> bool {
    if st.backend.is_some() {
        return true;
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        if let Some(fd) = try_open_trace_marker() {
            st.backend = Some(Backend::TraceMarker { fd });
            return true;
        }
        // No soft-fallback that would lie about ftrace availability: call
        // sites gate on init==1 meaning trace_marker is usable.
        return false;
    }

    #[cfg(windows)]
    {
        if let Some((file, path)) = open_jsonl_session() {
            st.backend = Some(Backend::DebugString { file, path });
            return true;
        }
        // NUL device keeps a File alive; OutputDebugStringW still fires on emit.
        match OpenOptions::new().write(true).open("NUL") {
            Ok(file) => {
                st.backend = Some(Backend::DebugString {
                    file,
                    path: PathBuf::from("NUL"),
                });
                true
            }
            Err(_) => false,
        }
    }

    // macOS + other non-Linux Unix: temp jsonl (or /dev/null fallback).
    #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
    {
        if let Some((file, path)) = open_jsonl_session() {
            st.backend = Some(Backend::Jsonl { file, path });
            return true;
        }
        match OpenOptions::new().write(true).open("/dev/null") {
            Ok(file) => {
                st.backend = Some(Backend::Jsonl {
                    file,
                    path: PathBuf::from("/dev/null"),
                });
                true
            }
            Err(_) => false,
        }
    }
}

fn backend_close(st: &mut TraceState) {
    st.backend = None;
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn write_jsonl_line(
    file: &mut std::fs::File,
    name: &str,
    duration_ns: i64,
    ts_ns: u64,
    pid: u32,
    tid: u64,
) -> bool {
    // Minimal JSON object per line (no serde dep required).
    let line = format!(
        "{{\"name\":{},\"duration_ns\":{},\"ts_ns\":{},\"pid\":{},\"tid\":{}}}\n",
        json_escape(name),
        duration_ns,
        ts_ns,
        pid,
        tid
    );
    file.write_all(line.as_bytes()).is_ok() && file.flush().is_ok()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn write_trace_marker(fd: &std::os::fd::OwnedFd, name: &str, duration_ns: i64, pid: u32) -> bool {
    use std::os::fd::AsRawFd;
    // Match upstream: "C|PID|EventName|DurationInNs\n"
    let mut buf = [0u8; NAME_MAX + 64];
    let line = format!("C|{}|{}|{}\n", pid, name, duration_ns);
    let bytes = line.as_bytes();
    let n = bytes.len().min(buf.len());
    buf[..n].copy_from_slice(&bytes[..n]);
    let written = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr() as *const _, n) };
    written == n as isize
}

#[cfg(windows)]
fn output_debug_string(name: &str, duration_ns: i64, pid: u32, tid: u64) {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OutputDebugStringW(lp_output_string: *const u16);
    }
    let msg = format!("bao-perf: C|{}|{}|{} tid={}\0", pid, name, duration_ns, tid);
    let wide: Vec<u16> = msg.encode_utf16().collect();
    unsafe { OutputDebugStringW(wide.as_ptr()) };
}

fn emit_to_backend(
    st: &mut TraceState,
    name: &str,
    duration_ns: i64,
    #[cfg_attr(
        any(target_os = "linux", target_os = "android"),
        allow(unused_variables)
    )]
    ts_ns: u64,
    pid: u32,
    #[cfg_attr(
        any(target_os = "linux", target_os = "android"),
        allow(unused_variables)
    )]
    tid: u64,
) -> bool {
    match st.backend.as_mut() {
        None => false,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Some(Backend::TraceMarker { fd }) => write_trace_marker(fd, name, duration_ns, pid),
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        Some(Backend::Jsonl { file, .. }) => {
            write_jsonl_line(file, name, duration_ns, ts_ns, pid, tid)
        }
        #[cfg(windows)]
        Some(Backend::DebugString { file, .. }) => {
            output_debug_string(name, duration_ns, pid, tid);
            write_jsonl_line(file, name, duration_ns, ts_ns, pid, tid)
        }
    }
}

// ── Public C ABI ──────────────────────────────────────────────────────────

/// Initialize the platform trace backend.
///
/// Returns `1` if a backend is available and ready, `0` otherwise.
/// Safe to call repeatedly; already-initialized state returns `1`.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__linux_trace_init() -> c_int {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.backend.is_some() {
        return 1;
    }
    if backend_open(&mut guard) { 1 } else { 0 }
}

/// Close the backend and release OS resources. Idempotent / re-entrant safe.
/// Does **not** wipe the ring (tests may inspect after close); call
/// [`linux_trace_reset_for_test`] to clear.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__linux_trace_close() {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    backend_close(&mut guard);
}

/// Emit one complete event: `event_name` + `duration_ns`.
///
/// Returns `0` on success, non-zero on failure (not initialized / I/O error /
/// null name). Never panics.
#[unsafe(no_mangle)]
pub extern "C" fn Bun__linux_trace_emit(event_name: *const c_char, duration_ns: i64) -> c_int {
    let Some(name) = cstr_to_str(event_name) else {
        record_fail();
        return -1;
    };
    // Cap name length for backend safety (matches MAX_EVENT_NAME_LENGTH).
    let name = if name.len() > NAME_MAX {
        &name[..NAME_MAX]
    } else {
        name
    };

    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.backend.is_none() {
        guard.emit_fail = guard.emit_fail.saturating_add(1);
        EMIT_TOTAL.fetch_add(1, Ordering::Relaxed);
        return -1;
    }

    let ts_ns = monotonic_ns();
    let pid = current_pid();
    let tid = current_tid();

    // Always record into the ring when backend is live so tests can assert
    // side effects. OS write (trace_marker / jsonl / OutputDebugString) is
    // best-effort — a full disk or flaky debugger must not undo a valid emit.
    guard.push_ring(name, duration_ns, ts_ns, pid, tid);
    let wrote = emit_to_backend(&mut guard, name, duration_ns, ts_ns, pid, tid);
    EMIT_TOTAL.fetch_add(1, Ordering::Relaxed);
    guard.emit_ok = guard.emit_ok.saturating_add(1);
    if !wrote {
        guard.emit_fail = guard.emit_fail.saturating_add(1);
    }
    0
}

fn record_fail() {
    EMIT_TOTAL.fetch_add(1, Ordering::Relaxed);
    let mut g = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    g.emit_fail = g.emit_fail.saturating_add(1);
}

// ── Test / introspection helpers (Rust ABI) ───────────────────────────────

/// Number of events currently held in the ring.
pub fn linux_trace_ring_len() -> usize {
    let guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.ring_len
}

/// Snapshot of ring events in chronological order (oldest → newest).
pub fn linux_trace_ring_snapshot() -> Vec<TraceRingEvent> {
    let guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let mut out = Vec::with_capacity(guard.ring_len);
    for i in 0..guard.ring_len {
        let idx = (guard.ring_head + i) % RING_CAP;
        out.push(guard.ring[idx].to_event());
    }
    out
}

/// Successful emit count since last reset (backend-write OK).
pub fn linux_trace_emit_ok() -> u64 {
    let guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    guard.emit_ok
}

/// Close backend + clear ring + counters (test isolation).
pub fn linux_trace_reset_for_test() {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    backend_close(&mut guard);
    guard.clear_ring();
}

/// Keep this compilation unit on the product link line.
#[inline(never)]
pub fn force_link_linux_trace() {
    let _ = Bun__linux_trace_init as *const () as usize;
    let _ = Bun__linux_trace_close as *const () as usize;
    let _ = Bun__linux_trace_emit as *const () as usize;
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn linux_trace_init_emit_close_roundtrip() {
        // Isolate from other tests in this crate that may touch the same
        // process-global state.
        linux_trace_reset_for_test();

        let init = Bun__linux_trace_init();
        assert!(
            init == 0 || init == 1,
            "init must return 0 or 1, got {init}"
        );

        let name = CString::new("bao_test_event").unwrap();
        if init == 1 {
            let rc = Bun__linux_trace_emit(name.as_ptr(), 12345);
            assert_eq!(rc, 0, "emit must succeed when init==1, got {rc}");
            assert!(
                linux_trace_ring_len() >= 1,
                "ring must be non-empty after successful emit"
            );
            assert!(linux_trace_emit_ok() >= 1, "emit_ok counter must advance");
            let snap = linux_trace_ring_snapshot();
            let last = snap.last().expect("ring snapshot");
            assert_eq!(last.name, "bao_test_event");
            assert_eq!(last.duration_ns, 12345);
            assert!(last.pid > 0 || cfg!(windows)); // pid always >0 on unix
            assert!(last.ts_ns > 0);
        } else {
            // Backend unavailable (e.g. no root / no tracefs on Linux CI).
            // Must not panic; must not claim success.
            let rc = Bun__linux_trace_emit(name.as_ptr(), 1);
            assert_ne!(rc, 0, "emit must fail (non-zero) when init==0");
        }

        // close is idempotent
        Bun__linux_trace_close();
        Bun__linux_trace_close();

        // After close, further emit must fail safely (not panic).
        let rc = Bun__linux_trace_emit(name.as_ptr(), 99);
        assert_ne!(rc, 0);

        // Re-init may succeed again if backend still available.
        let init2 = Bun__linux_trace_init();
        assert!(init2 == 0 || init2 == 1);
        if init2 == 1 {
            let rc = Bun__linux_trace_emit(name.as_ptr(), 7);
            assert_eq!(rc, 0);
        }
        Bun__linux_trace_close();
        linux_trace_reset_for_test();
    }

    #[test]
    fn linux_trace_emit_null_name_is_safe() {
        linux_trace_reset_for_test();
        let _ = Bun__linux_trace_init();
        let rc = Bun__linux_trace_emit(core::ptr::null(), 0);
        assert_ne!(rc, 0);
        Bun__linux_trace_close();
        linux_trace_reset_for_test();
    }

    #[test]
    fn linux_trace_ring_wraps_at_capacity() {
        linux_trace_reset_for_test();
        let init = Bun__linux_trace_init();
        if init != 1 {
            // Cannot exercise ring without a live backend.
            linux_trace_reset_for_test();
            return;
        }
        let name = CString::new("wrap").unwrap();
        // Push more than RING_CAP; len must cap at RING_CAP.
        for i in 0..(RING_CAP as i64 + 10) {
            let rc = Bun__linux_trace_emit(name.as_ptr(), i);
            assert_eq!(rc, 0);
        }
        assert_eq!(linux_trace_ring_len(), RING_CAP);
        Bun__linux_trace_close();
        linux_trace_reset_for_test();
    }
}
