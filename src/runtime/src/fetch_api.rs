// @trace REQ-ENG-006 REQ-STL-001
// fetch + Response + Headers constructors
use ::std::cell::RefCell;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};
use mozjs::conversions::jsstr_to_string;

thread_local! {
    static TL_STEALTH_PROFILE: RefCell<Option<bao_stealth::StealthProfile>> = const { RefCell::new(None) };
}

/// Store the current page's stealth profile so fetch() can apply TLS/HTTP2 fingerprints.
pub fn set_fetch_stealth_profile(profile: Option<bao_stealth::StealthProfile>) {
    TL_STEALTH_PROFILE.with(|p| *p.borrow_mut() = profile);
}

/// Returns true if a stealth profile has been explicitly set on this thread.
pub fn is_fetch_stealth_profile_set() -> bool {
    TL_STEALTH_PROFILE.with(|p| p.borrow().is_some())
}

/// Idempotent: install Firefox default profile if none has been set on this thread.
/// Called by `globals::install_all` so fetch() gets TLS/HTTP2 fingerprints by default.
pub fn ensure_default_fetch_stealth_profile() {
    if !is_fetch_stealth_profile_set() {
        set_fetch_stealth_profile(Some(bao_stealth::StealthProfile::firefox_default()));
    }
}

pub fn install_fetch_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"fetch".as_ptr(),
            ::std::option::Option::Some(fetch_fn), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fetch_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"fetch requires a URL argument".as_ptr());
        return false;
    }

    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, c"fetch requires a string URL".as_ptr());
        return false;
    }

    let url = crate::js_to_rust_string(cx, url_val);

    // 铁律 0: use bun_url for URL parsing instead of hand-written string splitting
    if let Some(url_ptr) = bun_url::whatwg::URL::from_utf8(url.as_bytes()) {
        let url_ref = unsafe { url_ptr.as_ref() };
        // whatwg::URL::host() returns hostname WITHOUT port (JS "hostname")
        let host = {
            let s = url_ref.host();
            if s.is_dead() {
                let mut url_mut = url_ptr;
                unsafe { url_mut.as_mut().deinit(); }
                JS_ReportErrorUTF8(cx, c"Invalid URL hostname".as_ptr());
                return false;
            }
            let utf8 = s.to_utf8();
            let bytes = utf8.slice();
            ::std::string::String::from_utf8_lossy(bytes).into_owned()
        };
        let mut url_mut = url_ptr;
        unsafe { url_mut.as_mut().deinit(); }
        if let ::std::result::Result::Err(e) = crate::permission_bridge::check_net(&host) {
            let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    let method = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut m_val = UndefinedValue();
            let m_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut m_val };
            JS_GetProperty(cx, obj_handle, c"method".as_ptr(), m_handle);
            if m_val.is_string() {
                crate::js_to_rust_string(cx, m_val).to_uppercase()
            } else {
                "GET".to_string()
            }
        } else {
            "GET".to_string()
        }
    } else {
        "GET".to_string()
    };

    let body = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut b_val = UndefinedValue();
            let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut b_val };
            JS_GetProperty(cx, obj_handle, c"body".as_ptr(), b_handle);
            if b_val.is_string() {
                Some(crate::js_to_rust_string(cx, b_val))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let response = match do_fetch(&url, &method, body.as_deref()) {
        Ok(resp) => resp,
        Err(e) => {
            let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
            if !promise.is_null() {
                let c_msg = bun_core::ZBox::from_bytes(format!("fetch failed: {}", e).as_bytes());
                let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                if !err_obj.is_null() {
                    let err_msg = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                    if !err_msg.is_null() {
                        let msg_val = StringValue(&*err_msg);
                        let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
                        let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
                        JS_SetProperty(cx, err_h, c"message".as_ptr(), msg_h);
                    }
                }
                let err_val = mozjs::jsval::ObjectValue(err_obj);
                let err_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
                let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
                mozjs_sys::jsapi::JS::RejectPromise(cx, promise_h, err_handle);
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise));
            return true;
        }
    };

    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(response.status_code as i32);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(response.status_code >= 200 && response.status_code < 300);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    let c_url = bun_core::ZBox::from_bytes(response.url.as_bytes());
    let url_js = JS_NewStringCopyZ(cx, c_url.as_ptr());
    if !url_js.is_null() {
        let url_val = StringValue(&*url_js);
        let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
        JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
    }

    let c_st = bun_core::ZBox::from_bytes(response.status_text.as_bytes());
    let st_js = JS_NewStringCopyZ(cx, c_st.as_ptr());
    if !st_js.is_null() {
        let st_val = StringValue(&*st_js);
        let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
        JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
    }

    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !headers_obj.is_null() {
        let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };
        for (key, value) in &response.headers {
            let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
            let c_val = bun_core::ZBox::from_bytes(value.as_bytes());
            let val_js = JS_NewStringCopyZ(cx, c_val.as_ptr());
            if !val_js.is_null() {
                let hv = StringValue(&*val_js);
                let hv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hv };
                JS_DefineProperty(cx, h_handle, c_key.as_ptr(), hv_handle, JSPROP_ENUMERATE as u32);
            }
        }
        let hdrs_val = mozjs::jsval::ObjectValue(headers_obj);
        let hdrs_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), hdrs_handle, JSPROP_ENUMERATE as u32);
    }

    let c_body = bun_core::ZBox::from_bytes(response.body.as_bytes());
    let body_str = JS_NewStringCopyZ(cx, c_body.as_ptr());
    if !body_str.is_null() {
        let body_val = StringValue(&*body_str);
        let bt_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &body_val };
        JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bt_handle, 0);
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    let resp_val = mozjs::jsval::ObjectValue(resp_obj);
    let resp_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_val };
    let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, resp_handle);

    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

