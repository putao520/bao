// @trace REQ-ENG-003
//! GC roots and storage for SpiderMonkey.
//!
//! This module provides:
//! - `gc_store`: GC-safe persistent storage for JSObject pointers via global-property trick
//! - `EnsureStillAlive<T>`: no-op GC guard (SM uses explicit rooting, not conservative GC)
//!
//! Types previously duplicated here (`Strong`, `MarkedArgumentBuffer`, etc.)
//! are now defined in their own dedicated modules and re-exported from the
//! crate root. See `strong`, `arguments`, `string_jsc`, `js_cell`, `array_buffer`.

use ::std::cell::RefCell;
use ::std::collections::HashSet;
use ::std::ffi::CString;
use ::std::sync::atomic::{AtomicU64, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;

// ---------------------------------------------------------------------------
// EnsureStillAlive — no-op GC guard
// ---------------------------------------------------------------------------

pub struct EnsureStillAlive<T> {
    _val: T,
}

impl<T> EnsureStillAlive<T> {
    #[inline]
    pub fn new(val: T) -> Self {
        EnsureStillAlive { _val: val }
    }
}

impl<T> ::std::ops::Deref for EnsureStillAlive<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self._val
    }
}

#[inline]
pub fn ensure_still_alive<T>(val: T) -> EnsureStillAlive<T> {
    EnsureStillAlive::new(val)
}

// ---------------------------------------------------------------------------
// gc_store module — GC-safe persistent storage for JSObject pointers
// ---------------------------------------------------------------------------

pub mod gc_store {
    use super::*;

    static GC_KEY_COUNTER: AtomicU64 = AtomicU64::new(1);

    thread_local! {
        static GC_STORE_KEYS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn insert(cx: *mut JSContext, key: &str, obj: *mut JSObject) {
        if obj.is_null() {
            return;
        }
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return;
        }
        let prop_name = CString::new(format!("__gc_cache_{key}")).unwrap_or_default();
        // BCE-20260619-012: root both global and obj_val before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let global_root = global);
        rooted!(&in(cx_ref) let obj_val_root = ObjectValue(obj));
        JS_DefineProperty(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
            obj_val_root.handle().into(),
            JSPROP_READONLY as u32,
        );
        GC_STORE_KEYS.with(|s| s.borrow_mut().insert(key.to_string()));
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn insert_ns(cx: *mut JSContext, namespace: &str, key: &str, obj: *mut JSObject) {
        if obj.is_null() {
            return;
        }
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return;
        }
        let prop_name = CString::new(format!("__gc_{namespace}_{key}")).unwrap_or_default();
        // BCE-20260619-012: root both global and obj_val before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let global_root = global);
        rooted!(&in(cx_ref) let obj_val_root = ObjectValue(obj));
        JS_DefineProperty(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
            obj_val_root.handle().into(),
            JSPROP_READONLY as u32,
        );
        GC_STORE_KEYS.with(|s| s.borrow_mut().insert(format!("{namespace}::{key}")));
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get(cx: *mut JSContext, key: &str) -> Option<*mut JSObject> {
        let has_key = GC_STORE_KEYS.with(|s| s.borrow().contains(key));
        if !has_key {
            return None;
        }
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return None;
        }
        let prop_name = CString::new(format!("__gc_cache_{key}")).unwrap_or_default();
        let mut val = UndefinedValue();
        // BCE-20260619-012: root global before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let global_root = global);
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            },
        );
        if val.is_object() {
            Some(val.to_object())
        } else {
            None
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_ns(cx: *mut JSContext, namespace: &str, key: &str) -> Option<*mut JSObject> {
        let tracking = format!("{namespace}::{key}");
        let has_key = GC_STORE_KEYS.with(|s| s.borrow().contains(&tracking));
        if !has_key {
            return None;
        }
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return None;
        }
        let prop_name = CString::new(format!("__gc_{namespace}_{key}")).unwrap_or_default();
        let mut val = UndefinedValue();
        // BCE-20260619-012: root global before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let global_root = global);
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            },
        );
        if val.is_object() {
            Some(val.to_object())
        } else {
            None
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn remove(cx: *mut JSContext, key: &str) {
        GC_STORE_KEYS.with(|s| s.borrow_mut().remove(key));
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return;
        }
        let prop_name = CString::new(format!("__gc_cache_{key}")).unwrap_or_default();
        // BCE-20260619-012: root global before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let global_root = global);
        JS_DeleteProperty1(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
        );
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn remove_ns(cx: *mut JSContext, namespace: &str, key: &str) {
        GC_STORE_KEYS.with(|s| s.borrow_mut().remove(&format!("{namespace}::{key}")));
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return;
        }
        let prop_name = CString::new(format!("__gc_{namespace}_{key}")).unwrap_or_default();
        // BCE-20260619-012: root global before passing as Handle to JS API.
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let global_root = global);
        JS_DeleteProperty1(
            cx,
            global_root.handle().into(),
            prop_name.as_ptr(),
        );
    }

    pub fn unique_key(namespace: &str) -> String {
        let id = GC_KEY_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("__gc_{namespace}_{id}")
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_still_alive_noop() {
        let val = 42i32;
        let guard = ensure_still_alive(val);
        assert_eq!(*guard, 42);
    }

    #[test]
    fn ensure_still_alive_deref() {
        let guard = EnsureStillAlive::new("hello");
        assert_eq!(*guard, "hello");
    }

    #[test]
    fn gc_store_unique_key() {
        let k1 = gc_store::unique_key("test");
        let k2 = gc_store::unique_key("test");
        assert!(k1.starts_with("__gc_test_"));
        assert!(k2.starts_with("__gc_test_"));
        assert_ne!(k1, k2);
    }

    #[test]
    fn ensure_still_alive_with_pointer() {
        let ptr = 0x1234 as *const u8;
        let guard = ensure_still_alive(ptr);
        assert_eq!(*guard, ptr);
    }
}
