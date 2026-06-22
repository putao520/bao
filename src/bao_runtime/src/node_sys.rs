// @trace REQ-ENG-006 [api:node:sys]
//
// Node.js sys module — deprecated alias for node:util.
// Re-exports the util module object from the builtin cache.

use mozjs::jsapi::*;
use mozjs::jsval::ObjectValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    // sys is a deprecated alias for util — look up the already-cached util module
    // and register it under the "sys" key too.
    if let Some(util_obj) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, "builtin:util") {
        if !util_obj.is_null() {
            cache_builtin(cx, "sys", util_obj);
            return;
        }
    }
    // Fallback: if util is not yet cached (should not happen since node_util::install_util
    // runs before node_sys::install in globals.rs), register an empty object.
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() { return; }
    cache_builtin(cx, "sys", obj.get());
}
