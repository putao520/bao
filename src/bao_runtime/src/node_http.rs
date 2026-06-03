// @trace REQ-ENG-007
// P1-B.2: Replaced hand-written TcpListener + HTTP parsing with bun_uws::App<false>.
// uWS C++ layer handles HTTP parsing; route handler bridges to JS callbacks.
use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, Int32Value, StringValue, ObjectValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::response::Response;
use bun_uws_sys::request::Request;
use bun_uws_sys::socket_context::BunSocketContextOptions;

use crate::require::cache_builtin;

thread_local! {
    /// Active uWS App handles. Each `server.listen()` creates one App.
    static ACTIVE_APPS: RefCell<Vec<*mut App<false>>> = const { RefCell::new(Vec::new()) };
}

pub fn has_active_servers() -> bool {
    ACTIVE_APPS.with(|s| !s.borrow().is_empty())
}

pub fn listener_fds() -> Vec<i32> {
    // uWS App sockets are managed by the event loop (bao_uloop), not by
    // manual epoll. Return empty — drain_and_check no longer needs to
    // epoll_wait on HTTP listener fds; uWS handles I/O internally.
    Vec::new()
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let http_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if http_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, http_obj.handle(), c"createServer".as_ptr(), Some(http_create_server), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, http_obj.handle(), c"request".as_ptr(), Some(http_request), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, http_obj.handle(), c"get".as_ptr(), Some(http_get), 2, JSPROP_ENUMERATE as u32);

        {
            let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"node:http".as_ptr(), 1);
            if !opts.is_null() {
                let mut src_text = mozjs::rust::transform_str_to_source_text(
                    "function Server(opts, cb) { if (typeof opts === 'function') { cb = opts; } if (cb) this.on('request', cb); }\
                     Server.prototype.listen = function() { return this; };\
                     Server.prototype.close = function() { return this; };\
                     Server.prototype.on = function(e, fn) { if (!this._events) this._events = {}; (this._events[e] || (this._events[e] = [])).push(fn); return this; };\
                     Server.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events && this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return this; };\
                     Server"
                );
                let mut rval = UndefinedValue();
                JS::Evaluate2(cx.raw_cx(), opts, &mut src_text, MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
                });
                libc::free(opts as *mut _);
                if rval.is_object() {
                    let server_ctor = ObjectValue(rval.to_object());
                    rooted!(&in(cx) let sv = server_ctor);
                    JS_DefineProperty(cx.raw_cx(), http_obj.handle().into(), c"Server".as_ptr(), sv.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
                }
            }
        }

        rooted!(&in(cx) let status_obj = w2::JS_NewPlainObject(cx));
        if !status_obj.get().is_null() {
            let codes: &[(&str, &str)] = &[
                ("200", "OK"), ("201", "Created"), ("204", "No Content"),
                ("301", "Moved Permanently"), ("302", "Found"), ("304", "Not Modified"),
                ("400", "Bad Request"), ("401", "Unauthorized"), ("403", "Forbidden"),
                ("404", "Not Found"), ("405", "Method Not Allowed"),
                ("500", "Internal Server Error"), ("502", "Bad Gateway"), ("503", "Service Unavailable"),
            ];
            for (code, msg) in codes {
                let c_code = CString::new(*code).unwrap_or_default();
                let c_msg = CString::new(*msg).unwrap_or_default();
                let js_msg = JS_NewStringCopyZ(cx.raw_cx(), c_msg.as_ptr());
                if !js_msg.is_null() {
                    let mv = StringValue(&*js_msg);
                    rooted!(&in(cx) let mvr = mv);
                    JS_DefineProperty(cx.raw_cx(), status_obj.handle().into(), c_code.as_ptr(), mvr.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            let status_val = ObjectValue(status_obj.get());
            rooted!(&in(cx) let status_r = status_val);
            JS_DefineProperty(cx.raw_cx(), http_obj.handle().into(), c"STATUS_CODES".as_ptr(), status_r.handle().into(), JSPROP_ENUMERATE as u32);
        }

        {
            let methods = "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,TRACE";
            let c_methods = CString::new(methods).unwrap_or_default();
            let js_methods = JS_NewStringCopyZ(cx.raw_cx(), c_methods.as_ptr());
            if !js_methods.is_null() {
                rooted!(&in(cx) let mv = StringValue(&*js_methods));
                JS_DefineProperty(cx.raw_cx(), http_obj.handle().into(), c"METHODS".as_ptr(), mv.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
    }

    cache_builtin(cx, "http", http_obj.get());
}

// ──────────────────────────────────────────────────────────────
// uWS route handler — bridges C++ HTTP events to JS callbacks
// ──────────────────────────────────────────────────────────────

/// Per-server user data passed to uWS route handler via `user_data`.
struct ServerUserData {
    /// JSContext* for creating JS objects and calling JS functions.
    cx: *mut JSContext,
    /// Global object for looking up the request handler.
    global: *mut JSObject,
    /// The JS request handler function (stored as _onRequest on the server object,
    /// or _httpRequestHandler on the global).
    handler: *mut JSObject,
}

/// uWS route handler callback. Called by uWS C++ when an HTTP request arrives.
///
/// Reads method/url/headers from the uWS `Request` (already parsed by C++),
/// builds JS req/res objects, and calls the JS request handler.
/// The res object's `writeHead`/`write`/`end` methods bridge to
/// `Response::<false>::write_status`/`write_header`/`end`.
#[allow(unsafe_op_in_unsafe_fn)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn uws_route_handler(
    res: *mut bun_uws_sys::response::c::uws_res,
    req: *mut bun_uws_sys::Request,
    user_data: *mut ::std::ffi::c_void,
) {
    if res.is_null() || req.is_null() || user_data.is_null() {
        return;
    }

    let ud = &*(user_data as *const ServerUserData);
    let cx = ud.cx;
    if cx.is_null() { return; }

    let raw_cx = cx;
    let global = ud.global;
    if global.is_null() { return; }

    // Read method/url from uWS Request (C++ already parsed).
    let req_ref = bun_opaque::opaque_deref_mut(req);
    let method_bytes = req_ref.method();
    let url_bytes = req_ref.url();
    let method_str = ::std::str::from_utf8_unchecked(method_bytes);
    let url_str = ::std::str::from_utf8_unchecked(url_bytes);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Build JS request object.
    rooted!(&in(cx_ref) let req_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if req_obj.get().is_null() { return; }

    let Ok(c_method) = CString::new(method_str) else { return };
    let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
    if !js_method.is_null() {
        let mv = StringValue(&*js_method);
        rooted!(&in(cx_ref) let mvr = mv);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"method".as_ptr(), mvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let Ok(c_url) = CString::new(url_str) else { return };
    let js_url = JS_NewStringCopyZ(raw_cx, c_url.as_ptr());
    if !js_url.is_null() {
        let uv = StringValue(&*js_url);
        rooted!(&in(cx_ref) let uvr = uv);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"url".as_ptr(), uvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // Build headers object from common headers.
    rooted!(&in(cx_ref) let headers_obj = w2::JS_NewPlainObject(cx_ref));
    if !headers_obj.get().is_null() {
        let common_headers: &[&[u8]] = &[
            b"host", b"content-type", b"content-length", b"accept",
            b"user-agent", b"connection", b"authorization", b"cookie",
        ];
        for &name in common_headers {
            if let Some(value) = req_ref.header(name) {
                let Ok(c_k) = CString::new(::std::str::from_utf8_unchecked(name)) else { continue };
                let Ok(c_v) = CString::new(::std::str::from_utf8_unchecked(value)) else { continue };
                let js_v = JS_NewStringCopyZ(raw_cx, c_v.as_ptr());
                if !js_v.is_null() {
                    let hv = StringValue(&*js_v);
                    rooted!(&in(cx_ref) let hvr = hv);
                    JS_DefineProperty(raw_cx, headers_obj.handle().into(), c_k.as_ptr(), hvr.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
        }
        let hdrs_val = ObjectValue(headers_obj.get());
        rooted!(&in(cx_ref) let hdrs_r = hdrs_val);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"headers".as_ptr(), hdrs_r.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // Build JS response object with writeHead/write/end bridging to uWS Response.
    rooted!(&in(cx_ref) let res_obj = w2::JS_NewPlainObject(cx_ref));
    if res_obj.get().is_null() { return; }

    w2::JS_DefineFunction(cx_ref, res_obj.handle(), c"writeHead".as_ptr(), Some(res_write_head), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, res_obj.handle(), c"write".as_ptr(), Some(res_write), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, res_obj.handle(), c"end".as_ptr(), Some(res_end), 1, JSPROP_ENUMERATE as u32);

    let status_val = Int32Value(200);
    rooted!(&in(cx_ref) let sv = status_val);
    JS_DefineProperty(raw_cx, res_obj.handle().into(), c"statusCode".as_ptr(), sv.handle().into(), JSPROP_ENUMERATE as u32);

    // Store uWS res pointer on the JS response object for write/end.
    let res_ptr_val = Int32Value(res as i32);
    rooted!(&in(cx_ref) let rv = res_ptr_val);
    JS_DefineProperty(raw_cx, res_obj.handle().into(), c"_uwsRes".as_ptr(), rv.handle().into(), 0);

    // Call the JS request handler: handler(req, res)
    let handler_val = ObjectValue(ud.handler);
    let handler_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &handler_val };
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let args_vals = [ObjectValue(req_obj.get()), ObjectValue(res_obj.get())];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: args_vals.as_ptr(),
    };

    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    JS_CallFunctionValue(raw_cx, global_h, handler_h, &call_args, rval_h);
    JS_ClearPendingException(raw_cx);
}

// ──────────────────────────────────────────────────────────────
// JS response methods — bridge to uWS Response::<false>
// ──────────────────────────────────────────────────────────────

/// Recover the `*mut uws_res` stored as `_uwsRes` on the JS response object.
#[inline]
unsafe fn get_uws_res(cx: *mut JSContext, obj: *mut JSObject) -> *mut bun_uws_sys::response::c::uws_res {
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut ptr_val = Int32Value(0);
    JS_GetProperty(cx, obj_h, c"_uwsRes".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ptr_val });
    if ptr_val.is_int32() {
        ptr_val.to_int32() as *mut bun_uws_sys::response::c::uws_res
    } else {
        core::ptr::null_mut()
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn res_write_head(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            let status = v.to_int32();
            let this = args.thisv();
            let obj = this.to_object();
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            JS_SetProperty(cx, obj_h, c"statusCode".as_ptr(), Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v });

            // Write status to uWS Response.
            let uws_res = get_uws_res(cx, obj);
            if !uws_res.is_null() {
                let status_str = format!("{} ", status);
                let res_mut = Response::<false>::cast_res(uws_res);
                (*res_mut).write_status(status_str.as_bytes());
            }

            // Write headers if arg[1] is an object.
            if argc > 1 {
                let hdrs_val = *args.get(1).ptr;
                if hdrs_val.is_object() {
                    let hdrs_obj = hdrs_val.to_object();
                    let uws_res = get_uws_res(cx, obj);
                    if !uws_res.is_null() {
                        let res_mut = Response::<false>::cast_res(uws_res);
                        // Iterate known header keys.
                        let common: &[&[u8]] = &[
                            b"content-type", b"content-length", b"location",
                            b"set-cookie", b"cache-control", b"x-",
                        ];
                        for &key in common {
                            let Ok(c_key) = CString::new(::std::str::from_utf8_unchecked(key)) else { continue };
                            let mut hv = UndefinedValue();
                            JS_GetProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_obj }, c_key.as_ptr(),
                                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hv });
                            if hv.is_string() {
                                let val = crate::js_to_rust_string(cx, hv);
                                let Ok(c_val) = CString::new(val.as_str()) else { continue };
                                (*res_mut).write_header(key, c_val.as_bytes());
                            }
                        }
                    }
                }
            }
        }
    }
    args.rval().set(ObjectValue(args.thisv().to_object()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn res_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            let data = crate::js_to_rust_string(cx, v);
            let this = args.thisv();
            let obj = this.to_object();

            // Accumulate body in JS _body property.
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut body_val = UndefinedValue();
            let body_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
            JS_GetProperty(cx, obj_h, c"_body".as_ptr(), body_mh);
            let existing = if body_val.is_string() {
                crate::js_to_rust_string(cx, body_val)
            } else {
                String::new()
            };
            let mut combined = existing;
            combined.push_str(&data);
            let Ok(c_combined) = CString::new(combined) else {
                args.rval().set(ObjectValue(obj));
                return true;
            };
            let js_combined = JS_NewStringCopyZ(cx, c_combined.as_ptr());
            if !js_combined.is_null() {
                let cv = StringValue(&*js_combined);
                let cv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cv };
                JS_SetProperty(cx, obj_h, c"_body".as_ptr(), cv_h);
            }
        }
    }
    args.rval().set(ObjectValue(args.thisv().to_object()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn res_end(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // Append final data if provided.
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            let data = crate::js_to_rust_string(cx, v);
            let this = args.thisv();
            let obj = this.to_object();
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut body_val = UndefinedValue();
            let body_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
            JS_GetProperty(cx, obj_h, c"_body".as_ptr(), body_mh);
            let existing = if body_val.is_string() {
                crate::js_to_rust_string(cx, body_val)
            } else { String::new() };
            let mut combined = existing;
            combined.push_str(&data);
            let Ok(c_combined) = CString::new(combined) else {
                args.rval().set(ObjectValue(obj));
                return true;
            };
            let js_combined = JS_NewStringCopyZ(cx, c_combined.as_ptr());
            if !js_combined.is_null() {
                let cv = StringValue(&*js_combined);
                let cv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cv };
                JS_SetProperty(cx, obj_h, c"_body".as_ptr(), cv_h);
            }
        }
    }

    // Send response via uWS Response.
    let this = args.thisv();
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut body_val = UndefinedValue();
    let body_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_h, c"_body".as_ptr(), body_mh);
    let body = if body_val.is_string() {
        crate::js_to_rust_string(cx, body_val)
    } else { String::new() };

    let uws_res = get_uws_res(cx, obj);
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);

        // If writeHead was not called, write a default status.
        let mut status_val = Int32Value(200);
        let status_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut status_val };
        JS_GetProperty(cx, obj_h, c"statusCode".as_ptr(), status_mh);
        let status = if status_val.is_int32() { status_val.to_int32() } else { 200 };

        // Check if status was already written (uWS state tracks this).
        if !(*res_mut).state().is_http_status_called() {
            let status_str = format!("{} ", status);
            (*res_mut).write_status(status_str.as_bytes());
        }

        (*res_mut).end(body.as_bytes(), false);
    }

    args.rval().set(ObjectValue(obj));
    true
}

