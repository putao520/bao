// @trace REQ-ENG-007 [api:node _tls_wrap module]
//
// Internal TLS wrap module. In Bun, `_tls_wrap` is an alias for `node:tls`
// (see HardcodedModule.zig: `.{ "_tls_wrap", .{ .path = "node:tls" ... } }`).
//
// Bao replicates this: look up the already-registered `builtin:tls` module
// from gc_store and cache it under the `_tls_wrap` key. This ensures
// `require("_tls_wrap")` returns the same object as `require("tls")`.

use mozjs::jsapi::*;
use mozjs::rooted;

/// Install _tls_wrap module — alias for the `tls` module.
pub fn install(cx: &mut mozjs::context::JSContext) {
    let cache_key = "builtin:_tls_wrap";
    // Guard: never clobber a natively-implemented module.
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    // Look up the tls module (already installed by node_tls::install)
    let tls_key = "builtin:tls";
    let tls_obj = match crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, tls_key) {
        Some(obj) if !obj.is_null() => obj,
        _ => return, // tls not yet registered; skip (will be available via stub fallback)
    };

    crate::require::cache_builtin(cx, "_tls_wrap", tls_obj);
}
