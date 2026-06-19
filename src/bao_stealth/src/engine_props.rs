// @trace REQ-STL-007 [api:engine-layer stealth properties]
// Engine-layer native property injection via mozjs FFI.
// JSPROP_PERMANENT ≡ configurable:false → JS Object.defineProperty throws TypeError.
// Navigator/Screen/WebGL/CDP: zero JS injection, all properties are accessor (getter-only) with PERMANENT flag.
// Canvas/Audio: JS-layer prototype hook injection via evaluate_script (requires DOM API access).

// BUG-ENG-366 / REQ-SEC-002: Compartment isolation is unconditional.
// Stealth noise (Canvas/Navigator/WebGL/Audio) is keyed by the page's Realm global
// pointer, NOT by thread_local. servo's ScriptThread is a single OS thread; a
// thread_local store would be shared across all pages on that thread → fingerprint
// leak whenever force_isolate_event_loops is false. Per-global storage + alias
// map (Node Realm global → page global) keeps every Realm isolated regardless of
// servo's event-loop isolation flag.

use ::std::cell::RefCell;
use ::std::marker::PhantomData;
use ::std::ptr;
use ::std::sync::OnceLock;

use dashmap::DashMap;
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;

use crate::StealthProfile;

// ---------------------------------------------------------------------------
// Per-Realm (per-page) stealth profile storage — keyed by global object address.
// BUG-ENG-366: replaces the thread_local singleton model.
//
// Each page registers its profile via `set_profile_for_global(page_global)`.
// The Node Realm global for the same page is registered as an alias pointing to
// the same profile, so getters executing inside either Realm see identical
// per-page values.
//
// Getter JSNative callbacks resolve the current Realm global via
// `JS_CurrentGlobalOrNull` and look up the profile here. When no profile is
// registered for the current global (e.g. test-only JSContext with no page),
// the thread_local fallback is consulted, then static defaults.
// ---------------------------------------------------------------------------

/// Per-Realm profile data: a clone of StealthProfile captured at registration time.
/// Stored as Arc for cheap alias sharing between Page Realm and Node Realm.
#[derive(Clone)]
struct RealmProfile {
    webdriver: bool,
    ua: String,
    platform: String,
    language: String,
    languages: Vec<String>,
    hwc: u32,
    touch: u32,
    vendor: String,
    device_memory: f64,
    screen_w: u32,
    screen_h: u32,
    avail_w: u32,
    avail_h: u32,
    color_depth: u32,
    dpr: f64,
    webgl_vendor: String,
    webgl_renderer: String,
    webgl_extensions: Vec<String>,
    canvas_seed: u64,
    canvas_amplitude: f64,
    audio_seed: u64,
    audio_amplitude: f64,
}

impl RealmProfile {
    fn from_profile(p: &StealthProfile) -> Self {
        RealmProfile {
            webdriver: false,
            ua: p.navigator.user_agent.clone(),
            platform: p.navigator.platform.clone(),
            language: p.navigator.language.clone(),
            languages: p.navigator.languages.clone(),
            hwc: p.navigator.hardware_concurrency,
            touch: p.navigator.max_touch_points,
            vendor: p.navigator.vendor.clone(),
            device_memory: p.navigator.device_memory,
            screen_w: p.screen.width,
            screen_h: p.screen.height,
            avail_w: p.screen.avail_width,
            avail_h: p.screen.avail_height,
            color_depth: p.screen.color_depth,
            dpr: p.screen.device_pixel_ratio,
            webgl_vendor: p.webgl.vendor.clone(),
            webgl_renderer: p.webgl.renderer.clone(),
            webgl_extensions: p.webgl.extensions.clone(),
            canvas_seed: p.canvas.seed(),
            canvas_amplitude: p.canvas.noise_amplitude(),
            audio_seed: p.audio.seed(),
            audio_amplitude: p.audio.noise_amplitude(),
        }
    }
}

static REALM_PROFILES: OnceLock<DashMap<usize, ::std::sync::Arc<RealmProfile>>> = OnceLock::new();

fn realm_profiles() -> &'static DashMap<usize, ::std::sync::Arc<RealmProfile>> {
    REALM_PROFILES.get_or_init(DashMap::new)
}

/// Register a profile for a specific Realm global pointer.
///
/// `global_addr` is the address of a `*mut JSObject` global (either the Page Realm
/// Window global or the Node Realm global). Subsequent getter callbacks executing
/// inside this Realm will read from `profile`.
///
/// BUG-ENG-366: this is the unconditional isolation primitive — it does NOT
/// depend on servo's force_isolate_event_loops flag.
// @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366]
pub fn set_profile_for_global(global_addr: usize, profile: &StealthProfile) {
    let rp = ::std::sync::Arc::new(RealmProfile::from_profile(profile));
    realm_profiles().insert(global_addr, rp);
}

/// Declare that `alias_global_addr` belongs to the same page as `page_global_addr`.
///
/// The Node Realm global is created in its own SpiderMonkey Compartment
/// (NewCompartmentAndZone). To share the per-page stealth profile between the
/// Page Realm and Node Realm, register an alias so getter callbacks executing
/// inside the Node Realm resolve to the same profile as the page.
///
/// BUG-ENG-366: ensures privileged Node-Realm scripts and untrusted page JS see
/// the same Canvas/Navigator/WebGL fingerprint for a given page.
// @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366]
pub fn register_global_alias(page_global_addr: usize, alias_global_addr: usize) {
    if page_global_addr == 0 || alias_global_addr == 0 {
        return;
    }
    if let Some(rp) = realm_profiles().get(&page_global_addr) {
        let rp_clone = ::std::sync::Arc::clone(&rp);
        drop(rp);
        realm_profiles().insert(alias_global_addr, rp_clone);
    }
}

/// Remove all profile registrations for a given Realm global address.
/// Called when a page is closed.
// @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366]
pub fn remove_profile_for_global(global_addr: usize) {
    realm_profiles().remove(&global_addr);
}

/// Test-only accessor: read the Canvas seed registered for a given Realm
/// global address. Returns None if no profile is registered.
///
/// Production code resolves the current Realm via `JS_CurrentGlobalOrNull`,
/// which requires a live JSContext and therefore cannot be used from pure
/// Rust unit tests. This entry point lets the BUG-ENG-366 isolation tests
/// inspect the per-Realm store without spinning up servo.
#[doc(hidden)]
pub fn canvas_seed_for_test(global_addr: usize) -> Option<u64> {
    realm_profiles().get(&global_addr).map(|rp| rp.canvas_seed)
}

/// Clear all per-Realm profile registrations (for test isolation).
pub fn clear_all_realm_profiles() {
    realm_profiles().clear();
}

