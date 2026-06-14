//! Weak reference — SM WeakRef wrapper.

use mozjs::jsapi::{JSContext, JSObject};

pub struct Weak<T> {
    _inner: core::marker::PhantomData<T>,
    obj: *mut JSObject,
}

impl<T> Weak<T> {
    pub unsafe fn new(_cx: *mut JSContext, obj: *mut JSObject) -> Self {
        Weak {
            _inner: core::marker::PhantomData,
            obj,
        }
    }

    pub fn is_dead(&self) -> bool {
        self.obj.is_null()
    }

    pub fn get(&self) -> *mut JSObject {
        self.obj
    }
}

pub enum WeakRefType {
    Object,
    Map,
    Set,
}