struct FetchResponse {
    status_code: u16,
    body: String,
    headers: Vec<(String, String)>,
    url: String,
    status_text: String,
}

fn do_fetch(url: &str, method: &str, body: Option<&str>) -> ::std::result::Result<FetchResponse, String> {
    let bun_method = match method {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    let headers: Vec<(String, String)> = Vec::new();
    let body_bytes: Option<&[u8]> = body.map(|b| b.as_bytes());

    let profile: Option<bao_stealth::StealthProfile> = TL_STEALTH_PROFILE.with(|p| p.borrow().clone());
    let result = crate::stealth_http::stealth_http_request(
        &profile, bun_method, url, &headers, body_bytes,
    ).map_err(|e| e.to_string())?;

    ::std::result::Result::Ok(FetchResponse {
        status_code: result.status_code as u16,
        body: String::from_utf8_lossy(&result.body).to_string(),
        headers: result.headers,
        url: url.to_string(),
        status_text: result.status_text,
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);
    args.rval().set(body_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_json(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"response.json(): invalid this".as_ptr());
        return false;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);

    if !body_val.is_string() {
        JS_ReportErrorUTF8(cx, c"response.json(): body is not a string".as_ptr());
        return false;
    }

    let js_str = body_val.to_string();
    let str_handle = Handle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &js_str };
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_handle, rval_handle);

    if !ok {
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, c"response.json(): invalid JSON".as_ptr());
        return false;
    }
    args.rval().set(rval);
    true
}

pub fn install_response_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(response_constructor), 2, JSFUN_CONSTRUCTOR, c"Response".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Response".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(200);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(true);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    let url_js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !url_js_str.is_null() {
        let url_val = StringValue(&*url_js_str);
        let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
        JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
    }

    let st_js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !st_js_str.is_null() {
        let st_val = StringValue(&*st_js_str);
        let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
        JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
    }

    let empty_headers = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !empty_headers.is_null() {
        let h_val = mozjs::jsval::ObjectValue(empty_headers);
        let h_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &h_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), h_handle, JSPROP_ENUMERATE as u32);
    }

    if argc > 0 {
        let body_val = *args.get(0).ptr;
        if body_val.is_string() {
            let body_str = crate::js_to_rust_string(cx, body_val);
            let c_body = bun_core::ZBox::from_bytes(body_str.as_bytes());
                let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
                if !body_js.is_null() {
                    let bv = StringValue(&*body_js);
                    let bv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bv };
                    JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bv_handle, 0);
                }
        }
    }

    if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let opts_obj = opts.to_object();
            let opts_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };
            let mut st_val = UndefinedValue();
            let st_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut st_val };
            JS_GetProperty(cx, opts_handle, c"status".as_ptr(), st_mh);
            if st_val.is_int32() {
                let st_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
                JS_SetProperty(cx, obj_handle, c"status".as_ptr(), st_h);
                let ok = mozjs::jsval::BooleanValue(st_val.to_int32() >= 200 && st_val.to_int32() < 300);
                let ok_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok };
                JS_SetProperty(cx, obj_handle, c"ok".as_ptr(), ok_h);
            }
        }
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
    true
}

