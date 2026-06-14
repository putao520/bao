//! Validation scope — RAII guard checking SM exception state.
//!
//! In JSC, `ValidationScope` / `ExceptionValidationScope` is a RAII guard
//! that asserts no exception was thrown during a scope. In SpiderMonkey,
//! the equivalent is checking `JS_IsExceptionPending(cx)`.
//!
//! This module provides both a `ValidationScope` struct and a
//! `validation_scope!` macro for API compatibility with `bun_jsc`.

use mozjs::jsapi::JS_IsExceptionPending;
use mozjs::jsapi::JSContext as RawJSContext;

use crate::JSGlobalObject;

/// RAII guard that checks whether an exception is pending on the JSContext.
///
/// Created via `validation_scope!` macro or `ValidationScope::new()`.
/// On `Drop`, if an exception is still pending, it logs a warning.
pub struct ValidationScope {
    cx: *mut RawJSContext,
    had_exception_on_enter: bool,
}

impl ValidationScope {
    /// Create a new validation scope from a `JSGlobalObject`.
    ///
    /// Records whether an exception was already pending at scope entry.
    pub fn new(global: &JSGlobalObject) -> Self {
        let cx = global.raw();
        let had_exception = if !cx.is_null() {
            unsafe { JS_IsExceptionPending(cx) }
        } else {
            false
        };
        ValidationScope {
            cx,
            had_exception_on_enter: had_exception,
        }
    }

    /// Create from a raw `*mut JSContext`.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext pointer.
    pub unsafe fn from_raw_cx(cx: *mut RawJSContext) -> Self {
        let had_exception = if !cx.is_null() {
            unsafe { JS_IsExceptionPending(cx) }
        } else {
            false
        };
        ValidationScope {
            cx,
            had_exception_on_enter: had_exception,
        }
    }

    /// Check if a new exception appeared during this scope
    /// (i.e., an exception is pending now that wasn't pending on entry).
    pub fn has_new_exception(&self) -> bool {
        if self.cx.is_null() {
            return false;
        }
        let now = unsafe { JS_IsExceptionPending(self.cx) };
        now && !self.had_exception_on_enter
    }

    /// Get the raw JSContext pointer.
    pub fn cx(&self) -> *mut RawJSContext {
        self.cx
    }
}

impl Drop for ValidationScope {
    fn drop(&mut self) {
        if self.has_new_exception() {
            log::warn!("ValidationScope: unexpected exception pending on scope exit");
        }
    }
}

impl Default for ValidationScope {
    fn default() -> Self {
        ValidationScope {
            cx: std::ptr::null_mut(),
            had_exception_on_enter: false,
        }
    }
}

/// Macro creating a `ValidationScope` RAII guard, compatible with
/// `bun_jsc::validation_scope!(scope, global_object)`.
///
/// Usage:
/// ```ignore
/// validation_scope!(scope, global_object);
/// // ... JS operations ...
/// // scope drops here, checking for unexpected exceptions
/// ```
#[macro_export]
macro_rules! validation_scope {
    ($scope_name:ident, $global_object:expr) => {
        let $scope_name = $crate::ValidationScope::new(&$global_object);
    };
}