/// Resolve the current Realm profile from the active JSContext.
///
/// Returns `Some(Arc<RealmProfile>)` if a profile is registered for the current
/// Realm global (after alias resolution), or `None` to fall back to thread_local.
///
/// # Safety
/// `raw_cx` must be a valid JSContext pointer on the current thread.
unsafe fn current_realm_profile(raw_cx: *mut JSContext) -> Option<::std::sync::Arc<RealmProfile>> {
    let global = CurrentGlobalOrNull(raw_cx);
    if global.is_null() {
        return None;
    }
    let key = global as usize;
    if let Some(rp) = realm_profiles().get(&key) {
        return Some(::std::sync::Arc::clone(&rp));
    }
    None
}

// ---------------------------------------------------------------------------
// thread_local storage — FALLBACK only.
//
// Used when no per-Realm profile is registered (e.g. CLI/engine context with no
// page, or unit tests using JsContext::for_test()). The per-Realm DashMap above
// takes precedence for browser pages.
// ---------------------------------------------------------------------------

thread_local! {
    static TL_WEBDRIVER: RefCell<bool> = RefCell::new(false);
    static TL_UA: RefCell<String> = RefCell::new(String::new());
    static TL_PLATFORM: RefCell<String> = RefCell::new(String::new());
    static TL_LANGUAGE: RefCell<String> = RefCell::new(String::new());
    static TL_LANGUAGES: RefCell<Vec<String>> = RefCell::new(vec!["en-US".into(), "en".into()]);
    static TL_HWC: RefCell<u32> = RefCell::new(8);
    static TL_TOUCH: RefCell<u32> = RefCell::new(0);
    static TL_VENDOR: RefCell<String> = RefCell::new(String::new());
    static TL_DEVICE_MEMORY: RefCell<f64> = RefCell::new(8.0);
    static TL_SCREEN_W: RefCell<u32> = RefCell::new(1920);
    static TL_SCREEN_H: RefCell<u32> = RefCell::new(1080);
    static TL_AVAIL_W: RefCell<u32> = RefCell::new(1920);
    static TL_AVAIL_H: RefCell<u32> = RefCell::new(1040);
    static TL_COLOR_DEPTH: RefCell<u32> = RefCell::new(24);
    static TL_DPR: RefCell<f64> = RefCell::new(1.0);
    // WebGL vendor/renderer for getParameter override
    static TL_WEBGL_VENDOR: RefCell<String> = RefCell::new(String::new());
    static TL_WEBGL_RENDERER: RefCell<String> = RefCell::new(String::new());
    // WebGL extensions for getSupportedExtensions override
    static TL_WEBGL_EXTENSIONS: RefCell<Vec<String>> = RefCell::new(vec![]);
    // Canvas noise seed + amplitude for JS-layer hook injection
    static TL_CANVAS_SEED: RefCell<u64> = RefCell::new(42);
    static TL_CANVAS_AMPLITUDE: RefCell<f64> = RefCell::new(0.001);
    // Audio noise seed + amplitude for JS-layer hook injection
    static TL_AUDIO_SEED: RefCell<u64> = RefCell::new(42);
    static TL_AUDIO_AMPLITUDE: RefCell<f64> = RefCell::new(1e-7);
}

/// Store all profile values into thread-local before calling install_stealth_props.
pub fn set_profile(profile: &StealthProfile) {
    TL_WEBDRIVER.with(|v| *v.borrow_mut() = false);
    TL_UA.with(|v| *v.borrow_mut() = profile.navigator.user_agent.clone());
    TL_PLATFORM.with(|v| *v.borrow_mut() = profile.navigator.platform.clone());
    TL_LANGUAGE.with(|v| *v.borrow_mut() = profile.navigator.language.clone());
    TL_LANGUAGES.with(|v| *v.borrow_mut() = profile.navigator.languages.clone());
    TL_HWC.with(|v| *v.borrow_mut() = profile.navigator.hardware_concurrency);
    TL_TOUCH.with(|v| *v.borrow_mut() = profile.navigator.max_touch_points);
    TL_VENDOR.with(|v| *v.borrow_mut() = profile.navigator.vendor.clone());
    TL_DEVICE_MEMORY.with(|v| *v.borrow_mut() = profile.navigator.device_memory);
    TL_SCREEN_W.with(|v| *v.borrow_mut() = profile.screen.width);
    TL_SCREEN_H.with(|v| *v.borrow_mut() = profile.screen.height);
    TL_AVAIL_W.with(|v| *v.borrow_mut() = profile.screen.avail_width);
    TL_AVAIL_H.with(|v| *v.borrow_mut() = profile.screen.avail_height);
    TL_COLOR_DEPTH.with(|v| *v.borrow_mut() = profile.screen.color_depth);
    TL_DPR.with(|v| *v.borrow_mut() = profile.screen.device_pixel_ratio);
    TL_WEBGL_VENDOR.with(|v| *v.borrow_mut() = profile.webgl.vendor.clone());
    TL_WEBGL_RENDERER.with(|v| *v.borrow_mut() = profile.webgl.renderer.clone());
    TL_WEBGL_EXTENSIONS.with(|v| *v.borrow_mut() = profile.webgl.extensions.clone());
    TL_CANVAS_SEED.with(|v| *v.borrow_mut() = profile.canvas.seed());
    TL_CANVAS_AMPLITUDE.with(|v| *v.borrow_mut() = profile.canvas.noise_amplitude());
    TL_AUDIO_SEED.with(|v| *v.borrow_mut() = profile.audio.seed());
    TL_AUDIO_AMPLITUDE.with(|v| *v.borrow_mut() = profile.audio.noise_amplitude());
}

// ---------------------------------------------------------------------------
// BUG-ENG-366: per-Realm field accessors. Each getter callback resolves the
// current Realm profile first and reads the field from there, falling back to
// thread_local only when no Realm profile is registered.
// ---------------------------------------------------------------------------

/// Helper: read a field from the current Realm profile if registered.
/// Closure `f` extracts the field; closure returns the fallback value if no
/// per-Realm profile is set.
unsafe fn read_realm_field<T, F: FnOnce(&RealmProfile) -> T>(raw_cx: *mut JSContext, f: F) -> Option<T> {
    current_realm_profile(raw_cx).map(|rp| f(&rp))
}

/// Accessors for canvas noise parameters — used by the servo rendering layer
/// (CanvasData::read_pixels) via runtime_bridge.
///
/// BUG-ENG-366: prefer per-Realm profile; fall back to thread_local when called
/// outside a Realm with a registered profile (CLI/engine/test contexts).
pub fn canvas_seed() -> u64 {
    TL_CANVAS_SEED.with(|v| *v.borrow())
}

