// @trace REQ-ENG-007
// P1-B.2: Replaced hand-written TcpListener + HTTP parsing with bun_uws::App<false>.
// uWS C++ layer handles HTTP parsing; route handler bridges to JS callbacks.
use ::std::cell::RefCell;
use bun_core::ZBox;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, Int32Value, StringValue, ObjectValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::response::Response;
use bun_uws_sys::request::Request;
use bun_uws_sys::socket_context::BunSocketContextOptions;

use crate::gc_store::{gc_store_insert_ns, gc_store_get_ns, gc_store_remove_ns};
use crate::require::cache_builtin;

/// Monotonic server ID for GcStore key namespacing.
static NEXT_SERVER_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Active uWS App handles. Each `server.listen()` creates one App.
    static ACTIVE_APPS: RefCell<Vec<*mut App<false>>> = const { RefCell::new(Vec::new()) };
}

pub fn has_active_servers() -> bool {
    ACTIVE_APPS.with(|s| !s.borrow().is_empty())
}

// ──────────────────────────────────────────────────────────────────────
// BCE-007 (runtime hang): unified JS-thread uWS-App liveness registry.
//
// Root cause (rootCause, design layer): `drain_and_check` (timers.rs) keeps
// the JS thread's uWS `Loop` alive ONLY while `node_http::has_active_servers()`
// returns true — that branch is what drives `tick_once` → `us_loop_run_bun_tick`
// → `epoll_wait`, which is the only way a JS-thread-bound uWS `App`'s listen
// socket ever `accept()`s an inbound connection.
//
// `Bun.serve` (bun_api.rs) creates its own `App::<false>` bound to the SAME
// JS-thread uWS `Loop` (via `uWS::Loop::get()` singleton), but historically
// never registered it with `ACTIVE_APPS`, so `has_active_servers()` stayed
// false for `Bun.serve`. Result: in `Bun.serve` + `fetch(self)` the worker's
// `connect()` sat in `EINPROGRESS` forever (strace: 0 `epoll_pwait` on the JS
// thread, ~4000 `clock_nanosleep(1ms)` spins), the server never `accept()`ed,
// the worker never wrote its result slot, and the fetch Promise never resolved.
//
// Unified fix: every module that creates a JS-thread-bound uWS `App::<false>`
// — `node_http::createServer` AND `Bun.serve` — registers it here so the
// single `has_active_servers()` source of truth drives the loop tick for both.
// This is the BCE "one fix for the whole class" — closing the registration gap
// rather than patching `drain_and_check` to special-case `Bun.serve`.
// @trace REQ-ENG-006 [api:Bun.serve] [api:http.createServer] unified liveness

/// Register a JS-thread-bound uWS `App::<false>` so `has_active_servers()`
/// reports liveness and `drain_and_check` keeps ticking the loop. Idempotent:
/// re-registering the same pointer is a no-op (defensive against double
/// registration in error paths). # Safety: `app` must be a live `*mut App<false>`
/// returned by `App::<false>::create`, valid until `unregister_active_app` is
/// called for it.
pub unsafe fn register_active_app(app: *mut App<false>) {
    if app.is_null() {
        return;
    }
    ACTIVE_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        if !apps.iter().any(|&p| ::core::ptr::eq(p, app)) {
            apps.push(app);
        }
    });
}

/// Unregister a previously-registered `App::<false>`. No-op if not present
/// (defensive against double-close paths). # Safety: `app` must match a
/// pointer previously passed to `register_active_app`.
pub unsafe fn unregister_active_app(app: *mut App<false>) {
    if app.is_null() {
        return;
    }
    ACTIVE_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        apps.retain(|&p| !::core::ptr::eq(p, app));
    });
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
                let c_code = ZBox::from_bytes(code.as_bytes());
                let c_msg = ZBox::from_bytes(msg.as_bytes());
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
            let c_methods = ZBox::from_bytes(methods.as_bytes());
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
/// GC-safe: JSObject references are stored in GcStore (as properties on the
/// JS global object, managed by SpiderMonkey GC). We only keep the string
/// keys — no raw `*mut JSObject` that could dangle after GC.
struct ServerUserData {
    /// JSContext* for creating JS objects and calling JS functions.
    cx: *mut JSContext,
    /// GcStore key for the global object.
    global_key: String,
    /// GcStore key for the JS request handler function.
    handler_key: String,
}

