//! Global reference — GC-rooted reference to a JS global object.

use mozjs::jsapi::{JSContext, JSObject};

pub struct GlobalRef {
    key: String,
    obj: *mut JSObject,
}

impl GlobalRef {
    pub unsafe fn new(cx: *mut JSContext, obj: *mut JSObject) -> Option<Self> {
        if obj.is_null() {
            return None;
        }
        let key = format!("__global_ref_{:p}", obj);
        unsafe { crate::gc::gc_store::insert(cx, &key, obj); }
        Some(GlobalRef { key, obj })
    }

    pub fn get(&self) -> *mut JSObject {
        self.obj
    }

    pub unsafe fn clear(&mut self, cx: *mut JSContext) {
        if !self.obj.is_null() {
            unsafe { crate::gc::gc_store::remove(cx, &self.key); }
            self.obj = std::ptr::null_mut();
        }
    }
}

impl Drop for GlobalRef {
    fn drop(&mut self) {}
}

pub struct GlobalData {
    _private: (),
}
