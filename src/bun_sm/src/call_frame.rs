// @trace REQ-ENG-002 [module:bun_sm]
//! `CallFrame` — wrapper over SpiderMonkey's `CallArgs`.
//!
//! # ABI Difference
//!
//! ```text
//! JSC: unsafe extern "C" fn(*mut JSGlobalObject, *mut CallFrame) -> JSValue
//! SM:  unsafe extern "C" fn(*mut JSContext, argc: u32, vp: *mut JSVal) -> bool
//! ```
//!
//! This wrapper bridges the gap: `CallFrame` wraps SM's `CallArgs` and
//! provides JSC-compatible methods like `argument()`, `this_object()`.

use mozjs::jsapi::{CallArgs, JSContext as RawJSContext, JSObject};
use mozjs::jsval::{JSVal, UndefinedValue};

use crate::js_value::JSValue;
use crate::global_object::JSGlobalObject;

/// Wrapper over SpiderMonkey's `CallArgs`, providing JSC-compatible API.
///
/// # Safety
///
/// The `CallArgs` must be valid for the duration of the JSNative callback.
pub struct CallFrame {
    cx: *mut RawJSContext,
    args: CallArgs,
}

impl CallFrame {
    /// Create a CallFrame from SM's callback parameters.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. `vp` must point to a valid `JSVal` array.
    #[inline]
    pub unsafe fn from_vp(cx: *mut RawJSContext, vp: *mut JSVal, argc: u32) -> Self {
        CallFrame {
            cx,
            args: unsafe { CallArgs::from_vp(vp, argc) },
        }
    }

    /// Create from existing `CallArgs`.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext for the duration of this call.
    #[inline]
    pub unsafe fn from_call_args(cx: *mut RawJSContext, args: CallArgs) -> Self {
        CallFrame { cx, args }
    }

    /// Number of arguments passed (excluding `this`).
    #[inline]
    pub fn argument_count(&self) -> u32 {
        self.args.argc_
    }

    /// Number of arguments including `this` (JSC convention).
    #[inline]
    pub fn argument_count_including_this(&self) -> u32 {
        self.args.argc_ + 1
    }

    /// Get argument at index, or `JSValue::UNDEFINED` if out of bounds.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn argument(&self, index: u32) -> JSValue {
        if index < self.args.argc_ {
            unsafe { JSValue::from_raw(self.cx, *self.args.get(index).ptr) }
        } else {
            JSValue::UNDEFINED
        }
    }

    /// Get argument at index, or `JSValue::UNDEFINED` if out of bounds.
    /// Alias for `argument()` with JSC naming convention.
    pub fn argument_or_undefined(&self, index: u32) -> JSValue {
        self.argument(index)
    }

    /// Get all arguments as a slice of raw JSVals.
    #[inline]
    pub unsafe fn arguments_slice(&self) -> &[JSVal] {
        unsafe { std::slice::from_raw_parts(self.args.argv_, self.args.argc_ as usize) }
    }

    /// Get the `this` value as a `JSValue`.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn this_value(&self) -> JSValue {
        let thisv = self.args.thisv();
        unsafe { JSValue::from_raw(self.cx, thisv.get()) }
    }

    /// Get the `this` value as a `*mut JSObject`, or null.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn this_object(&self) -> *mut JSObject {
        let thisv = self.args.thisv();
        if thisv.is_object() {
            thisv.to_object()
        } else {
            std::ptr::null_mut()
        }
    }

    /// Get the `this` value as a `*mut JSObject`, panicking if not an object.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn unwrap_this(&self) -> *mut JSObject {
        let thisv = self.args.thisv();
        assert!(thisv.is_object(), "CallFrame::unwrap_this: this is not an object");
        thisv.to_object()
    }

    /// Get the callee (the function being called).
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn callee(&self) -> *mut JSObject {
        self.args.callee()
    }

    /// Get the `JSGlobalObject` (the JSContext) for this call.
    pub fn global_object(&self) -> JSGlobalObject {
        JSGlobalObject(self.cx)
    }

    /// Get the raw `*mut JSContext`.
    #[inline]
    pub fn cx(&self) -> *mut RawJSContext {
        self.cx
    }

    /// Get the underlying SM `CallArgs`.
    #[inline]
    pub fn call_args(&self) -> &CallArgs {
        &self.args
    }

    /// Set the return value.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn set_return_value(&mut self, val: JSValue) {
        self.args.rval().set(val.into_inner().to_jsval(self.cx));
    }

    /// Set the return value to undefined.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_undefined(&mut self) {
        self.args.rval().set(UndefinedValue());
    }

    /// Consume the first argument and return it, shifting remaining arguments.
    /// Decrements argc and increments argv pointer by one slot.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub fn shift(&mut self) -> JSValue {
        if self.args.argc_ == 0 {
            return JSValue::UNDEFINED;
        }
        let first = unsafe { JSValue::from_raw(self.cx, *self.args.argv_) };
        self.args.argv_ = unsafe { self.args.argv_.add(1) };
        self.args.argc_ -= 1;
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_frame_type_size() {
        let size = std::mem::size_of::<CallFrame>();
        assert!(size <= 32, "CallFrame is too large: {size} bytes");
    }
}
