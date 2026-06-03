// @trace REQ-ENG-006
// fetch + Response + Headers constructors
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};
use mozjs::conversions::jsstr_to_string;

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

    if let ::std::option::Option::Some(pos) = url.find("://") {
        let host_part = &url[pos + 3..];
        let host = host_part.split('/').next().unwrap_or(host_part).split(':').next().unwrap_or(host_part);
        if let ::std::result::Result::Err(e) = crate::permission_bridge::check_net(host) {
            let c_msg = ::std::ffi::CString::new(e).unwrap_or_default();
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
                let msg = format!("fetch failed: {}", e);
                let Ok(c_msg) = ::std::ffi::CString::new(msg) else {
                    args.rval().set(mozjs::jsval::ObjectValue(promise));
                    return true;
                };
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

    if let Ok(c_url) = ::std::ffi::CString::new(response.url.as_str()) {
        let url_js = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !url_js.is_null() {
            let url_val = StringValue(&*url_js);
            let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
            JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
        }
    }

    if let Ok(c_st) = ::std::ffi::CString::new(response.status_text.as_str()) {
        let st_js = JS_NewStringCopyZ(cx, c_st.as_ptr());
        if !st_js.is_null() {
            let st_val = StringValue(&*st_js);
            let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
            JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
        }
    }

    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !headers_obj.is_null() {
        let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };
        for (key, value) in &response.headers {
            let Ok(c_key) = ::std::ffi::CString::new(key.as_str()) else { continue };
            let Ok(c_val) = ::std::ffi::CString::new(value.as_str()) else { continue };
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

    let Ok(c_body) = ::std::ffi::CString::new(response.body.clone()) else {
        args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
        return true;
    };
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
    // Fast pre-check: ensure the host:port is reachable before delegating to
    // AsyncHTTP, which may otherwise hang for minutes on SYN to dead endpoints
    // (root cause of the fetch_api_tests SIGTERM during the suite — port 1 on
    // loopback never responds and the bun_http internals lack a connect timeout).
    if let Some((host, port)) = extract_host_port(url) {
        let addr = format!("{}:{}", host, port);
        if let Ok(addrs) = ::std::net::ToSocketAddrs::to_socket_addrs(&addr) {
            let collected: Vec<_> = addrs.collect();
            let any_reachable = collected.iter().take(3).any(|sa| {
                ::std::net::TcpStream::connect_timeout(sa, ::std::time::Duration::from_millis(250)).is_ok()
            });
            if !any_reachable {
                return ::std::result::Result::Err(format!("connect refused: {}", addr));
            }
        } else {
            return ::std::result::Result::Err(format!("DNS resolution failed for {}", addr));
        }
    }

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

    let result = crate::stealth_http::stealth_http_request(
        &None, bun_method, url, &headers, body_bytes,
    ).map_err(|e| e.to_string())?;

    ::std::result::Result::Ok(FetchResponse {
        status_code: result.status_code as u16,
        body: String::from_utf8_lossy(&result.body).to_string(),
        headers: result.headers,
        url: url.to_string(),
        status_text: result.status_text,
    })
}

fn extract_host_port(url: &str) -> ::std::option::Option<(String, u16)> {
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let authority = rest.split('/').next()?;
    let (hostport, _) = authority.split_once('?').unwrap_or((authority, ""));
    let (host, port) = if let Some(idx) = hostport.rfind(':') {
        let (h, p) = hostport.split_at(idx);
        (h.to_string(), p[1..].parse::<u16>().unwrap_or(80))
    } else {
        (hostport.to_string(), if url.starts_with("https://") { 443 } else { 80 })
    };
    Some((host, port))
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
            if let Ok(c_body) = ::std::ffi::CString::new(body_str.as_str()) {
                let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
                if !body_js.is_null() {
                    let bv = StringValue(&*body_js);
                    let bv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bv };
                    JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bv_handle, 0);
                }
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
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    };
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
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(UndefinedValue());
        return true;
    };
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
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    };
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
    let method_cstr = CString::new(method_str).unwrap_or_default();
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
    fn extract_host_port_http_default() {
        let (host, port) = extract_host_port("http://example.com/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn extract_host_port_https_default() {
        let (host, port) = extract_host_port("https://example.com/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn extract_host_port_with_port() {
        let (host, port) = extract_host_port("http://localhost:8080/api").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
    }

    #[test]
    fn extract_host_port_with_query() {
        let (host, port) = extract_host_port("http://host:3000/path?q=1").unwrap();
        assert_eq!(host, "host");
        assert_eq!(port, 3000);
    }

    #[test]
    fn extract_host_port_no_path() {
        let (host, port) = extract_host_port("http://example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn extract_host_port_no_scheme() {
        assert!(extract_host_port("example.com/path").is_none());
    }

    #[test]
    fn extract_host_port_empty() {
        assert!(extract_host_port("").is_none());
    }

    #[test]
    fn extract_host_port_ipv4() {
        let (host, port) = extract_host_port("http://127.0.0.1:9222/json").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9222);
    }

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
}
