// @trace REQ-ENG-007 [api:node sub-path alias modules]
//
// Sub-path alias modules that re-export a sub-property of a parent module.
// In Bun/Node.js these are typically 2-3 line modules:
//   assert/strict     → require("node:assert").strict
//   dns/promises      → require("node:dns").promises
//   path/posix        → require("node:path").posix
//   path/win32        → require("node:path").win32
//   readline/promises → require("node:readline").promises
//   stream/promises   → require("node:stream").promises
//
// Each alias is resolved by looking up the parent builtin from gc_store,
// extracting the named sub-property, and caching it under the sub-path key.

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Sub-path alias definitions: (subpath_name, parent_module, property_name)
const SUBPATH_ALIASES: &[(&str, &str, &str)] = &[
    ("dns/promises", "dns", "promises"),
    ("path/posix", "path", "posix"),
    ("path/win32", "path", "win32"),
    ("readline/promises", "readline", "promises"),
    ("stream/promises", "stream", "promises"),
];

/// Install all sub-path alias modules.
pub fn install(cx: &mut mozjs::context::JSContext) {
    for &(subpath, parent, prop) in SUBPATH_ALIASES {
        install_subpath_alias(cx, subpath, parent, prop);
    }
}

/// Look up the parent module from the builtin cache, extract the sub-property,
/// and cache the result as the subpath module.
fn install_subpath_alias(
    cx: &mut mozjs::context::JSContext,
    subpath: &str,
    parent: &str,
    prop: &str,
) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = format!("builtin:{}", subpath);
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, &cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    let parent_key = format!("builtin:{}", parent);
    let parent_obj = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, &parent_key);
    let Some(parent_obj) = parent_obj else { return };
    if parent_obj.is_null() {
        return;
    }

    unsafe {
        let raw_cx = cx.raw_cx();
        rooted!(&in(cx) let parent_root = parent_obj);
        let c_prop = bun_core::ZBox::from_bytes(prop.as_bytes());
        let mut prop_val = UndefinedValue();
        JS_GetProperty(
            raw_cx,
            parent_root.handle().into(),
            c_prop.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut prop_val,
            },
        );

        if prop_val.is_object() {
            cache_builtin(cx, subpath, prop_val.to_object());
        } else {
            // Fallback: create an empty object if the property doesn't exist yet
            // (e.g. dns.promises might not be set up yet). This is better than
            // having nothing cached.
            rooted!(&in(cx) let alias_obj = w2::JS_NewPlainObject(cx));
            if !alias_obj.get().is_null() {
                cache_builtin(cx, subpath, alias_obj.get());
            }
        }
    }
}