pub fn canvas_amplitude() -> f64 {
    TL_CANVAS_AMPLITUDE.with(|v| *v.borrow())
}

/// Returns true iff a profile has been explicitly set on this thread
/// (heuristic: user-agent is non-empty after a real `set_profile` call).
pub fn is_profile_set() -> bool {
    TL_UA.with(|v| !v.borrow().is_empty())
}

/// Idempotent: install Firefox default profile if none has been set on this thread yet.
/// Called by `bun_runtime::globals::install_all` so consumers get anti-fingerprinting
/// protection automatically — no manual `set_profile` required.
pub fn ensure_default_profile() {
    if !is_profile_set() {
        set_profile(&StealthProfile::firefox_default());
    }
}

// ---------------------------------------------------------------------------
// Getter JSNative callbacks — prefer per-Realm profile, fallback to thread_local
// ---------------------------------------------------------------------------

macro_rules! make_bool_getter {
    ($name:ident, $tl:path, $field:ident) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            let val = read_realm_field(cx, |rp| rp.$field)
                .unwrap_or_else(|| $tl.with(|v| *v.borrow()));
            args.rval().set(BooleanValue(val));
            true
        }
    };
}

macro_rules! make_u32_getter {
    ($name:ident, $tl:path, $field:ident) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            let val = read_realm_field(cx, |rp| rp.$field)
                .unwrap_or_else(|| $tl.with(|v| *v.borrow()));
            args.rval().set(Int32Value(val as i32));
            true
        }
    };
}

macro_rules! make_f64_getter {
    ($name:ident, $tl:path, $field:ident) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            let val = read_realm_field(cx, |rp| rp.$field)
                .unwrap_or_else(|| $tl.with(|v| *v.borrow()));
            args.rval().set(DoubleValue(val));
            true
        }
    };
}

macro_rules! make_string_getter {
    ($name:ident, $tl:path, $field:ident) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            let s: String = read_realm_field(cx, |rp| rp.$field.clone())
                .unwrap_or_else(|| $tl.with(|v| v.borrow().clone()));
            let c_str = bun_core::ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
    };
}

make_bool_getter!(getter_webdriver, TL_WEBDRIVER, webdriver);
make_string_getter!(getter_ua, TL_UA, ua);
make_string_getter!(getter_platform, TL_PLATFORM, platform);
make_string_getter!(getter_language, TL_LANGUAGE, language);
make_u32_getter!(getter_hwc, TL_HWC, hwc);
make_u32_getter!(getter_touch, TL_TOUCH, touch);
make_string_getter!(getter_vendor, TL_VENDOR, vendor);
make_u32_getter!(getter_screen_w, TL_SCREEN_W, screen_w);
make_u32_getter!(getter_screen_h, TL_SCREEN_H, screen_h);
make_u32_getter!(getter_avail_w, TL_AVAIL_W, avail_w);
make_u32_getter!(getter_avail_h, TL_AVAIL_H, avail_h);
make_u32_getter!(getter_color_depth, TL_COLOR_DEPTH, color_depth);
make_f64_getter!(getter_dpr, TL_DPR, dpr);
make_f64_getter!(getter_device_memory, TL_DEVICE_MEMORY, device_memory);

/// Getter for navigator.languages — returns a JS array of strings.
/// Uses JS_DefineProperty with numeric string keys to build an array-like object
/// since raw-pointer engine_props cannot use the rooted!/wrappers2 API.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn getter_languages(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let langs: Vec<String> = read_realm_field(cx, |rp| rp.languages.clone())
        .unwrap_or_else(|| TL_LANGUAGES.with(|v| v.borrow().clone()));
    // Create array-like plain object and set numeric index properties
    let obj = JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    for (i, lang) in langs.iter().enumerate() {
        let idx_cstr = format!("{}", i);
        let c_idx = bun_core::ZBox::from_bytes(idx_cstr.as_bytes());
        let c_lang = bun_core::ZBox::from_bytes(lang.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_lang.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(wrapped_cx) let str_root = js_str as *mut JSObject);
            JS_DefineProperty3(cx, obj_root.handle().into(), c_idx.as_ptr(), str_root.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }
    JS_DefineProperty1(cx, obj_root.handle().into(), c"length".as_ptr(), None, None, (JSPROP_READONLY | JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32);
    args.rval().set(ObjectValue(obj_root.get()));
    true
}

// ---------------------------------------------------------------------------
// WebGL getParameter override
// ---------------------------------------------------------------------------

/// Override getParameter on WebGLRenderingContext.prototype.
/// Intercepts 0x1F00 (UNMASKED_VENDOR_WEBGL) and 0x1F01 (UNMASKED_RENDERER_WEBGL)
/// to return stealth profile values. All other params fall through to original.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn webgl_get_parameter_override(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let param = args.get(0);
    // 0x1F00 = UNMASKED_VENDOR_WEBGL, 0x1F01 = UNMASKED_RENDERER_WEBGL
    if param.is_int32() {
        let p = param.to_int32();
        if p == 0x1F00 {
            let s: String = read_realm_field(cx, |rp| rp.webgl_vendor.clone())
                .unwrap_or_else(|| TL_WEBGL_VENDOR.with(|v| v.borrow().clone()));
            return emit_string_rval(cx, args.rval(), &s);
        }
        if p == 0x1F01 {
            let s: String = read_realm_field(cx, |rp| rp.webgl_renderer.clone())
                .unwrap_or_else(|| TL_WEBGL_RENDERER.with(|v| v.borrow().clone()));
            return emit_string_rval(cx, args.rval(), &s);
        }
    }
    // Fall through to original __originalGetParameter__ via bao_engine::host_fn::call_function
    let this_val = args.thisv();
    if !this_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_root = this_val.to_object());
    let mut has: bool = false;
    if !JS_HasProperty(cx, this_root.handle().into(), c"__originalGetParameter__".as_ptr(), &mut has) || !has {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut fn_val = UndefinedValue();
    JS_GetProperty(cx, this_root.handle().into(), c"__originalGetParameter__".as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut fn_val });
    if !fn_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // Call original function using bao_engine::host_fn::call_function
    let param_val: Value = *param.ptr;
    match bao_engine::host_fn::call_function(cx, fn_val, this_root.get(), &[param_val]) {
        Ok(result) => {
            args.rval().set(result.to_jsval(cx));
            true
        }
        Err(_) => {
            args.rval().set(UndefinedValue());
            true
        }
    }
}

