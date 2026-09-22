//! `bun_runtime::socket` — the real bodies behind `uws_sys`'s opaque
//! link-time-dispatch handles (cycle-break pattern: the low-tier crate holds
//! `opaque_extern!` handles + `extern "C"` declarations; this runtime crate
//! exports the `#[unsafe(no_mangle)]` definitions the linker resolves).
//!
//! win-cross, #18 (W7 link face): `WindowsNamedPipe` first — the windows
//! named-pipe socket face consumed by `uws_sys::socket`'s
//! `SocketUnion::Pipe` dispatch. Behavior parity with upstream
//! `src/runtime/socket/WindowsNamedPipe.zig` on the synchronous named-pipe
//! base shared with `ipc_channel`'s windows half (ruling (a), sync bridge —
//! ruling recorded 2026-09-22): upstream drives this class through bun.io's
//! async Writer/Reader; the sync base gives the same externally-visible
//! method semantics (state flags, write, flush, half-shutdown, timeout slot)
//! with blocking pipe I/O underneath.
//!
//! TLS: the `ssl`/`ssl_error` surface exists because the socket class is a
//! shared interface shape — named pipes never TLS-upgrade in the socket
//! module (upstream sets the SSL slot only on TLS-upgraded sockets), so the
//! slot stays `None` and `ssl_error` returns the zero `us_bun_verify_error_t`.

use ::std::io;
use bun_uws_sys::us_bun_verify_error_t;
use bun_windows_sys as w;
use bun_windows_sys::kernel32::{
    DisconnectNamedPipe, FlushFileBuffers, ReadFile, WriteFile,
};

/// Windows named-pipe socket — the real body of `uws_sys::WindowsNamedPipe`
/// (opaque `UnsafeCell<[u8; 0]>` handle there; ABI is a non-null pointer).
pub struct WindowsNamedPipe {
    handle: w::HANDLE,
    established: bool,
    closed: bool,
    shutdown_write: bool,
    shutdown_read: bool,
    paused: bool,
    /// TLS slot — see module doc. Always `None` for named pipes (parity).
    ssl: Option<*mut bun_boringssl_sys::SSL>,
    ssl_error: us_bun_verify_error_t,
    timeout_seconds: u32,
}

// SAFETY: a Win32 HANDLE has no thread affinity; the pipe handle is used
// under the owner's serialization (same contract as the unix UnixStream,
// which is Send+Sync).
unsafe impl Send for WindowsNamedPipe {}
unsafe impl Sync for WindowsNamedPipe {}

impl WindowsNamedPipe {
    /// Wrap an already-connected named-pipe HANDLE. Construction IS the
    /// established state (the caller hands us a connected pipe, mirroring
    /// upstream's `onConnect`/`onOpen` completion).
    pub fn from_handle(handle: w::HANDLE) -> Box<Self> {
        Box::new(Self {
            handle,
            established: true,
            closed: false,
            shutdown_write: false,
            shutdown_read: false,
            paused: false,
            ssl: None,
            ssl_error: us_bun_verify_error_t {
                error_no: 0,
                code: ::std::ptr::null(),
                reason: ::std::ptr::null(),
            },
            timeout_seconds: 0,
        })
    }

    fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut written: w::DWORD = 0;
            // SAFETY: synchronous WriteFile on a HANDLE we own; buffer/length
            // describe the caller's slice; out-param is a stack DWORD.
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    bytes.as_ptr(),
                    bytes.len() as w::DWORD,
                    &mut written,
                    ::std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "WindowsNamedPipe: write made no progress",
                ));
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }
}


// ── Link-time-dispatch methods (uws_sys/lib.rs extern declarations) ─────────
// Signatures must match `uws_sys`'s `unsafe extern "C"` block one-for-one.

/// `WindowsNamedPipe__ssl` — TLS slot query. Named pipes never TLS-upgrade;
/// parity: null.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__ssl(this: &WindowsNamedPipe) -> *mut bun_boringssl_sys::SSL {
    this.ssl.unwrap_or(::std::ptr::null_mut())
}

/// `WindowsNamedPipe__ssl_error` — last TLS handshake verification result.
/// Named pipes never TLS-upgrade; parity: zero-valued (no error).
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__ssl_error(
    this: &WindowsNamedPipe,
) -> us_bun_verify_error_t {
    this.ssl_error
}