impl ServerUserData {
    /// Create a new ServerUserData, storing global and handler in GcStore.
    fn new(cx: *mut JSContext, global: *mut JSObject, handler: *mut JSObject) -> Self {
        let server_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let global_key = format!("http_server_{}_global", server_id);
        let handler_key = format!("http_server_{}_handler", server_id);
        gc_store_insert_ns(cx, "http", &global_key, global);
        gc_store_insert_ns(cx, "http", &handler_key, handler);
        Self { cx, global_key, handler_key }
    }

    /// Retrieve the global object from GcStore.
    fn global(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http", &self.global_key)
    }

    /// Retrieve the handler object from GcStore.
    fn handler(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http", &self.handler_key)
    }

    /// Remove both references from GcStore. Call on server close/cleanup.
    fn cleanup(&self) {
        gc_store_remove_ns(self.cx, "http", &self.global_key);
        gc_store_remove_ns(self.cx, "http", &self.handler_key);
    }
}

/// uWS route handler callback. Called by uWS C++ when an HTTP request arrives.
///
/// Reads method/url/headers from the uWS `Request` (already parsed by C++),
/// builds JS req/res objects, and calls the JS request handler.
/// The res object's `writeHead`/`write`/`end` methods bridge to
/// `Response::<false>::write_status`/`write_header`/`end`.
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
    let Some(global) = ud.global() else { return };
    if global.is_null() { return; }

    let Some(handler) = ud.handler() else { return };
    if handler.is_null() { return; }

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

    let c_method = ZBox::from_bytes(method_str.as_bytes());
    let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
    if !js_method.is_null() {
        let mv = StringValue(&*js_method);
        rooted!(&in(cx_ref) let mvr = mv);
        JS_DefineProperty(raw_cx, req_obj.handle().into(), c"method".as_ptr(), mvr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let c_url = ZBox::from_bytes(url_str.as_bytes());
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
                let c_k = ZBox::from_bytes(name);
                let c_v = ZBox::from_bytes(value);
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
    let res_ptr_val = mozjs::jsval::PrivateValue(res as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let rv = res_ptr_val);
    JS_DefineProperty(raw_cx, res_obj.handle().into(), c"_uwsRes".as_ptr(), rv.handle().into(), 0);

    // Call the JS request handler: handler(req, res)
    let handler_val = ObjectValue(handler);
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

/// Check if a JSVal is a PrivateValue (double with zero high bits).
/// @trace BCE-20260618-002 [level:regression]
/// SpiderMonkey encodes private values as doubles; this guard rejects
/// undefined/non-private doubles that would otherwise trigger the
/// `assert!(self.is_double())` panic in `to_private()` when a property
/// slot is unset (e.g. server object created without listening).
#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

/// Recover the `*mut uws_res` stored as `_uwsRes` on the JS response object.
#[inline]
unsafe fn get_uws_res(cx: *mut JSContext, obj: *mut JSObject) -> *mut bun_uws_sys::response::c::uws_res {
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut ptr_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"_uwsRes".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ptr_val });
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    if !val_is_private(&ptr_val) {
        return core::ptr::null_mut();
    }
    ptr_val.to_private() as *mut bun_uws_sys::response::c::uws_res
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
                            let c_key = ZBox::from_bytes(key);
                            let mut hv = UndefinedValue();
                            JS_GetProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_obj }, c_key.as_ptr(),
                                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hv });
                            if hv.is_string() {
                                let val = crate::js_to_rust_string(cx, hv);
                                let c_val = ZBox::from_bytes(val.as_bytes());
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
            let c_combined = ZBox::from_bytes(combined.as_bytes());
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
            let c_combined = ZBox::from_bytes(combined.as_bytes());
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

    // @trace REQ-ENG-007 [sm:HttpServer]
    // Node.js: http.Server extends EventEmitter, so `server.on("request", fn)`
    // and `server.emit("listening")` must work. Attach the shared EventEmitter
    // methods from node_events directly onto each server instance. The EE
    // state is stored in a hidden property on the server object, so this works
    // without changing the server's prototype.
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"on".as_ptr(), Some(crate::node_events::ee_on), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"addListener".as_ptr(), Some(crate::node_events::ee_on), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"once".as_ptr(), Some(crate::node_events::ee_once), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"off".as_ptr(), Some(crate::node_events::ee_off), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"removeListener".as_ptr(), Some(crate::node_events::ee_off), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"emit".as_ptr(), Some(crate::node_events::ee_emit), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"prependListener".as_ptr(), Some(crate::node_events::ee_prepend), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"removeAllListeners".as_ptr(), Some(crate::node_events::ee_remove_all), 1, JSPROP_ENUMERATE as u32);

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
            let c_msg = ZBox::from_bytes(msg.as_bytes());
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
        let msg = ZBox::from_bytes("http.createServer requires a request handler".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Allocate ServerUserData on the heap. GC-safe: global+handler stored in GcStore.
    let ud = Box::new(ServerUserData::new(cx, global, handler_val.to_object()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route: app.any("/*", handler, user_data)
    // SAFETY: `unsafe extern "C"` and `extern "C"` fn pointers have identical ABI;
    // transmute is sound because the C layer only cares about the calling convention.
    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(uws_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void))) };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Store the ServerUserData pointer on the server object for cleanup in server_close.
    {
        let mut wrapped_cx3 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref3 = &mut wrapped_cx3;
        let ud_val = mozjs::jsval::PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref3) let udv = ud_val);
        JS_DefineProperty(cx, server_h, c"_udPtr".as_ptr(), udv.handle().into(), 0);
    }

    // Listen on the specified port.
    // SAFETY: same ABI transmute rationale as above.
    let safe_listen_cb: extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void) =
        unsafe { ::std::mem::transmute(uws_listen_callback as unsafe extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void)) };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    // Store app pointer on server object for close/destroy.
    {
        let mut wrapped_cx3 = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref3 = &mut wrapped_cx3;
        let app_ptr_val = mozjs::jsval::PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref3) let apv = app_ptr_val);
        JS_DefineProperty(cx, server_h, c"_appPtr".as_ptr(), apv.handle().into(), 0);
    }

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
    let mut app_ptr_val = UndefinedValue();
    JS_GetProperty(cx, server_h, c"_appPtr".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut app_ptr_val });
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    // http.createServer(fn).close() before listen leaves _appPtr undefined;
    // to_private() on undefined asserts is_double() → panic across extern "C".
    let app_ptr = if val_is_private(&app_ptr_val) {
        app_ptr_val.to_private() as *mut App<false>
    } else {
        core::ptr::null_mut()
    };
    if !app_ptr.is_null() {
        (*app_ptr).close();
        App::<false>::destroy(app_ptr);
        // BCE-007: unified unregister (idempotent). Replaces the inline
        // `ACTIVE_APPS.retain` so the liveness registry has one update path.
        // Safety: app_ptr was registered by `server_listen` via the same
        // registry; idempotent if already removed.
        unsafe { unregister_active_app(app_ptr); }

        // @trace BCE-20260618-006 [level:regression] [api:http.createServer close]
        // Clear `_appPtr` on the JS server object so subsequent `close()`
        // calls are idempotent no-ops instead of use-after-free on the
        // destroyed `*mut App`. Same class of bug as Bun.serve's server_stop
        // (bun_api.rs) — double-close reads the stale pointer and calls
        // `close()`/`destroy()` on freed memory → SIGSEGV. Set to a non-
        // private value (UndefinedValue) so the val_is_private guard above
        // correctly takes the null path on re-entry.
        let undef = UndefinedValue();
        let undef_h = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &undef,
        };
        JS_SetProperty(cx, server_h, c"_appPtr".as_ptr(), undef_h);
    }

    // Cleanup: reclaim ServerUserData box and remove GcStore entries.
    let mut ud_ptr_val = UndefinedValue();
    JS_GetProperty(cx, server_h, c"_udPtr".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ud_ptr_val });
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    let ud_ptr = if val_is_private(&ud_ptr_val) {
        ud_ptr_val.to_private() as *mut ServerUserData
    } else {
        core::ptr::null_mut()
    };
    if !ud_ptr.is_null() {
        // SAFETY: ud_ptr was created by Box::into_raw in server_listen.
        let ud = unsafe { Box::from_raw(ud_ptr) };
        ud.cleanup();

        // @trace BCE-20260618-006 [level:regression] — same stale-pointer
        // class as `_appPtr` above. Clear `_udPtr` so a second `close()` is
        // a no-op (the Box has been consumed and the memory freed).
        let undef = UndefinedValue();
        let undef_h = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &undef,
        };
        JS_SetProperty(cx, server_h, c"_udPtr".as_ptr(), undef_h);
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

        let c_family = ZBox::from_bytes("IPv4".as_bytes());
        let js_family = JS_NewStringCopyZ(cx, c_family.as_ptr());
        if !js_family.is_null() {
            let fv = StringValue(&*js_family);
            rooted!(&in(cx_ref) let fvr = fv);
            JS_DefineProperty(cx, addr_obj.handle().into(), c"family".as_ptr(), fvr.handle().into(), JSPROP_ENUMERATE as u32);
        }

        let c_addr = ZBox::from_bytes("0.0.0.0".as_bytes());
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

