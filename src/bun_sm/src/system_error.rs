//! System error — re-export of `bun_sys::SystemError` as a newtype wrapper.
//!
//! `bun_sys::SystemError` is the canonical `#[repr(C)]` struct matching the
//! Zig `extern struct` layout. We wrap it in a newtype to add JSC-specific
//! extension methods (`from_os_error`) without violating the orphan rule.

/// System error — wraps `bun_sys::SystemError` with JSC extension methods.
#[repr(transparent)]
pub struct SystemError(pub bun_sys::SystemError);

impl std::fmt::Debug for SystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemError")
            .field("errno", &self.0.errno)
            .finish_non_exhaustive()
    }
}

impl Default for SystemError {
    fn default() -> Self {
        SystemError(bun_sys::SystemError::default())
    }
}

impl SystemError {
    /// Create a `SystemError` from an OS error number, syscall, and path.
    pub fn from_os_error(errno: i32, syscall: &str, path: &str) -> Self {
        let mut inner = bun_sys::SystemError::default();
        inner.errno = -errno;
        inner.syscall = bun_core::String::clone_utf8(syscall.as_bytes());
        inner.path = bun_core::String::clone_utf8(path.as_bytes());
        inner.message = bun_core::String::clone_utf8(
            format!("{}: {}", syscall, errno).as_bytes()
        );
        SystemError(inner)
    }

    /// Create from a raw `std::io::Error`.
    pub fn from_io_error(err: &std::io::Error) -> Self {
        let errno = err.raw_os_error().unwrap_or(0);
        let msg = err.to_string();
        let mut inner = bun_sys::SystemError::default();
        inner.errno = -errno;
        inner.message = bun_core::String::clone_utf8(msg.as_bytes());
        SystemError(inner)
    }

    /// Access the inner `bun_sys::SystemError`.
    pub fn inner(&self) -> &bun_sys::SystemError {
        &self.0
    }
}

impl std::fmt::Display for SystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SystemError {}

impl From<bun_sys::SystemError> for SystemError {
    fn from(inner: bun_sys::SystemError) -> Self {
        SystemError(inner)
    }
}

impl From<SystemError> for bun_sys::SystemError {
    fn from(wrapper: SystemError) -> Self {
        wrapper.0
    }
}

/// JSC extension trait for SystemError — maps errno to JS Error types.
pub trait SysErrorJsc {
    /// Map errno to a JS Error class name.
    fn to_js_error_name(&self) -> &str;

    /// Map errno to a JS Error message prefix.
    fn to_js_error_prefix(&self) -> &str;

    /// Check if this is a "not found" error (ENOENT).
    fn is_not_found(&self) -> bool;

    /// Check if this is a permission error (EACCES/EPERM).
    fn is_permission_denied(&self) -> bool;

    /// Check if this is already exists (EEXIST).
    fn is_already_exists(&self) -> bool;
}

impl SysErrorJsc for SystemError {
    fn to_js_error_name(&self) -> &str {
        match self.0.errno {
            -2 | -4048 => "NotFoundError",
            -13 | -1 => "PermissionError",
            -17 => "AlreadyExistsError",
            -22 => "TypeError",
            -28 => "QuotaExceededError",
            -32 => "BrokenPipeError",
            _ => "SystemError",
        }
    }

    fn to_js_error_prefix(&self) -> &str {
        match self.0.errno {
            -2 => "ENOENT",
            -4048 => "ENOENT",
            -13 => "EACCES",
            -1 => "EPERM",
            -17 => "EEXIST",
            -22 => "EINVAL",
            -28 => "ENOSPC",
            -32 => "EPIPE",
            _ => "ERR",
        }
    }

    fn is_not_found(&self) -> bool {
        self.0.errno == -2 || self.0.errno == -4048
    }

    fn is_permission_denied(&self) -> bool {
        self.0.errno == -13 || self.0.errno == -1
    }

    fn is_already_exists(&self) -> bool {
        self.0.errno == -17
    }
}