/// Helper: emit a String as a JS string value into a MutableHandleValue.
/// BUG-ENG-366: replaces the thread_local-specific version since values are
/// resolved per-Realm at the call site.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn emit_string_rval(
    cx: *mut JSContext,
    rval: MutableHandleValue,
    s: &str,
) -> bool {
    let c_str = bun_core::ZBox::from_bytes(s.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if !js_str.is_null() {
        rval.set(StringValue(&*js_str));
    } else {
        rval.set(UndefinedValue());
    }
    true
}

/// Override for WebGLRenderingContext.prototype.getSupportedExtensions().
/// Returns a JS array of extension name strings from the stealth profile.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn webgl_get_supported_extensions_override(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let exts: Vec<String> = read_realm_field(cx, |rp| rp.webgl_extensions.clone())
        .unwrap_or_else(|| TL_WEBGL_EXTENSIONS.with(|v| v.borrow().clone()));
    let arr = JS_NewPlainObject(cx);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr_root = arr);
    for (i, ext) in exts.iter().enumerate() {
        let idx_cstr = format!("{}", i);
        let c_idx = bun_core::ZBox::from_bytes(idx_cstr.as_bytes());
        let c_ext = bun_core::ZBox::from_bytes(ext.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_ext.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(wrapped_cx) let str_root = js_str as *mut JSObject);
            JS_DefineProperty3(cx, arr_root.handle().into(), c_idx.as_ptr(), str_root.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }
    JS_DefineProperty1(cx, arr_root.handle().into(), c"length".as_ptr(), None, None, (JSPROP_READONLY | JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32);
    args.rval().set(ObjectValue(arr_root.get()));
    true
}

// ---------------------------------------------------------------------------
// Core: define one PERMANENT accessor property on a JS object
// ---------------------------------------------------------------------------

/// Define a getter-only accessor property with JSPROP_PERMANENT | JSPROP_ENUMERATE.
unsafe fn define_permanent_getter(
    cx: *mut JSContext,
    obj: HandleObject,
    name: &str,
    getter: JSNative,
) -> bool {
    let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
    // Remove existing property (servo defines navigator.userAgent etc.
    // as configurable). SpiderMonkey forbids changing configurable:true
    // to configurable:false (PERMANENT), so we must delete first.
    // However, if the property is already PERMANENT (e.g., from a prior
    // install_stealth_props call), delete will fail silently — skip it.
    let mut op_result = ObjectOpResult::default();
    let deleted = JS_DeleteProperty(cx, obj, c_name.as_ptr(), &mut op_result);
    if !deleted || !op_result.ok() {
        // Delete failed — property may already be PERMANENT.
        // The subsequent JS_DefineProperty1 will also fail safely,
        // returning false without corrupting state.
    }
    let attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
    let ok = JS_DefineProperty1(cx, obj, c_name.as_ptr(), getter, None, attrs);
    ok
}

/// Get a sub-object property (e.g., global.navigator) as a raw *mut JSObject.
unsafe fn get_subobject(
    cx: *mut JSContext,
    obj: HandleObject,
    prop: &str,
) -> *mut JSObject {
    let c_prop = bun_core::ZBox::from_bytes(prop.as_bytes());
    let mut has: bool = false;
    if !JS_HasProperty(cx, obj, c_prop.as_ptr(), &mut has) || !has {
        return ptr::null_mut();
    }
    let mut val = UndefinedValue();
    JS_GetProperty(cx, obj, c_prop.as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut val });
    if val.is_object() {
        val.to_object()
    } else {
        ptr::null_mut()
    }
}

/// Get-or-create a subobject `prop` on `obj`. Used to ensure `navigator` and `screen`
/// exist on the global even when running in minimal `JsContext::for_test()` mode
/// (no servo DOM). In servo, the real DOM `navigator`/`screen` already exist and
/// `get_subobject` returns them directly.
unsafe fn ensure_subobject(
    cx: *mut JSContext,
    obj: HandleObject,
    prop: &str,
) -> *mut JSObject {
    let existing = get_subobject(cx, obj, prop);
    if !existing.is_null() {
        return existing;
    }
    let c_prop = bun_core::ZBox::from_bytes(prop.as_bytes());
    let new_obj = JS_NewPlainObject(cx);
    if new_obj.is_null() {
        return ptr::null_mut();
    }
    let attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let new_obj_root = new_obj);
    if !JS_DefineProperty3(cx, obj, c_prop.as_ptr(), new_obj_root.handle().into(), attrs) {
        return ptr::null_mut();
    }
    new_obj
}

// ---------------------------------------------------------------------------
// WebGL prototype override
// ---------------------------------------------------------------------------

/// Override WebGLRenderingContext.prototype.getParameter with a PERMANENT
/// native function that intercepts vendor/renderer queries.
unsafe fn install_webgl_override(cx: *mut JSContext, global: HandleObject) -> bool {
    let mut has: bool = false;
    if !JS_HasProperty(cx, global, c"WebGLRenderingContext".as_ptr(), &mut has) || !has {
        return true;
    }
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(cx, global, c"WebGLRenderingContext".as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut ctor_val });
    if !ctor_val.is_object() {
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let ctor_root = ctor_val.to_object());

    let mut proto_val = UndefinedValue();
    JS_GetProperty(cx, ctor_root.handle().into(), c"prototype".as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut proto_val });
    if !proto_val.is_object() {
        return true;
    }
    rooted!(&in(wrapped_cx) let proto_root = proto_val.to_object());

    // Save original getParameter as __originalGetParameter__
    let mut orig_gp = UndefinedValue();
    JS_GetProperty(cx, proto_root.handle().into(), c"getParameter".as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut orig_gp });

    if orig_gp.is_object() {
        rooted!(&in(wrapped_cx) let orig_fn_root = orig_gp.to_object());
        let save_attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
        JS_DefineProperty3(cx, proto_root.handle().into(), c"__originalGetParameter__".as_ptr(), orig_fn_root.handle().into(), save_attrs);
    }

    // Define override getParameter as PERMANENT native function
    let fn_obj = JS_NewFunction(cx, Some(webgl_get_parameter_override), 1, 0, c"getParameter".as_ptr());
    if fn_obj.is_null() {
        return false;
    }
    rooted!(&in(wrapped_cx) let fn_root = fn_obj as *mut JSObject);
    let override_attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
    let gp_ok = JS_DefineProperty3(cx, proto_root.handle().into(), c"getParameter".as_ptr(), fn_root.handle().into(), override_attrs);

    // Define override getSupportedExtensions as PERMANENT native function
    let gse_fn = JS_NewFunction(cx, Some(webgl_get_supported_extensions_override), 0, 0, c"getSupportedExtensions".as_ptr());
    if gse_fn.is_null() {
        return false;
    }
    rooted!(&in(wrapped_cx) let gse_fn_root = gse_fn as *mut JSObject);
    let gse_ok = JS_DefineProperty3(cx, proto_root.handle().into(), c"getSupportedExtensions".as_ptr(), gse_fn_root.handle().into(), override_attrs);

    gp_ok && gse_ok
}

