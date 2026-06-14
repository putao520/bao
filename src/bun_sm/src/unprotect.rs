//! Unprotect — removes GC protection from a JSObject.

use mozjs::jsapi::{JSContext, JSObject};

pub struct Unprotect {
    obj: *mut JSObject,
}

impl Unprotect {
    pub unsafe fn new(_cx: *mut JSContext, obj: *mut JSObject) -> Self {
        Unprotect { obj }
    }

    pub fn get(&self) -> *mut JSObject {
        self.obj
    }
}