// ──────────────────────────────────────────────────────────────
// JS host functions: createServer, listen, close, address
// ──────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_create_server(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let server_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_object() {
            let cb_val = ObjectValue(v.to_object());
            rooted!(&in(cx_ref) let cb_root = cb_val);
            JS_DefineProperty(cx, server_obj.handle().into(), c"_onRequest".as_ptr(), cb_root.handle().into(), JSPROP_ENUMERATE as u32);

            let global = CurrentGlobalOrNull(cx);
            if !global.is_null() {
                let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                JS_SetProperty(cx, global_h, c"_httpRequestHandler".as_ptr(), cb_root.handle().into());
            }
        }
    }

    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"listen".as_ptr(), Some(server_listen), 3, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"close".as_ptr(), Some(server_close), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"address".as_ptr(), Some(server_address), 0, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

/// uWS listen callback — called when the server starts listening.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn uws_listen_callback(
    _listen_socket: *mut bun_uws_sys::listen_socket::ListenSocket,
    _user_data: *mut ::std::ffi::c_void,
) {
    // No-op: listening is confirmed by uWS. Port is read from ListenSocket
    // if needed, but we already know the port from the JS call.
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn server_listen(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let port: u16 = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() as u16 }
        else if v.is_double() { v.to_double() as u16 }
        else { 3000 }
    } else { 3000 };

    let callback = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_object() { Some(v.to_object()) } else { None }
    } else if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_object() { Some(v.to_object()) } else { None }
    } else { None };

    // Create uWS App<false> (non-SSL).
    let opts = BunSocketContextOptions::default();
    let app_ptr = match App::<false>::create(&opts) {
        Some(p) => p,
        None => {
            let msg = format!("Failed to create HTTP server on port {}", port);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // Get the JS request handler from the server object.
    let this = args.thisv();
    let server_obj = this.to_object();
    let server_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &server_obj };

    let mut handler_val = UndefinedValue();
    let handler_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val };
    JS_GetProperty(cx, server_h, c"_onRequest".as_ptr(), handler_mh);

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() || !handler_val.is_object() {
        // No handler — destroy app and return.
        App::<false>::destroy(app_ptr);
        let msg = CString::new("http.createServer requires a request handler").unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Allocate ServerUserData on the heap. Leaked until server_close.
    let ud = Box::new(ServerUserData {
        cx,
        global,
        handler: handler_val.to_object(),
    });
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route: app.any("/*", handler, user_data)
    // SAFETY: `unsafe extern "C"` and `extern "C"` fn pointers have identical ABI;
    // transmute is sound because the C layer only cares about the calling convention.
    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(uws_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void))) };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Listen on the specified port.
    // SAFETY: same ABI transmute rationale as above.
    let safe_listen_cb: extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void) =
        unsafe { ::std::mem::transmute(uws_listen_callback as unsafe extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void)) };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    // Store app pointer on server object for close/destroy.
    let mut wrapped_cx3 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref3 = &mut wrapped_cx3;
    let app_ptr_val = Int32Value(app_ptr as i32);
    rooted!(&in(cx_ref3) let apv = app_ptr_val);
    JS_DefineProperty(cx, server_h, c"_appPtr".as_ptr(), apv.handle().into(), 0);

    let port_val = Int32Value(port as i32);
    let port_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &port_val };
    JS_DefineProperty(cx, server_h, c"_listeningPort".as_ptr(), port_h, JSPROP_ENUMERATE as u32);

    let listening_val = mozjs::jsval::BooleanValue(true);
    let listening_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &listening_val };
    JS_DefineProperty(cx, server_h, c"listening".as_ptr(), listening_h, JSPROP_ENUMERATE as u32);

    ACTIVE_APPS.with(|s| s.borrow_mut().push(app_ptr));

    // Call listen callback if provided.
    if let Some(cb) = callback {
        let fval = ObjectValue(cb);
        let fval_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fval };
        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        JS_CallFunctionValue(cx, global_h, fval_h, &HandleValueArray::empty(), rval_h);
        JS_ClearPendingException(cx);
    }

    args.rval().set(ObjectValue(server_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn server_close(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let this = args.thisv();
    let server_obj = this.to_object();
    let server_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &server_obj };

    // Destroy the uWS App if it exists.
    let mut app_ptr_val = Int32Value(0);
    JS_GetProperty(cx, server_h, c"_appPtr".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut app_ptr_val });
    if app_ptr_val.is_int32() {
        let app_ptr = app_ptr_val.to_int32() as *mut App<false>;
        if !app_ptr.is_null() {
            App::<false>::destroy(app_ptr);
            ACTIVE_APPS.with(|s| {
                let mut apps = s.borrow_mut();
                apps.retain(|&p| p != app_ptr);
            });
        }
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn server_address(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let addr_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if addr_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let this = args.thisv();
    let server_obj = this.to_object();
    let server_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &server_obj };

    let mut port_val = UndefinedValue();
    let port_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut port_val };
    JS_GetProperty(cx, server_h, c"_listeningPort".as_ptr(), port_mh);

    if port_val.is_int32() {
        let p = port_val.to_int32();
        rooted!(&in(cx_ref) let pvr = Int32Value(p));
        JS_DefineProperty(cx, addr_obj.handle().into(), c"port".as_ptr(), pvr.handle().into(), JSPROP_ENUMERATE as u32);

        let c_family = CString::new("IPv4").unwrap_or_default();
        let js_family = JS_NewStringCopyZ(cx, c_family.as_ptr());
        if !js_family.is_null() {
            let fv = StringValue(&*js_family);
            rooted!(&in(cx_ref) let fvr = fv);
            JS_DefineProperty(cx, addr_obj.handle().into(), c"family".as_ptr(), fvr.handle().into(), JSPROP_ENUMERATE as u32);
        }

        let c_addr = CString::new("0.0.0.0").unwrap_or_default();
        let js_addr = JS_NewStringCopyZ(cx, c_addr.as_ptr());
        if !js_addr.is_null() {
            let av = StringValue(&*js_addr);
            rooted!(&in(cx_ref) let avr = av);
            JS_DefineProperty(cx, addr_obj.handle().into(), c"address".as_ptr(), avr.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(addr_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_request(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let req_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if req_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let url_str = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else { String::new() }
    } else { String::new() };

    let method = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else { "GET".to_string() }
    } else { "GET".to_string() };

    let c_url = CString::new(url_str).unwrap_or_default();
    let js_url = JS_NewStringCopyZ(cx, c_url.as_ptr());
    if !js_url.is_null() {
        let uv = StringValue(&*js_url);
        rooted!(&in(cx_ref) let uvr = uv);
        JS_DefineProperty(cx, req_obj.handle().into(), c"url".as_ptr(), uvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let c_method = CString::new(method).unwrap_or_default();
    let js_method = JS_NewStringCopyZ(cx, c_method.as_ptr());
    if !js_method.is_null() {
        let mv = StringValue(&*js_method);
        rooted!(&in(cx_ref) let mvr = mv);
        JS_DefineProperty(cx, req_obj.handle().into(), c"method".as_ptr(), mvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    args.rval().set(ObjectValue(req_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_get(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    http_request(cx, argc, vp)
}