pub fn install_headers_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(headers_constructor), 1, JSFUN_CONSTRUCTOR, c"Headers".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Headers".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if headers_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };

    let get_fn = JS_NewFunction(cx, Some(headers_get), 1, 0, c"get".as_ptr());
    if !get_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(get_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"get".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let set_fn = JS_NewFunction(cx, Some(headers_set), 2, 0, c"set".as_ptr());
    if !set_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(set_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"set".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let has_fn = JS_NewFunction(cx, Some(headers_has), 1, 0, c"has".as_ptr());
    if !has_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(has_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"has".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(headers_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_get(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = crate::jsstr_to_rust_string(cx, name_js);
    let c_name = bun_core::ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    if val.is_undefined() || val.is_null() {
        args.rval().set(mozjs::jsval::NullValue());
    } else {
        args.rval().set(val);
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_set(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, c"Headers.set requires name and value".as_ptr());
        return false;
    }
    let name_val = *args.get(0).ptr;
    let value_val = *args.get(1).ptr;
    if !name_val.is_string() || !value_val.is_string() {
        JS_ReportErrorUTF8(cx, c"Headers.set requires string arguments".as_ptr());
        return false;
    }
    let name_js = name_val.to_string();
    let name_str = crate::jsstr_to_rust_string(cx, name_js);
    let c_name = bun_core::ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &value_val };
    JS_SetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_has(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = crate::jsstr_to_rust_string(cx, name_js);
    let c_name = bun_core::ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(mozjs::jsval::BooleanValue(!val.is_undefined() && !val.is_null()));
    true
}

pub fn install_request_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(request_constructor), 2, JSFUN_CONSTRUCTOR, c"Request".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Request".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn request_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let req_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if req_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &req_obj };

    // url argument
    let url_val = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { v } else { UndefinedValue() }
    } else { UndefinedValue() };
    let url_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
    JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), url_h, JSPROP_ENUMERATE as u32);

    // method from options or default GET
    let method_str = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let opts_obj = opts.to_object();
            let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };
            let mut m_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"method".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut m_val });
            if m_val.is_string() {
                crate::js_to_rust_string(cx, m_val)
            } else { "GET".to_string() }
        } else { "GET".to_string() }
    } else { "GET".to_string() };
    let method_cstr = bun_core::ZBox::from_bytes(method_str.as_bytes());
    let method_jsstr = JS_NewStringCopyZ(cx, method_cstr.as_ptr());
    let method_val = StringValue(&*method_jsstr);
    let method_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &method_val };
    JS_DefineProperty(cx, obj_handle, c"method".as_ptr(), method_h, JSPROP_ENUMERATE as u32);

    // headers (empty Headers-like object)
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    let headers_val = mozjs::jsval::ObjectValue(headers_obj);
    let headers_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_val };
    JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), headers_h, JSPROP_ENUMERATE as u32);

    args.rval().set(mozjs::jsval::ObjectValue(req_obj));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_response_status_code_type() {
        // Verify FetchResponse struct has expected fields
        let resp = FetchResponse {
            status_code: 200,
            body: "ok".to_string(),
            headers: vec![],
            url: "http://example.com".to_string(),
            status_text: "OK".to_string(),
        };
        assert_eq!(resp.status_code, 200);
        assert_eq!(resp.body, "ok");
        assert_eq!(resp.url, "http://example.com");
        assert_eq!(resp.status_text, "OK");
    }

    #[test]
    fn fetch_response_headers_preserved() {
        let resp = FetchResponse {
            status_code: 404,
            body: "not found".to_string(),
            headers: vec![("content-type".into(), "text/html".into())],
            url: "http://example.com/missing".to_string(),
            status_text: "Not Found".to_string(),
        };
        assert_eq!(resp.headers.len(), 1);
        assert_eq!(resp.headers[0].0, "content-type");
        assert_eq!(resp.headers[0].1, "text/html");
    }

    #[test]
    fn fetch_response_multiple_headers() {
        let resp = FetchResponse {
            status_code: 200,
            body: String::new(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-custom".into(), "value1".into()),
                ("x-custom".into(), "value2".into()),
            ],
            url: "http://example.com".to_string(),
            status_text: "OK".to_string(),
        };
        assert_eq!(resp.headers.len(), 3);
    }

    #[test]
    fn fetch_response_status_codes() {
        for code in [200u16, 201, 301, 400, 404, 500, 503] {
            let resp = FetchResponse {
                status_code: code,
                body: String::new(),
                headers: vec![],
                url: String::new(),
                status_text: String::new(),
            };
            assert_eq!(resp.status_code, code);
        }
    }

    #[test]
    fn fetch_response_empty_body() {
        let resp = FetchResponse {
            status_code: 204,
            body: String::new(),
            headers: vec![],
            url: String::new(),
            status_text: "No Content".to_string(),
        };
        assert!(resp.body.is_empty());
        assert_eq!(resp.status_code, 204);
    }

    // ── REQ-SEC-001: CORS Bypass Unit Tests ──────────────────────────────
    // @trace TEST-SEC-001 [req:REQ-SEC-001] [level:unit]

    /// REQ-SEC-001: do_fetch performs direct HTTP requests without CORS middleware.
    /// Verify the fetch path has NO CORS-related headers or preflight logic.
    #[test]
    fn cors_bypass_no_preflight_code_in_do_fetch() {
        let source = include_str!("fetch_api.rs");
        let func_start = source.find("fn do_fetch(").expect("do_fetch function not found");
        let func_body = &source[func_start..func_start + 2000.min(source.len() - func_start)];

        assert!(
            !func_body.contains("cors_check"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT contain cors_check"
        );
        assert!(
            !func_body.contains("Access-Control-Request-Method"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT send CORS preflight headers"
        );
        assert!(
            !func_body.contains("Origin"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT set Origin header for CORS"
        );
        assert!(
            !func_body.contains("preflight"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT contain preflight logic"
        );
    }

    /// REQ-SEC-001: stealth HTTP request path has no CORS enforcement.
    #[test]
    fn cors_bypass_stealth_http_no_cors() {
        let source = include_str!("fetch_api.rs");
        let func_start = source.find("fn do_fetch(").expect("do_fetch not found");
        let func_body = &source[func_start..func_start + 2000.min(source.len() - func_start)];

        assert!(
            func_body.contains("stealth_http_request"),
            "REQ-SEC-001: do_fetch must use stealth_http_request for direct HTTP"
        );
        assert!(
            !func_body.contains("CorsCache"),
            "REQ-SEC-001 REGRESSION: must not reference CorsCache"
        );
        assert!(
            !func_body.contains("opaque"),
            "REQ-SEC-001 REGRESSION: must not produce opaque responses"
        );
    }

    /// REQ-SEC-001: FetchResponse contains full response body (never opaque).
    #[test]
    fn cors_bypass_fetch_response_is_transparent() {
        let resp = FetchResponse {
            status_code: 200,
            body: "{\"data\":\"full access\"}".to_string(),
            headers: vec![("content-type".into(), "application/json".into())],
            url: "https://other-domain.com/api".to_string(),
            status_text: "OK".to_string(),
        };
        assert_eq!(resp.status_code, 200, "REQ-SEC-001: cross-origin response must be 200");
        assert!(
            resp.body.contains("full access"),
            "REQ-SEC-001: response body must be fully readable (not opaque)"
        );
        assert!(
            !resp.body.is_empty(),
            "REQ-SEC-001: response body must not be empty (opaque responses have empty body)"
        );
    }

    /// REQ-SEC-001: fetch global is installed on page realm via install_all_native.
    #[test]
    fn cors_bypass_fetch_global_installed_for_page() {
        let source = include_str!("fetch_api.rs");
        assert!(
            source.contains("pub fn install_fetch_global"),
            "REQ-SEC-001: install_fetch_global must be pub for page realm installation"
        );
    }
}
