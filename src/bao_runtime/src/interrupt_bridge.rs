// @trace REQ-CLI-001 [entity:BaoRuntime] — SIGINT→cancel bridge wiring the
// SM-EVOLUTION #24 S1 ExecutionControl to the CLI product surface
// (ledger S1 legislation proposal consumed by user ruling 2026-09-10).
//!
//! # SIGINT → `ExecutionControl::cancel` bridge (self-pipe + watcher)
//!
//! ## Why a pipe — async-signal-safety of `cancel()`
//!
//! `ExecutionControl::cancel()` submits an atomic flag plus
//! `JS_RequestInterruptCallback(cx)`. That JSAPI entry is NOT
//! async-signal-safe: the vendored engine implements it as
//! `JSContext::requestInterrupt` (`js/src/vm/Runtime.cpp`), which takes the
//! `FutexThread` lock and calls `wasm::InterruptRunningCode` (more locks).
//! Calling it from inside a signal handler could deadlock against the owner
//! JS thread holding those locks. Therefore the handler body is only:
//!
//! - `write(pipe_wr, 1 byte)` — a POSIX async-signal-safe operation.
//!
//! A dedicated watcher thread (normal thread context) blocks on `read` of the
//! pipe and performs the real `cancel()` on the currently armed control. This
//! mirrors the S1 `ArmedExecutionGuard` deadline-watcher shape
//! (`bao_engine::execution_control.rs`): cross-thread submission is atomic +
//! the documented thread-safe interrupt request, never a JSObject.
//!
//! ## Existing signal surface interaction (checked, per task contract)
//!
//! - `Bun__registerSignalsForForwarding` (`product_native_symbols.rs`)
//!   installs its own SIGINT/SIGTERM/SIGHUP/SIGQUIT handlers around
//!   synchronous child spawns and restores `SIG_DFL` on unregister. A script
//!   that performs a sync spawn inside a `--timeout` run therefore REPLACES
//!   this bridge's handler for the rest of the process: SIGINT falls back to
//!   exactly the pre-bridge behavior (default disposition / child
//!   forwarding). Degradation to status-quo semantics, never corruption —
//!   documented, accepted for this slice.
//! - There is no `process.on('SIGINT')` dispatch machinery in bun_runtime
//!   today (constants table only), so no listener conflict exists.
//!
//! ## Swallowing guarantee
//!
//! A SIGINT that arrives while no control is armed (entry/exit edges of the
//! controlled run) is NOT silently eaten: the bridge records it, and `Drop`
//! restores the previous disposition and re-raises — reproducing the default
//! Ctrl-C kill the user expected.
//!
//! # Status: internal experimental surface
//!
//! `#[doc(hidden)]` — NOT a stable public API commitment (SM-EVOLUTION #24
//! S1 CLI wiring; one bridge per process, CLI lifecycle only).

use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bao_engine::execution_control::ExecutionControl;

/// Write end of the self-pipe, published for the signal handler.
/// -1 = no bridge installed. Relaxed load in the handler is sufficient: a
/// torn/stale read at worst misses one byte (the pipe is torn down only when
/// no handler can fire anymore).
static PIPE_WRITE_FD: AtomicI32 = AtomicI32::new(-1);

/// One bridge per process (CLI lifecycle). Fail-closed on double install.
static BRIDGE_ACTIVE: AtomicBool = AtomicBool::new(false);

struct Shared {
    /// Control currently armed for SIGINT cancellation. `None` = disarmed
    /// (SIGINT bytes land as `eaten_unhandled` instead).
    control: Mutex<Option<ExecutionControl>>,
    /// A SIGINT byte arrived with no armed control — Drop re-raises it.
    eaten_unhandled: AtomicBool,
}

/// The entire signal context: one async-signal-safe `write(2)`.
extern "C" fn bridge_sigint_handler(_sig: libc::c_int) {
    let fd = PIPE_WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte: u8 = 1;
        // SAFETY: write(2) on a pipe is async-signal-safe. Errors are
        // benign: EAGAIN means the pipe already holds a pending byte (the
        // watcher will wake), EINTR cannot occur for pipe writes of 1 byte
        // below PIPE_BUF on Linux.
        unsafe {
            let _ = libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
        }
    }
}

