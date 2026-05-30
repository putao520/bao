use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::io::{BufRead, BufReader, Read as StdRead, Write as StdWrite};
use ::std::net::{TcpListener, TcpStream, SocketAddr};
use ::std::os::unix::io::{AsRawFd, FromRawFd};
use ::std::ptr::NonNull;
use ::std::sync::Mutex;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, Int32Value, StringValue, ObjectValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

struct PendingRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
    fd: i32,
}

static PENDING_HTTP: Mutex<Vec<PendingRequest>> = Mutex::new(Vec::new());

thread_local! {
    static ACTIVE_SERVERS: RefCell<Vec<SocketAddr>> = RefCell::new(Vec::new());
    static SERVER_LISTENERS: RefCell<Vec<TcpListener>> = RefCell::new(Vec::new());
}

pub fn has_active_servers() -> bool {
    ACTIVE_SERVERS.with(|s| !s.borrow().is_empty())
}

pub fn listener_fds() -> Vec<i32> {
    SERVER_LISTENERS.with(|l| {
        l.borrow().iter().map(|ln| ln.as_raw_fd()).collect()
    })
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
            let _server_src = CString::new(r#"
              function Server(opts, cb) { if (typeof opts === "function") { cb = opts; } if (cb) this.on("request", cb); }
              Server.prototype.listen = function() { return this; };
              Server.prototype.close = function() { return this; };
              Server.prototype.on = function(e, fn) { if (!this._events) this._events = {}; (this._events[e] || (this._events[e] = [])).push(fn); return this; };
              Server.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events && this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return this; };
              Server;
            "#).unwrap_or_default();
            let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), b"node:http\0".as_ptr() as *const ::std::os::raw::c_char, 1);
            if !opts.is_null() {
                let mut src_text = mozjs::rust::transform_str_to_source_text("function Server(opts, cb) { if (typeof opts === 'function') { cb = opts; } if (cb) this.on('request', cb); } Server.prototype.listen = function() { return this; }; Server.prototype.close = function() { return this; }; Server.prototype.on = function(e, fn) { if (!this._events) this._events = {}; (this._events[e] || (this._events[e] = [])).push(fn); return this; }; Server.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events && this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return this; }; Server");
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

/// Poll for pending HTTP requests and invoke JS callbacks.
/// Called from the main event loop tick.
pub fn poll_http_requests(cx: &mut mozjs::context::JSContext) -> bool {
    let requests: Vec<PendingRequest> = {
        let mut guard = match PENDING_HTTP.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        ::std::mem::take(&mut *guard)
    };

    if requests.is_empty() {
        return false;
    }

    for req in requests {
        unsafe { handle_request(cx, req); }
    }
    true
}

/// Accept new connections from listening servers and spawn request handling.
pub fn accept_connections() {
    SERVER_LISTENERS.with(|listeners| {
        let mut listeners = listeners.borrow_mut();
        for listener in listeners.iter_mut() {
            loop {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        stream.set_nonblocking(true).ok();
                        if let Ok(req) = parse_http_request(stream) {
                            if let Ok(mut guard) = PENDING_HTTP.lock() {
                                guard.push(req);
                            }
                        }
                    }
                    Err(ref e) if e.kind() == ::std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
    });
}

fn parse_http_request(stream: TcpStream) -> ::std::result::Result<PendingRequest, String> {
    let fd = stream.as_raw_fd();
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).map_err(|e| e.to_string())? == 0 {
        return Err("empty request".into());
    }
    let request_line = request_line.trim_end();

    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    let method = parts.get(0).unwrap_or(&"GET").to_string();
    let url = parts.get(1).unwrap_or(&"/").to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = line.trim_end();
                if line.is_empty() { break; }
                if let Some((k, v)) = line.split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        }
    }

    let content_length: usize = headers.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);

    let mut body = String::new();
    if content_length > 0 {
        let mut buf = vec![0u8; content_length.min(65536)];
        if let Ok(n) = reader.read(&mut buf) {
            body = String::from_utf8_lossy(&buf[..n]).into_owned();
        }
    }

    // Keep the original stream alive by leaking it; res_end will from_raw_fd + close
    ::std::mem::forget(stream);

    ::std::result::Result::Ok(PendingRequest {
        method,
        url,
        headers,
        body,
        fd,
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn handle_request(cx: &mut mozjs::context::JSContext, req: PendingRequest) {
    let raw_cx = cx.raw_cx();
    let global = CurrentGlobalOrNull(raw_cx);
    if global.is_null() { return; }

    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let mut on_req_val = UndefinedValue();
    let on_req_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut on_req_val };
    JS_GetProperty(raw_cx, global_h.into(), c"_httpRequestHandler".as_ptr(), on_req_mh);
    if !on_req_val.is_object() { return; }

    rooted!(&in(cx) let req_obj = w2::JS_NewPlainObject(cx));
    if req_obj.get().is_null() { return; }

    let c_method = CString::new(req.method).unwrap_or_default();
    let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
    if !js_method.is_null() {
        let mv = StringValue(&*js_method);
        rooted!(&in(cx) let mvr = mv);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"method".as_ptr(), mvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let c_url = CString::new(req.url).unwrap_or_default();
    let js_url = JS_NewStringCopyZ(raw_cx, c_url.as_ptr());
    if !js_url.is_null() {
        let uv = StringValue(&*js_url);
        rooted!(&in(cx) let uvr = uv);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"url".as_ptr(), uvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    rooted!(&in(cx) let headers_obj = w2::JS_NewPlainObject(cx));
    if !headers_obj.get().is_null() {
        for (k, v) in &req.headers {
            let Ok(c_k) = CString::new(k.as_str()) else { continue };
            let Ok(c_v) = CString::new(v.as_str()) else { continue };
            let js_v = JS_NewStringCopyZ(raw_cx, c_v.as_ptr());
            if !js_v.is_null() {
                let hv = StringValue(&*js_v);
                rooted!(&in(cx) let hvr = hv);
                JS_DefineProperty(raw_cx, headers_obj.handle().into(), c_k.as_ptr(), hvr.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        let hdrs_val = ObjectValue(headers_obj.get());
        rooted!(&in(cx) let hdrs_r = hdrs_val);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"headers".as_ptr(), hdrs_r.handle().into(), JSPROP_ENUMERATE as u32);
    }

    if !req.body.is_empty() {
        let Ok(c_body) = CString::new(req.body.as_str()) else { return };
        let js_body = JS_NewStringCopyZ(raw_cx, c_body.as_ptr());
        if !js_body.is_null() {
            let bv = StringValue(&*js_body);
            rooted!(&in(cx) let bvr = bv);
            JS_DefineProperty(raw_cx, req_obj.handle().into(), c"body".as_ptr(), bvr.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // Create response object with writeHead/write/end
    rooted!(&in(cx) let res_obj = w2::JS_NewPlainObject(cx));
    if res_obj.get().is_null() { return; }

    w2::JS_DefineFunction(cx, res_obj.handle(), c"writeHead".as_ptr(), Some(res_write_head), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx, res_obj.handle(), c"write".as_ptr(), Some(res_write), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx, res_obj.handle(), c"end".as_ptr(), Some(res_end), 1, JSPROP_ENUMERATE as u32);

    let status_val = Int32Value(200);
    rooted!(&in(cx) let sv = status_val);
    JS_DefineProperty(raw_cx, res_obj.handle().into(), c"statusCode".as_ptr(), sv.handle().into(), JSPROP_ENUMERATE as u32);

    // Store stream fd on response for write/end
    let fd_val = Int32Value(req.fd);
    rooted!(&in(cx) let fv = fd_val);
    JS_DefineProperty(raw_cx, res_obj.handle().into(), c"_fd".as_ptr(), fv.handle().into(), 0);

    // Call the request handler: handler(req, res)
    let cb_val = ObjectValue(on_req_val.to_object());
    let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };

    let args_vals = [ObjectValue(req_obj.get()), ObjectValue(res_obj.get())];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: args_vals.as_ptr(),
    };

    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    JS_CallFunctionValue(raw_cx, global_h, cb_h, &call_args, rval_h);
    JS_ClearPendingException(raw_cx);
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
            let this = args.thisv();
            let obj = this.to_object();
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            JS_SetProperty(cx, obj_h, c"statusCode".as_ptr(), Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v });
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
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut body_val = UndefinedValue();
            let body_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
            JS_GetProperty(cx, obj_h, c"_body".as_ptr(), body_mh);
            let existing = if body_val.is_string() {
                crate::js_to_rust_string(cx, body_val)
            } else {
                String::new()
            };
            let combined = format!("{}{}", existing, data);
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

    // Append final data if provided
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
            let combined = format!("{}{}", existing, data);
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

    // Build and send HTTP response
    let this = args.thisv();
    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut status_val = Int32Value(200);
    let status_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut status_val };
    JS_GetProperty(cx, obj_h, c"statusCode".as_ptr(), status_mh);
    let status = if status_val.is_int32() { status_val.to_int32() } else { 200 };

    let mut body_val = UndefinedValue();
    let body_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_h, c"_body".as_ptr(), body_mh);
    let body = if body_val.is_string() {
        crate::js_to_rust_string(cx, body_val)
    } else { String::new() };

    let mut fd_val = Int32Value(-1);
    let fd_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fd_val };
    JS_GetProperty(cx, obj_h, c"_fd".as_ptr(), fd_mh);
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    if fd >= 0 {
        let response = format!(
            "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{}",
            status, body.len(), body
        );
        let mut stream = unsafe { TcpStream::from_raw_fd(fd) };
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        ::std::mem::forget(stream);
    }

    args.rval().set(ObjectValue(obj));
    true
}

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

            // Also store as global for request handler access
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

    let host = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_string() {
            let s = crate::js_to_rust_string(cx, v);
            if !s.is_empty() { s } else { "0.0.0.0".to_string() }
        } else { "0.0.0.0".to_string() }
    } else { "0.0.0.0".to_string() };

    let callback = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_object() { Some(v.to_object()) } else { None }
    } else if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_object() { Some(v.to_object()) } else { None }
    } else { None };

    let bind_addr = format!("{}:{}", host, port);
    let listener = match TcpListener::bind(&bind_addr) {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("Failed to bind {}: {}", bind_addr, e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let actual_addr = listener.local_addr().unwrap_or_else(|_| {
        format!("{}:{}", host, port).parse().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
    });
    listener.set_nonblocking(true).ok();

    let this = args.thisv();
    let server_obj = this.to_object();
    let server_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &server_obj };

    let port_val = Int32Value(actual_addr.port() as i32);
    let port_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &port_val };
    JS_DefineProperty(cx, server_h, c"_listeningPort".as_ptr(), port_h, JSPROP_ENUMERATE as u32);

    let addr_str = actual_addr.to_string();
    let c_addr = CString::new(addr_str).unwrap_or_default();
    let js_addr = JS_NewStringCopyZ(cx, c_addr.as_ptr());
    if !js_addr.is_null() {
        let av = StringValue(&*js_addr);
        let av_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &av };
        JS_DefineProperty(cx, server_h, c"_address".as_ptr(), av_h, JSPROP_ENUMERATE as u32);
    }

    let fd = listener.as_raw_fd();
    let fd_val = Int32Value(fd as i32);
    let fd_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fd_val };
    JS_DefineProperty(cx, server_h, c"_fd".as_ptr(), fd_h, JSPROP_ENUMERATE as u32);

    let listening_val = mozjs::jsval::BooleanValue(true);
    let listening_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &listening_val };
    JS_DefineProperty(cx, server_h, c"listening".as_ptr(), listening_h, JSPROP_ENUMERATE as u32);

    ACTIVE_SERVERS.with(|s| s.borrow_mut().push(actual_addr));
    SERVER_LISTENERS.with(|l| l.borrow_mut().push(listener));

    if let Some(cb) = callback {
        let fval = ObjectValue(cb);
        let fval_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fval };
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
            JS_CallFunctionValue(cx, global_h, fval_h, &HandleValueArray::empty(), rval_h);
            JS_ClearPendingException(cx);
        }
    }

    args.rval().set(ObjectValue(server_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn server_close(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
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
    JS_GetProperty(cx, server_h.into(), c"_listeningPort".as_ptr(), port_mh);

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
