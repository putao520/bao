use ::std::cell::RefCell;
use ::std::collections::HashSet;
use ::std::ffi::CString;
use ::std::ptr;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};

/// GC-safe module cache: stores cached objects as properties on the JS global.
/// SpiderMonkey's GC manages these naturally — no raw pointer caching needed.
/// We only track which keys are set (a HashSet of strings).
struct GcStore {
    keys: HashSet<String>,
}

impl GcStore {
    fn new() -> Self {
        GcStore {
            keys: HashSet::new(),
        }
    }

    fn insert(&mut self, cx: *mut JSContext, key: &str, obj: *mut JSObject) {
        if obj.is_null() {
            return;
        }
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return;
        }
        let prop_name = CString::new(format!("__gc_cache_{}", key)).unwrap_or_default();
        let obj_val = ObjectValue(obj);
        let obj_h = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &obj_val,
        };
        unsafe {
            JS_DefineProperty(
                cx,
                Handle::<*mut JSObject> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &global,
                },
                prop_name.as_ptr(),
                obj_h,
                (JSPROP_READONLY) as u32,
            );
        }
        self.keys.insert(key.to_string());
    }

    fn get(&self, cx: *mut JSContext, key: &str) -> Option<*mut JSObject> {
        if !self.keys.contains(key) {
            return None;
        }
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return None;
        }
        let prop_name = CString::new(format!("__gc_cache_{}", key)).unwrap_or_default();
        let mut val = UndefinedValue();
        unsafe {
            JS_GetProperty(
                cx,
                Handle::<*mut JSObject> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &global,
                },
                prop_name.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut val,
                },
            );
        }
        if val.is_object() {
            Some(val.to_object())
        } else {
            None
        }
    }

    fn remove(&mut self, cx: *mut JSContext, key: &str) {
        if !self.keys.remove(key) {
            return;
        }
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return;
        }
        let prop_name = CString::new(format!("__gc_cache_{}", key)).unwrap_or_default();
        unsafe {
            JS_DeleteProperty1(
                cx,
                Handle::<*mut JSObject> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &global,
                },
                prop_name.as_ptr(),
            );
        }
    }
}

thread_local! {
    static GC_STORE: RefCell<GcStore> = RefCell::new(GcStore::new());
}

pub fn gc_store_insert(cx: *mut JSContext, key: &str, obj: *mut JSObject) {
    GC_STORE.with(|s| {
        s.borrow_mut().insert(cx, key, obj);
    });
}

pub fn gc_store_get(cx: *mut JSContext, key: &str) -> Option<*mut JSObject> {
    GC_STORE.with(|s| s.borrow().get(cx, key))
}

pub fn gc_store_remove(cx: *mut JSContext, key: &str) {
    GC_STORE.with(|s| {
        s.borrow_mut().remove(cx, key);
    });
}
