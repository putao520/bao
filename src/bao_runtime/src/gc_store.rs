// @trace REQ-ENG-001 [entity:BaoRuntime]
use ::std::collections::HashMap;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue};

/// GC-safe persistent store for JS objects that must survive past the
/// script/eval that registered them (HTTP server handlers, socket callbacks,
/// test callbacks...).
///
/// BCE (crash-class): `JsContext::eval` creates a fresh global realm per
/// call. Storing handlers as properties on the *current* global and looking
/// them up via `CurrentGlobalOrNull` breaks the moment an async event
/// dispatches after the eval returned (realm popped → current global is
/// NULL) — the handler is "lost", node:http returns from the uWS route
/// handler without responding (uWS `std::terminate`, SIGABRT) and
/// Bun.serve/Bun.listen mask the same hole with a default echo response.
///
/// Fix: every entry is a heap `Value` slot registered with
/// `JS_AddExtraRoot` (`AddRawValueRoot`) — the mozjs 0.21.4 equivalent of
/// `JS::PersistentRooted`. Two slots per entry:
///   * `global_val` — the global of the realm the object was created in.
///     Rooting the global pins the whole realm (and everything reachable
///     from it) for the entry's lifetime, so a later eval's realm cannot
///     displace it.
///   * `obj_val` — the stored object itself.
/// Lookup reads the rooted slot directly — it never consults
/// `CurrentGlobalOrNull`, so it works from ANY realm (or none, e.g. inside
/// `drain_and_check`). Callers that need to *invoke* the object must enter
/// its realm first via `gc_store_get_*_with_global` + `AutoRealm`.
///
/// Thread discipline (iron rule): entries are keyed per-thread
/// (`thread_local!`) and the rooted slots belong to that thread's
/// JSContext. Never share a `*mut JSObject` from this store across threads.
struct GcStore {
    /// tracking key (`namespace::key`) → GC-rooted entry (boxed: the rooted
    /// Value slots must live at a stable address until `RemoveRawValueRoot`).
    entries: HashMap<String, Box<RootedEntry>>,
}

/// One persistent-rooted store entry.
///
/// The two `Value` fields are registered with `AddRawValueRoot` at insert
/// time; SpiderMonkey's GC scans (and updates, for moving GC) the memory at
/// those addresses until `RemoveRawValueRoot` runs. The box therefore must
/// not be moved or freed before both roots are removed.
struct RootedEntry {
    /// JSContext the roots were registered on (also the thread's context).
    /// Used as a liveness guard before `RemoveRawValueRoot` (unrooting on a
    /// destroyed context is UB — see `Drop`).
    cx: *mut JSContext,
    /// Rooted slot: the global of the realm the object belongs to.
    global_val: JSVal,
    /// Rooted slot: the stored object.
    obj_val: JSVal,
}

impl Drop for RootedEntry {
    fn drop(&mut self) {
        // Only unroot while the registration context is still the live
        // JSContext on this thread. If it was destroyed (test teardown calls
        // JS_DestroyContext, which frees the extra-roots table wholesale) or
        // replaced, the registration no longer exists — unrooting would be
        // use-after-free.
        if Self::cx_alive(self.cx) {
            unsafe {
                RemoveRawValueRoot(self.cx, &mut self.obj_val);
                RemoveRawValueRoot(self.cx, &mut self.global_val);
            }
        }
    }
}

impl RootedEntry {
    /// A registration context is removable iff it is still the live
    /// thread JSContext (`Runtime::get()`), i.e. its extra-roots table still
    /// holds our slots.
    fn cx_alive(cx: *mut JSContext) -> bool {
        !cx.is_null() && mozjs::rust::Runtime::get().map(|c| c.as_ptr()) == Some(cx)
    }
}

impl GcStore {
    fn new() -> Self {
        GcStore {
            entries: HashMap::new(),
        }
    }

    /// Format a namespaced tracking key: `namespace::key`, or just `key`
    /// when the namespace is empty (backward compat).
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
        // Insert must happen inside the object's realm (the eval that
        // registered the handler) — that is where the owning global lives.
        let global = unsafe { CurrentGlobalOrNull(cx) };
        if global.is_null() {
            return;
        }

        let tracking_key = Self::tracking_key(namespace, key);
        // Overwrite semantics: fully retire the previous entry first.
        // Dropping a still-rooted box would leave the GC tracing freed
        // memory, so removal (unroot) must happen before the box is freed.
        self.entries.remove(&tracking_key);