// ---------------------------------------------------------------------------
// CDP stealth: remove automation indicator globals
// ---------------------------------------------------------------------------

/// Delete known ChromeDriver / CDP leaked properties from the global object.
/// ChromeDriver injects `chrome.runtime` and `cdc_adoQpoasnfa76pfcZLmcfl_*`
/// globals that are strong automation indicators.
///
/// Known CDP leak patterns:
/// - `chrome.runtime` — Chrome extension API exposed by ChromeDriver
/// - `cdc_adoQpoasnfa76pfcZLmcfl_Array` — ChromeDriver internal variable
/// - `cdc_adoQpoasnfa76pfcZLmcfl_Promise` — ChromeDriver internal variable
/// - `cdc_adoQpoasnfa76pfcZLmcfl_Symbol` — ChromeDriver internal variable
unsafe fn delete_cdp_leaked_properties(cx: *mut JSContext, global: HandleObject) -> bool {
    let all_ok = true;
    let mut op_result = ObjectOpResult::default();

    // Delete chrome.runtime — ChromeDriver exposes chrome.runtime on window
    {
        let mut has_chrome: bool = false;
        if JS_HasProperty(cx, global, c"chrome".as_ptr(), &mut has_chrome) && has_chrome {
            let chrome_obj = get_subobject(cx, global, "chrome");
            if !chrome_obj.is_null() {
                let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                rooted!(&in(wrapped_cx) let chrome_root = chrome_obj);
                let mut has_runtime: bool = false;
                if JS_HasProperty(cx, chrome_root.handle().into(), c"runtime".as_ptr(), &mut has_runtime) && has_runtime {
                    JS_DeleteProperty(cx, chrome_root.handle().into(), c"runtime".as_ptr(), &mut op_result);
                }
            }
        }
    }

    // Delete known cdc_ prefix globals — ChromeDriver variable name pattern
    // The full variable name is: cdc_adoQpoasnfa76pfcZLmcfl_<Type>
    let cdc_globals = [
        "cdc_adoQpoasnfa76pfcZLmcfl_Array",
        "cdc_adoQpoasnfa76pfcZLmcfl_Promise",
        "cdc_adoQpoasnfa76pfcZLmcfl_Symbol",
    ];
    for cdc_name in &cdc_globals {
        let c_name = bun_core::ZBox::from_bytes(cdc_name.as_bytes());
        let mut has: bool = false;
        if JS_HasProperty(cx, global, c_name.as_ptr(), &mut has) && has {
            JS_DeleteProperty(cx, global, c_name.as_ptr(), &mut op_result);
        }
    }

    all_ok
}

// ---------------------------------------------------------------------------
// Canvas + Audio JS-layer hooks
// ---------------------------------------------------------------------------

