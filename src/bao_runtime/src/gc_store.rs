// @trace REQ-ENG-001 [entity:BaoRuntime]
use ::std::cell::RefCell;
use ::std::collections::HashSet;
use bun_core::ZBox;
use ::std::ptr;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};

/// GC-safe module cache: stores cached objects as properties on the JS global.
/// SpiderMonkey's GC manages these naturally — no raw pointer caching needed.
/// We only track which keys are set (a HashSet of strings).
///
/// Per-struct namespacing: keys are formatted as `__gc_{namespace}_{key}`
/// so different structs (ServerUserData, BaoTimeoutObject, EmitterState, etc.)
/// never collide even if they use the same local key (e.g. "handler").
struct GcStore {
    keys: HashSet<String>,
}

impl GcStore {
    fn new() -> Self {
        GcStore {
            keys: HashSet::new(),
        }
    }

    /// Format a namespaced property name: `__gc_{namespace}_{key}`.
    /// If namespace is empty, falls back to `__gc_cache_{key}` for backward compat.
    fn prop_name(namespace: &str, key: &str) -> ZBox {
        if namespace.is_empty() {
            ZBox::from_vec(format!("__gc_cache_{}", key).into_bytes())
        } else {
            ZBox::from_vec(format!("__gc_{}_{}", namespace, key).into_bytes())
        }
    }

    /// Full tracking key: `namespace::key` or just `key` if namespace is empty.
    fn tracking_key(namespace: &str, key: &str) -> String {
        if namespace.is_empty() {
            key.to_string()
        } else {
            format!("{}::{}", namespace, key)
        }
    }

    fn insert(&mut self, cx: *mut JSContext, namespace: &str, key: &str, obj: *mut JSObject) {
        if obj.is_null() {
            return;
        }
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return;
        }
        let prop_name = Self::prop_name(namespace, key);
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
        self.keys.insert(Self::tracking_key(namespace, key));
    }

    fn get(&self, cx: *mut JSContext, namespace: &str, key: &str) -> Option<*mut JSObject> {
        if !self.keys.contains(&Self::tracking_key(namespace, key)) {
            return None;
        }
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return None;
        }
        let prop_name = Self::prop_name(namespace, key);
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

    fn remove(&mut self, cx: *mut JSContext, namespace: &str, key: &str) {
        if !self.keys.remove(&Self::tracking_key(namespace, key)) {
            return;
        }
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return;
        }
        let prop_name = Self::prop_name(namespace, key);
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

/// Store a JSObject in the GC-safe store under a simple key.
/// The object is set as a property on the JS global, so SpiderMonkey's GC
/// manages it naturally. Uses `__gc_cache_{key}` as the property name.
pub fn gc_store_insert(cx: *mut JSContext, key: &str, obj: *mut JSObject) {
    GC_STORE.with(|s| {
        s.borrow_mut().insert(cx, "", key, obj);
    });
}

/// Retrieve a JSObject from the GC-safe store by key.
/// Returns None if the key is not tracked or the global is unavailable.
pub fn gc_store_get(cx: *mut JSContext, key: &str) -> Option<*mut JSObject> {
    GC_STORE.with(|s| s.borrow().get(cx, "", key))
}

/// Remove a JSObject from the GC-safe store by key.
/// Deletes the property from the JS global and removes the tracking key.
pub fn gc_store_remove(cx: *mut JSContext, key: &str) {
    GC_STORE.with(|s| {
        s.borrow_mut().remove(cx, "", key);
    });
}

/// Store a JSObject in the GC-safe store under a namespaced key.
/// The object is set as a property on the JS global. `namespace` prevents
/// key collisions between structs (e.g., `"ServerUserData"` vs `"EmitterState"`).
/// Property name format: `__gc_{namespace}_{key}`.
pub fn gc_store_insert_ns(cx: *mut JSContext, namespace: &str, key: &str, obj: *mut JSObject) {
    GC_STORE.with(|s| {
        s.borrow_mut().insert(cx, namespace, key, obj);
    });
}

/// Retrieve a JSObject from the GC-safe store by namespaced key.
pub fn gc_store_get_ns(cx: *mut JSContext, namespace: &str, key: &str) -> Option<*mut JSObject> {
    GC_STORE.with(|s| s.borrow().get(cx, namespace, key))
}

/// Remove a JSObject from the GC-safe store by namespaced key.
pub fn gc_store_remove_ns(cx: *mut JSContext, namespace: &str, key: &str) {
    GC_STORE.with(|s| {
        s.borrow_mut().remove(cx, namespace, key);
    });
}

/// Generate a namespaced GcStore key. Format: `"__gc_{namespace}_{id}"`.
/// Use this to avoid key collisions between different modules storing objects
/// in the global GcStore (e.g., `"http_server_1_handler"` vs `"timer_cb_42"`).
pub fn gc_store_key(namespace: &str, id: u64) -> String {
    format!("__gc_{}_{}", namespace, id)
}

/// Atomic counter for generating unique GcStore keys.
use ::std::sync::atomic::{AtomicU64, Ordering};
static GC_KEY_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique GcStore key with auto-incrementing ID.
/// Format: `"__gc_{namespace}_{auto_id}"`.
pub fn gc_store_unique_key(namespace: &str) -> String {
    let id = GC_KEY_COUNTER.fetch_add(1, Ordering::Relaxed);
    gc_store_key(namespace, id)
}

// ── Unit tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use bun_core::ByteSlice;

    #[test]
    fn prop_name_empty_namespace_uses_cache_prefix() {
        let c = GcStore::prop_name("", "foo");
        let s = c.to_str().unwrap();
        assert_eq!(s, "__gc_cache_foo");
    }

    #[test]
    fn prop_name_with_namespace() {
        let c = GcStore::prop_name("ServerUserData", "handler");
        let s = c.to_str().unwrap();
        assert_eq!(s, "__gc_ServerUserData_handler");
    }

    #[test]
    fn tracking_key_empty_namespace() {
        assert_eq!(GcStore::tracking_key("", "foo"), "foo");
    }

    #[test]
    fn tracking_key_with_namespace() {
        assert_eq!(GcStore::tracking_key("EmitterState", "data:0"), "EmitterState::data:0");
    }

    #[test]
    fn tracking_key_uniqueness() {
        let a = GcStore::tracking_key("ServerUserData", "handler");
        let b = GcStore::tracking_key("BunServeUserData", "handler");
        assert_ne!(a, b, "same key in different namespaces must be distinct");
    }

    #[test]
    fn gc_store_new_is_empty() {
        let store = GcStore::new();
        assert!(store.keys.is_empty());
    }

    #[test]
    fn gc_store_key_format() {
        assert_eq!(gc_store_key("timer", 42), "__gc_timer_42");
    }

    #[test]
    fn gc_store_unique_key_format() {
        let k1 = gc_store_unique_key("http");
        let k2 = gc_store_unique_key("http");
        assert!(k1.starts_with("__gc_http_"));
        assert!(k2.starts_with("__gc_http_"));
        // IDs should be different
        assert_ne!(k1, k2);
    }

    #[test]
    fn gc_store_unique_key_counter_increments() {
        // Use fetch_add return value instead of global counter comparison,
        // since other test threads also increment the shared AtomicU64.
        let before = GC_KEY_COUNTER.fetch_add(0, Ordering::SeqCst);
        let _ = gc_store_unique_key("test_ns");
        let after = GC_KEY_COUNTER.fetch_add(0, Ordering::SeqCst);
        assert!(after > before, "counter must increment: before={before}, after={after}");
    }
}