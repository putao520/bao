// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch]
// @trace REQ-ENG-006 REQ-STL-001
// fetch + Response + Headers constructors
//
// ## BCE-007/R4 + BCE-20260619-010: FetchTasklet event-driven paradigm
//
// fetch() now delegates to `fetch_async::start` which uses
// `AsyncHTTP::init + HTTPThread::schedule` (single epoll thread, O(1) OS
// threads). The HTTPThread calls back `on_http_done` (pure-Rust), which
// enqueues a `ConcurrentTask` on the JS thread's MiniEventLoop. The JS
// thread auto-wakes and resolves/rejects the Promise in `resolve_tasklet`.
//
// This replaced the `thread::spawn` + `drain_pending` polling model which
// had three flaws (O(N) OS threads, busy-poll sleep, fragile drain coupling).
// See `fetch_async.rs` module-level doc for the full BCE analysis.
use bun_core::ZBox;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

thread_local! {
    static TL_STEALTH_PROFILE: ::std::cell::RefCell<Option<bao_stealth::StealthProfile>> = const { ::std::cell::RefCell::new(None) };
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
            cx,
            global,
            c"fetch".as_ptr(),
            ::std::option::Option::Some(fetch_fn),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fetch_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
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
        let host = host_part
            .split('/')
            .next()
            .unwrap_or(host_part)
            .split(':')
            .next()
            .unwrap_or(host_part);
        if let ::std::result::Result::Err(e) = crate::permission_bridge::check_net(host) {
            let c_msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    let method = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            // BCE-012: root to_object() result — JS_GetProperty can trigger GC
            rooted!(&in(wrapped_cx) let obj = opts.to_object());
            let mut m_val = UndefinedValue();
            let m_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut m_val,
            };
            JS_GetProperty(cx, obj.handle().into(), c"method".as_ptr(), m_handle);
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

    let headers: Vec<(String, String)> = Vec::new();

    let body = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            // BCE-012: root to_object() result — JS_GetProperty can trigger GC
            rooted!(&in(wrapped_cx) let obj = opts.to_object());
            let mut b_val = UndefinedValue();
            let b_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut b_val,
            };
            JS_GetProperty(cx, obj.handle().into(), c"body".as_ptr(), b_handle);
            if b_val.is_string() {
                Some(crate::js_to_rust_string(cx, b_val).into_bytes())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // ── FetchTasklet event-driven: create PENDING Promise, delegate to fetch_async ──
    // @trace REQ-ENG-010 [entity:FetchTasklet] — O(1) OS threads
    rooted!(&in(wrapped_cx) let null_global = ::std::ptr::null_mut::<JSObject>());
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_global.handle().into());
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let profile: Option<bao_stealth::StealthProfile> =
        TL_STEALTH_PROFILE.with(|p| p.borrow().clone());

    let bun_method = match method.as_str() {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    let promise_val = ObjectValue(promise);

    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    unsafe {
        crate::fetch_async::start(cx, promise_val, profile, bun_method, url, headers, body);
    }

    args.rval().set(promise_val);
    true
}

