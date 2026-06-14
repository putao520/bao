// @trace REQ-ENG-001
//! Headers JSC — type compatibility stub for `bun_jsc::HeadersJsc`.
//!
//! JS Headers object registration is done in `bao_runtime::fetch_api`.
//! This module only provides the type surface for downstream compilation.

use mozjs::jsapi::{JSContext, JSObject};

pub struct HeadersJsc {
    _private: (),
}

impl HeadersJsc {
    /// Registration is handled by bao_runtime::fetch_api.
    /// This is a placeholder for API compatibility.
    pub fn install(_cx: *mut JSContext, _global: *mut JSObject) {}
}