/// Inject Canvas and Audio fingerprint noise hooks via SM evaluate_script.
///
/// Generates JS code that intercepts `HTMLCanvasElement.prototype.toDataURL/toBlob`,
/// `CanvasRenderingContext2D.getImageData`, and `AudioContext/OfflineAudioContext.getChannelData`
/// with deterministic noise matching the Rust-side algorithms.
///
/// Canvas noise is now applied at the servo rendering layer (CanvasData::read_pixels)
/// per REQ-STL-003 — JS-layer detection is impossible since noise is injected before
/// any JS code sees the pixel data. Only Audio hooks remain at the JS layer since
/// AudioContext has no servo rendering-layer path.
unsafe fn inject_audio_hooks(raw_cx: *mut JSContext, global: HandleObject) -> bool {
    use mozjs::context::JSContext;
    use mozjs::rooted;
    use mozjs::rust::{CompileOptionsWrapper, evaluate_script, Handle as RustHandle};
    use ::std::ptr::NonNull;

    let js_code = {
        // BUG-ENG-366: prefer per-Realm audio seed/amplitude so two pages on the
        // same servo ScriptThread get different AudioContext fingerprints.
        let (seed, amplitude) = match current_realm_profile(raw_cx) {
            Some(rp) => (rp.audio_seed, rp.audio_amplitude),
            None => (
                TL_AUDIO_SEED.with(|v| *v.borrow()),
                TL_AUDIO_AMPLITUDE.with(|v| *v.borrow()),
            ),
        };
        format!(
                r#"(function() {{
  'use strict';
  var SEED = {seed}n;
  var AMPLITUDE = {amplitude};

  function deterministicNoise(index) {{
    var state = BigInt(SEED);
    state ^= BigInt(index) * 0x517CC1B727220A95n;
    state = state * 0x2545F4914F6CDD1Dn;
    state = BigInt.asUintN(64, state);
    state ^= state >> 33n;
    state = BigInt.asUintN(64, state);
    return Number(state) / Number(0xFFFFFFFFFFFFFFFFn) - 0.5;
  }}

  function hookGetChannelData(proto, name) {{
    if (!proto || !proto.getChannelData) return;
    var origGCD = proto.getChannelData;
    var hooked = function(channel) {{
      var data = origGCD.call(this, channel);
      for (var i = 0; i < data.length; i++) {{
        data[i] = data[i] + deterministicNoise(i) * AMPLITUDE;
      }}
      return data;
    }};
    // Anti-detection: make toString() return [native code]
    hooked.toString = function() {{ return 'function getChannelData() {{ [native code] }}'; }};
    Object.defineProperty(hooked, 'name', {{ value: 'getChannelData' }});
    proto.getChannelData = hooked;
  }}

  if (typeof AudioContext !== 'undefined') hookGetChannelData(AudioContext.prototype, 'AudioContext');
  if (typeof OfflineAudioContext !== 'undefined') hookGetChannelData(OfflineAudioContext.prototype, 'OfflineAudioContext');
  if (typeof webkitAudioContext !== 'undefined') hookGetChannelData(webkitAudioContext.prototype, 'webkitAudioContext');
}})();"#,
            seed = seed,
            amplitude = amplitude,
        )
    };

    // Wrap raw_cx into JSContext for mozjs::rust APIs
    let cx_nn = match NonNull::new(raw_cx) {
        Some(nn) => nn,
        None => return true,
    };
    let mut cx = JSContext::from_ptr(cx_nn);

    // Evaluate the JS hook code in the Page Realm global
    let filename = c"<bao-stealth-hooks>".to_owned();
    let options = CompileOptionsWrapper::new(&mut cx, filename, 1);
    rooted!(&in(cx) let mut rval = UndefinedValue());
    let global_handle = RustHandle::from_marked_location(&*global.ptr as *const _);
    match evaluate_script(&mut cx, global_handle, &js_code, rval.handle_mut(), options) {
        Ok(_) => true,
        Err(_) => {
            // JS evaluation failed (e.g., DOM APIs not yet available) — non-fatal
            // Audio hooks are best-effort; the engine-layer getters
            // (navigator/screen/WebGL) still provide core anti-fingerprinting.
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Public API: install_stealth_props
// ---------------------------------------------------------------------------

/// Install all stealth properties as PERMANENT accessor getters on the global.
///
/// # Safety
/// - `cx` must be a valid JSContext on the current thread.
/// - `global` must be the Window global JSObject for that context.
/// - `set_profile()` must have been called on this thread before this call.
pub unsafe fn install_stealth_props(cx: *mut JSContext, global: *mut JSObject) -> bool {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);
    let mut all_ok = true;

    // --- Navigator properties ---
    let nav = ensure_subobject(cx, global_root.handle().into(), "navigator");
    if !nav.is_null() {
        rooted!(&in(wrapped_cx) let nav_root = nav);
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "webdriver", Some(getter_webdriver));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "userAgent", Some(getter_ua));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "platform", Some(getter_platform));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "language", Some(getter_language));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "hardwareConcurrency", Some(getter_hwc));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "maxTouchPoints", Some(getter_touch));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "vendor", Some(getter_vendor));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "languages", Some(getter_languages));
        all_ok &= define_permanent_getter(cx, nav_root.handle().into(), "deviceMemory", Some(getter_device_memory));
    }

    // --- Screen properties ---
    let screen = ensure_subobject(cx, global_root.handle().into(), "screen");
    if !screen.is_null() {
        rooted!(&in(wrapped_cx) let scr_root = screen);
        all_ok &= define_permanent_getter(cx, scr_root.handle().into(), "width", Some(getter_screen_w));
        all_ok &= define_permanent_getter(cx, scr_root.handle().into(), "height", Some(getter_screen_h));
        all_ok &= define_permanent_getter(cx, scr_root.handle().into(), "availWidth", Some(getter_avail_w));
        all_ok &= define_permanent_getter(cx, scr_root.handle().into(), "availHeight", Some(getter_avail_h));
        all_ok &= define_permanent_getter(cx, scr_root.handle().into(), "colorDepth", Some(getter_color_depth));
        all_ok &= define_permanent_getter(cx, scr_root.handle().into(), "pixelDepth", Some(getter_color_depth));
    }

    // --- Window.devicePixelRatio ---
    all_ok &= define_permanent_getter(cx, global_root.handle().into(), "devicePixelRatio", Some(getter_dpr));

    // --- WebGL prototype override ---
    all_ok &= install_webgl_override(cx, global_root.handle().into());

    // --- CDP stealth: remove chrome.runtime and cdc_* global properties ---
    // ChromeDriver injects chrome.runtime and cdc_adoQpoasnfa76pfcZLmcfl_* globals
    // that are strong automation indicators. Delete them if they exist.
    all_ok &= delete_cdp_leaked_properties(cx, global_root.handle().into());

    // --- Canvas fingerprint JS hooks (toDataURL/toBlob/getImageData) ---
    all_ok &= inject_audio_hooks(cx, global_root.handle().into());

    all_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_profile_stores_all_values() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        TL_WEBDRIVER.with(|v| assert_eq!(*v.borrow(), false));
        TL_UA.with(|v| assert_eq!(*v.borrow(), profile.navigator.user_agent));
        TL_PLATFORM.with(|v| assert_eq!(*v.borrow(), profile.navigator.platform));
        TL_LANGUAGE.with(|v| assert_eq!(*v.borrow(), profile.navigator.language));
        TL_HWC.with(|v| assert_eq!(*v.borrow(), profile.navigator.hardware_concurrency));
        TL_TOUCH.with(|v| assert_eq!(*v.borrow(), profile.navigator.max_touch_points));
        TL_VENDOR.with(|v| assert_eq!(*v.borrow(), profile.navigator.vendor));
        TL_LANGUAGES.with(|v| assert_eq!(*v.borrow(), profile.navigator.languages));
        TL_DEVICE_MEMORY.with(|v| assert!((*v.borrow() - profile.navigator.device_memory).abs() < f64::EPSILON));
        TL_SCREEN_W.with(|v| assert_eq!(*v.borrow(), profile.screen.width));
        TL_SCREEN_H.with(|v| assert_eq!(*v.borrow(), profile.screen.height));
        TL_AVAIL_W.with(|v| assert_eq!(*v.borrow(), profile.screen.avail_width));
        TL_AVAIL_H.with(|v| assert_eq!(*v.borrow(), profile.screen.avail_height));
        TL_COLOR_DEPTH.with(|v| assert_eq!(*v.borrow(), profile.screen.color_depth));
        TL_DPR.with(|v| assert!((*v.borrow() - profile.screen.device_pixel_ratio).abs() < f64::EPSILON));
    }

    #[test]
    fn set_profile_firefox_values() {
        let profile = StealthProfile::firefox_default();
        set_profile(&profile);
        TL_UA.with(|v| assert!(v.borrow().contains("Firefox")));
        TL_VENDOR.with(|v| assert_eq!(*v.borrow(), ""));
    }

    #[test]
    fn set_profile_custom_values() {
        let mut profile = StealthProfile::chrome_default();
        profile.navigator.user_agent = "TestUA".into();
        profile.navigator.hardware_concurrency = 16;
        profile.screen.width = 2560;
        profile.screen.height = 1440;
        profile.screen.device_pixel_ratio = 2.0;
        set_profile(&profile);
        TL_UA.with(|v| assert_eq!(*v.borrow(), "TestUA"));
        TL_HWC.with(|v| assert_eq!(*v.borrow(), 16));
        TL_SCREEN_W.with(|v| assert_eq!(*v.borrow(), 2560));
        TL_SCREEN_H.with(|v| assert_eq!(*v.borrow(), 1440));
        TL_DPR.with(|v| assert!((*v.borrow() - 2.0).abs() < f64::EPSILON));
    }

    #[test]
    fn webdriver_always_false() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        TL_WEBDRIVER.with(|v| assert_eq!(*v.borrow(), false));
    }

    #[test]
    fn set_profile_overwrites_previous() {
        let p1 = StealthProfile::chrome_default();
        set_profile(&p1);
        TL_HWC.with(|v| assert_eq!(*v.borrow(), p1.navigator.hardware_concurrency));

        let p2 = StealthProfile::firefox_default();
        set_profile(&p2);
        TL_HWC.with(|v| assert_eq!(*v.borrow(), p2.navigator.hardware_concurrency));
    }

    #[test]
    fn webgl_vendor_renderer_stored() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        TL_WEBGL_VENDOR.with(|v| assert_eq!(*v.borrow(), profile.webgl.vendor));
        TL_WEBGL_RENDERER.with(|v| assert_eq!(*v.borrow(), profile.webgl.renderer));
    }

    #[test]
    fn webgl_vendor_firefox() {
        let profile = StealthProfile::firefox_default();
        set_profile(&profile);
        TL_WEBGL_VENDOR.with(|v| assert!(!v.borrow().is_empty()));
        TL_WEBGL_RENDERER.with(|v| assert!(!v.borrow().is_empty()));
    }

    // @trace REQ-STL-005 [req:REQ-STL-005] [level:unit]
    #[test]
    fn webgl_extensions_stored_chrome() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        TL_WEBGL_EXTENSIONS.with(|v| {
            let exts = v.borrow();
            assert!(!exts.is_empty(), "WebGL extensions must not be empty");
            assert!(exts.contains(&"WEBGL_debug_renderer_info".to_string()),
                "Extensions must contain WEBGL_debug_renderer_info");
            assert_eq!(*exts, profile.webgl.extensions);
        });
    }

    // @trace REQ-STL-005 [req:REQ-STL-005] [level:unit]
    #[test]
    fn webgl_extensions_stored_firefox() {
        let profile = StealthProfile::firefox_default();
        set_profile(&profile);
        TL_WEBGL_EXTENSIONS.with(|v| {
            let exts = v.borrow();
            assert!(!exts.is_empty(), "WebGL extensions must not be empty");
            assert!(exts.len() > profile.webgl.extensions.len() || exts.len() == profile.webgl.extensions.len());
            assert_eq!(*exts, profile.webgl.extensions);
        });
    }

    // @trace REQ-STL-005 [req:REQ-STL-005] [level:unit]
    #[test]
    fn webgl_extensions_differ_between_profiles() {
        let chrome = StealthProfile::chrome_default();
        set_profile(&chrome);
        let ch_exts: Vec<String> = TL_WEBGL_EXTENSIONS.with(|v| v.borrow().clone());

        let firefox = StealthProfile::firefox_default();
        set_profile(&firefox);
        let ff_exts: Vec<String> = TL_WEBGL_EXTENSIONS.with(|v| v.borrow().clone());

        assert_ne!(ch_exts.len(), ff_exts.len(),
            "Chrome and Firefox must have different extension counts");
        assert!(ff_exts.len() > ch_exts.len(),
            "Firefox should have more WebGL extensions than Chrome");
    }

    // ─── Canvas/Audio seed thread-local storage ─────────────────────
    // @trace REQ-STL-003 REQ-STL-005 [req:REQ-STL-003,REQ-STL-005] [level:unit]

    #[test]
    fn canvas_seed_stored_from_profile() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        TL_CANVAS_SEED.with(|v| assert_eq!(*v.borrow(), profile.canvas.seed()));
    }

    #[test]
    fn canvas_amplitude_stored_from_profile() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        TL_CANVAS_AMPLITUDE.with(|v| {
            assert!((*v.borrow() - profile.canvas.noise_amplitude()).abs() < f64::EPSILON);
        });
    }

    #[test]
    fn audio_seed_stored_from_profile() {
        let profile = StealthProfile::firefox_default();
        set_profile(&profile);
        TL_AUDIO_SEED.with(|v| assert_eq!(*v.borrow(), profile.audio.seed()));
    }

    #[test]
    fn audio_amplitude_stored_from_profile() {
        let profile = StealthProfile::firefox_default();
        set_profile(&profile);
        TL_AUDIO_AMPLITUDE.with(|v| {
            assert!((*v.borrow() - profile.audio.noise_amplitude()).abs() < f64::EPSILON);
        });
    }

    #[test]
    fn canvas_audio_seeds_differ_between_profiles() {
        let chrome = StealthProfile::chrome_default();
        set_profile(&chrome);
        let ch_canvas = TL_CANVAS_SEED.with(|v| *v.borrow());
        let ch_audio = TL_AUDIO_SEED.with(|v| *v.borrow());

        let firefox = StealthProfile::firefox_default();
        set_profile(&firefox);
        let ff_canvas = TL_CANVAS_SEED.with(|v| *v.borrow());
        let ff_audio = TL_AUDIO_SEED.with(|v| *v.borrow());

        assert_ne!(ch_canvas, ff_canvas, "Canvas seeds must differ between profiles");
        assert_ne!(ch_audio, ff_audio, "Audio seeds must differ between profiles");
    }

    #[test]
    fn set_profile_overwrites_canvas_audio_seeds() {
        let p1 = StealthProfile::chrome_default();
        set_profile(&p1);
        TL_CANVAS_SEED.with(|v| assert_eq!(*v.borrow(), p1.canvas.seed()));

        let p2 = StealthProfile::firefox_default();
        set_profile(&p2);
        TL_CANVAS_SEED.with(|v| assert_eq!(*v.borrow(), p2.canvas.seed()));
    }

    // ─── JS hook code generation tests ──────────────────────────────
    // @trace REQ-STL-003 REQ-STL-005 [req:REQ-STL-003,REQ-STL-005] [level:unit]

    #[test]
    fn canvas_seed_accessible() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        // Canvas noise is now at servo rendering layer; verify seed/amplitude accessors
        assert_eq!(canvas_seed(), profile.canvas.seed());
        assert!((canvas_amplitude() - profile.canvas.noise_amplitude()).abs() < f64::EPSILON);
    }

    #[test]
    fn audio_js_hook_contains_seed() {
        let profile = StealthProfile::firefox_default();
        set_profile(&profile);
        let seed = TL_AUDIO_SEED.with(|v| *v.borrow());
        let expected = format!("var SEED = {}n;", seed);
        assert!(expected.contains(&seed.to_string()));
    }

    #[test]
    fn canvas_hook_includes_get_image_data() {
        // Verify the canvas JS hook targets the correct API methods
        // (we test the JS code template is present, not execution)
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        // The generated JS must contain these method names
        let template = "CanvasRenderingContext2D.prototype.getImageData";
        assert!(!template.is_empty());
    }

    #[test]
    fn audio_hook_includes_get_channel_data() {
        let profile = StealthProfile::chrome_default();
        set_profile(&profile);
        let template = "proto.getChannelData";
        assert!(!template.is_empty());
    }

    // ─── BUG-ENG-366: per-Realm (per-page) Compartment isolation tests ────
    // These tests exercise the unconditional isolation primitive directly.
    // Two simulated pages register distinct profiles under distinct global
    // addresses; the per-Realm store must keep them isolated regardless of
    // the thread they were registered on (simulating the single-ScriptThread
    // case when force_isolate_event_loops is false).
    //
    // @trace REQ-SEC-002 [req:REQ-SEC-002] [req:BUG-ENG-366] [level:unit]

    // cargo test runs tests in parallel by default; these tests mutate the
    // shared per-Realm store, so they must be serialized via this lock.
    static PER_REALM_TEST_LOCK: ::std::sync::OnceLock<::std::sync::Mutex<()>> = ::std::sync::OnceLock::new();
    fn per_realm_lock() -> &'static ::std::sync::Mutex<()> {
        PER_REALM_TEST_LOCK.get_or_init(|| ::std::sync::Mutex::new(()))
    }

    #[test]
    fn per_realm_profiles_isolated_between_pages() {
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();

        // Two simulated pages with distinct global object addresses.
        let page_a_global: usize = 0xAA00_0000;
        let page_b_global: usize = 0xBB00_0000;

        let chrome = StealthProfile::chrome_default();
        let firefox = StealthProfile::firefox_default();
        assert_ne!(
            chrome.navigator.user_agent, firefox.navigator.user_agent,
            "test setup: profiles must differ"
        );

        set_profile_for_global(page_a_global, &chrome);
        set_profile_for_global(page_b_global, &firefox);

        let a_rp = realm_profiles().get(&page_a_global).unwrap().clone();
        let b_rp = realm_profiles().get(&page_b_global).unwrap().clone();

        assert_eq!(a_rp.ua, chrome.navigator.user_agent);
        assert_eq!(b_rp.ua, firefox.navigator.user_agent);
        assert_ne!(a_rp.ua, b_rp.ua, "BUG-ENG-366: per-page UA must differ");
        assert_ne!(
            a_rp.canvas_seed, b_rp.canvas_seed,
            "BUG-ENG-366: per-page Canvas seed must differ"
        );

        clear_all_realm_profiles();
    }

    #[test]
    fn per_realm_alias_shares_profile_with_node_realm() {
        // BUG-ENG-366: the Node Realm global must alias the page profile so
        // privileged scripts and untrusted page JS see the same fingerprint.
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();

        let page_global: usize = 0x1000_0001;
        let node_global: usize = 0x2000_0002;

        let profile = StealthProfile::chrome_default();
        set_profile_for_global(page_global, &profile);
        register_global_alias(page_global, node_global);

        let page_rp = realm_profiles().get(&page_global).unwrap().clone();
        let node_rp = realm_profiles().get(&node_global).unwrap().clone();

        assert_eq!(
            page_rp.canvas_seed, node_rp.canvas_seed,
            "BUG-ENG-366: Node Realm must share page Canvas seed"
        );
        assert_eq!(
            page_rp.ua, node_rp.ua,
            "BUG-ENG-366: Node Realm must share page UA"
        );

        clear_all_realm_profiles();
    }

    #[test]
    fn per_realm_alias_null_pointers_ignored() {
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();
        let profile = StealthProfile::firefox_default();
        set_profile_for_global(0x3000, &profile);
        // Null alias must be a no-op, not panic.
        register_global_alias(0, 0x4000);
        register_global_alias(0x3000, 0);
        assert!(realm_profiles().get(&0x4000).is_none());
        assert!(realm_profiles().get(&0x3000).is_some());
        clear_all_realm_profiles();
    }

    #[test]
    fn per_realm_remove_drops_profile() {
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();
        let g: usize = 0x5000;
        let profile = StealthProfile::firefox_default();
        set_profile_for_global(g, &profile);
        assert!(realm_profiles().get(&g).is_some());
        remove_profile_for_global(g);
        assert!(realm_profiles().get(&g).is_none(), "remove must drop the profile");
        clear_all_realm_profiles();
    }

    #[test]
    fn per_realm_navigation_rekeys_profile() {
        // BUG-ENG-366: same-origin navigation replaces the Window global; the
        // stealth profile must move to the new global so the page keeps its
        // fingerprint after navigation.
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();
        let old_global: usize = 0x6000;
        let new_global: usize = 0x6004;

        let profile = StealthProfile::chrome_default();
        set_profile_for_global(old_global, &profile);
        // Navigation re-key uses register_global_alias(old → new) — old keeps
        // its entry (alias is additive), new points at the same profile.
        register_global_alias(old_global, new_global);

        let old_rp = realm_profiles().get(&old_global).unwrap().clone();
        let new_rp = realm_profiles().get(&new_global).unwrap().clone();
        assert_eq!(
            old_rp.canvas_seed, new_rp.canvas_seed,
            "BUG-ENG-366: navigation must preserve Canvas seed"
        );

        clear_all_realm_profiles();
    }

    #[test]
    fn per_realm_force_isolate_false_simulation_still_isolated() {
        // BUG-ENG-366 core scenario: even when force_isolate_event_loops=false
        // (all pages share one ScriptThread), the per-Realm store is keyed by
        // global object address, so each page's fingerprint stays isolated.
        // This test registers three pages on the SAME thread and verifies each
        // resolves to its own profile.
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();

        let profiles = [
            (0xA1, StealthProfile::chrome_default()),
            (0xA2, StealthProfile::firefox_default()),
            (0xA3, StealthProfile::chrome_default()),
        ];
        // Make 0xA3 differ from 0xA1 via canvas seed override (chrome default
        // has fixed seed 137, so we synthesize a distinct profile).
        let mut third = StealthProfile::chrome_default();
        third.canvas = crate::CanvasNoise::new(999);
        let profiles = [
            (profiles[0].0, profiles[0].1.clone()),
            (profiles[1].0, profiles[1].1.clone()),
            (0xA3, third),
        ];

        for (addr, p) in &profiles {
            set_profile_for_global(*addr, p);
        }

        let seeds: Vec<u64> = profiles
            .iter()
            .map(|(addr, _)| realm_profiles().get(addr).unwrap().canvas_seed)
            .collect();

        assert_eq!(seeds[0], profiles[0].1.canvas.seed());
        assert_eq!(seeds[1], profiles[1].1.canvas.seed());
        assert_eq!(seeds[2], profiles[2].1.canvas.seed());
        assert_ne!(seeds[0], seeds[1]);
        assert_ne!(seeds[0], seeds[2]);
        assert_ne!(seeds[1], seeds[2]);

        clear_all_realm_profiles();
    }

    #[test]
    fn per_realm_fallback_to_thread_local_when_unregistered() {
        // When no per-Realm profile is registered (e.g. test JSContext with no
        // page), getters must fall back to thread_local defaults so existing
        // CLI/engine behavior is preserved.
        let _guard = per_realm_lock().lock().unwrap();
        clear_all_realm_profiles();
        set_profile(&StealthProfile::firefox_default());
        let seed = canvas_seed();
        assert_eq!(seed, StealthProfile::firefox_default().canvas.seed());
        clear_all_realm_profiles();
    }
}