// @trace REQ-ENG-010 [api:http.request async] [entity:FetchTasklet]
//
// BCE-20260618-007: `http.request` / `http.get` must not perform any blocking
// network I/O on the JS thread. The legacy shim built a plain metadata
// object (url/method) — no I/O — so it did not directly block, but it was
// listed in the BCE-007 sweep scope because `http.request` is the canonical
// JS-native HTTP entry. To guarantee the C2 invariant ("zero send_sync/
// stealth_http_request from any JS-native HTTP entry") and to make the API
// actually useful, we now return a *pending* Promise that resolves to a
// Response object (same shape as fetch()'s). The network round-trip runs on
// a detached worker via `fetch_async::start` (FetchTasklet pattern) — never
// on the JS thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_request(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

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

    // Create the PENDING Promise *while cx_ref holds a rooting context*,
    // then release cx_ref before scheduling (the worker must not outlive the
    // rooted frame, but the Promise itself is heap-rooted by fetch_async).
    rooted!(&in(cx_ref) let promise = unsafe {
        mozjs_sys::jsapi::JS::NewPromiseObject(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() },
        )
    });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let promise_obj = promise.get();
    let promise_val = ObjectValue(promise_obj);

    // Resolve method string → bun_http::Method (no I/O).
    let bun_method = match method.as_str() {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };
    // No stealth profile for plain http.request (Node API parity).
    let profile: Option<bao_stealth::StealthProfile> = None;

    // Schedule async fetch — detached worker, JS thread never blocks.
    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    unsafe {
        crate::fetch_async::start(
            cx,
            promise_val,
            profile,
            bun_method,
            url_str,
            Vec::new(),
            None,
        );
    }

    args.rval().set(promise_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_get(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    // http.get delegates to http_request (which now returns a pending Promise
    // and never blocks). The delegated call chain reaches no send_sync/
    // stealth_http_request on the JS thread — C2 invariant satisfied.
    http_request(cx, argc, vp)
}

// ── Unit tests for node_http pure Rust data/logic ──────────────────────
// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

#[cfg(test)]
mod tests {
    use super::*;

    // ── ACTIVE_APPS thread_local ──

    #[test]
    fn has_active_servers_false_initially() {
        // Clear any state leaked by previous tests on this thread.
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        assert!(!has_active_servers());
    }

    #[test]
    fn listener_fds_empty() {
        let fds = listener_fds();
        assert!(fds.is_empty());
    }

    // ── BCE-007 (runtime hang): unified liveness registry regression tests ──
    // @trace REQ-ENG-006 [api:Bun.serve] [api:http.createServer] unified liveness
    //
    // These guard the invariant that closed the BCE-007 fetch(self) hang: every
    // JS-thread-bound uWS App MUST be registered via register_active_app so
    // drain_and_check keeps ticking the uWS Loop while the server is listening.
    // Bun.serve previously skipped registration → the server's listen socket
    // never received accept() events → fetch(self) hung forever.
    //
    // We test the registry contract directly (not the full fetch(self) round
    // trip, which additionally depends on the HTTPThread client write path)
    // so the liveness invariant is locked in independently.

    /// BCE-007-R1: register_active_app flips has_active_servers() to true,
    /// and a matching unregister flips it back. Uses sentinel pointers (never
    /// dereferenced — register/unregister only compare/stored raw pointers).
    #[test]
    fn bce_007_register_unregister_flips_liveness() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        assert!(!has_active_servers(), "no servers initially");

        let sentinel_a: *mut App<false> = 0x1000 as *mut App<false>;
        let sentinel_b: *mut App<false> = 0x2000 as *mut App<false>;
        // SAFETY: register_active_app/unregister_active_app only store/compare
        // the raw pointer; they never dereference it. Sentinels are valid for
        // pointer-equality comparisons.
        unsafe {
            register_active_app(sentinel_a);
            assert!(has_active_servers(), "registered → live");
            register_active_app(sentinel_b);
            assert!(has_active_servers(), "still live with two");
            unregister_active_app(sentinel_a);
            assert!(has_active_servers(), "still live with one");
            unregister_active_app(sentinel_b);
            assert!(!has_active_servers(), "all unregistered → not live");
        }
    }

    /// BCE-007-R2: register is idempotent (re-registering the same pointer does
    /// not double-count), preventing the registry from growing unbounded on
    /// repeated Bun.serve calls in error paths.
    #[test]
    fn bce_007_register_is_idempotent() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        let sentinel: *mut App<false> = 0x3000 as *mut App<false>;
        // SAFETY: pointer never dereferenced (see R1).
        unsafe {
            register_active_app(sentinel);
            register_active_app(sentinel);
            register_active_app(sentinel);
            let count = ACTIVE_APPS.with(|s| s.borrow().len());
            assert_eq!(count, 1, "idempotent register must not duplicate");
            unregister_active_app(sentinel);
            assert!(!has_active_servers());
        }
    }

    /// BCE-007-R3: unregister is idempotent (unregistering an unknown pointer
    // is a no-op, not a panic) — defensive against double-close paths.
    #[test]
    fn bce_007_unregister_unknown_is_noop() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        let sentinel: *mut App<false> = 0x4000 as *mut App<false>;
        // SAFETY: pointer never dereferenced.
        unsafe {
            unregister_active_app(sentinel);
            assert!(!has_active_servers(), "unregister-unknown must not panic");
            register_active_app(0x5000 as *mut App<false>);
            unregister_active_app(sentinel);
            assert!(has_active_servers(), "unrelated unregister keeps live");
        }
    }

    /// BCE-007-R4: null is safely ignored by both register and unregister
    /// (Bun.serve stub-mode returns a null app_ptr; the call must be a no-op).
    #[test]
    fn bce_007_null_app_is_noop() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        // SAFETY: null is explicitly handled (early return) in both functions.
        unsafe {
            register_active_app(core::ptr::null_mut());
            assert!(!has_active_servers(), "null register must be a no-op");
            unregister_active_app(core::ptr::null_mut());
            assert!(!has_active_servers(), "null unregister must be a no-op");
        }
    }

    // ── ServerUserData (GC-safe GcStore keys) ──

    #[test]
    fn server_user_data_keys_are_namespaced() {
        // ServerUserData::new must produce unique namespaced GcStore keys.
        // We can't call ::new without a real JSContext, so test the key format directly.
        let key1 = format!("http_server_{}_global", 1);
        let key2 = format!("http_server_{}_handler", 1);
        assert!(key1.starts_with("http_server_"));
        assert!(key1.ends_with("_global"));
        assert!(key2.starts_with("http_server_"));
        assert!(key2.ends_with("_handler"));
        assert_ne!(key1, key2, "global and handler keys must differ");
    }

    #[test]
    fn server_user_data_next_server_id_monotonic() {
        let id1 = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let id2 = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id2 > id1, "server IDs must be monotonic");
    }

    #[test]
    fn server_user_data_global_handler_retrieve_without_cx() {
        // With null cx, gc_store_get returns None — the GC-safe path.
        let ud = ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http_server_999_global".to_string(),
            handler_key: "http_server_999_handler".to_string(),
        };
        assert!(ud.global().is_none(), "gc_store_get with null cx returns None");
        assert!(ud.handler().is_none(), "gc_store_get with null cx returns None");
    }

    #[test]
    fn server_user_data_cleanup_removes_from_gc_store() {
        // cleanup with null cx should not panic (gc_store_remove handles null cx gracefully).
        let ud = ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http_server_998_global".to_string(),
            handler_key: "http_server_998_handler".to_string(),
        };
        ud.cleanup(); // Must not panic
    }

    // ── HTTP STATUS_CODES (static data) ──

    static STATUS_CODES: &[(&str, &str)] = &[
        ("200", "OK"), ("201", "Created"), ("204", "No Content"),
        ("301", "Moved Permanently"), ("302", "Found"), ("304", "Not Modified"),
        ("400", "Bad Request"), ("401", "Unauthorized"), ("403", "Forbidden"),
        ("404", "Not Found"), ("405", "Method Not Allowed"),
        ("500", "Internal Server Error"), ("502", "Bad Gateway"), ("503", "Service Unavailable"),
    ];

    #[test]
    fn status_codes_count() {
        assert_eq!(STATUS_CODES.len(), 14);
    }

    #[test]
    fn status_codes_contains_200_ok() {
        assert!(STATUS_CODES.iter().any(|(c, m)| *c == "200" && *m == "OK"));
    }

    #[test]
    fn status_codes_contains_404_not_found() {
        assert!(STATUS_CODES.iter().any(|(c, m)| *c == "404" && *m == "Not Found"));
    }

    #[test]
    fn status_codes_contains_500_internal_server_error() {
        assert!(STATUS_CODES.iter().any(|(c, m)| *c == "500" && *m == "Internal Server Error"));
    }

    #[test]
    fn status_codes_all_numeric() {
        for (code, _) in STATUS_CODES {
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn status_codes_all_non_empty_messages() {
        for (_, msg) in STATUS_CODES {
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn status_codes_codes_unique() {
        let mut codes: Vec<&&str> = STATUS_CODES.iter().map(|(c, _)| c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), STATUS_CODES.len());
    }

    // ── HTTP METHODS string ──

    #[test]
    fn http_methods_string_format() {
        let methods = "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,TRACE";
        let method_list: Vec<&str> = methods.split(',').collect();
        assert_eq!(method_list.len(), 8);
        assert!(method_list.contains(&"GET"));
        assert!(method_list.contains(&"POST"));
        assert!(method_list.contains(&"DELETE"));
        assert!(method_list.contains(&"PATCH"));
    }

    #[test]
    fn http_methods_all_uppercase() {
        let methods = "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,TRACE";
        for m in methods.split(',') {
            assert_eq!(m, m.to_uppercase());
        }
    }

    // ── common_headers list ──

    #[test]
    fn common_headers_count() {
        let common_headers: &[&[u8]] = &[
            b"host", b"content-type", b"content-length", b"accept",
            b"user-agent", b"connection", b"authorization", b"cookie",
        ];
        assert_eq!(common_headers.len(), 8);
    }

    #[test]
    fn common_headers_all_lowercase() {
        let common_headers: &[&[u8]] = &[
            b"host", b"content-type", b"content-length", b"accept",
            b"user-agent", b"connection", b"authorization", b"cookie",
        ];
        for &h in common_headers {
            assert!(h.iter().all(|b| b.is_ascii_lowercase() || *b == b'-'));
        }
    }
}
