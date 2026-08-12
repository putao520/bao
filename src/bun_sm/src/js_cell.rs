//! JsCell — interior mutability for GC-managed state.

use std::cell::UnsafeCell;

/// Interior mutability cell for JS class payloads.
/// In JSC, this wraps `WriteBarrier`. In SM, we use `UnsafeCell`
/// since SM's GC uses reserved slots, not write barriers.
pub struct JsCell<T> {
    inner: UnsafeCell<T>,
}

impl<T> JsCell<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
        }
    }

    pub fn get(&self) -> &T {
        unsafe { &*self.inner.get() }
    }

    pub fn get_mut(&self) -> &mut T {
        unsafe { &mut *self.inner.get() }
    }
}

unsafe impl<T: Send> Send for JsCell<T> {}
unsafe impl<T: Sync> Sync for JsCell<T> {}