/// `WindowsNamedPipe__is_established`.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__is_established(this: &WindowsNamedPipe) -> bool {
    this.established && !this.closed
}

/// `WindowsNamedPipe__is_closed`.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__is_closed(this: &WindowsNamedPipe) -> bool {
    this.closed
}

/// `WindowsNamedPipe__is_shutdown` — write side half-closed.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__is_shutdown(this: &WindowsNamedPipe) -> bool {
    this.shutdown_write
}

/// `WindowsNamedPipe__pause_stream` — stop the read side. Parity with
/// upstream `pauseStream`: `true` when the pipe exists and readStop
/// succeeded (no pipe → `false`).
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__pause_stream(this: &mut WindowsNamedPipe) -> bool {
    if this.closed {
        return false;
    }
    this.paused = true;
    true
}

/// `WindowsNamedPipe__resume_stream` — restart the read side. Parity with
/// upstream `resumeStream`: `false` when `readStart` fails (here: closed).
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__resume_stream(this: &mut WindowsNamedPipe) -> bool {
    if this.closed {
        return false;
    }
    this.paused = false;
    true
}

/// `WindowsNamedPipe__flush` — upstream flushes the TLS wrapper and the
/// async writer; the synchronous base writes straight through, so flush is
/// the pipe-level data drain.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__flush(this: &mut WindowsNamedPipe) {
    if !this.closed && !this.shutdown_write {
        // SAFETY: FlushFileBuffers on a HANDLE we own; failure (e.g. a pipe
        // peer race) is not fatal for a best-effort flush.
        unsafe { FlushFileBuffers(this.handle) };
    }
}

/// `WindowsNamedPipe__set_timeout` — store the inactivity timeout. The
/// synchronous base has no io-layer timer wheel; the stored value is the
/// state the runtime's timeout checks read (upstream `onTimeout` fires from
/// its io timer — tracked for the real-machine pass).
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__set_timeout(this: &mut WindowsNamedPipe, seconds: u32) {
    this.timeout_seconds = seconds;
}

/// `WindowsNamedPipe__encode_and_write` — TLS-encode then write. Named pipes
/// never TLS-upgrade, so this is the raw write; returns the accepted byte
/// count, or -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__encode_and_write(
    this: &mut WindowsNamedPipe,
    ptr: *const u8,
    len: usize,
) -> i32 {
    if this.closed || this.shutdown_write {
        return -1;
    }
    // SAFETY: caller contract — ptr/len describe a readable buffer.
    let bytes = unsafe { ::std::slice::from_raw_parts(ptr, len) };
    match this.write_all(bytes) {
        Ok(()) => len as i32,
        Err(_) => -1,
    }
}

/// `WindowsNamedPipe__raw_write` — unencoded write; same contract as
/// `encode_and_write`.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__raw_write(
    this: &mut WindowsNamedPipe,
    ptr: *const u8,
    len: usize,
) -> i32 {
    WindowsNamedPipe__encode_and_write(this, ptr, len)
}

/// `WindowsNamedPipe__shutdown` — half-close the write side: mark, then
/// drain (`FlushFileBuffers`) so the peer sees EOF after buffered data.
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__shutdown(this: &mut WindowsNamedPipe) {
    if this.closed {
        return;
    }
    this.shutdown_write = true;
    // SAFETY: FlushFileBuffers on a HANDLE we own.
    unsafe { FlushFileBuffers(this.handle) };
}

/// `WindowsNamedPipe__shutdown_read` — half-close the read side (the
/// consumer stops reading; the flag is the observable state).
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__shutdown_read(this: &mut WindowsNamedPipe) {
    this.shutdown_read = true;
}

/// `WindowsNamedPipe__close` — full teardown: sever then close the HANDLE.
/// Idempotent via the `closed` flag (the C++ wrapper's destructor and the
/// explicit close path both land here).
#[unsafe(no_mangle)]
pub extern "C" fn WindowsNamedPipe__close(this: &mut WindowsNamedPipe) {
    if this.closed {
        return;
    }
    this.closed = true;
    // SAFETY: HANDLE we own; sever-then-close is the documented pipe
    // teardown sequence.
    unsafe {
        DisconnectNamedPipe(this.handle);
        w::CloseHandle(this.handle);
    }
}