        let mut entry = Box::new(RootedEntry {
            cx,
            global_val: ObjectValue(global),
            obj_val: ObjectValue(obj),
        });
        let ok_global = unsafe {
            AddRawValueRoot(
                cx,
                &mut entry.global_val,
                b"gc_store_global\0".as_ptr() as *const ::std::os::raw::c_char,
            )
        };
        let ok_obj = unsafe {
            AddRawValueRoot(
                cx,
                &mut entry.obj_val,
                b"gc_store_obj\0".as_ptr() as *const ::std::os::raw::c_char,
            )
        };
        if !ok_global || !ok_obj {
            // Fail-closed: unroot whatever succeeded and do not track —
            // a half-rooted entry could be swept mid-flight.
            if ok_obj {
                unsafe { RemoveRawValueRoot(cx, &mut entry.obj_val) };
            }
            if ok_global {
                unsafe { RemoveRawValueRoot(cx, &mut entry.global_val) };
            }
            return;
        }
        self.entries.insert(tracking_key, entry);
    }

    /// Resolve the stored object from its rooted slot.
    ///
    /// Realm-independent: works with no realm entered (async dispatch after
    /// the script ended). The returned object belongs to its entry's realm —
    /// callers invoking it must enter that realm (see `get_with_global`).
    fn get(&self, cx: *mut JSContext, namespace: &str, key: &str) -> Option<*mut JSObject> {
        let _ = cx; // liveness of the slots is guaranteed by the roots
        let entry = self.entries.get(&Self::tracking_key(namespace, key))?;
        if entry.obj_val.is_object() {
            Some(entry.obj_val.to_object())
        } else {
            None
        }
    }

    /// Resolve both the stored object and its owning global.
    ///
    /// Dispatch sites use the global to `AutoRealm` into the object's realm
    /// before creating sibling JS objects or invoking it.
    fn get_with_global(
        &self,
        cx: *mut JSContext,
        namespace: &str,
        key: &str,
    ) -> Option<(*mut JSObject, *mut JSObject)> {
        let entry = self.entries.get(&Self::tracking_key(namespace, key))?;
        if !entry.obj_val.is_object() || !entry.global_val.is_object() {
            return None;
        }
        let _ = cx;
        Some((entry.obj_val.to_object(), entry.global_val.to_object()))
    }

    fn remove(&mut self, cx: *mut JSContext, namespace: &str, key: &str) {
        if self
            .entries
            .remove(&Self::tracking_key(namespace, key))
            .is_some()
        {
            // RootedEntry::drop unroots both slots against `entry.cx`;
            // `cx` is kept for signature compatibility.
            let _ = cx;
        }
    }
}

thread_local! {
    static GC_STORE: ::std::cell::RefCell<GcStore> = ::std::cell::RefCell::new(GcStore::new());
}

/// Store a JSObject in the GC-safe store under a simple key.
/// The object AND its owning global are persistent-rooted until removed.
pub fn gc_store_insert(cx: *mut JSContext, key: &str, obj: *mut JSObject) {
    GC_STORE.with(|s| {
        s.borrow_mut().insert(cx, "", key, obj);
    });
}

/// Retrieve a JSObject from the GC-safe store by key.
/// Realm-independent — see [`GcStore::get`].
pub fn gc_store_get(cx: *mut JSContext, key: &str) -> Option<*mut JSObject> {
    GC_STORE.with(|s| s.borrow().get(cx, "", key))
}

/// Remove a JSObject from the GC-safe store by key (unroots both slots).
pub fn gc_store_remove(cx: *mut JSContext, key: &str) {
    GC_STORE.with(|s| {
        s.borrow_mut().remove(cx, "", key);
    });
}

/// Store a JSObject in the GC-safe store under a namespaced key.
pub fn gc_store_insert_ns(cx: *mut JSContext, namespace: &str, key: &str, obj: *mut JSObject) {
    GC_STORE.with(|s| {
        s.borrow_mut().insert(cx, namespace, key, obj);
    });
}

/// Retrieve a JSObject from the GC-safe store by namespaced key.
pub fn gc_store_get_ns(cx: *mut JSContext, namespace: &str, key: &str) -> Option<*mut JSObject> {
    GC_STORE.with(|s| s.borrow().get(cx, namespace, key))
}

/// Retrieve a JSObject AND its owning global from the namespaced store.
///
/// Returns `(obj, global)` where `global` is the global of the realm the
/// object was created in. Dispatch sites must wrap both in `rooted!()`
/// immediately and enter `AutoRealm` on the global before any JS API use.
pub fn gc_store_get_ns_with_global(
    cx: *mut JSContext,
    namespace: &str,
    key: &str,
) -> Option<(*mut JSObject, *mut JSObject)> {
    GC_STORE.with(|s| s.borrow().get_with_global(cx, namespace, key))
}

/// Retrieve a JSObject AND its owning global from the store (no namespace).
pub fn gc_store_get_with_global(
    cx: *mut JSContext,
    key: &str,
) -> Option<(*mut JSObject, *mut JSObject)> {
    GC_STORE.with(|s| s.borrow().get_with_global(cx, "", key))
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

    #[test]
    fn tracking_key_empty_namespace() {
        assert_eq!(GcStore::tracking_key("", "foo"), "foo");
    }

    #[test]
    fn tracking_key_with_namespace() {
        assert_eq!(
            GcStore::tracking_key("EmitterState", "data:0"),
            "EmitterState::data:0"
        );
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
        assert!(store.entries.is_empty());
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
        assert!(
            after > before,
            "counter must increment: before={before}, after={after}"
        );
    }
}
