// @trace REQ-ENG-001 [module:realm_policy]
//
// SM-EVOLUTION #28 (verdict consumed 2026-09-10, user ruling — REQ-STL
// existing identity-consistency defect fix, not new legislation): the three
// host-derived identity dimensions that leaked from every page realm before
// this module, and their ENGINE-NATIVE sinks:
//
//   - locale    — `Intl.*` default locale (and locale-sensitive Date string
//                 methods) derived from the process environment (LANG/LC_*
//                 → ICU default): a zh-CN host leaked zh-CN into every page
//                 (`Intl.DateTimeFormat().resolvedOptions().locale`).
//   - timezone  — Date local-time methods ran in the host zone (a +0800 host
//                 leaked via `getTimezoneOffset` / `toString` offsets).
//   - precision — `Date.now()` / `getTime` carried full engine precision
//                 while the performance.now JS-hook layer was already
//                 quantized (the two-layer inconsistency is itself a
//                 fingerprint signal).
//
// Sink granularity (recorded honestly — engine-level, not per-page):
//
//   - locale: `JS_SetDefaultLocale` is RUNTIME-scoped. SpiderMonkey gives
//     every `JS_NewContext` a private JSRuntime, so the sink is per
//     ScriptThread context: pages sharing one script thread share the last
//     written locale. Per-realm locale (`RealmCreationOptions::
//     setLocaleCopyZ`) is C++-only — bindgen cannot construct its
//     `RefPtr<LocaleString>` field (SM-EVOLUTION #28 census, 28-1).
//   - timezone: `forceUTC_` is a CREATION-time-only per-realm flag with no
//     post-creation setter. This module does not touch it for bao-owned
//     realms directly — arming lives in `bun_sm::global_object`
//     (`set_node_force_utc`, consumed by `node_realm_options`) for
//     Node-semantics realms and in servo's realm-creation global
//     (`servo::set_force_utc_realms`, consumed by script_bindings'
//     `create_global_object`) for the DOM realms. Both MUST be armed before
//     the target realm is created (bao_browser arms them at page creation,
//     before the pipeline's realms exist).
//   - time precision: `JS::SetTimeResolutionUsec` writes a PROCESS-wide
//     static gating every Date read path (`jsdate.cpp` NowAsMillis) per
//     `RealmBehaviors::clampAndJitterTime_`, whose C++ default `true` is
//     preserved by both realm constructors (mozjs glue + servo
//     `RealmOptions::default()`). The DOM layer (performance.now & friends)
//     is clamped separately at servo's `ToDOMHighResTimeStamp` choke point —
//     bao_browser feeds BOTH sinks from the same `StealthProfile::timing`
//     field so the two layers stay on one grid.
//
// All three sinks are no-ops for stealth-free callers: nothing here runs
// unless a `StealthProfile` wires it (bao_browser is the only wiring site),
// so CLI/Node runs and stealth-free pages keep upstream host-derived
// behavior byte-for-byte.

use mozjs::jsapi::{
    JSContext as RawJSContext, JS_GetRuntime, JS_ResetDefaultLocale, JS_SetDefaultLocale,
    SetTimeResolutionUsec,
};

/// Set the JSRuntime default locale — the engine-native `Intl.*` identity.
///
/// Affects the default-locale resolution of every locale-sensitive builtin
/// (`Intl.DateTimeFormat()`, `toLocaleString`, locale-sensitive Date string
/// methods, ...) across ALL realms of the runtime owning `raw_cx`. The
/// locale string is copied by the engine; the CString only needs to outlive
/// the call. Returns `false` when the engine rejects the locale tag.
///
/// # Safety
/// `raw_cx` must be a live `*mut JSContext` on the caller's thread (SM
/// contexts are thread-affine). Callers on the servo script thread satisfy
/// this by construction (the embedder callback hands over the thread's own
/// context).
pub unsafe fn set_default_locale(raw_cx: *mut RawJSContext, locale: &str) -> bool {
    let c_locale = match std::ffi::CString::new(locale) {
        Ok(c) => c,
        Err(_) => return false,
    };
    JS_SetDefaultLocale(JS_GetRuntime(raw_cx), c_locale.as_ptr())
}

/// Drop any locale override and re-derive the runtime default from the OS
/// (restores upstream host-derived behavior for stealth-free pages).
///
/// # Safety
/// Same contract as [`set_default_locale`].
pub unsafe fn reset_default_locale(raw_cx: *mut RawJSContext) {
    JS_ResetDefaultLocale(JS_GetRuntime(raw_cx));
}

/// Apply the engine-native Date time-resolution clamp (microsecond grid,
/// no jitter — Chrome desktop shape; Firefox RFP enables jitter, Chrome
/// only coarsens).
///
/// `resolution_us == 0` disables clamping. PROCESS-wide static: gates every
/// Date read path in every realm of the process (per-realm opt-out does not
/// exist for the resolution itself; per-realm gating is only the
/// `clampAndJitterTime_` behaviors flag, kept at its C++ default `true`).
pub fn set_time_resolution_usec(resolution_us: u32) {
    unsafe { SetTimeResolutionUsec(resolution_us, false) };
}
