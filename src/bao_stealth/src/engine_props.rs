// @trace REQ-STL-007 [api:engine-layer stealth properties]
// Engine-layer native property injection via mozjs FFI.
// JSPROP_PERMANENT ≡ configurable:false → JS Object.defineProperty throws TypeError.
// Zero JS injection. All properties are accessor (getter-only) with PERMANENT flag.

use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::marker::PhantomData;
use ::std::ptr;

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};

use crate::StealthProfile;

// ---------------------------------------------------------------------------
// thread_local storage for profile values — read by getter JSNative callbacks
// ---------------------------------------------------------------------------

thread_local! {
    static TL_WEBDRIVER: RefCell<bool> = RefCell::new(false);
    static TL_UA: RefCell<String> = RefCell::new(String::new());
    static TL_PLATFORM: RefCell<String> = RefCell::new(String::new());
    static TL_LANGUAGE: RefCell<String> = RefCell::new(String::new());
    static TL_HWC: RefCell<u32> = RefCell::new(8);
    static TL_TOUCH: RefCell<u32> = RefCell::new(0);
    static TL_VENDOR: RefCell<String> = RefCell::new(String::new());
    static TL_SCREEN_W: RefCell<u32> = RefCell::new(1920);
    static TL_SCREEN_H: RefCell<u32> = RefCell::new(1080);
    static TL_AVAIL_W: RefCell<u32> = RefCell::new(1920);
    static TL_AVAIL_H: RefCell<u32> = RefCell::new(1040);
    static TL_COLOR_DEPTH: RefCell<u32> = RefCell::new(24);
    static TL_DPR: RefCell<f64> = RefCell::new(1.0);
    // WebGL vendor/renderer for getParameter override
    static TL_WEBGL_VENDOR: RefCell<String> = RefCell::new(String::new());
    static TL_WEBGL_RENDERER: RefCell<String> = RefCell::new(String::new());
}

/// Store all profile values into thread-local before calling install_stealth_props.
pub fn set_profile(profile: &StealthProfile) {
    TL_WEBDRIVER.with(|v| *v.borrow_mut() = false);
    TL_UA.with(|v| *v.borrow_mut() = profile.navigator.user_agent.clone());
    TL_PLATFORM.with(|v| *v.borrow_mut() = profile.navigator.platform.clone());
    TL_LANGUAGE.with(|v| *v.borrow_mut() = profile.navigator.language.clone());
    TL_HWC.with(|v| *v.borrow_mut() = profile.navigator.hardware_concurrency);
    TL_TOUCH.with(|v| *v.borrow_mut() = profile.navigator.max_touch_points);
    TL_VENDOR.with(|v| *v.borrow_mut() = profile.navigator.vendor.clone());
    TL_SCREEN_W.with(|v| *v.borrow_mut() = profile.screen.width);
    TL_SCREEN_H.with(|v| *v.borrow_mut() = profile.screen.height);
    TL_AVAIL_W.with(|v| *v.borrow_mut() = profile.screen.avail_width);
    TL_AVAIL_H.with(|v| *v.borrow_mut() = profile.screen.avail_height);
    TL_COLOR_DEPTH.with(|v| *v.borrow_mut() = profile.screen.color_depth);
    TL_DPR.with(|v| *v.borrow_mut() = profile.screen.device_pixel_ratio);
    TL_WEBGL_VENDOR.with(|v| *v.borrow_mut() = profile.webgl.vendor.clone());
    TL_WEBGL_RENDERER.with(|v| *v.borrow_mut() = profile.webgl.renderer.clone());
}

/// Returns true iff a profile has been explicitly set on this thread
/// (heuristic: user-agent is non-empty after a real `set_profile` call).
pub fn is_profile_set() -> bool {
    TL_UA.with(|v| !v.borrow().is_empty())
}

/// Idempotent: install Firefox default profile if none has been set on this thread yet.
/// Called by `bao_runtime::globals::install_all` so consumers get anti-fingerprinting
/// protection automatically — no manual `set_profile` required.
pub fn ensure_default_profile() {
    if !is_profile_set() {
        set_profile(&StealthProfile::firefox_default());
    }
}

