// @trace REQ-ENG-006 [api:Bun.build CYCLEBREAK plugin seam] [req:REQ-ENG-006] [level:library]
//! SM-bridge definitions for the `JSBundlerPlugin` C ABI — the JS-plugin
//! (onLoad/onResolve/onBeforeParse) hooks `bun_bundler`'s pipeline references
//! at link time.
//!
//! Upstream these live in C++ (`JSBundlerPlugin.cpp`, backed by the JS
//! plugin registry). Bao registers no bundler plugins and every BundleV2
//! built through `build_api` passes `plugins: None`, so the runtime never
//! reaches a live plugin. The link-time bodies below implement the honest
//! **no-plugins** semantics: no match, no before-parse handlers, nothing
//! deferred to drain. When a JS plugin registry lands, these are replaced
//! by real dispatch — same symbols.

use bun_core::String as BunString;

/// Opaque plugin handle (mirrors `bun_bundler::bundle_v2::api::JSBundler::Plugin`).
#[allow(non_camel_case_types)]
pub type JSBundlerPlugin = bun_bundler::bundle_v2::JSBundlerPlugin;

/// `Plugin.anyMatches` — no plugins registered: never a match.
#[unsafe(no_mangle)]
pub extern "C" fn JSBundlerPlugin__anyMatches(
    _this: &JSBundlerPlugin,
    _namespace: &mut BunString,
    _path: &mut BunString,
    _is_on_load: bool,
) -> bool {
    false
}

/// `Plugin.matchOnLoad` — no plugins: the context callback is never invoked
/// (callers treat an un-invoked context as "no plugin matched").
#[unsafe(no_mangle)]
pub extern "C" fn JSBundlerPlugin__matchOnLoad(
    _this: &mut JSBundlerPlugin,
    _namespace_string: &mut BunString,
    _path: &mut BunString,
    _context: *mut core::ffi::c_void,
    _default_loader: u8,
    _is_server_side: bool,
) {
}

/// `Plugin.matchOnResolve` — no plugins: the context callback is never
/// invoked.
#[unsafe(no_mangle)]
pub extern "C" fn JSBundlerPlugin__matchOnResolve(
    _this: &mut JSBundlerPlugin,
    _namespace_string: &mut BunString,
    _path: &mut BunString,
    _importer: &mut BunString,
    _context: *mut core::ffi::c_void,
    _kind: u8,
) {
}

/// `Plugin.drainDeferred` — nothing was deferred: no-op.
#[unsafe(no_mangle)]
pub extern "C" fn JSBundlerPlugin__drainDeferred(_this: &mut JSBundlerPlugin, _rejected: bool) {}

/// `Plugin.hasOnBeforeParsePlugins` — no plugins: 0.
#[unsafe(no_mangle)]
pub extern "C" fn JSBundlerPlugin__hasOnBeforeParsePlugins(_this: &JSBundlerPlugin) -> i32 {
    0
}

/// `Plugin.callOnBeforeParsePlugins` — no plugins: 0 (nothing handled;
/// parsing proceeds normally).
#[unsafe(no_mangle)]
pub extern "C" fn JSBundlerPlugin__callOnBeforeParsePlugins(
    _this: &JSBundlerPlugin,
    _ctx: *mut core::ffi::c_void,
    _namespace: &BunString,
    _path: &BunString,
    _args: *mut core::ffi::c_void,
    _result: *mut core::ffi::c_void,
    _should_continue_running: *mut i32,
) -> i32 {
    0
}
