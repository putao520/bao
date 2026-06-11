//! UpgradedDuplex C FFI — real TLS implementations via `TlsConnection`.
//!
//! These `#[unsafe(no_mangle)]` functions replace the stubs in `bao_native_stubs`.
//! They are called by `bun_uws_sys` through the `UpgradedDuplex` opaque handle,
//! which in Bao holds a `Box<TlsConnection>` pointer.

use core::ffi::c_void;
use std::ffi::c_int;
use std::ffi::c_uint;

use crate::connection::{TlsConnection, TlsError};

/// Verify error structure matching `us_bun_verify_error_t` from uSockets.
#[repr(C)]
pub struct VerifyError {
    pub error: c_int,
    pub reason: *const u8,
}

// ─── UpgradedDuplex FFI implementations ────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__ssl_error(this: *const c_void) -> VerifyError {
    if this.is_null() {
        return VerifyError { error: 2, reason: b"invalid TLS connection\0".as_ptr() };
    }
    unsafe {
        let conn = &mut *(this as *mut TlsConnection);
        if conn.is_handshaking() {
            VerifyError { error: 1, reason: b"handshake in progress\0".as_ptr() }
        } else {
            VerifyError { error: 0, reason: core::ptr::null() }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__is_established(this: *const c_void) -> bool {
    if this.is_null() {
        return false;
    }
    unsafe {
        let conn = &mut *(this as *mut TlsConnection);
        !conn.is_handshaking()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__is_closed(this: *const c_void) -> bool {
    if this.is_null() {
        return true;
    }
    unsafe {
        let conn = &mut *(this as *mut TlsConnection);
        conn.peer_closed()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__is_shutdown(this: *const c_void) -> bool {
    unsafe { UpgradedDuplex__is_closed(this) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__ssl(_: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__set_timeout(_: *mut c_void, _seconds: c_uint) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__flush(_: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__encode_and_write(
    this: *mut c_void,
    ptr: *const u8,
    len: usize,
) -> c_int {
    if this.is_null() {
        return -1;
    }
    if ptr.is_null() || len == 0 {
        return 0;
    }
    unsafe {
        let conn = &mut *(this as *mut TlsConnection);
        let data = core::slice::from_raw_parts(ptr, len);
        match conn.write(data) {
            Ok(n) => n as c_int,
            Err(TlsError::NotReady) => 0,
            Err(_) => -1,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__raw_write(
    this: *mut c_void,
    ptr: *const u8,
    len: usize,
) -> c_int {
    unsafe { UpgradedDuplex__encode_and_write(this, ptr, len) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__shutdown(this: *mut c_void) {
    if this.is_null() {
        return;
    }
    unsafe {
        let conn = &mut *(this as *mut TlsConnection);
        let _ = conn.queue_close_notify();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__shutdown_read(this: *mut c_void) {
    unsafe { UpgradedDuplex__shutdown(this) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UpgradedDuplex__close(this: *mut c_void) {
    if this.is_null() {
        return;
    }
    unsafe {
        let _ = Box::from_raw(this as *mut TlsConnection);
    }
}