// ---------------------------------------------------------------------------
// Getter JSNative callbacks — each reads from thread_local and sets rval
// ---------------------------------------------------------------------------

macro_rules! make_bool_getter {
    ($name:ident, $tl:path) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            $tl.with(|v| args.rval().set(BooleanValue(*v.borrow())));
            true
        }
    };
}

macro_rules! make_u32_getter {
    ($name:ident, $tl:path) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            $tl.with(|v| args.rval().set(Int32Value(*v.borrow() as i32)));
            true
        }
    };
}

macro_rules! make_f64_getter {
    ($name:ident, $tl:path) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            $tl.with(|v| args.rval().set(DoubleValue(*v.borrow())));
            true
        }
    };
}

macro_rules! make_string_getter {
    ($name:ident, $tl:path) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            $tl.with(|v| {
                let s = v.borrow().clone();
                let c_str = CString::new(s.as_str()).unwrap_or_default();
                let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
                if !js_str.is_null() {
                    args.rval().set(StringValue(&*js_str));
                } else {
                    args.rval().set(UndefinedValue());
                }
            });
            true
        }
    };
}

make_bool_getter!(getter_webdriver, TL_WEBDRIVER);
make_string_getter!(getter_ua, TL_UA);
make_string_getter!(getter_platform, TL_PLATFORM);
make_string_getter!(getter_language, TL_LANGUAGE);
make_u32_getter!(getter_hwc, TL_HWC);
make_u32_getter!(getter_touch, TL_TOUCH);
make_string_getter!(getter_vendor, TL_VENDOR);
make_u32_getter!(getter_screen_w, TL_SCREEN_W);
make_u32_getter!(getter_screen_h, TL_SCREEN_H);
make_u32_getter!(getter_avail_w, TL_AVAIL_W);
make_u32_getter!(getter_avail_h, TL_AVAIL_H);
make_u32_getter!(getter_color_depth, TL_COLOR_DEPTH);
make_f64_getter!(getter_dpr, TL_DPR);

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
            return emit_tl_string_rval(cx, args.rval(), &TL_WEBGL_VENDOR);
        }
        if p == 0x1F01 {
            return emit_tl_string_rval(cx, args.rval(), &TL_WEBGL_RENDERER);
        }
    }
    // Fall through to original __originalGetParameter__ via bao_engine::host_fn::call_function
    let this_val = args.thisv();
    if !this_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let this_obj = this_val.to_object();
    let this_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &this_obj };
    let Ok(c_orig) = CString::new("__originalGetParameter__") else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let mut has: bool = false;
    if !JS_HasProperty(cx, this_h, c_orig.as_ptr(), &mut has) || !has {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut fn_val = UndefinedValue();
    JS_GetProperty(cx, this_h, c_orig.as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut fn_val });
    if !fn_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // Call original function using bao_engine::host_fn::call_function
    let param_val: Value = *param.ptr;
    match bao_engine::host_fn::call_function(cx, fn_val, this_obj, &[param_val]) {
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

/// Helper: emit a thread_local String as a JS string value into a MutableHandleValue.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn emit_tl_string_rval(
    cx: *mut JSContext,
    rval: MutableHandleValue,
    tl: &'static ::std::thread::LocalKey<RefCell<String>>,
) -> bool {
    tl.with(|v| {
        let s = v.borrow().clone();
        let c_str = CString::new(s.as_str()).unwrap_or_default();
        let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
        if !js_str.is_null() {
            rval.set(StringValue(&*js_str));
        } else {
            rval.set(UndefinedValue());
        }
    });
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
    let Ok(c_name) = CString::new(name) else { return false };
    // Remove existing property (servo defines navigator.userAgent etc.
    // as configurable). SpiderMonkey forbids changing configurable:true
    // to configurable:false (PERMANENT), so we must delete first.
    let mut op_result = ObjectOpResult::default();
    JS_DeleteProperty(cx, obj, c_name.as_ptr(), &mut op_result);
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
    let Ok(c_prop) = CString::new(prop) else { return ptr::null_mut() };
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
    let Ok(c_prop) = CString::new(prop) else { return ptr::null_mut() };
    let new_obj = JS_NewPlainObject(cx);
    if new_obj.is_null() {
        return ptr::null_mut();
    }
    let attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
    let new_obj_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &new_obj };
    if !JS_DefineProperty3(cx, obj, c_prop.as_ptr(), new_obj_h, attrs) {
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
    let Ok(c_name) = CString::new("WebGLRenderingContext") else { return false };
    let mut has: bool = false;
    if !JS_HasProperty(cx, global, c_name.as_ptr(), &mut has) || !has {
        return true;
    }
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(cx, global, c_name.as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut ctor_val });
    if !ctor_val.is_object() {
        return true;
    }
    let ctor = ctor_val.to_object();
    let ctor_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &ctor };

    let Ok(c_proto) = CString::new("prototype") else { return false };
    let mut proto_val = UndefinedValue();
    JS_GetProperty(cx, ctor_h, c_proto.as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut proto_val });
    if !proto_val.is_object() {
        return true;
    }
    let proto = proto_val.to_object();
    let proto_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &proto };

    // Save original getParameter as __originalGetParameter__
    let Ok(c_gp) = CString::new("getParameter") else { return false };
    let mut orig_gp = UndefinedValue();
    JS_GetProperty(cx, proto_h, c_gp.as_ptr(),
        MutableHandle::<Value> { _phantom_0: PhantomData, ptr: &mut orig_gp });

    if orig_gp.is_object() {
        let Ok(c_orig_name) = CString::new("__originalGetParameter__") else { return false };
        let orig_fn = orig_gp.to_object();
        let orig_fn_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &orig_fn };
        let save_attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
        JS_DefineProperty3(cx, proto_h, c_orig_name.as_ptr(), orig_fn_h, save_attrs);
    }

    // Define override getParameter as PERMANENT native function
    let Ok(c_fn_name) = CString::new("getParameter") else { return false };
    let fn_obj = JS_NewFunction(cx, Some(webgl_get_parameter_override), 1, 0, c_fn_name.as_ptr());
    if fn_obj.is_null() {
        return false;
    }
    let fn_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &(fn_obj as *mut JSObject) };
    let override_attrs = (JSPROP_PERMANENT | JSPROP_ENUMERATE) as u32;
    JS_DefineProperty3(cx, proto_h, c_fn_name.as_ptr(), fn_h, override_attrs)
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
    let global_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &global };
    let mut all_ok = true;

    // --- Navigator properties ---
    let nav = ensure_subobject(cx, global_h, "navigator");
    if !nav.is_null() {
        let nav_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &nav };
        all_ok &= define_permanent_getter(cx, nav_h, "webdriver", Some(getter_webdriver));
        all_ok &= define_permanent_getter(cx, nav_h, "userAgent", Some(getter_ua));
        all_ok &= define_permanent_getter(cx, nav_h, "platform", Some(getter_platform));
        all_ok &= define_permanent_getter(cx, nav_h, "language", Some(getter_language));
        all_ok &= define_permanent_getter(cx, nav_h, "hardwareConcurrency", Some(getter_hwc));
        all_ok &= define_permanent_getter(cx, nav_h, "maxTouchPoints", Some(getter_touch));
        all_ok &= define_permanent_getter(cx, nav_h, "vendor", Some(getter_vendor));
    }

    // --- Screen properties ---
    let screen = ensure_subobject(cx, global_h, "screen");
    if !screen.is_null() {
        let scr_h = Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &screen };
        all_ok &= define_permanent_getter(cx, scr_h, "width", Some(getter_screen_w));
        all_ok &= define_permanent_getter(cx, scr_h, "height", Some(getter_screen_h));
        all_ok &= define_permanent_getter(cx, scr_h, "availWidth", Some(getter_avail_w));
        all_ok &= define_permanent_getter(cx, scr_h, "availHeight", Some(getter_avail_h));
        all_ok &= define_permanent_getter(cx, scr_h, "colorDepth", Some(getter_color_depth));
        all_ok &= define_permanent_getter(cx, scr_h, "pixelDepth", Some(getter_color_depth));
    }

    // --- Window.devicePixelRatio ---
    all_ok &= define_permanent_getter(cx, global_h, "devicePixelRatio", Some(getter_dpr));

    // --- WebGL prototype override ---
    all_ok &= install_webgl_override(cx, global_h);

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
}
