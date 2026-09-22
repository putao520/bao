use core::ffi::c_int;

// ─── MOVE-IN: Winsize (TYPE_ONLY from bun_sys → bun_core) ─────────────────
// Zig: `std.posix.winsize` — used by output.rs::TERMINAL_SIZE. Field names
// match the move-out forward-ref in output.rs (row/col, not ws_row/ws_col).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Winsize {
    pub row: u16,
    pub col: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}
// SAFETY: four `u16` fields; all-zero is a valid `Winsize`.
unsafe impl crate::ffi::Zeroable for Winsize {}
// SAFETY: `#[repr(C)]` over four `u16` — exactly 8 bytes, no padding.
crate::unsafe_impl_atom!(Winsize);

#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Mode {
    Normal = 0,
    Raw = 1,
    Io = 2,
}

pub fn set_mode(fd: c_int, mode: Mode) -> c_int {
    #[cfg(windows)]
    {
        // Map the bao Mode onto the upstream windows tty contract:
        // Normal=0, Raw=1, Io=2 (Io behaves as Normal for the input flags).
        let upstream_mode = match mode {
            Mode::Raw => 1,
            Mode::Io => 2,
            Mode::Normal => 0,
        };
        let _ = upstream_mode;
        windows_set_tty_mode(fd, mode)
    }
    #[cfg(not(windows))]
    Bun__ttySetMode(fd, mode as c_int)
}

/// RAII guard: sets `fd` to [`Mode::Raw`] on construction and restores
/// [`Mode::Normal`] on `Drop`. Replaces the Zig
/// `defer { _ = bun.tty.set_mode(0, .Normal); }` pattern at call sites.
pub struct RawModeGuard {
    fd: c_int,
}

impl RawModeGuard {
    #[inline]
    pub fn new(fd: c_int) -> Self {
        let _ = set_mode(fd, Mode::Raw);
        Self { fd }
    }
}

impl Drop for RawModeGuard {
    #[inline]
    fn drop(&mut self) {
        let _ = set_mode(self.fd, Mode::Normal);
    }
}

// windows arm: SetConsoleMode over the std handle the fd selects (0=input,
// 1=output, 2=error). Raw clears line/echo/processed; Normal/Io set them.
// Output handles also get ENABLE_VIRTUAL_TERMINAL_PROCESSING so ANSI
// escapes render (node windows parity for the VT gate).
#[cfg(windows)]
mod win_tty {
    // GetStdHandle/GetConsoleMode/SetConsoleMode come from the
    // bun_windows_sys::kernel32 face (root-re-exported); only the console
    // mode flag consts are local.
    pub(super) use ::bun_windows_sys::kernel32::{
        GetConsoleMode, GetStdHandle, SetConsoleMode,
    };
    pub(super) const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    pub(super) const ENABLE_LINE_INPUT: u32 = 0x0002;
    pub(super) const ENABLE_ECHO_INPUT: u32 = 0x0004;
    pub(super) const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    pub(super) const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
}

#[cfg(windows)]
fn windows_set_tty_mode(fd: c_int, mode: Mode) -> c_int {
    use win_tty::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
        ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    };
    let selector: u32 = match fd {
        0 => 0xFFFF_FFF6, // STD_INPUT_HANDLE
        2 => 0xFFFF_FFF4, // STD_ERROR_HANDLE
        _ => 0xFFFF_FFF5, // STD_OUTPUT_HANDLE
    };
    let handle = GetStdHandle(selector);
    if handle.is_null() || handle as usize == usize::MAX {
        return -1;
    }
    let is_input = fd == 0;
    let mut current: u32 = 0;
    // SAFETY: handle is the GetStdHandle result (valid or skipped above).
    if unsafe { GetConsoleMode(handle, &mut current) } == 0 {
        // Not a console handle (pipe/file/redirected NUL): nothing to
        // mode-switch — report success, the flag is a no-op there.
        return 0;
    }
    let mut next = current;
    if is_input {
        const LINE_ECHO_PROCESSED: u32 =
            ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT;
        match mode {
            Mode::Raw => next &= !LINE_ECHO_PROCESSED,
            _ => next |= LINE_ECHO_PROCESSED,
        }
    } else {
        // Output handles: keep VT processing on for every mode (ANSI
        // rendering parity with the posix faces).
        next |= ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT;
    }
    // SAFETY: handle is the live console handle from GetConsoleMode above.
    if unsafe { SetConsoleMode(handle, next) } == 0 {
        return -1;
    }
    0
}

#[cfg(not(windows))]
unsafe extern "C" {
    safe fn Bun__ttySetMode(fd: c_int, mode: c_int) -> c_int;
}

// ported from: src/bun_core/tty.zig