/// Watcher thread body: pipe byte → `cancel()` on the armed control, or mark
/// the SIGINT unhandled. Owns `read_fd` and closes it on exit (every path).
fn watcher_loop(read_fd: RawFd, shared: &Shared) {
    let mut byte = [0u8; 1];
    loop {
        // SAFETY: read(2) on our own pipe fd; the write end stays open until
        // Drop closes it, so EOF is a well-defined shutdown signal.
        let n = unsafe { libc::read(read_fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
        if n == 1 {
            let guard = shared.control.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(control) = guard.as_ref() {
                // In-flight controlled entry: the signal is consumed by the
                // engine-level cancel (uncatchable termination → Cancelled
                // terminal state). Normal thread context — cancel() may take
                // engine-internal locks safely here.
                control.cancel();
            } else {
                drop(guard);
                shared.eaten_unhandled.store(true, Ordering::Release);
            }
        } else if n == 0 {
            break; // write end closed — shutdown
        } else {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break; // EBADF (teardown) or unreadable pipe — exit; fd closed below.
        }
    }
    // SAFETY: read_fd is owned exclusively by this thread from here on.
    unsafe {
        libc::close(read_fd);
    }
}

/// SIGINT interception bridge. `install()` → `arm(&control)` before the
/// controlled entry → `disarm()` after it returns → `Drop` (or scope exit)
/// uninstalls the handler and re-raises any swallowed-but-unhandled SIGINT.
pub struct InterruptBridge {
    shared: Arc<Shared>,
    watcher: Option<JoinHandle<()>>,
    write_fd: RawFd,
    prev_action: libc::sigaction,
}

#[doc(hidden)]
impl InterruptBridge {
    /// Install the SIGINT handler + self-pipe + watcher thread. At most one
    /// bridge per process (panics otherwise — CLI lifecycle is singleton).
    pub fn install() -> Self {
        assert!(
            !BRIDGE_ACTIVE.swap(true, Ordering::AcqRel),
            "InterruptBridge: at most one bridge per process"
        );
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: valid out-pointer for two fds; O_CLOEXEC so exec'd children
        // never hold the write end (read-end EOF semantics stay correct).
        unsafe {
            assert_eq!(
                libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC),
                0,
                "InterruptBridge: pipe2 failed"
            );
        }
        let (read_fd, write_fd) = (fds[0], fds[1]);
        PIPE_WRITE_FD.store(write_fd, Ordering::Release);

        let shared = Arc::new(Shared {
            control: Mutex::new(None),
            eaten_unhandled: AtomicBool::new(false),
        });
        let watcher_shared = Arc::clone(&shared);
        let watcher = std::thread::Builder::new()
            .name("bao-sigint-bridge".to_string())
            .spawn(move || watcher_loop(read_fd, &watcher_shared))
            .expect("InterruptBridge: watcher spawn failed");

        // Install the handler, preserving the previous disposition for Drop.
        // SAFETY: sigaction with a valid oldact out-pointer.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = bridge_sigint_handler as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            // SA_RESTART (libuv-compatible): interrupted host syscalls
            // restart; the JS engine observes the cancel on its next loop
            // back-edge / JIT stack check. Scripts blocked in non-restartable
            // long syscalls terminate when the syscall next returns to JS —
            // the same limitation class the S1 deadline watcher documents.
            sa.sa_flags = libc::SA_RESTART;
            let mut prev: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGINT, &sa, &mut prev),
                0,
                "InterruptBridge: sigaction failed"
            );
            InterruptBridge {
                shared,
                watcher: Some(watcher),
                write_fd,
                prev_action: prev,
            }
        }
    }

    /// Arm: from now on, SIGINT cancels `control` (the in-flight controlled
    /// entry consumes the signal; see the module doc).
    pub fn arm(&self, control: &ExecutionControl) {
        *self.shared.control.lock().unwrap_or_else(|e| e.into_inner()) = Some(control.clone());
    }

    /// Disarm: SIGINTs beyond this point find no control and are re-raised
    /// on Drop (faithful default — Ctrl-C is never silently swallowed).
    pub fn disarm(&mut self) {
        *self.shared.control.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Drop for InterruptBridge {
    fn drop(&mut self) {
        // 1. Restore the previous SIGINT disposition FIRST — after this no
        //    new handler writes can originate from us.
        // SAFETY: prev_action was captured by our own sigaction install.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.prev_action, std::ptr::null_mut());
        }
        // 2. Detach the static fd and close the write end: the watcher
        //    drains any pending byte, then sees EOF, closes the read end and
        //    exits — every request it could make happened inside our frame.
        PIPE_WRITE_FD.store(-1, Ordering::Release);
        // SAFETY: write_fd is ours and is not touched after this point.
        unsafe {
            libc::close(self.write_fd);
        }
        if let Some(handle) = self.watcher.take() {
            let _ = handle.join();
        }
        BRIDGE_ACTIVE.store(false, Ordering::Release);
        // 3. Faithful default: re-raise a SIGINT we intercepted but could
        //    not deliver to an armed control. The handler is already
        //    uninstalled, so this hits the restored (usually default)
        //    disposition — the process dies by SIGINT exactly as it would
        //    have without the bridge.
        if self.shared.eaten_unhandled.load(Ordering::Acquire) {
            // SAFETY: raise(2) targets the calling process.
            unsafe {
                libc::raise(libc::SIGINT);
            }
        }
    }
}
