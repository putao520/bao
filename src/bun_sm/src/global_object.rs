// @trace REQ-ENG-002 [module:bun_sm]
//! `JSGlobalObject` — wrapper over `*mut JSContext`.
//!
//! In JSC, `JSGlobalObject` is a heap-allocated object owning the global scope.
//! In SpiderMonkey, `JSContext` owns the heap and the global is a `*mut JSObject`
//! rooted within it. We wrap `*mut JSContext` as `JSGlobalObject` because JSC's
//! API passes `JSGlobalObject*` everywhere that SM passes `JSContext*`.

use ::std::ffi::CString;
use ::std::marker::PhantomData;

use mozjs::jsapi::JSContext as RawJSContext;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::SIMPLE_GLOBAL_CLASS;

use crate::error::JsError;
use crate::js_value::JSValue;
use crate::virtual_machine::VirtualMachine;

/// Wrapper over SpiderMonkey's `*mut JSContext`.
///
/// This type is API-compatible with `bun_jsc::JSGlobalObject`.
/// In JSC, JSGlobalObject is a GC-managed heap object; in SM, JSContext
/// is the runtime's central context pointer. The semantic mapping:
///
/// | JSC                         | SM (bun_sm)              |
/// |-----------------------------|--------------------------|
/// | `JSGlobalObject*`          | `*mut JSContext`         |
/// | `globalObject->vm()`       | `Runtime::get()` (TLS)   |
///
/// # Safety
///
/// The raw pointer must remain valid for the lifetime of this wrapper.
/// In browser mode, servo owns the JSContext. In CLI mode, `SmRuntimeGuard` does.
#[repr(transparent)]
pub struct JSGlobalObject(pub(crate) *mut RawJSContext);

impl JSGlobalObject {
    /// Create from a raw `*mut JSContext`.
    ///
    /// # Safety
    /// `ptr` must be a valid, non-null JSContext pointer.
    #[inline]
    pub unsafe fn from_raw(ptr: *mut RawJSContext) -> Self {
        debug_assert!(!ptr.is_null(), "JSGlobalObject::from_raw: null pointer");
        JSGlobalObject(ptr)
    }

    /// Get the raw `*mut JSContext` pointer.
    #[inline]
    pub fn raw(&self) -> *mut RawJSContext {
        self.0
    }

