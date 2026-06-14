//! SM-backed GC root handles.
//!
//! In JSC, `Strong<T>` is a write barrier + root. In SM, we use the
//! global-property caching trick: store the JSObject as a named property
//! on the global object, and retrieve it by name.

use ::std::ffi::CString;
use ::std::marker::PhantomData;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::js_promise::JSPromise;

// ─── Strong<T> ──────────────────────────────────────────────────────────────

/// GC-rooted strong reference to a JS object.
///
/// Stores the object as a hidden property on the global object identified
/// by a unique key.
pub struct Strong<T> {
    /// Unique key used to store/retrieve the object on the global.
    key: String,
    /// JSContext pointer for property access.
    cx: *mut JSContext,
    _marker: PhantomData<T>,
}

impl<T> Strong<T> {
    /// Create an empty (null) strong reference.
    pub fn empty() -> Self {
        Self {
            key: String::new(),
            cx: ::std::ptr::null_mut(),
            _marker: PhantomData,
        }
    }

    /// Create a strong reference from a JSObject and JSContext.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. `obj` must be a valid JSObject.
    pub unsafe fn new(cx: *mut JSContext, obj: *mut JSObject, key: String) -> Self {
        if !cx.is_null() && !obj.is_null() {
            unsafe {
                let global = CurrentGlobalOrNull(cx);
                if !global.is_null() {
                    let c_key = CString::new(&*key).unwrap_or_default();
                    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                    rooted!(&in(wrapped_cx) let rooted_global = global);
                    rooted!(&in(wrapped_cx) let rooted_obj = obj);
                    w2::JS_DefineProperty3(
                        &mut wrapped_cx,
                        rooted_global.handle(),
                        c_key.as_ptr(),
                        rooted_obj.handle(),
                        JSPROP_PERMANENT as u32,
                    );
                }
            }
        }
        Self {
            key,
            cx,
            _marker: PhantomData,
        }
    }

    /// Check if this reference is empty (null).
    pub fn is_empty(&self) -> bool {
        self.key.is_empty() || self.cx.is_null()
    }

    /// Retrieve the JSObject pointer from the global.
    ///
    /// # Safety
    /// The JSContext must still be valid.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get(&self) -> *mut JSObject {
        if self.is_empty() {
            return ::std::ptr::null_mut();
        }
        let global = CurrentGlobalOrNull(self.cx);
        if global.is_null() {
            return ::std::ptr::null_mut();
        }
        let c_key = CString::new(&*self.key).unwrap_or_default();
        let global_h = Handle::<*mut JSObject> {
            _phantom_0: PhantomData,
            ptr: &global,
        };
        let mut val = UndefinedValue();
        JS_GetProperty(
            self.cx,
            global_h,
            c_key.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: PhantomData,
                ptr: &mut val,
            },
        );
        if val.is_object() {
            val.to_object()
        } else {
            ::std::ptr::null_mut()
        }
    }

    /// Remove the root, allowing the object to be collected.
    ///
    /// # Safety
    /// The JSContext must still be valid.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn clear(&mut self) {
        if self.is_empty() {
            return;
        }
        let global = CurrentGlobalOrNull(self.cx);
        if global.is_null() {
            return;
        }
        let c_key = CString::new(&*self.key).unwrap_or_default();
        let global_h = Handle::<*mut JSObject> {
            _phantom_0: PhantomData,
            ptr: &global,
        };
        JS_DeleteProperty1(self.cx, global_h, c_key.as_ptr());
        self.key.clear();
        self.cx = ::std::ptr::null_mut();
    }

    /// Get the JSContext pointer.
    pub fn cx(&self) -> *mut JSContext {
        self.cx
    }
}

impl<T> Default for Strong<T> {
    fn default() -> Self {
        Self::empty()
    }
}

// ─── StrongOptional<T> ──────────────────────────────────────────────────────

/// Optional strong reference.
pub enum StrongOptional<T> {
    Some(Strong<T>),
    None,
}

impl<T> StrongOptional<T> {
    pub fn none() -> Self {
        StrongOptional::None
    }

    pub fn is_some(&self) -> bool {
        matches!(self, StrongOptional::Some(_))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, StrongOptional::None)
    }
}

impl<T> Default for StrongOptional<T> {
    fn default() -> Self {
        StrongOptional::None
    }
}

// ─── JSPromiseStrong ────────────────────────────────────────────────────────

/// GC-rooted strong reference to a JSPromise.
pub type JSPromiseStrong = Strong<JSPromise>;
