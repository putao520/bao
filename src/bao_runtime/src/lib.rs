#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused_imports)]
// REQ-IMPL-01: Phase 1 SpiderMonkey engine replacement (completed)
// REQ-IMPL-02: Phase 2 servo engine integration + rendering (completed)
// REQ-IMPL-03: Phase 3 CDP Server implementation (completed)
// REQ-IMPL-04: Phase 4 Stealth anti-fingerprinting (completed)
// REQ-IMPL-05: Phase 5 Integration testing and release (completed)
pub mod bun_api;
pub mod fetch_api;
pub mod globals;
pub mod web_api;
pub mod node_buffer;
pub mod node_child_process;
pub mod node_crypto;
pub mod node_dns;
pub mod node_events;
pub mod node_fs;
pub mod node_http;
pub mod node_https;
pub mod node_net;
pub mod node_os;
pub mod node_path;
pub mod node_perf_hooks;
pub mod node_querystring;
pub mod node_readline;
pub mod node_stream;
pub mod node_string_decoder;
pub mod node_timers_module;
pub mod node_url;
pub mod node_util;
pub mod node_zlib;
pub mod require;
pub mod runtime;
pub mod timers;

pub use runtime::BaoRuntime;

/// Safe JS string conversion: returns "" if JS string allocation fails
pub unsafe fn js_to_rust_string(cx: *mut mozjs::jsapi::JSContext, val: mozjs::jsval::JSVal) -> String {
    let ptr = val.to_string();
    match ::std::ptr::NonNull::new(ptr) {
        Some(nn) => mozjs::conversions::jsstr_to_string(cx, nn),
        None => String::new(),
    }
}

/// Safe JSString pointer conversion: returns "" if pointer is null  
pub unsafe fn jsstr_to_rust_string(cx: *mut mozjs::jsapi::JSContext, s: *mut mozjs::jsapi::JSString) -> String {
    match ::std::ptr::NonNull::new(s) {
        Some(nn) => mozjs::conversions::jsstr_to_string(cx, nn),
        None => String::new(),
    }
}
