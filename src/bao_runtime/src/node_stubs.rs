// @trace REQ-ENG-006 [api:Node.js builtin stubs]
//
// Stub registrations for Node.js builtin modules that Bao does not yet
// implement natively. Each stub is an empty plain object so `require()` /
// `import` succeeds; Node.js code that probes for capability (e.g.
// `stubs.test.js`) gets a namespace instead of a `Cannot find module` error.
//
// Modules covered (Node.js builtin inventory, except those already
// implemented natively — fs/path/buffer/crypto/etc. live in their own
// `node_*` modules):
//
//   - `async_hooks`, `cluster`, `console`, `constants`, `dgram`,
//     `diagnostics_channel`, `domain`, `http2`, `inspector`,
//     `inspector/promises`, `punycode`, `repl`, `trace_events`, `v8`,
//     `worker_threads`, `sys`, `_http_*`, `_stream_*`, `_tls_*`
//   - Sub-path modules: `assert/strict`, `dns/promises`, `fs/promises`,
//     `path/posix`, `path/win32`, `readline/promises`, `stream/consumers`,
//     `stream/promises`, `stream/web`, `timers/promises`, `util/types`,
//     `inspector/promises`
//
// Each registered object carries a `__stub: true` marker for debugging
// (non-enumerable). Real implementations replace the stub when they ship.

use mozjs::jsapi::*;
use mozjs::jsval::ObjectValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// All stubbed module specifiers (bare name; `node:` prefix is added
/// automatically by `cache_builtin` consumers via strip_prefix).
const STUB_MODULES: &[&str] = &[
    // Top-level Node.js builtins not natively implemented
    "async_hooks",
    "cluster",
    "console",
    "constants",
    "dgram",
    "diagnostics_channel",
    "domain",
    "http2",
    "inspector",
    "punycode",
    "repl",
    "trace_events",
    "v8",
    "worker_threads",
    "sys",
    // Internal underscore-prefixed (Node.js internal modules exposed as builtins)
    "_http_agent",
    "_http_client",
    "_http_common",
    "_http_incoming",
    "_http_outgoing",
    "_http_server",
    "_stream_duplex",
    "_stream_passthrough",
    "_stream_readable",
    "_stream_transform",
    "_stream_wrap",
    "_stream_writable",
    "_tls_common",
    "_tls_wrap",
    // Sub-path modules
    "assert/strict",
    "dns/promises",
    "fs/promises",
    "path/posix",
    "path/win32",
    "readline/promises",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "util/types",
    "inspector/promises",
];

/// Register a single empty stub object under the given builtin key.
fn register_stub(cx: &mut mozjs::context::JSContext, name: &str) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() {
        return;
    }
    // Tag the stub so user code can detect that it is a placeholder.
    // Non-enumerable to keep `Object.keys()` clean.
    unsafe {
        let raw_cx = cx.raw_cx();
        let true_val = mozjs::jsval::BooleanValue(true);
        let h = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &true_val,
        };
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"__stub".as_ptr(),
            h,
            0,
        );
    }
    cache_builtin(cx, name, obj.get());
}

/// Register all unimplemented Node.js builtins as empty stubs.
pub fn install(cx: &mut mozjs::context::JSContext) {
    for &name in STUB_MODULES {
        register_stub(cx, name);
    }
}