    /// Get a `mozjs::context::JSContext` wrapper.
    ///
    /// # Safety
    /// The returned value borrows the raw pointer. Caller must ensure
    /// the JSContext is still valid.
    #[inline]
    pub unsafe fn cx(&self) -> mozjs::context::JSContext {
        unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(self.0)) }
    }

    /// Check if the pointer is null.
    #[inline]
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }

    /// Get the VirtualMachine for this context.
    pub fn vm(&self) -> Option<VirtualMachine> {
        VirtualMachine::get()
    }

    /// Get the JSGlobalObject from the current thread's VirtualMachine.
    pub fn get() -> Option<Self> {
        VirtualMachine::get().map(|vm| JSGlobalObject(vm.raw()))
    }

    /// Create a new global object on the given JSContext.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn create(cx: *mut RawJSContext) -> ::std::result::Result<Self, JsError> {
        let options = mozjs::rust::RealmOptions::default();
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx)
        );
        let global = unsafe {
            mozjs::rust::wrappers2::JS_NewGlobalObject(
                cx_ref,
                &SIMPLE_GLOBAL_CLASS,
                ::std::ptr::null_mut(),
                OnNewGlobalHookOption::FireOnNewGlobalHook,
                &*options,
            )
        };
        if global.is_null() {
            return Err(JsError {
                message: "Failed to create global object".into(),
                filename: String::new(),
                line: 0,
                column: 0,
                stack: None,
            });
        }
        Ok(JSGlobalObject(cx))
    }

    /// Get the global JSObject for this context.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn global_object(&self) -> *mut JSObject {
        CurrentGlobalOrNull(self.0)
    }

    /// Define an indexed property on the global.
    /// BCE-20260619-012: global must be rooted before passing to JS API.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn put_indexed_property(&self, cx: *mut RawJSContext, index: u32, value: JSValue) -> bool {
        let global = self.global_object();
        if global.is_null() {
            return false;
        }
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let global_root = global);
        let js_val = value.into_inner().to_jsval(cx);
        // BCE-20260619-012: root js_val (may be ObjectValue/StringValue) before passing as Handle.
        rooted!(&in(cx_ref) let js_val_root = js_val);
        JS_DefineElement(
            cx,
            global_root.handle().into(),
            index,
            js_val_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        )
    }

    /// Get an indexed property from the global.
    /// BCE-20260619-012: global must be rooted before passing to JS API.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_indexed_property(&self, cx: *mut RawJSContext, index: u32) -> JSValue {
        let global = self.global_object();
        if global.is_null() {
            return JSValue::UNDEFINED;
        }
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let global_root = global);
        let mut val = UndefinedValue();
        JS_GetElement(
            cx,
            global_root.handle().into(),
            index,
            MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut val },
        );
        JSValue::from_raw(cx, val)
    }

    /// Set a named property on the global.
    /// BCE-20260619-012: global must be rooted before passing to JS API.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn put_property(&self, cx: *mut RawJSContext, name: &str, value: JSValue) -> bool {
        let global = self.global_object();
        if global.is_null() {
            return false;
        }
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let global_root = global);
        let c_name = CString::new(name).unwrap_or_default();
        let js_val = value.into_inner().to_jsval(cx);
        // BCE-20260619-012: root js_val (may be ObjectValue/StringValue) before passing as Handle.
        rooted!(&in(cx_ref) let js_val_root = js_val);
        JS_SetProperty(
            cx,
            global_root.handle().into(),
            c_name.as_ptr(),
            js_val_root.handle().into(),
        )
    }

    /// Get a named property from the global.
    /// BCE-20260619-012: global must be rooted before passing to JS API.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_property(&self, cx: *mut RawJSContext, name: &str) -> JSValue {
        let global = self.global_object();
        if global.is_null() {
            return JSValue::UNDEFINED;
        }
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let global_root = global);
        let c_name = CString::new(name).unwrap_or_default();
        let mut val = UndefinedValue();
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            c_name.as_ptr(),
            MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut val },
        );
        JSValue::from_raw(cx, val)
    }

    /// Check if the global has a named property.
    /// BCE-20260619-012: global must be rooted before passing to JS API.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn has_property(&self, cx: *mut RawJSContext, name: &str) -> bool {
        let global = self.global_object();
        if global.is_null() {
            return false;
        }
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let global_root = global);
        let c_name = CString::new(name).unwrap_or_default();
        let mut found = false;
        JS_HasProperty(
            cx,
            global_root.handle().into(),
            c_name.as_ptr(),
            &mut found,
        );
        found
    }

    /// Delete a named property from the global.
    /// BCE-20260619-012: global must be rooted before passing to JS API.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn delete_property(&self, cx: *mut RawJSContext, name: &str) -> bool {
        let global = self.global_object();
        if global.is_null() {
            return false;
        }
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let global_root = global);
        let c_name = CString::new(name).unwrap_or_default();
        JS_DeleteProperty1(
            cx,
            global_root.handle().into(),
            c_name.as_ptr(),
        )
    }

    /// Get the string representation of the global object.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn to_string(&self, _cx: *mut RawJSContext) -> String {
        let global = self.global_object();
        if global.is_null() {
            return "[object global]".into();
        }
        let clasp = mozjs::rust::get_object_class(global);
        if clasp.is_null() {
            return "[object global]".into();
        }
        let name_ptr = (*clasp).name;
        if name_ptr.is_null() {
            return "[object global]".into();
        }
        let name = ::std::ffi::CStr::from_ptr(name_ptr);
        format!("[object {}]", name.to_string_lossy())
    }
}

// JSGlobalObject is Send because JSContext is thread-local.
unsafe impl Send for JSGlobalObject {}

impl ::std::fmt::Debug for JSGlobalObject {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("JSGlobalObject")
            .field("ptr", &self.0)
            .finish()
    }
}

impl Clone for JSGlobalObject {
    fn clone(&self) -> Self {
        JSGlobalObject(self.0)
    }
}

impl Copy for JSGlobalObject {}

impl PartialEq for JSGlobalObject {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for JSGlobalObject {}

/// Type alias for JSC API compatibility.
/// In JSC: `RangeErrorOptions<'a> = bun_core::fmt::OutOfRangeOptions<'a>`.
pub type RangeErrorOptions<'a> = bun_core::fmt::OutOfRangeOptions<'a>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_raw_roundtrip() {
        let ptr = 1usize as *mut RawJSContext;
        let global = unsafe { JSGlobalObject::from_raw(ptr) };
        assert_eq!(global.raw(), ptr);
    }

    #[test]
    fn copy_semantics() {
        let ptr = 1usize as *mut RawJSContext;
        let global = unsafe { JSGlobalObject::from_raw(ptr) };
        let copy = global;
        assert_eq!(global, copy);
    }

    #[test]
    fn not_null() {
        let ptr = 1usize as *mut RawJSContext;
        let global = unsafe { JSGlobalObject::from_raw(ptr) };
        assert!(!global.is_null());
    }
}
