//! SM-backed promise types.
//!
//! `JSPromise` and `JSInternalPromise` are newtypes over `*mut JSObject`.
//! In SpiderMonkey, a Promise is a JSObject with internal slots.

use ::std::ffi::c_void;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::rooted;

use crate::js_value::JSValue;
use crate::strong::Strong;

// ─── JSPromise ──────────────────────────────────────────────────────────────

/// SpiderMonkey-backed JSPromise — newtype over `*mut JSObject`.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct JSPromise(pub(crate) NonNull<JSObject>);

impl JSPromise {
    /// Create a JSPromise from a raw JSObject pointer.
    ///
    /// # Safety
    /// `obj` must be a valid, non-null JSObject that is a Promise.
    pub unsafe fn from_object(obj: *mut JSObject) -> Option<Self> {
        NonNull::new(obj).map(JSPromise)
    }

    /// Get the underlying JSObject pointer.
    pub fn as_object(&self) -> *mut JSObject {
        self.0.as_ptr()
    }

    /// Opaque reference for FFI.
    pub fn opaque_ref(&self) -> *const c_void {
        self.0.as_ptr() as *const c_void
    }

    /// Opaque mutable reference for FFI.
    pub fn opaque_mut(&mut self) -> *mut c_void {
        self.0.as_ptr() as *mut c_void
    }

    /// Get the promise state (pending/fulfilled/rejected).
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn state(&self, cx: *mut JSContext) -> PromiseResult {
        let obj = self.as_object();
        // BCE-20260619-012: root obj before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = obj);
        if JS::IsPromiseObject(obj_root.handle().into()) {
            let state = JS::GetPromiseState(obj_root.handle().into());
            match state {
                PromiseState::Pending => PromiseResult::Pending,
                PromiseState::Fulfilled => PromiseResult::Fulfilled,
                PromiseState::Rejected => PromiseResult::Rejected,
                #[allow(unreachable_patterns)]
                _ => PromiseResult::Pending,
            }
        } else {
            PromiseResult::Pending
        }
    }

    /// Resolve the promise with a value.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn resolve(&self, cx: *mut JSContext, value: JS::Handle<Value>) {
        let obj = self.as_object();
        // BCE-20260619-012: root obj before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = obj);
        JS::ResolvePromise(cx, obj_root.handle().into(), value);
    }

    /// Reject the promise with a value.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn reject(&self, cx: *mut JSContext, value: JS::Handle<Value>) {
        let obj = self.as_object();
        // BCE-20260619-012: root obj before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = obj);
        JS::RejectPromise(cx, obj_root.handle().into(), value);
    }
}

// ─── JSInternalPromise ──────────────────────────────────────────────────────

/// SpiderMonkey-backed JSInternalPromise — newtype over `*mut JSObject`.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct JSInternalPromise(pub(crate) NonNull<JSObject>);

impl JSInternalPromise {
    /// Create from a raw JSObject pointer.
    pub unsafe fn from_object(obj: *mut JSObject) -> Option<Self> {
        NonNull::new(obj).map(JSInternalPromise)
    }

    /// Get the underlying JSObject pointer.
    pub fn as_object(&self) -> *mut JSObject {
        self.0.as_ptr()
    }

    /// Opaque reference for FFI.
    pub fn opaque_ref(&self) -> *const c_void {
        self.0.as_ptr() as *const c_void
    }

    /// Opaque mutable reference for FFI.
    pub fn opaque_mut(&mut self) -> *mut c_void {
        self.0.as_ptr() as *mut c_void
    }
}

// ─── AnyPromise ─────────────────────────────────────────────────────────────

/// Union of JSPromise and JSInternalPromise.
#[derive(Debug, Clone, Copy)]
pub enum AnyPromise {
    Normal(JSPromise),
    Internal(JSInternalPromise),
}

impl AnyPromise {
    /// Get the underlying JSObject pointer.
    pub fn as_object(&self) -> *mut JSObject {
        match self {
            AnyPromise::Normal(p) => p.as_object(),
            AnyPromise::Internal(p) => p.as_object(),
        }
    }

    /// Convert to a PromiseResult by checking the promise state.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn to_result(&self, cx: *mut JSContext) -> PromiseResult {
        let obj = self.as_object();
        // BCE-20260619-012: root obj before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = obj);
        if JS::IsPromiseObject(obj_root.handle().into()) {
            let state = JS::GetPromiseState(obj_root.handle().into());
            match state {
                PromiseState::Pending => PromiseResult::Pending,
                PromiseState::Fulfilled => PromiseResult::Fulfilled,
                PromiseState::Rejected => PromiseResult::Rejected,
                #[allow(unreachable_patterns)]
                _ => PromiseResult::Pending,
            }
        } else {
            PromiseResult::Pending
        }
    }
}

// ─── JSPromiseStrong ────────────────────────────────────────────────────────

/// GC-rooted strong reference to a JSPromise.
pub type JSPromiseStrong = Strong<JSPromise>;

// ─── PromiseResult ──────────────────────────────────────────────────────────

/// Promise result state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseResult {
    Pending,
    Fulfilled,
    Rejected,
}

// ─── Promise submodule ──────────────────────────────────────────────────────

/// Promise module re-export.
pub mod js_promise {
    pub use super::{JSPromise, JSInternalPromise, AnyPromise, PromiseResult};

    /// Promise unwrap mode.
    #[derive(Debug, Clone, Copy)]
    pub enum UnwrapMode {
        MarkHandled,
        DontMarkHandled,
    }

    /// Promise status.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Status {
        Pending,
        Fulfilled,
        Rejected,
    }
}