pub fn install_response_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(
            cx.raw_cx(),
            Some(response_constructor),
            2,
            JSFUN_CONSTRUCTOR,
            c"Response".as_ptr(),
        );
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(
                    cx,
                    global,
                    c"Response".as_ptr(),
                    co.handle(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    // BCE-012: root JS_NewPlainObject result — JS_DefineProperty/JS_SetProperty can trigger GC
    rooted!(&in(wrapped_cx) let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if resp_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    rooted!(&in(wrapped_cx) let status_val = Int32Value(200));
    JS_DefineProperty(
        cx,
        resp_obj.handle().into(),
        c"status".as_ptr(),
        status_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(wrapped_cx) let ok_val = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(
        cx,
        resp_obj.handle().into(),
        c"ok".as_ptr(),
        ok_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    let url_js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !url_js_str.is_null() {
        rooted!(&in(wrapped_cx) let url_val = StringValue(&*url_js_str));
        JS_DefineProperty(
            cx,
            resp_obj.handle().into(),
            c"url".as_ptr(),
            url_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let st_js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !st_js_str.is_null() {
        rooted!(&in(wrapped_cx) let st_val = StringValue(&*st_js_str));
        JS_DefineProperty(
            cx,
            resp_obj.handle().into(),
            c"statusText".as_ptr(),
            st_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let empty_headers = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !empty_headers.is_null() {
        // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
        rooted!(&in(wrapped_cx) let h_val = mozjs::jsval::ObjectValue(empty_headers));
        JS_DefineProperty(
            cx,
            resp_obj.handle().into(),
            c"headers".as_ptr(),
            h_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    if argc > 0 {
        let body_val = *args.get(0).ptr;
        if body_val.is_string() {
            let body_str = crate::js_to_rust_string(cx, body_val);
            {
                let c_body = ZBox::from_bytes(body_str.as_bytes());
                let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
                if !body_js.is_null() {
                    rooted!(&in(wrapped_cx) let bv = StringValue(&*body_js));
                    JS_DefineProperty(
                        cx,
                        resp_obj.handle().into(),
                        c"_bodyText".as_ptr(),
                        bv.handle().into(),
                        0,
                    );
                }
            }
        }
    }

    if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            // BCE-012: root to_object() result — JS_GetProperty can trigger GC
            rooted!(&in(wrapped_cx) let opts_obj = opts.to_object());
            let mut st_val = UndefinedValue();
            let st_mh = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut st_val,
            };
            JS_GetProperty(cx, opts_obj.handle().into(), c"status".as_ptr(), st_mh);
            if st_val.is_int32() {
                rooted!(&in(wrapped_cx) let st_root = st_val);
                JS_SetProperty(
                    cx,
                    resp_obj.handle().into(),
                    c"status".as_ptr(),
                    st_root.handle().into(),
                );
                let ok =
                    mozjs::jsval::BooleanValue(st_val.to_int32() >= 200 && st_val.to_int32() < 300);
                rooted!(&in(wrapped_cx) let ok_root = ok);
                JS_SetProperty(
                    cx,
                    resp_obj.handle().into(),
                    c"ok".as_ptr(),
                    ok_root.handle().into(),
                );
            }
        }
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
        rooted!(&in(wrapped_cx) let text_val = mozjs::jsval::ObjectValue(fn_ptr));
        JS_DefineProperty(
            cx,
            resp_obj.handle().into(),
            c"text".as_ptr(),
            text_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
        rooted!(&in(wrapped_cx) let json_val = mozjs::jsval::ObjectValue(fn_ptr));
        JS_DefineProperty(
            cx,
            resp_obj.handle().into(),
            c"json".as_ptr(),
            json_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval().set(mozjs::jsval::ObjectValue(resp_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // BCE-012: root to_object() result — JS_GetProperty can trigger GC
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = this.to_object());
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut body_val,
    };
    JS_GetProperty(cx, obj.handle().into(), c"_bodyText".as_ptr(), b_handle);
    args.rval().set(body_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_json(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"response.json(): invalid this".as_ptr());
        return false;
    }
    // BCE-012: root to_object() result — JS_GetProperty can trigger GC
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = this.to_object());
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut body_val,
    };
    JS_GetProperty(cx, obj.handle().into(), c"_bodyText".as_ptr(), b_handle);

    if !body_val.is_string() {
        JS_ReportErrorUTF8(cx, c"response.json(): body is not a string".as_ptr());
        return false;
    }

    // BCE-012: root JSString — JS_ParseJSON1 can trigger GC
    let js_str = body_val.to_string();
    rooted!(&in(wrapped_cx) let str_root = js_str);
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_root.handle().into(), rval_handle);

    if !ok {
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, c"response.json(): invalid JSON".as_ptr());
        return false;
    }
    args.rval().set(rval);
    true
}

pub fn install_headers_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(
            cx.raw_cx(),
            Some(headers_constructor),
            1,
            JSFUN_CONSTRUCTOR,
            c"Headers".as_ptr(),
        );
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(
                    cx,
                    global,
                    c"Headers".as_ptr(),
                    co.handle(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    // BCE-012: root JS_NewPlainObject result — JS_DefineProperty can trigger GC
    rooted!(&in(wrapped_cx) let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if headers_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let get_fn = JS_NewFunction(cx, Some(headers_get), 1, 0, c"get".as_ptr());
    if !get_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(get_fn);
        // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
        rooted!(&in(wrapped_cx) let fn_val = mozjs::jsval::ObjectValue(fn_ptr));
        JS_DefineProperty(
            cx,
            headers_obj.handle().into(),
            c"get".as_ptr(),
            fn_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let set_fn = JS_NewFunction(cx, Some(headers_set), 2, 0, c"set".as_ptr());
    if !set_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(set_fn);
        // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
        rooted!(&in(wrapped_cx) let fn_val = mozjs::jsval::ObjectValue(fn_ptr));
        JS_DefineProperty(
            cx,
            headers_obj.handle().into(),
            c"set".as_ptr(),
            fn_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let has_fn = JS_NewFunction(cx, Some(headers_has), 1, 0, c"has".as_ptr());
    if !has_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(has_fn);
        // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
        rooted!(&in(wrapped_cx) let fn_val = mozjs::jsval::ObjectValue(fn_ptr));
        JS_DefineProperty(
            cx,
            headers_obj.handle().into(),
            c"has".as_ptr(),
            fn_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval()
        .set(mozjs::jsval::ObjectValue(headers_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_get(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    let c_name = ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    // BCE-012: root to_object() result — JS_GetProperty can trigger GC
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = this.to_object());
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    };
    JS_GetProperty(cx, obj.handle().into(), c_name.as_ptr(), val_handle);
    if val.is_undefined() || val.is_null() {
        args.rval().set(mozjs::jsval::NullValue());
    } else {
        args.rval().set(val);
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_set(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    let c_name = ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // BCE-012: root to_object() result — JS_SetProperty can trigger GC
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = this.to_object());
    rooted!(&in(wrapped_cx) let val_root = value_val);
    JS_SetProperty(
        cx,
        obj.handle().into(),
        c_name.as_ptr(),
        val_root.handle().into(),
    );
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_has(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    let c_name = ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    // BCE-012: root to_object() result — JS_GetProperty can trigger GC
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = this.to_object());
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    };
    JS_GetProperty(cx, obj.handle().into(), c_name.as_ptr(), val_handle);
    args.rval().set(mozjs::jsval::BooleanValue(
        !val.is_undefined() && !val.is_null(),
    ));
    true
}

pub fn install_request_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(
            cx.raw_cx(),
            Some(request_constructor),
            2,
            JSFUN_CONSTRUCTOR,
            c"Request".as_ptr(),
        );
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(
                    cx,
                    global,
                    c"Request".as_ptr(),
                    co.handle(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn request_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    // BCE-012: root JS_NewPlainObject result — JS_DefineProperty can trigger GC
    rooted!(&in(wrapped_cx) let req_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if req_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // url argument
    let url_val = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { v } else { UndefinedValue() }
    } else {
        UndefinedValue()
    };
    rooted!(&in(wrapped_cx) let url_root = url_val);
    JS_DefineProperty(
        cx,
        req_obj.handle().into(),
        c"url".as_ptr(),
        url_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // method from options or default GET
    let method_str = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            // BCE-012: root to_object() result — JS_GetProperty can trigger GC
            rooted!(&in(wrapped_cx) let opts_obj = opts.to_object());
            let mut m_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_obj.handle().into(),
                c"method".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut m_val,
                },
            );
            if m_val.is_string() {
                crate::js_to_rust_string(cx, m_val)
            } else {
                "GET".to_string()
            }
        } else {
            "GET".to_string()
        }
    } else {
        "GET".to_string()
    };
    let method_cstr = ZBox::from_bytes(method_str.as_bytes());
    let method_jsstr = JS_NewStringCopyZ(cx, method_cstr.as_ptr());
    let method_val = StringValue(&*method_jsstr);
    rooted!(&in(wrapped_cx) let method_root = method_val);
    JS_DefineProperty(
        cx,
        req_obj.handle().into(),
        c"method".as_ptr(),
        method_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // headers (empty Headers-like object)
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    // BCE-012: root ObjectValue of GC-managed pointer — JS_DefineProperty can trigger GC
    rooted!(&in(wrapped_cx) let headers_val = mozjs::jsval::ObjectValue(headers_obj));
    JS_DefineProperty(
        cx,
        req_obj.handle().into(),
        c"headers".as_ptr(),
        headers_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(mozjs::jsval::ObjectValue(req_obj.get()));
    true
}

#[cfg(test)]
mod tests {
    // ── REQ-SEC-001: CORS Bypass Unit Tests ──────────────────────────────
    // @trace TEST-SEC-001 [req:REQ-SEC-001] [level:unit]

    /// REQ-SEC-001: fetch global is installed on page realm via install_all_native.
    #[test]
    fn cors_bypass_fetch_global_installed_for_page() {
        let source = include_str!("fetch_api.rs");
        assert!(
            source.contains("pub fn install_fetch_global"),
            "REQ-SEC-001: install_fetch_global must be pub for page realm installation"
        );
    }

    /// REQ-SEC-001: fetch delegates to fetch_async::start (event-driven, no CORS).
    #[test]
    fn cors_bypass_fetch_uses_event_driven_no_cors() {
        let source = include_str!("fetch_api.rs");
        assert!(
            source.contains("crate::fetch_async::start"),
            "REQ-SEC-001: fetch must delegate to fetch_async::start"
        );
        // Split string literal to avoid self-match in include_str source
        let forbidden_cors = ["cors", "_check"].join("");
        assert!(
            !source.contains(&forbidden_cors),
            "REQ-SEC-001 REGRESSION: fetch must NOT contain cors check"
        );
        // Split string literal to avoid self-match in include_str source
        let forbidden_cors_preflight = ["Access-Control", "-Request-Method"].join("");
        assert!(
            !source.contains(&forbidden_cors_preflight),
            "REQ-SEC-001 REGRESSION: fetch must NOT send CORS preflight headers"
        );
    }

    /// BCE-20260619-010: old thread::spawn/drain code is removed.
    #[test]
    fn bce_010_no_spawn_or_drain() {
        let source = include_str!("fetch_api.rs");
        // Split string literals to avoid self-match in include_str source
        let forbidden_spawn = ["spawn", "_fetch_worker"].join("");
        let forbidden_drain = ["drain", "_pending_fetches"].join("");
        let forbidden_blocking = ["do_fetch", "_blocking"].join("");
        assert!(
            !source.contains(&forbidden_spawn),
            "BCE-010 REGRESSION: spawn fetch worker must be removed"
        );
        assert!(
            !source.contains(&forbidden_drain),
            "BCE-010 REGRESSION: drain pending fetches must be removed"
        );
        assert!(
            !source.contains(&forbidden_blocking),
            "BCE-010 REGRESSION: do fetch blocking must be removed"
        );
    }
}
