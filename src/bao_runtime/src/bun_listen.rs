// @trace REQ-BAO-API-017 [api:Bun.listen/connect/udpSocket] — network bridge
//! Bun.listen / Bun.connect / Bun.udpSocket — bridge to bun_uws + bun_uws_sys.
//!
//! Reuses bun_uws::App (HTTP), bun_uws_sys::SocketGroup (TCP),
//! bun_uws_sys::udp::Socket (UDP) — no hand-written HTTP/TCP/UDP code.

use ::std::cell::Cell;
use ::std::collections::HashMap;
use ::std::ptr::{self, NonNull};
use ::std::sync::atomic::{AtomicU16, AtomicU64, Ordering};

use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue,
    UndefinedValue,
};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::listen_socket::ListenSocket;
use bun_uws_sys::request::Request;
use bun_uws_sys::response::Response;
use bun_uws_sys::socket_context::BunSocketContextOptions;
use bun_uws_sys::socket_group::{SocketGroup, VTable};
use bun_uws_sys::udp::PacketBuffer;
use bun_uws_sys::udp::Socket as UdpSocket;
use bun_uws_sys::{CloseCode, ConnectResult, Loop, SocketKind, us_socket_t};

use crate::gc_store::{gc_store_get, gc_store_insert, gc_store_remove, gc_store_unique_key};

// ──────────────────── ID counters ────────────────────

static LISTEN_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
static CONNECT_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
static UDP_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

// ──────────────────── Event loop helper ────────────────────

/// Get the uSockets event loop, ensuring bao_uloop is initialized.
fn get_loop() -> *mut Loop {
    bao_uloop::force_link();
    bao_uloop::uws_get_loop()
}

// ──────────────────── Shared JS helpers ────────────────────

/// Extract a JS function callback from a property on a JS object.
/// Returns `None` if the property is missing or not a function.
/// @trace REQ-BAO-API-017 [api:Bun.listen/connect/udpSocket]
fn extract_js_callback(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    prop: &str,
) -> Option<*mut JSObject> {
    let mut cv = UndefinedValue();
    let c_prop = ZBox::from_bytes(prop.as_bytes());
    unsafe {
        JS_GetProperty(
            cx,
            obj_h,
            c_prop.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut cv,
            },
        );
    }
    if cv.is_object() {
        let cx2 = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
        rooted!(&in(cx2) let co = cv.to_object());
        if unsafe { JS_ObjectIsFunction(co.get()) } {
            return Some(co.get());
        }
    }
    None
}

/// Call a JS callback stored in GcStore under the given key.
/// Passes `args` as arguments to the JS function. Returns `true` on success.
///
/// Enters the context's persistent realm before resolving/invoking the
/// callback: the callback may fire from a pump long after the registering
/// eval returned, and GcStore stores the callback as a property on the
/// realm's global (so the realm must be current for the lookup to resolve).
/// @trace REQ-BAO-API-017 [api:Bun.listen/connect/udpSocket]
unsafe fn invoke_js_callback(cx: *mut JSContext, cb_key: &Option<String>, args: &[JSVal]) -> bool {
    let key = match cb_key {
        Some(k) => k,
        None => return false,
    };
    if cx.is_null() {
        return false;
    }

    let Some(global) = bao_engine::context::thread_realm_global() else {
        return false;
    };
    if global.is_null() {
        return false;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    // Inside the persistent realm: GcStore resolves the registered callback.
    let Some(cb_obj) = gc_store_get(cx, key) else {
        return false;
    };
    if cb_obj.is_null() {
        return false;
    }
    rooted!(&in(realm_cx) let handler_val = ObjectValue(cb_obj));

    // Build HandleValueArray from args slice
    let mut rooted_args: Vec<JSVal> = args.to_vec();
    let call_args = HandleValueArray {
        length_: rooted_args.len(),
        elements_: rooted_args.as_mut_ptr(),
    };

    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = JS_CallFunctionValue(
        realm_cx.raw_cx(),
        global_root.handle().into(),
        handler_val.handle().into(),
        &call_args,
        rval_h,
    );
    if !ok {
        JS_ClearPendingException(realm_cx.raw_cx());
    }
    ok
}

// ──────────────────── Thread-local state ────────────────────

thread_local! {
    /// Socket groups owned by Bun.listen TCP servers and Bun.connect.
    static CONNECT_GROUPS: ::std::cell::RefCell<HashMap<usize, Box<SocketGroup>>> = ::std::cell::RefCell::new(HashMap::new());
    /// Connected socket pointers.
    static LISTEN_TCP_SOCKETS: ::std::cell::RefCell<HashMap<usize, bool>> = ::std::cell::RefCell::new(HashMap::new());
    /// Result of a pending connect.
    static CONNECT_RESULT: Cell<Option<usize>> = Cell::new(None);
    /// Whether a connect error occurred.
    static CONNECT_ERROR: Cell<bool> = Cell::new(false);
}

// ──────────────────── Bun.listen ────────────────────

/// User data for Bun.listen HTTP mode (same shape as BunServeUserData).
#[allow(dead_code)]
struct ListenHttpUserData {
    fetch_cb_key: Option<String>,
    websocket_cb_key: Option<String>,
    app_ptr: *mut ::std::ffi::c_void,
    hostname: String,
    port: u16,
    actual_port: AtomicU16,
    cx: *mut JSContext,
}

impl ListenHttpUserData {
    /// Resolve the fetch handler from GcStore. Must be called inside the
    /// realm (dispatch sites `AutoRealm` into the persistent realm first).
    fn fetch_handler(&self) -> Option<*mut JSObject> {
        let key = self.fetch_cb_key.as_ref()?;
        if self.cx.is_null() {
            return None;
        }
        gc_store_get(self.cx, key)
    }
}

/// User data for Bun.listen TCP mode (socket callbacks).
#[allow(dead_code)]
struct ListenTcpUserData {
    /// GcStore keys for JS callbacks: onconnect, ondata, onclose, onend.
    connect_cb_key: Option<String>,
    data_cb_key: Option<String>,
    close_cb_key: Option<String>,
    end_cb_key: Option<String>,
    group_ptr: *mut SocketGroup,
    hostname: String,
    port: u16,
    actual_port: AtomicU16,
    cx: *mut JSContext,
}

/// @trace REQ-BAO-API-017 [api:Bun.listen] Bun.listen(options) -> Server
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut port: u16 = 0;
    let mut hostname = "0.0.0.0".to_string();
    let mut fetch_handler: Option<*mut JSObject> = None;
    let mut websocket_handler: Option<*mut JSObject> = None;
    // TCP mode: when `socket` option is present, Bun.listen creates a TCP server
    let mut is_tcp = false;
    let mut tcp_connect_handler: Option<*mut JSObject> = None;
    let mut tcp_data_handler: Option<*mut JSObject> = None;
    let mut tcp_close_handler: Option<*mut JSObject> = None;
    let mut tcp_end_handler: Option<*mut JSObject> = None;

    if argc > 0 {
        let opts_val = *args.get(0).ptr;
        if opts_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let opts_obj = opts_val.to_object());
            let opts_h = opts_obj.handle().into();

            // Parse port
            let mut pv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"port".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut pv,
                },
            );
            if pv.is_int32() {
                port = pv.to_int32().max(0) as u16;
            } else if pv.is_double() {
                port = pv.to_double().max(0.0) as u16;
            }

            // Parse hostname
            let mut hv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"hostname".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut hv,
                },
            );
            if hv.is_string() {
                hostname = crate::js_to_rust_string(cx, hv);
            }

            // Parse fetch handler
            let mut fv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"fetch".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut fv,
                },
            );
            if fv.is_object() {
                rooted!(&in(cx_ref) let fo = fv.to_object());
                if JS_ObjectIsFunction(fo.get()) {
                    fetch_handler = Some(fo.get());
                }
            }

            // Parse websocket handler
            let mut wv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"websocket".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut wv,
                },
            );
            if wv.is_object() {
                rooted!(&in(cx_ref) let wo = wv.to_object());
                if JS_ObjectIsFunction(wo.get()) {
                    websocket_handler = Some(wo.get());
                }
            }

            // Check for TCP mode: socket option indicates TCP server
            let mut sv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"socket".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut sv,
                },
            );
            if sv.is_object() {
                is_tcp = true;
                rooted!(&in(cx_ref) let so = sv.to_object());
                let so_h = so.handle().into();

                // socket.open/ondata/onclose/onend — use shared helper
                tcp_connect_handler = extract_js_callback(cx, so_h, "open");
                tcp_data_handler = extract_js_callback(cx, so_h, "data");
                tcp_close_handler = extract_js_callback(cx, so_h, "close");
                tcp_end_handler = extract_js_callback(cx, so_h, "end");
            }
        }
    }

    // Default: if no fetch and no socket, treat as HTTP server with default response
    if is_tcp {
        build_tcp_server(
            cx,
            args,
            port,
            &hostname,
            tcp_connect_handler,
            tcp_data_handler,
            tcp_close_handler,
            tcp_end_handler,
        )
    } else {
        build_http_server(cx, args, port, &hostname, fetch_handler, websocket_handler)
    }
}

/// Build an HTTP server via bun_uws::App (same pattern as bun_serve).
#[allow(unsafe_op_in_unsafe_fn)]
fn build_http_server(
    cx: *mut JSContext,
    args: CallArgs,
    port: u16,
    hostname: &str,
    fetch_handler: Option<*mut JSObject>,
    websocket_handler: Option<*mut JSObject>,
) -> bool {
    let listen_id = LISTEN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Store callbacks in GcStore
    let fetch_cb_key = fetch_handler.map(|cb| {
        let key = gc_store_unique_key(&format!("listen_fetch_{}", listen_id));
        gc_store_insert(cx, &key, cb);
        key
    });
    let websocket_cb_key = websocket_handler.map(|cb| {
        let key = gc_store_unique_key(&format!("listen_ws_{}", listen_id));
        gc_store_insert(cx, &key, cb);
        key
    });
    let fetch_cb_key_for_js = fetch_cb_key.clone();
    let websocket_cb_key_for_js = websocket_cb_key.clone();

    // Ensure event loop is initialized
    crate::timers::with_event_loop(|_| {});

    // Create uWS App
    // Note: HttpFlags::isNodeHttp stays at its default `false` here — Bun.listen
    // is serve semantics: RFC 9112 6.1 rejects an HTTP/1.0 request bearing
    // Transfer-Encoding with 400 (node_http::server_listen opts into llhttp
    // parity via set_is_node_http(true)).
    let opts = BunSocketContextOptions::default();
    let app_ptr = App::<false>::create(&opts).unwrap_or(ptr::null_mut());

    // Register with liveness registry (BCE-007 pattern)
    unsafe {
        crate::node_http::register_active_app(app_ptr);
    }

    // Build user data
    let ud = Box::new(ListenHttpUserData {
        fetch_cb_key,
        websocket_cb_key: websocket_cb_key.clone(),
        app_ptr: app_ptr as *mut ::std::ffi::c_void,
        hostname: hostname.to_string(),
        port,
        actual_port: AtomicU16::new(0),
        cx,
    });
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Route handler — mirrors bun_serve_route_handler
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn listen_route_handler(
        res: *mut bun_uws_sys::response::c::uws_res,
        req: *mut Request,
        user_data: *mut ::std::ffi::c_void,
    ) {
        let ud = &*(user_data as *const ListenHttpUserData);
        let res_mut = Response::<false>::cast_res(res);
        let req_ref = bun_opaque::opaque_deref_mut(req);

        // Default response ONLY when the user truly did not register a
        // `fetch` handler. (Bun.listen HTTP can be created without one as a
        // diagnostic echo server.)
        if ud.fetch_cb_key.is_none() {
            write_default_listen_response(&mut *res_mut, &*req_ref);
            return;
        }

        let cx = ud.cx;
        if cx.is_null() {
            eprintln!("[bun:listen] fetch handler registered but cx is null — responding 500");
            (*res_mut).write_status(b"500 Internal Server Error");
            (*res_mut).write_header(b"Content-Type", b"text/plain");
            (*res_mut).end(b"no JS context", true);
            return;
        }

        // Enter the context's persistent realm (first-principles realm model:
        // one realm per JsContext, held for the context's lifetime). Async
        // dispatch runs with no realm entered; the fetch handler is stored as
        // a property on this realm's global (GcStore), so we must be in the
        // realm to resolve it.
        let global = match bao_engine::context::thread_realm_global() {
            Some(g) if !g.is_null() => g,
            _ => {
                eprintln!("[bun:listen] no JS realm on this thread — responding 500");
                (*res_mut).write_status(b"500 Internal Server Error");
                (*res_mut).write_header(b"Content-Type", b"text/plain");
                (*res_mut).end(b"no JS realm", true);
                return;
            }
        };

        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let global_root = global);
        let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
        let cx_ref: &mut mozjs::context::JSContext = &mut realm;

        // Inside the persistent realm: GcStore resolves the fetch handler.
        // Registered-but-unresolvable is an explicit dispatch failure (never
        // a silent default echo that impersonates the handler's response).
        let fetch_handler = match ud.fetch_handler() {
            Some(h) if !h.is_null() => h,
            _ => {
                eprintln!("[bun:listen] fetch handler registered but unresolvable — responding 500");
                (*res_mut).write_status(b"500 Internal Server Error");
                (*res_mut).write_header(b"Content-Type", b"text/plain");
                (*res_mut).end(b"fetch handler unavailable", true);
                return;
            }
        };
        rooted!(&in(cx_ref) let handler_val = ObjectValue(fetch_handler));

        // Build JS Request object
        rooted!(&in(cx_ref) let req_obj = build_request_object(cx_ref, &*req_ref));
        if req_obj.get().is_null() {
            write_default_listen_response(&mut *res_mut, &*req_ref);
            return;
        }

        rooted!(&in(cx_ref) let req_val_elem = ObjectValue(req_obj.get()));
        let call_args = HandleValueArray {
            length_: 1,
            elements_: &*req_val_elem.handle(),
        };
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = JS_CallFunctionValue(
            cx,
            global_root.handle().into(),
            handler_val.handle().into(),
            &call_args,
            rval_h,
        );
        if !ok {
            JS_ClearPendingException(cx);
            (*res_mut).write_status(b"500 Internal Server Error");
            (*res_mut).write_header(b"Content-Type", b"text/plain");
            (*res_mut).end(b"fetch handler threw", true);
            return;
        }

        // Resolve response (support Promise return)
        let resp_obj = resolve_response_value(cx_ref, rval);
        if resp_obj.is_null() {
            (*res_mut).write_status(b"404 Not Found");
            (*res_mut).write_header(b"Content-Type", b"text/plain");
            (*res_mut).end(b"Not Found", true);
            return;
        }
        write_response_object(cx, &mut *res_mut, resp_obj);
    }

    if !app_ptr.is_null() {
        let safe_handler: Option<
            extern "C" fn(
                *mut bun_uws_sys::response::c::uws_res,
                *mut Request,
                *mut ::std::ffi::c_void,
            ),
        > = unsafe {
            ::std::mem::transmute(Some(
                listen_route_handler
                    as unsafe extern "C" fn(
                        *mut bun_uws_sys::response::c::uws_res,
                        *mut Request,
                        *mut ::std::ffi::c_void,
                    ),
            ))
        };

        unsafe {
            (*app_ptr).any(b"/*", safe_handler, ud_ptr);
        }

        // Listen callback for actual port
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn listen_listen_cb(
            listen_socket: *mut ListenSocket,
            user_data: *mut ::std::ffi::c_void,
        ) {
            if !listen_socket.is_null() {
                let ls_ref = bun_opaque::opaque_deref_mut(listen_socket);
                let ls_port = ls_ref.get_local_port();
                if ls_port > 0 && !user_data.is_null() {
                    let ud = &*(user_data as *const ListenHttpUserData);
                    ud.actual_port.store(ls_port as u16, Ordering::Release);
                }
                log::info!("Bun.listen() HTTP server listening (uWS port={})", ls_port);
            }
        }
        let safe_listen_cb: extern "C" fn(*mut ListenSocket, *mut ::std::ffi::c_void) = unsafe {
            ::std::mem::transmute(
                listen_listen_cb
                    as unsafe extern "C" fn(*mut ListenSocket, *mut ::std::ffi::c_void),
            )
        };
        unsafe {
            (*app_ptr).listen(port as i32, safe_listen_cb, ud_ptr);
        }
    }

    // Build JS Server object
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let server_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let srv_h = server_obj.handle().into();

    // Expose bound port
    let bound_port = unsafe {
        (*(ud_ptr as *const ListenHttpUserData))
            .actual_port
            .load(Ordering::Acquire)
    };
    let exposed_port = if bound_port > 0 { bound_port } else { port } as i32;
    rooted!(&in(cx_ref) let port_root = Int32Value(exposed_port));
    unsafe {
        JS_DefineProperty(
            cx,
            srv_h,
            c"port".as_ptr(),
            port_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Expose hostname
    let c_hn = ZBox::from_bytes(hostname.as_bytes());
    unsafe {
        let hn_str = JS_NewStringCopyZ(cx, c_hn.as_ptr());
        if !hn_str.is_null() {
            rooted!(&in(cx_ref) let hn_v = StringValue(&*hn_str));
            JS_DefineProperty(
                cx,
                srv_h,
                c"hostname".as_ptr(),
                hn_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Store app_ptr as private property
    let app_val = mozjs::jsval::PrivateValue(app_ptr as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let app_h = app_val);
    unsafe {
        JS_DefineProperty(cx, srv_h, c"_appPtr".as_ptr(), app_h.handle().into(), 0);
    }

    // Store ud_ptr for cleanup
    let ud_val = mozjs::jsval::PrivateValue(ud_ptr as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let ud_h = ud_val);
    unsafe {
        JS_DefineProperty(cx, srv_h, c"_udPtr".as_ptr(), ud_h.handle().into(), 0);
    }

    // Store GcStore keys
    if let Some(ref fk) = fetch_cb_key_for_js {
        let c_fk = ZBox::from_bytes(fk.as_bytes());
        unsafe {
            let fk_str = JS_NewStringCopyZ(cx, c_fk.as_ptr());
            if !fk_str.is_null() {
                rooted!(&in(cx_ref) let v = StringValue(&*fk_str));
                JS_DefineProperty(cx, srv_h, c"_fetchCbKey".as_ptr(), v.handle().into(), 0);
            }
        }
    }
    if let Some(ref wk) = websocket_cb_key_for_js {
        let c_wk = ZBox::from_bytes(wk.as_bytes());
        unsafe {
            let wk_str = JS_NewStringCopyZ(cx, c_wk.as_ptr());
            if !wk_str.is_null() {
                rooted!(&in(cx_ref) let v = StringValue(&*wk_str));
                JS_DefineProperty(cx, srv_h, c"_wsCbKey".as_ptr(), v.handle().into(), 0);
            }
        }
    }

    // server.stop()
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn http_server_stop(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        // Read and nullify _appPtr
        let mut app_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_appPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut app_val,
            },
        );
        let app_ptr = if app_val.is_double() && (app_val.asBits_ & 0xFFFF000000000000) == 0 {
            app_val.to_private() as *mut App<false>
        } else {
            ptr::null_mut()
        };
        if !app_ptr.is_null() {
            (*app_ptr).close();
            unsafe {
                crate::node_http::unregister_active_app(app_ptr);
            }
            App::<false>::destroy(app_ptr);
            // Nullify _appPtr to prevent double-free
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_appPtr".as_ptr(), undef_h);
        }

        // Free user data
        let mut ud_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_udPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ud_val,
            },
        );
        if ud_val.is_double() && (ud_val.asBits_ & 0xFFFF000000000000) == 0 {
            let ud_ptr = ud_val.to_private() as *mut ListenHttpUserData;
            if !ud_ptr.is_null() {
                drop(Box::from_raw(ud_ptr));
            }
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_udPtr".as_ptr(), undef_h);
        }

        // Clean up GcStore entries
        cleanup_gc_key(cx, this_h, c"_fetchCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_wsCbKey".as_ptr());

        args.rval().set(UndefinedValue());
        true
    }

    unsafe extern "C" fn server_ref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    unsafe extern "C" fn server_unref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    unsafe {
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            srv_h,
            c"stop".as_ptr(),
            Some(http_server_stop),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            srv_h,
            c"ref".as_ptr(),
            Some(server_ref),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            srv_h,
            c"unref".as_ptr(),
            Some(server_unref),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

/// Build a TCP server via SocketGroup.
#[allow(unsafe_op_in_unsafe_fn)]
fn build_tcp_server(
    cx: *mut JSContext,
    args: CallArgs,
    port: u16,
    hostname: &str,
    connect_handler: Option<*mut JSObject>,
    data_handler: Option<*mut JSObject>,
    close_handler: Option<*mut JSObject>,
    end_handler: Option<*mut JSObject>,
) -> bool {
    let listen_id = LISTEN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Store callbacks in GcStore
    let connect_cb_key = connect_handler.map(|cb| {
        let key = gc_store_unique_key(&format!("listen_tcp_connect_{}", listen_id));
        gc_store_insert(cx, &key, cb);
        key
    });
    let data_cb_key = data_handler.map(|cb| {
        let key = gc_store_unique_key(&format!("listen_tcp_data_{}", listen_id));
        gc_store_insert(cx, &key, cb);
        key
    });
    let close_cb_key = close_handler.map(|cb| {
        let key = gc_store_unique_key(&format!("listen_tcp_close_{}", listen_id));
        gc_store_insert(cx, &key, cb);
        key
    });
    let end_cb_key = end_handler.map(|cb| {
        let key = gc_store_unique_key(&format!("listen_tcp_end_{}", listen_id));
        gc_store_insert(cx, &key, cb);
        key
    });

    // Ensure event loop
    crate::timers::with_event_loop(|_| {});

    let loop_ = get_loop();
    if loop_.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Build user data BEFORE SocketGroup::init so we can pass it as owner
    let ud = Box::new(ListenTcpUserData {
        connect_cb_key,
        data_cb_key,
        close_cb_key,
        end_cb_key,
        group_ptr: ptr::null_mut(), // filled in after group creation
        hostname: hostname.to_string(),
        port,
        actual_port: AtomicU16::new(port), // updated after listen
        cx,
    });
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Create SocketGroup with VTable, passing user data as owner
    let mut group = Box::new(SocketGroup::default());
    group.init(loop_, Some(&TCP_LISTEN_VTABLE), ud_ptr);
    let group_ptr = Box::into_raw(group);

    // Back-fill group_ptr into user data
    unsafe {
        (*(ud_ptr as *mut ListenTcpUserData)).group_ptr = group_ptr;
    }

    let host_cstr = ZBox::from_bytes(hostname.as_bytes());
    let mut err: ::std::ffi::c_int = 0;

    let listen_socket = unsafe {
        (*group_ptr).listen(
            SocketKind::UwsHttp,
            None,
            Some((*host_cstr).as_cstr()),
            port as i32,
            0,
            0,
            &mut err,
        )
    };

    // The return value is the authoritative success signal (NULL ⟺ failure,
    // the uWS contract) — same usockets divergence root-caused in node_net
    // (bbe20a81): us_internal_bind_and_listen writes *error = LIBUS_ERR after
    // a SUCCESSFUL listen() (bsd.c:921), so `err != 0` here misjudged every
    // live listen socket as failed and destroyed its still-linked group —
    // SIGABRT in us_socket_group_deinit's head_listen_sockets assert on every
    // Bun.listen TCP path.
    let _ = err;
    if listen_socket.is_null() {
        unsafe {
            SocketGroup::destroy(group_ptr);
        }
        args.rval().set(UndefinedValue());
        return true;
    }

    // JS-idle tick surface (BCE-007 registration gap, Bun.listen TCP variant):
    // drain_and_check only drives the uWS loop while
    // node_http::has_active_servers() is true — without this registration an
    // idle script never accepts inbound connections on this group. Liveness
    // token only: the registry does ptr-eq bookkeeping and never dereferences
    // (the same representation-preserving alias node_http2 uses for App<true>).
    unsafe {
        crate::node_http::register_active_app(group_ptr as *mut bun_uws_sys::app::App<false>)
    };

    // Read actual bound port
    let ls_ref = bun_opaque::opaque_deref_mut(listen_socket);
    let p = ls_ref.get_local_port();
    let actual_port = if p > 0 { p as u16 } else { port };

    // Update actual_port in user data (created before group init)
    unsafe {
        (*(ud_ptr as *mut ListenTcpUserData))
            .actual_port
            .store(actual_port, Ordering::Release);
    }

    // Build JS Server object
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let server_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if server_obj.get().is_null() {
        // Cleanup
        let _ = unsafe { Box::from_raw(ud_ptr as *mut ListenTcpUserData) };
        args.rval().set(UndefinedValue());
        return true;
    }
    let srv_h = server_obj.handle().into();

    // Expose port
    rooted!(&in(cx_ref) let port_root = Int32Value(actual_port as i32));
    unsafe {
        JS_DefineProperty(
            cx,
            srv_h,
            c"port".as_ptr(),
            port_root.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Expose hostname
    let c_hn = ZBox::from_bytes(hostname.as_bytes());
    unsafe {
        let hn_str = JS_NewStringCopyZ(cx, c_hn.as_ptr());
        if !hn_str.is_null() {
            rooted!(&in(cx_ref) let hn_v = StringValue(&*hn_str));
            JS_DefineProperty(
                cx,
                srv_h,
                c"hostname".as_ptr(),
                hn_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Store pointers as private properties
    let group_val = mozjs::jsval::PrivateValue(group_ptr as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let g_h = group_val);
    unsafe {
        JS_DefineProperty(cx, srv_h, c"_groupPtr".as_ptr(), g_h.handle().into(), 0);
    }

    let ud_val = mozjs::jsval::PrivateValue(ud_ptr as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let u_h = ud_val);
    unsafe {
        JS_DefineProperty(cx, srv_h, c"_udPtr".as_ptr(), u_h.handle().into(), 0);
    }

    // server.stop()
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn tcp_server_stop(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        // Close all sockets in the group and destroy it
        let mut g_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_groupPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut g_val,
            },
        );
        if g_val.is_double() && (g_val.asBits_ & 0xFFFF000000000000) == 0 {
            let group_ptr = g_val.to_private() as *mut SocketGroup;
            if !group_ptr.is_null() {
                (*group_ptr).close_all();
                SocketGroup::destroy(group_ptr);
                // Drop the JS-idle liveness token registered in
                // build_tcp_server (see the BCE-007 note there).
                unsafe {
                    crate::node_http::unregister_active_app(
                        group_ptr as *mut bun_uws_sys::app::App<false>,
                    )
                };
            }
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_groupPtr".as_ptr(), undef_h);
        }

        // Free user data
        let mut ud_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_udPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ud_val,
            },
        );
        if ud_val.is_double() && (ud_val.asBits_ & 0xFFFF000000000000) == 0 {
            let ud_ptr = ud_val.to_private() as *mut ListenTcpUserData;
            if !ud_ptr.is_null() {
                drop(Box::from_raw(ud_ptr));
            }
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_udPtr".as_ptr(), undef_h);
        }

        // Clean up GcStore entries
        cleanup_gc_key(cx, this_h, c"_connectCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_dataCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_closeCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_endCbKey".as_ptr());

        args.rval().set(UndefinedValue());
        true
    }

    unsafe extern "C" fn server_ref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    unsafe extern "C" fn server_unref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    unsafe {
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            srv_h,
            c"stop".as_ptr(),
            Some(tcp_server_stop),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            srv_h,
            c"ref".as_ptr(),
            Some(server_ref),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            srv_h,
            c"unref".as_ptr(),
            Some(server_unref),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

// ──────────────────── Bun.connect ────────────────────

/// User data for Bun.connect TCP client.
struct ConnectUserData {
    data_cb_key: Option<String>,
    error_cb_key: Option<String>,
    close_cb_key: Option<String>,
    open_cb_key: Option<String>,
    end_cb_key: Option<String>,
    group_ptr: *mut SocketGroup,
    cx: *mut JSContext,
    /// Pending Promise to resolve on open / reject on error.
    promise: *mut JSObject,
    /// Whether the promise has been settled (resolved or rejected).
    promise_settled: Cell<bool>,
    /// Whether the `open` callback has been delivered. Loopback connects
    /// complete synchronously inside bun_connect (which fires `open` there),
    /// yet uWS still dispatches connect_on_open on the next loop pass for the
    /// same socket — without this guard the event fired twice and every
    /// open-time write duplicated its payload.
    open_fired: Cell<bool>,
}

/// @trace REQ-BAO-API-017 [api:Bun.connect] Bun.connect(options) -> Promise<Socket>
///
/// Creates a TCP client connection via bun_uws_sys::SocketGroup::connect().
/// Returns a Promise that resolves with the Socket on open, or rejects on error.
///
/// Options shape (matching Bun API):
///   Bun.connect({ hostname, port, tls?, socket: { data?, open?, close?, error?, end? } })
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut port: u16 = 0;
    let mut hostname = "127.0.0.1".to_string();
    let mut _tls = false;
    let mut on_data: Option<*mut JSObject> = None;
    let mut on_error: Option<*mut JSObject> = None;
    let mut on_close: Option<*mut JSObject> = None;
    let mut on_open: Option<*mut JSObject> = None;
    let mut on_end: Option<*mut JSObject> = None;

    if argc > 0 {
        let opts_val = *args.get(0).ptr;
        if opts_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let opts_obj = opts_val.to_object());
            let opts_h = opts_obj.handle().into();

            // Parse port
            let mut pv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"port".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut pv,
                },
            );
            if pv.is_int32() {
                port = pv.to_int32().max(0) as u16;
            } else if pv.is_double() {
                port = pv.to_double().max(0.0) as u16;
            }

            // Parse hostname
            let mut hv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"hostname".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut hv,
                },
            );
            if hv.is_string() {
                hostname = crate::js_to_rust_string(cx, hv);
            }

            // Parse tls (boolean)
            let mut tv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"tls".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut tv,
                },
            );
            _tls = tv.is_boolean() && tv.to_boolean();

            // Parse socket sub-object for callbacks (Bun API pattern)
            // Bun.connect({ hostname, port, socket: { data, open, close, error, end } })
            let mut sv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"socket".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut sv,
                },
            );
            if sv.is_object() {
                rooted!(&in(cx_ref) let so = sv.to_object());
                let so_h = so.handle().into();
                on_data = extract_js_callback(cx, so_h, "data");
                on_error = extract_js_callback(cx, so_h, "error");
                on_close = extract_js_callback(cx, so_h, "close");
                on_open = extract_js_callback(cx, so_h, "open");
                on_end = extract_js_callback(cx, so_h, "end");
            }

            // Fallback: also check top-level callbacks (if no socket sub-object)
            if on_data.is_none() {
                on_data = extract_js_callback(cx, opts_h, "data");
            }
            if on_error.is_none() {
                on_error = extract_js_callback(cx, opts_h, "error");
            }
            if on_close.is_none() {
                on_close = extract_js_callback(cx, opts_h, "close");
            }
            if on_open.is_none() {
                on_open = extract_js_callback(cx, opts_h, "open");
            }
            if on_end.is_none() {
                on_end = extract_js_callback(cx, opts_h, "end");
            }
        }
    }

    let connect_id = CONNECT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Store callbacks in GcStore
    let data_cb_key = on_data.map(|cb| {
        let k = gc_store_unique_key(&format!("connect_data_{}", connect_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let error_cb_key = on_error.map(|cb| {
        let k = gc_store_unique_key(&format!("connect_error_{}", connect_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let close_cb_key = on_close.map(|cb| {
        let k = gc_store_unique_key(&format!("connect_close_{}", connect_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let open_cb_key = on_open.map(|cb| {
        let k = gc_store_unique_key(&format!("connect_open_{}", connect_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let end_cb_key = on_end.map(|cb| {
        let k = gc_store_unique_key(&format!("connect_end_{}", connect_id));
        gc_store_insert(cx, &k, cb);
        k
    });

    // Create Promise for async result — SPEC requires Promise<Socket>
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let null_global = ::std::ptr::null_mut::<JSObject>());
    let promise =
        unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_global.handle().into()) };
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    crate::timers::with_event_loop(|_| {});

    let loop_ = get_loop();
    if loop_.is_null() {
        // Reject the promise — no event loop
        unsafe {
            reject_connect_promise(cx, promise, "Bun.connect: event loop not available");
        }
        rooted!(&in(cx_ref) let p = promise);
        args.rval().set(ObjectValue(p.get()));
        return true;
    }

    // Build user data BEFORE group init so we can pass it as owner for VTable callbacks
    let ud = Box::new(ConnectUserData {
        data_cb_key,
        error_cb_key,
        close_cb_key,
        open_cb_key,
        end_cb_key,
        group_ptr: ptr::null_mut(), // filled in after group creation
        cx,
        promise,
        promise_settled: Cell::new(false),
        open_fired: Cell::new(false),
    });
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Create per-connection socket group, passing user data as owner
    let mut group = Box::new(SocketGroup::default());
    group.init(loop_, Some(&CONNECT_VTABLE), ud_ptr);
    let group_ptr = Box::into_raw(group);

    // Back-fill group_ptr into user data
    unsafe {
        (*(ud_ptr as *mut ConnectUserData)).group_ptr = group_ptr;
    }

    let host_cstr = ZBox::from_bytes(hostname.as_bytes());

    // Reset thread-local connect state
    CONNECT_RESULT.with(|r| r.set(None));
    CONNECT_ERROR.with(|e| e.set(false));

    // Determine SocketKind based on tls option
    let socket_kind = if _tls {
        SocketKind::BunSocketTls
    } else {
        SocketKind::BunSocketTcp
    };

    let result = unsafe {
        (*group_ptr).connect(socket_kind, None, (*host_cstr).as_cstr(), port as i32, 0, 0)
    };

    let socket_key = match result {
        ConnectResult::Socket(socket) => {
            // Synchronous connect (e.g. localhost)
            let key = socket as usize;
            // Keep group alive alongside the socket
            CONNECT_GROUPS.with(|g| {
                g.borrow_mut()
                    .insert(key, unsafe { Box::from_raw(group_ptr) })
            });
            key
        }
        ConnectResult::Connecting(_) => {
            // Async connect — tick the loop until on_open or on_connect_error fires
            let group_key = group_ptr as usize;
            CONNECT_GROUPS.with(|g| {
                g.borrow_mut()
                    .insert(group_key, unsafe { Box::from_raw(group_ptr) })
            });

            let max_ticks: u32 = 5000;
            for _ in 0..max_ticks {
                if CONNECT_RESULT.with(|r| r.get().is_some()) {
                    break;
                }
                unsafe {
                    bao_uloop::bao_loop_tick(loop_, ptr::null());
                }
            }

            let error = CONNECT_ERROR.with(|e| e.get());
            let result_key = CONNECT_RESULT.with(|r| r.get().unwrap_or(0));
            if error || result_key == 0 {
                0
            } else {
                result_key
            }
        }
        ConnectResult::Failed => {
            unsafe {
                SocketGroup::destroy(group_ptr);
            }
            // Free user data since no socket was created
            let _ = unsafe { Box::from_raw(ud_ptr as *mut ConnectUserData) };
            0
        }
    };

    // Update group_ptr in user data based on connect result
    if socket_key == 0 {
        unsafe {
            (*(ud_ptr as *mut ConnectUserData)).group_ptr = ptr::null_mut();
        }
    } else {
        // JS-idle tick surface (BCE-007 class): connect-side data/close events
        // fire from vtable callbacks, which only run while the loop is ticked
        // (node_http::has_active_servers). Register the client socket as a
        // liveness token; connect_on_close drops it. Token only — the registry
        // never dereferences.
        unsafe {
            crate::node_http::register_active_app(socket_key as *mut bun_uws_sys::app::App<false>)
        };
    }

    // Build JS Socket object
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let socket_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if socket_obj.get().is_null() {
        let _ = unsafe { Box::from_raw(ud_ptr as *mut ConnectUserData) };
        unsafe {
            reject_connect_promise(cx, promise, "Bun.connect: failed to create socket object");
        }
        rooted!(&in(cx_ref) let p = promise);
        args.rval().set(ObjectValue(p.get()));
        return true;
    }
    let sock_h = socket_obj.handle().into();

    // Store socket pointer as double value (up to 2^53 lossless)
    rooted!(&in(cx_ref) let ptr_val = DoubleValue(socket_key as f64));
    unsafe {
        JS_DefineProperty(
            cx,
            sock_h,
            c"_socketPtr".as_ptr(),
            ptr_val.handle().into(),
            0,
        );
    }

    // Store ud_ptr
    let ud_jsval = mozjs::jsval::PrivateValue(ud_ptr as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let ud_h = ud_jsval);
    unsafe {
        JS_DefineProperty(cx, sock_h, c"_udPtr".as_ptr(), ud_h.handle().into(), 0);
    }

    // Store GcStore keys as private properties for cleanup
    // (keys are inside ConnectUserData which was Box::into_raw, so we access via ud_ptr)
    let ud_ref = unsafe { &*(ud_ptr as *const ConnectUserData) };
    store_gc_key_on_obj(
        cx,
        cx_ref,
        sock_h,
        c"_dataCbKey".as_ptr(),
        &ud_ref.data_cb_key,
    );
    store_gc_key_on_obj(
        cx,
        cx_ref,
        sock_h,
        c"_errorCbKey".as_ptr(),
        &ud_ref.error_cb_key,
    );
    store_gc_key_on_obj(
        cx,
        cx_ref,
        sock_h,
        c"_closeCbKey".as_ptr(),
        &ud_ref.close_cb_key,
    );
    store_gc_key_on_obj(
        cx,
        cx_ref,
        sock_h,
        c"_openCbKey".as_ptr(),
        &ud_ref.open_cb_key,
    );
    store_gc_key_on_obj(
        cx,
        cx_ref,
        sock_h,
        c"_endCbKey".as_ptr(),
        &ud_ref.end_cb_key,
    );

    // Store remote address info
    let c_hn = ZBox::from_bytes(hostname.as_bytes());
    unsafe {
        let hn_str = JS_NewStringCopyZ(cx, c_hn.as_ptr());
        if !hn_str.is_null() {
            rooted!(&in(cx_ref) let hn_v = StringValue(&*hn_str));
            JS_DefineProperty(
                cx,
                sock_h,
                c"remoteAddress".as_ptr(),
                hn_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let port_v = Int32Value(port as i32));
    unsafe {
        JS_DefineProperty(
            cx,
            sock_h,
            c"remotePort".as_ptr(),
            port_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // @trace REQ-BAO-API-017 [api:Bun.connect] localAddress — resolved from connected socket
    if socket_key > 0 {
        let socket_ptr = socket_key as *mut us_socket_t;
        let local_port = unsafe { (*socket_ptr).local_port() };
        let mut ip_buf = [0u8; 64];
        if let Ok(ip_slice) = unsafe { (*socket_ptr).local_address(&mut ip_buf) } {
            let ip_str = ::std::str::from_utf8(ip_slice).unwrap_or("0.0.0.0");
            let c_ip = ZBox::from_bytes(ip_str.as_bytes());
            unsafe {
                let ip_js = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                if !ip_js.is_null() {
                    rooted!(&in(cx_ref) let ip_v = StringValue(&*ip_js));
                    JS_DefineProperty(
                        cx,
                        sock_h,
                        c"localAddress".as_ptr(),
                        ip_v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(cx_ref) let lp_v = Int32Value(local_port as i32));
            unsafe {
                JS_DefineProperty(
                    cx,
                    sock_h,
                    c"localPort".as_ptr(),
                    lp_v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }

    // Store the socket object on user data so VTable callbacks can resolve the Promise with it
    // We keep a rooted reference via the Promise resolution
    unsafe {
        (*(ud_ptr as *mut ConnectUserData)).promise = promise;
    }

    // If connect failed synchronously, reject the promise now
    if socket_key == 0 {
        unsafe {
            reject_connect_promise(cx, promise, "Bun.connect: connection failed");
        }
    } else {
        // Resolve with the socket identity. Async connects registered the
        // identity and fired `open(socket)` in connect_on_open during the
        // wait loop above — re-fetch that SAME object so the resolved promise
        // and the open callback expose one socket. Already-open sockets
        // (loopback sync connects) never dispatch connect_on_open, so THIS is
        // where their identity gets registered and their open callback fires
        // (the promise used to stay pending forever on this path).
        unsafe {
            if let Some(global) = bao_engine::context::thread_realm_global() {
                if !global.is_null() {
                    let socket_ptr = socket_key as *mut us_socket_t;
                    // Same refused-connect discriminator as connect_on_open's
                    // guard: an inline-FAILED loopback connect (kernel returns
                    // ECONNREFUSED at syscall time) surfaces as a "Socket"
                    // result whose remote port is unset. Fire no open, resolve
                    // nothing — the still-registered liveness token keeps the
                    // loop ticking so the level-triggered EPOLLERR dispatches
                    // connect_on_connect_error, which rejects the promise and
                    // terminates the socket.
                    if (*socket_ptr).remote_port() > 0 {
                        let store_key = listen_sock_store_key(socket_ptr);
                        let sock_obj = match gc_store_get(cx, &store_key) {
                            Some(o) if !o.is_null() => o,
                            _ => {
                                let built = build_tcp_socket_obj(
                                    cx,
                                    socket_ptr,
                                    None,
                                    Some(&store_key),
                                );
                                if !built.is_null() {
                                    // Sync-connect arm: connect() completed
                                    // immediately, so deliver open HERE — but
                                    // exactly once (uWS still dispatches
                                    // connect_on_open for this socket on the
                                    // next loop pass; the flag there skips the
                                    // re-fire).
                                    if !ud_ref.open_fired.replace(true) {
                                        let open_val = ObjectValue(built);
                                        let _ = invoke_js_callback(
                                            cx,
                                            &ud_ref.open_cb_key,
                                            &[open_val],
                                        );
                                    }
                                }
                                built
                            }
                        };
                        if !sock_obj.is_null() {
                            let mut wrapped_cx_r = mozjs::context::JSContext::from_ptr(
                                NonNull::new_unchecked(cx),
                            );
                            let cx_ref_r = &mut wrapped_cx_r;
                            rooted!(&in(cx_ref_r) let p = promise);
                            rooted!(&in(cx_ref_r) let val = ObjectValue(sock_obj));
                            mozjs_sys::jsapi::JS::ResolvePromise(
                                cx,
                                p.handle().into(),
                                val.handle().into(),
                            );
                            mozjs_sys::jsapi::js::RunJobs(cx);
                        }
                    }
                }
            }
        }
    }

    // socket.write(data)
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn socket_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let mut ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_socketPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ptr_val,
            },
        );
        let socket_key = if ptr_val.is_double() {
            ptr_val.to_double() as usize
        } else {
            0
        };
        if socket_key == 0 {
            args.rval().set(BooleanValue(false));
            return true;
        }

        let data = if argc > 0 && (*args.get(0).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(0).ptr)
        } else {
            String::new()
        };

        let socket_ptr = socket_key as *mut us_socket_t;
        let written = unsafe { (*socket_ptr).write(data.as_bytes()) };
        args.rval().set(Int32Value(written));
        true
    }

    // socket.end()
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn socket_end(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let mut ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_socketPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ptr_val,
            },
        );
        if ptr_val.is_double() {
            let socket_ptr = ptr_val.to_double() as usize as *mut us_socket_t;
            unsafe {
                (*socket_ptr).close(CloseCode::normal);
            }
        }

        // Fire end callback
        let ud = get_connect_ud(cx, this_h);
        if let Some(ud_ref) = ud {
            let _ = invoke_js_callback(ud_ref.cx, &ud_ref.end_cb_key, &[]);
        }

        args.rval().set(UndefinedValue());
        true
    }

    // socket.destroy()
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn socket_destroy(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let mut ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_socketPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ptr_val,
            },
        );
        if ptr_val.is_double() {
            let socket_ptr = ptr_val.to_double() as usize as *mut us_socket_t;
            unsafe {
                (*socket_ptr).close(CloseCode::failure);
            }
        }

        // Clean up GcStore + user data
        cleanup_gc_key(cx, this_h, c"_dataCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_errorCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_closeCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_openCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_endCbKey".as_ptr());

        let mut ud_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_udPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ud_val,
            },
        );
        if ud_val.is_double() && (ud_val.asBits_ & 0xFFFF000000000000) == 0 {
            let ud_ptr = ud_val.to_private() as *mut ConnectUserData;
            if !ud_ptr.is_null() {
                drop(Box::from_raw(ud_ptr));
            }
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_udPtr".as_ptr(), undef_h);
        }

        args.rval().set(UndefinedValue());
        true
    }

    unsafe {
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            sock_h,
            c"write".as_ptr(),
            Some(socket_write),
            1,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            sock_h,
            c"end".as_ptr(),
            Some(socket_end),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            sock_h,
            c"destroy".as_ptr(),
            Some(socket_destroy),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    // Return the Promise
    rooted!(&in(cx_ref) let promise_root = promise);
    args.rval().set(ObjectValue(promise_root.get()));
    true
}

// ──────────────────── Bun.udpSocket ────────────────────

/// User data for Bun.udpSocket.
struct UdpUserData {
    data_cb_key: Option<String>,
    drain_cb_key: Option<String>,
    close_cb_key: Option<String>,
    error_cb_key: Option<String>,
    cx: *mut JSContext,
}

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] Bun.udpSocket(hostname, port) -> UDPSocket
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_udp_socket(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // Bun.udpSocket(options) — parse options object
    let mut port: u16 = 0;
    let mut hostname = "0.0.0.0".to_string();
    let mut on_data: Option<*mut JSObject> = None;
    let mut on_drain: Option<*mut JSObject> = None;
    let mut on_close: Option<*mut JSObject> = None;
    let mut on_error: Option<*mut JSObject> = None;

    if argc > 0 {
        let opts_val = *args.get(0).ptr;
        // Support both Bun.udpSocket(port, hostname) and Bun.udpSocket(options)
        if opts_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let opts_obj = opts_val.to_object());
            let opts_h = opts_obj.handle().into();

            let mut pv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"port".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut pv,
                },
            );
            if pv.is_int32() {
                port = pv.to_int32().max(0) as u16;
            } else if pv.is_double() {
                port = pv.to_double().max(0.0) as u16;
            }

            let mut hv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"hostname".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut hv,
                },
            );
            if hv.is_string() {
                hostname = crate::js_to_rust_string(cx, hv);
            }

            on_data = extract_js_callback(cx, opts_h, "data");
            on_drain = extract_js_callback(cx, opts_h, "drain");
            on_close = extract_js_callback(cx, opts_h, "close");
            on_error = extract_js_callback(cx, opts_h, "error");
        } else if opts_val.is_int32() {
            // Bun.udpSocket(port) shorthand
            port = opts_val.to_int32().max(0) as u16;
            if argc > 1 && (*args.get(1).ptr).is_string() {
                hostname = crate::js_to_rust_string(cx, *args.get(1).ptr);
            }
        }
    }

    let udp_id = UDP_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Store callbacks
    let data_cb_key = on_data.map(|cb| {
        let k = gc_store_unique_key(&format!("udp_data_{}", udp_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let drain_cb_key = on_drain.map(|cb| {
        let k = gc_store_unique_key(&format!("udp_drain_{}", udp_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let close_cb_key = on_close.map(|cb| {
        let k = gc_store_unique_key(&format!("udp_close_{}", udp_id));
        gc_store_insert(cx, &k, cb);
        k
    });
    let error_cb_key = on_error.map(|cb| {
        let k = gc_store_unique_key(&format!("udp_error_{}", udp_id));
        gc_store_insert(cx, &k, cb);
        k
    });

    crate::timers::with_event_loop(|_| {});

    let loop_ = get_loop();
    if loop_.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Build user data (stored as udp socket user_data)
    let ud = Box::new(UdpUserData {
        data_cb_key,
        drain_cb_key,
        close_cb_key,
        error_cb_key,
        cx,
    });
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    let host_cstr = ZBox::from_bytes(hostname.as_bytes());
    let mut err: ::std::ffi::c_int = 0;

    let udp_socket = UdpSocket::create(
        loop_,
        udp_on_data,
        udp_on_drain,
        udp_on_close,
        udp_on_recv_error,
        (*host_cstr).as_cstr().as_ptr(),
        port,
        0, // options
        Some(&mut err),
        ud_ptr,
    );

    if udp_socket.is_null() || err != 0 {
        let _ = unsafe { Box::from_raw(ud_ptr as *mut UdpUserData) };
        args.rval().set(UndefinedValue());
        return true;
    }

    // Read actual bound port
    let bound_port = unsafe { (*udp_socket).bound_port() };

    // Build JS UDPSocket object
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let udp_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if udp_obj.get().is_null() {
        unsafe {
            (*udp_socket).close();
        }
        let _ = Box::from_raw(ud_ptr as *mut UdpUserData);
        args.rval().set(UndefinedValue());
        return true;
    }
    let udp_h = udp_obj.handle().into();

    // Store socket pointer as private value
    let sock_val = mozjs::jsval::PrivateValue(udp_socket as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let sv = sock_val);
    unsafe {
        JS_DefineProperty(cx, udp_h, c"_socketPtr".as_ptr(), sv.handle().into(), 0);
    }

    // Store ud_ptr for cleanup
    let ud_jsval = mozjs::jsval::PrivateValue(ud_ptr as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let uv = ud_jsval);
    unsafe {
        JS_DefineProperty(cx, udp_h, c"_udPtr".as_ptr(), uv.handle().into(), 0);
    }

    // Expose address info
    let exposed_port = if bound_port > 0 {
        bound_port
    } else {
        port as i32
    };
    rooted!(&in(cx_ref) let port_v = Int32Value(exposed_port));
    unsafe {
        JS_DefineProperty(
            cx,
            udp_h,
            c"port".as_ptr(),
            port_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let c_hn = ZBox::from_bytes(hostname.as_bytes());
    unsafe {
        let hn_str = JS_NewStringCopyZ(cx, c_hn.as_ptr());
        if !hn_str.is_null() {
            rooted!(&in(cx_ref) let hn_v = StringValue(&*hn_str));
            JS_DefineProperty(
                cx,
                udp_h,
                c"hostname".as_ptr(),
                hn_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // udp.send(data, port, address)
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_send(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let mut sv = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_socketPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut sv,
            },
        );
        if !sv.is_double() || (sv.asBits_ & 0xFFFF000000000000) != 0 {
            args.rval().set(Int32Value(-1));
            return true;
        }
        let socket_ptr = sv.to_private() as *mut UdpSocket;

        // Parse arguments: send(data, port, address)
        let data = if argc > 0 && (*args.get(0).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(0).ptr)
        } else {
            String::new()
        };
        let target_port: u16 = if argc > 1 {
            (*args.get(1).ptr).to_int32().max(0) as u16
        } else {
            0
        };
        let target_addr = if argc > 2 && (*args.get(2).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(2).ptr)
        } else {
            "127.0.0.1".to_string()
        };

        // Build sockaddr for target address
        let mut addr_storage: libc::sockaddr_storage = unsafe { ::std::mem::zeroed() };
        let addr_len = build_sockaddr(&target_addr, target_port, &mut addr_storage);

        if addr_len == 0 {
            args.rval().set(Int32Value(-1));
            return true;
        }

        let payloads: [*const u8; 1] = [data.as_ptr()];
        let lengths: [usize; 1] = [data.len()];
        let addresses: [*const ::std::ffi::c_void; 1] =
            [&addr_storage as *const _ as *const ::std::ffi::c_void];

        let sent = unsafe { (*socket_ptr).send(&payloads, &lengths, &addresses) };
        args.rval().set(Int32Value(sent));
        true
    }

    // udp.close()
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_close_fn(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let mut sv = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_socketPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut sv,
            },
        );
        if sv.is_double() && (sv.asBits_ & 0xFFFF000000000000) == 0 {
            let socket_ptr = sv.to_private() as *mut UdpSocket;
            if !socket_ptr.is_null() {
                unsafe {
                    (*socket_ptr).close();
                }
            }
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_socketPtr".as_ptr(), undef_h);
        }

        // Clean up GcStore
        cleanup_gc_key(cx, this_h, c"_dataCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_drainCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_closeCbKey".as_ptr());
        cleanup_gc_key(cx, this_h, c"_errorCbKey".as_ptr());

        // Free user data
        let mut ud_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_udPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ud_val,
            },
        );
        if ud_val.is_double() && (ud_val.asBits_ & 0xFFFF000000000000) == 0 {
            let ud_ptr = ud_val.to_private() as *mut UdpUserData;
            if !ud_ptr.is_null() {
                drop(Box::from_raw(ud_ptr));
            }
            let undef = UndefinedValue();
            let undef_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &undef,
            };
            JS_SetProperty(cx, this_h, c"_udPtr".as_ptr(), undef_h);
        }

        args.rval().set(UndefinedValue());
        true
    }

    // udp.address()
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_address(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let mut sv = UndefinedValue();
        JS_GetProperty(
            cx,
            this_h,
            c"_socketPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut sv,
            },
        );
        if !sv.is_double() || (sv.asBits_ & 0xFFFF000000000000) != 0 {
            args.rval().set(UndefinedValue());
            return true;
        }
        let socket_ptr = sv.to_private() as *mut UdpSocket;
        let mut buf = [0u8; 64];
        let mut len = 64i32;
        unsafe {
            (*socket_ptr).bound_ip(buf.as_mut_ptr(), &mut len);
        }

        let ret_obj = unsafe { w2::JS_NewPlainObject(cx_ref) };
        rooted!(&in(cx_ref) let ret_root = ret_obj);
        if !ret_root.get().is_null() {
            let ip_str = ::std::str::from_utf8(&buf[..len.max(0) as usize]).unwrap_or("0.0.0.0");
            let c_ip = ZBox::from_bytes(ip_str.as_bytes());
            let js_ip = JS_NewStringCopyZ(cx, c_ip.as_ptr());
            if !js_ip.is_null() {
                rooted!(&in(cx_ref) let ip_v = StringValue(&*js_ip));
                JS_DefineProperty(
                    cx,
                    ret_root.handle().into(),
                    c"address".as_ptr(),
                    ip_v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let bound = unsafe { (*socket_ptr).bound_port() };
            rooted!(&in(cx_ref) let p_v = Int32Value(bound));
            JS_DefineProperty(
                cx,
                ret_root.handle().into(),
                c"port".as_ptr(),
                p_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            args.rval().set(ObjectValue(ret_root.get()));
        } else {
            args.rval().set(UndefinedValue());
        }
        true
    }

    // udp.ref() / udp.unref() — no-ops (event loop driven)
    unsafe extern "C" fn udp_ref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }
    unsafe extern "C" fn udp_unref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        args.rval().set(UndefinedValue());
        true
    }

    unsafe {
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"send".as_ptr(),
            Some(udp_send),
            3,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"close".as_ptr(),
            Some(udp_close_fn),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"address".as_ptr(),
            Some(udp_address),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"ref".as_ptr(),
            Some(udp_ref),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"unref".as_ptr(),
            Some(udp_unref),
            0,
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval().set(ObjectValue(udp_obj.get()));
    true
}

// ──────────────────── TCP VTable callbacks ────────────────────

/// VTable for Bun.listen TCP server sockets.
static TCP_LISTEN_VTABLE: VTable = VTable {
    on_open: Some(tcp_on_open),
    on_data: Some(tcp_on_data),
    on_fd: None,
    on_writable: Some(tcp_on_writable),
    on_close: Some(tcp_on_close),
    on_timeout: Some(tcp_on_timeout),
    on_long_timeout: Some(tcp_on_long_timeout),
    on_end: Some(tcp_on_end),
    on_connect_error: Some(tcp_on_connect_error),
    on_connecting_error: Some(tcp_on_connecting_error),
    on_handshake: Some(tcp_on_handshake),
};

/// VTable for Bun.connect client sockets.
static CONNECT_VTABLE: VTable = VTable {
    on_open: Some(connect_on_open),
    on_data: Some(connect_on_data),
    on_fd: None,
    on_writable: Some(tcp_on_writable),
    on_close: Some(connect_on_close),
    on_timeout: Some(tcp_on_timeout),
    on_long_timeout: Some(tcp_on_long_timeout),
    on_end: Some(tcp_on_end),
    on_connect_error: Some(connect_on_connect_error),
    on_connecting_error: Some(tcp_on_connecting_error),
    on_handshake: Some(tcp_on_handshake),
};

/// ──────────────────── shared TCP socket JS object builder ────────────────────
/// Builds the JS socket object for a connected usockets TCP socket. Shared by
/// the Bun.listen accept bridge (tcp_on_open) and the Bun.connect promise
/// resolution (connect_on_open) — previously the accept callbacks carried NO
/// socket identity and the promise resolved with a bare {_socketPtr} that had
/// no write/end methods (both filed from the #21 sweep follow-up).

/// socket.write(data) — string or Buffer/Uint8Array/ArrayBuffer payload.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_sock_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    let this_h = this_obj.handle().into();

    let mut ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_h,
        c"_socketPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ptr_val,
        },
    );
    let socket_key = if ptr_val.is_double() {
        ptr_val.to_double() as usize
    } else {
        0
    };
    if socket_key == 0 || argc == 0 {
        args.rval().set(Int32Value(-1));
        return true;
    }

    // Node/Bun accept string | bytes — same collect_byte_view contract as
    // node_net's __net_write (non-string input must not silently write empty).
    let data: Vec<u8> = if (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr).into_bytes()
    } else {
        match crate::node_buffer::collect_byte_view(cx, *args.get(0).ptr) {
            Some(b) => b,
            None => {
                args.rval().set(Int32Value(-1));
                return true;
            }
        }
    };

    let socket_ptr = socket_key as *mut us_socket_t;
    let written = (*socket_ptr).write(&data);
    args.rval().set(Int32Value(written));
    true
}

/// socket.end() — graceful close (FIN).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_sock_end(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    let this_h = this_obj.handle().into();

    let mut ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_h,
        c"_socketPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ptr_val,
        },
    );
    if ptr_val.is_double() {
        let socket_ptr = ptr_val.to_double() as usize as *mut us_socket_t;
        (*socket_ptr).close(CloseCode::normal);
    }
    args.rval().set(UndefinedValue());
    true
}

/// socket.destroy() — abortive close.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_sock_destroy(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    let this_h = this_obj.handle().into();

    let mut ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_h,
        c"_socketPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ptr_val,
        },
    );
    if ptr_val.is_double() {
        let socket_ptr = ptr_val.to_double() as usize as *mut us_socket_t;
        (*socket_ptr).close(CloseCode::failure);
    }
    args.rval().set(UndefinedValue());
    true
}

/// Build a JS socket object (write/end/destroy + remote/local address info)
/// for a connected `us_socket_t`. Caller must be in the target realm.
///
/// `sock_store_key` — when Some, the object is ALSO stored in GcStore under
/// this key so the vtable callbacks (data/end/close) can resolve the SAME
/// object and pass it to the JS handlers as the socket identity.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_tcp_socket_obj(
    cx: *mut JSContext,
    s: *mut us_socket_t,
    remote_ip: Option<&[u8]>,
    sock_store_key: Option<&str>,
) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let sock_obj = w2::JS_NewPlainObject(cx_ref));
    if sock_obj.get().is_null() {
        return ptr::null_mut();
    }
    let sock_h = sock_obj.handle().into();

    rooted!(&in(cx_ref) let ptr_val = DoubleValue(s as usize as f64));
    JS_DefineProperty(
        cx,
        sock_h,
        c"_socketPtr".as_ptr(),
        ptr_val.handle().into(),
        0,
    );

    // Remote address: prefer the dispatch-provided ip, fall back to the
    // socket's own remote_address.
    let mut ip_buf = [0u8; 64];
    let remote: Option<Vec<u8>> = match remote_ip {
        Some(ip) => Some(ip.to_vec()),
        None => (*s).remote_address(&mut ip_buf).ok().map(|s| s.to_vec()),
    };
    if let Some(ip) = remote {
        let c_ip = ZBox::from_bytes(&ip);
        let ip_js = JS_NewStringCopyZ(cx, c_ip.as_ptr());
        if !ip_js.is_null() {
            rooted!(&in(cx_ref) let ip_v = StringValue(&*ip_js));
            JS_DefineProperty(
                cx,
                sock_h,
                c"remoteAddress".as_ptr(),
                ip_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let rp_v = Int32Value((*s).remote_port()));
    JS_DefineProperty(
        cx,
        sock_h,
        c"remotePort".as_ptr(),
        rp_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let lp_v = Int32Value((*s).local_port()));
    JS_DefineProperty(
        cx,
        sock_h,
        c"localPort".as_ptr(),
        lp_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    JS_DefineFunction(
        cx,
        sock_h,
        c"write".as_ptr(),
        Some(tcp_sock_write),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        sock_h,
        c"end".as_ptr(),
        Some(tcp_sock_end),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        sock_h,
        c"destroy".as_ptr(),
        Some(tcp_sock_destroy),
        0,
        JSPROP_ENUMERATE as u32,
    );

    if let Some(key) = sock_store_key {
        gc_store_insert(cx, key, sock_obj.get());
    }

    sock_obj.get()
}

/// GcStore key under which the accepted socket's JS object is registered for
/// the vtable callbacks (tcp_on_data / tcp_on_close / tcp_on_end).
fn listen_sock_store_key(s: *mut us_socket_t) -> String {
    format!("listen_tcp_sock_{}", s as usize)
}

/// @trace REQ-BAO-API-017 [api:Bun.listen] TCP on_open callback — fires socket.open JS callback
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_on_open(
    s: *mut us_socket_t,
    _is_client: ::std::ffi::c_int,
    ip: *mut u8,
    ip_length: ::std::ffi::c_int,
) -> *mut us_socket_t {
    let key = s as usize;
    LISTEN_TCP_SOCKETS.with(|m| m.borrow_mut().insert(key, true));

    // Retrieve user data from the socket's group owner
    let ud = &*((*s).group().owner::<ListenTcpUserData>() as *const ListenTcpUserData);

    // Accept bridge: build the JS socket identity and pass it to the open
    // handler (Bun API: open(socket)) — previously fired with NO arguments,
    // so JS could never address the accepted connection.
    if !ud.cx.is_null() {
        if let Some(global) = bao_engine::context::thread_realm_global() {
            if !global.is_null() {
                let mut wrapped_cx =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(ud.cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let global_root = global);
                let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;

                let remote_ip = if !ip.is_null() && ip_length > 0 {
                    Some(::std::slice::from_raw_parts(ip, ip_length as usize))
                } else {
                    None
                };
                let store_key = listen_sock_store_key(s);
                let sock_obj = build_tcp_socket_obj(realm_cx.raw_cx(), s, remote_ip, Some(&store_key));
                if !sock_obj.is_null() {
                    rooted!(&in(realm_cx) let sock_val = ObjectValue(sock_obj));
                    let _ = invoke_js_callback(ud.cx, &ud.connect_cb_key, &[*sock_val.handle()]);
                    return s;
                }
            }
        }
    }

    // Fallback: no realm / object build failed — fire without identity.
    let _ = invoke_js_callback(ud.cx, &ud.connect_cb_key, &[]);

    s
}

/// @trace REQ-BAO-API-017 [api:Bun.listen] TCP on_data callback — fires socket.data JS callback
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_on_data(
    s: *mut us_socket_t,
    data: *mut u8,
    length: ::std::ffi::c_int,
) -> *mut us_socket_t {
    if length <= 0 || data.is_null() {
        return s;
    }

    let ud = &*((*s).group().owner::<ListenTcpUserData>() as *const ListenTcpUserData);
    let cx = ud.cx;
    if cx.is_null() {
        return s;
    }

    // Enter the persistent realm BEFORE allocating: JS_NewStringCopyN is a GC
    // allocation and SM resolves the AllocSite from the context's current
    // realm — with no realm entered (async dispatch has none) the allocator
    // crashed (SIGSEGV in AllocNurseryOrTenuredCell, site=0x0). Latent until
    // the BCE-007 liveness registration actually started delivering accepts
    // during JS-idle. Same class as connect_on_data / udp_on_data below.
    let Some(global) = bao_engine::context::thread_realm_global() else {
        return s;
    };
    if global.is_null() {
        return s;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let slice = ::std::slice::from_raw_parts(data, length as usize);
    let js_str = JS_NewStringCopyN(
        realm_cx.raw_cx(),
        slice.as_ptr() as *const _,
        length as usize,
    );
    if js_str.is_null() {
        return s;
    }
    rooted!(&in(realm_cx) let data_root = StringValue(&*js_str));
    let data_val = *data_root.handle();
    // Bun API: data(socket, data) — resolve the accepted socket's JS object
    // from the accept registry and pass it as the first argument.
    let sock_key = listen_sock_store_key(s);
    if let Some(sock_obj) = gc_store_get(cx, &sock_key) {
        rooted!(&in(realm_cx) let sock_val = ObjectValue(sock_obj));
        let _ = invoke_js_callback(cx, &ud.data_cb_key, &[*sock_val.handle(), data_val]);
    } else {
        let _ = invoke_js_callback(cx, &ud.data_cb_key, &[data_val]);
    }

    s
}

unsafe extern "C" fn tcp_on_writable(s: *mut us_socket_t) -> *mut us_socket_t {
    s
}

/// @trace REQ-BAO-API-017 [api:Bun.listen] TCP on_close callback — fires socket.close JS callback
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_on_close(
    s: *mut us_socket_t,
    code: ::std::ffi::c_int,
    _reason: *mut ::std::ffi::c_void,
) -> *mut us_socket_t {
    let key = s as usize;
    LISTEN_TCP_SOCKETS.with(|m| m.borrow_mut().remove(&key));

    let ud = &*((*s).group().owner::<ListenTcpUserData>() as *const ListenTcpUserData);

    // Resolve the accepted socket's JS object, drop it from the accept
    // registry, and deliver close(socket, code).
    let sock_key = listen_sock_store_key(s);
    let sock_obj = if ud.cx.is_null() {
        None
    } else {
        gc_store_get(ud.cx, &sock_key)
    };
    if !ud.cx.is_null() {
        gc_store_remove(ud.cx, &sock_key);
    }

    let code_val = Int32Value(code);
    match sock_obj {
        Some(obj) if !obj.is_null() => {
            if let Some(global) = bao_engine::context::thread_realm_global() {
                if !global.is_null() {
                    let mut wrapped_cx =
                        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(ud.cx));
                    let cx_ref = &mut wrapped_cx;
                    rooted!(&in(cx_ref) let global_root = global);
                    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
                    let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                    rooted!(&in(realm_cx) let sock_val = ObjectValue(obj));
                    let _ = invoke_js_callback(
                        ud.cx,
                        &ud.close_cb_key,
                        &[*sock_val.handle(), code_val],
                    );
                    return s;
                }
            }
            let _ = invoke_js_callback(ud.cx, &ud.close_cb_key, &[code_val]);
        }
        _ => {
            let _ = invoke_js_callback(ud.cx, &ud.close_cb_key, &[code_val]);
        }
    }

    s
}

unsafe extern "C" fn tcp_on_timeout(s: *mut us_socket_t) -> *mut us_socket_t {
    s
}

unsafe extern "C" fn tcp_on_long_timeout(s: *mut us_socket_t) -> *mut us_socket_t {
    s
}

/// @trace REQ-BAO-API-017 [api:Bun.listen] TCP on_end callback — fires socket.end JS callback
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tcp_on_end(s: *mut us_socket_t) -> *mut us_socket_t {
    let ud = &*((*s).group().owner::<ListenTcpUserData>() as *const ListenTcpUserData);

    // Deliver end(socket) when the accepted identity is registered.
    let sock_key = listen_sock_store_key(s);
    if !ud.cx.is_null() {
        if let Some(sock_obj) = gc_store_get(ud.cx, &sock_key) {
            if !sock_obj.is_null() {
                if let Some(global) = bao_engine::context::thread_realm_global() {
                    if !global.is_null() {
                        let mut wrapped_cx =
                            mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(ud.cx));
                        let cx_ref = &mut wrapped_cx;
                        rooted!(&in(cx_ref) let global_root = global);
                        let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
                        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                        rooted!(&in(realm_cx) let sock_val = ObjectValue(sock_obj));
                        let _ =
                            invoke_js_callback(ud.cx, &ud.end_cb_key, &[*sock_val.handle()]);
                        return s;
                    }
                }
            }
        }
    }

    let _ = invoke_js_callback(ud.cx, &ud.end_cb_key, &[]);
    s
}

unsafe extern "C" fn tcp_on_connect_error(
    s: *mut us_socket_t,
    _code: ::std::ffi::c_int,
) -> *mut us_socket_t {
    // BCE (connect-error socket spin — same class as HTTPContext's
    // on_connect_error): uSockets hands close responsibility to this
    // handler for the single-address connect fast path; leaving the
    // never-opened socket registered in epoll is a level-triggered EPOLLERR
    // that re-fires this callback every loop pass (CPU spin). close(Failure)
    // unregisters the fd and stops the redispatch. on_close may or may not
    // dispatch afterwards (semi-socket contract) — no state was registered
    // for a socket that never opened, so there is nothing else to clean.
    (*s).close(CloseCode::failure);
    s
}

unsafe extern "C" fn tcp_on_connecting_error(
    c: *mut bun_uws_sys::ConnectingSocket,
    _code: ::std::ffi::c_int,
) -> *mut bun_uws_sys::ConnectingSocket {
    CONNECT_ERROR.with(|e| e.set(true));
    CONNECT_RESULT.with(|r| r.set(Some(0)));

    // If this is a Bun.connect connecting socket, reject the pending Promise
    let conn_ref = bun_opaque::opaque_deref_mut(c);
    let group_ptr = conn_ref.group();
    if !group_ptr.is_null() {
        let owner = unsafe { (*group_ptr).owner::<ConnectUserData>() as *const ConnectUserData };
        if !owner.is_null() {
            let ud = unsafe { &*owner };
            if !ud.promise_settled.get() && !ud.promise.is_null() {
                ud.promise_settled.set(true);
                unsafe {
                    reject_connect_promise(ud.cx, ud.promise, "Bun.connect: connecting error");
                }
            }
        }
    }

    c
}

unsafe extern "C" fn tcp_on_handshake(
    _s: *mut us_socket_t,
    _success: ::std::ffi::c_int,
    _err: bun_uws_sys::us_bun_verify_error_t,
    _custom_data: *mut ::std::ffi::c_void,
) {
}

// Connect-specific VTable callbacks

/// @trace REQ-BAO-API-017 [api:Bun.connect] on_open callback — fires socket.open JS callback
/// and resolves the pending Promise with the socket object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn connect_on_open(
    s: *mut us_socket_t,
    _is_client: ::std::ffi::c_int,
    _ip: *mut u8,
    _ip_length: ::std::ffi::c_int,
) -> *mut us_socket_t {
    // Refused-connect guard: on EPOLLERR uSockets' after-open path can
    // dispatch on_open for a socket that never connected (remote_port() < 0;
    // observed on refused loopback connects — a bogus OPEN fired and the
    // promise RESOLVED with a dead identity right before the connect_error
    // spin). A genuinely connected TCP socket always carries a positive
    // remote port. Bail BEFORE the CONNECT_RESULT bookkeeping so the wait
    // loop keeps ticking until on_connect_error reports the failure and the
    // promise rejects.
    if (*s).remote_port() <= 0 {
        return s;
    }

    let key = s as usize;
    LISTEN_TCP_SOCKETS.with(|m| m.borrow_mut().insert(key, true));
    CONNECT_RESULT.with(|r| {
        if r.get().is_none() {
            r.set(Some(key));
        }
    });

    // Retrieve user data and call JS `open` callback
    // group() returns &mut SocketGroup (never null for live sockets per uSockets contract)
    let ud = &*((*s).group().owner::<ConnectUserData>() as *const ConnectUserData);

    // Exactly-once open delivery: loopback connects complete synchronously in
    // bun_connect (which fires `open` there), yet uWS still dispatches this
    // callback on the next loop pass for the same socket.
    let open_already_fired = ud.open_fired.replace(true);

    // Client identity bridge (parity with the listen-side accept bridge in
    // tcp_on_open): build the JS socket identity, register it in the
    // pointer-scoped GcStore slot the data/close vtable callbacks resolve,
    // and pass it to the open handler (Bun API: open(socket)) — previously
    // fired with NO arguments, so `open(sock) { sock.write(..) }` threw a
    // TypeError that invoke_js_callback silently cleared: the client never
    // sent and the peer's data events never fired (data-never-delivered).
    let mut has_identity = false;
    if !ud.cx.is_null() {
        if let Some(global) = bao_engine::context::thread_realm_global() {
            if !global.is_null() {
                let mut wrapped_cx =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(ud.cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let global_root = global);
                let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
                let realm_cx: &mut mozjs::context::JSContext = &mut realm;

                let store_key = listen_sock_store_key(s);
                // Reuse the sync arm's registration when present (same
                // pointer-scoped key) so open() and the resolved promise
                // always expose ONE socket object per connection.
                let sock_obj = match gc_store_get(ud.cx, &store_key) {
                    Some(o) if !o.is_null() => o,
                    _ => build_tcp_socket_obj(realm_cx.raw_cx(), s, None, Some(&store_key)),
                };
                if !sock_obj.is_null() {
                    has_identity = true;
                    if !open_already_fired {
                        rooted!(&in(realm_cx) let sock_val = ObjectValue(sock_obj));
                        let _ =
                            invoke_js_callback(ud.cx, &ud.open_cb_key, &[*sock_val.handle()]);
                    }
                }
            }
        }
    }
    if !has_identity && !open_already_fired {
        // Fallback: no realm / object build failed — fire without identity.
        let _ = invoke_js_callback(ud.cx, &ud.open_cb_key, &[]);
    }

    // Resolve the pending Promise with the socket identity.
    // Bun.connect's own wait loop resolves the common paths (ud.promise is
    // still null while that loop ticks through this callback); this arm
    // covers dispatch orderings where the promise is already stored. Re-fetch
    // the registered identity from GcStore instead of reusing a raw pointer
    // across the open callback — invoke_js_callback can allocate (GC moves).
    if !ud.promise_settled.get() && !ud.promise.is_null() {
        ud.promise_settled.set(true);
        let cx = ud.cx;
        if !cx.is_null() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let p = ud.promise);
            let store_key = listen_sock_store_key(s);
            let sock_obj = match gc_store_get(cx, &store_key) {
                Some(o) if !o.is_null() => o,
                _ => build_tcp_socket_obj(cx, s, None, None),
            };
            if !sock_obj.is_null() {
                rooted!(&in(cx_ref) let val = ObjectValue(sock_obj));
                mozjs_sys::jsapi::JS::ResolvePromise(cx, p.handle().into(), val.handle().into());
            }
            mozjs_sys::jsapi::js::RunJobs(cx);
        }
    }

    s
}

/// @trace REQ-BAO-API-017 [api:Bun.connect] on_data callback — fires socket.data JS callback
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn connect_on_data(
    s: *mut us_socket_t,
    data: *mut u8,
    length: ::std::ffi::c_int,
) -> *mut us_socket_t {
    if length <= 0 || data.is_null() {
        return s;
    }

    let ud = &*((*s).group().owner::<ConnectUserData>() as *const ConnectUserData);
    let cx = ud.cx;
    if cx.is_null() {
        return s;
    }

    // Enter the persistent realm before the GC allocation — see tcp_on_data.
    let Some(global) = bao_engine::context::thread_realm_global() else {
        return s;
    };
    if global.is_null() {
        return s;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let slice = ::std::slice::from_raw_parts(data, length as usize);
    let js_str = JS_NewStringCopyN(
        realm_cx.raw_cx(),
        slice.as_ptr() as *const _,
        length as usize,
    );
    if js_str.is_null() {
        return s;
    }
    rooted!(&in(realm_cx) let data_root = StringValue(&*js_str));
    let data_val = *data_root.handle();
    // Bun API: data(socket, data) — resolve the client socket's JS identity
    // from the registry connect_on_open populated (parity with tcp_on_data).
    let sock_key = listen_sock_store_key(s);
    if let Some(sock_obj) = gc_store_get(cx, &sock_key) {
        if !sock_obj.is_null() {
            rooted!(&in(realm_cx) let sock_val = ObjectValue(sock_obj));
            let _ = invoke_js_callback(cx, &ud.data_cb_key, &[*sock_val.handle(), data_val]);
            return s;
        }
    }
    let _ = invoke_js_callback(cx, &ud.data_cb_key, &[data_val]);

    s
}

/// @trace REQ-BAO-API-017 [api:Bun.connect] on_close callback — fires socket.close JS callback
/// and socket.end JS callback.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn connect_on_close(
    s: *mut us_socket_t,
    code: ::std::ffi::c_int,
    _reason: *mut ::std::ffi::c_void,
) -> *mut us_socket_t {
    let key = s as usize;
    LISTEN_TCP_SOCKETS.with(|m| m.borrow_mut().remove(&key));
    // Resolve owner BEFORE dropping the group Box: the sync-connect path
    // stores the Box under the socket ptr, and dropping it HERE (as this
    // callback used to do) freed the group while usockets was still
    // dispatching on it — the subsequent (*s).group() read freed memory
    // (0xdfdfdfdfdfdfdfdf misaligned-deref abort). Dispatch first, free after.
    let ud_ptr = (*s).group().owner::<ConnectUserData>() as *const ConnectUserData;
    let ud = &*ud_ptr;

    // Resolve the client socket's JS identity, drop it from the registry
    // (pointer-scoped key MUST go before the socket memory can be reused,
    // else a later socket at the same address would resolve a stale object),
    // and deliver close(socket, code) + end(socket) — parity with tcp_on_close.
    let sock_key = listen_sock_store_key(s);
    let sock_obj = if ud.cx.is_null() {
        None
    } else {
        gc_store_get(ud.cx, &sock_key)
    };
    if !ud.cx.is_null() {
        gc_store_remove(ud.cx, &sock_key);
    }

    let code_val = Int32Value(code);
    let mut delivered_with_identity = false;
    if let Some(obj) = sock_obj {
        if !obj.is_null() {
            if let Some(global) = bao_engine::context::thread_realm_global() {
                if !global.is_null() {
                    let mut wrapped_cx =
                        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(ud.cx));
                    let cx_ref = &mut wrapped_cx;
                    rooted!(&in(cx_ref) let global_root = global);
                    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
                    let realm_cx: &mut mozjs::context::JSContext = &mut realm;
                    rooted!(&in(realm_cx) let sock_val = ObjectValue(obj));
                    let _ = invoke_js_callback(
                        ud.cx,
                        &ud.close_cb_key,
                        &[*sock_val.handle(), code_val],
                    );
                    let _ = invoke_js_callback(ud.cx, &ud.end_cb_key, &[*sock_val.handle()]);
                    delivered_with_identity = true;
                }
            }
        }
    }
    if !delivered_with_identity {
        let _ = invoke_js_callback(ud.cx, &ud.close_cb_key, &[code_val]);
        // Also fire end callback on close (Bun API: close implies end)
        let _ = invoke_js_callback(ud.cx, &ud.end_cb_key, &[]);
    }

    CONNECT_GROUPS.with(|g| g.borrow_mut().remove(&key));
    // Drop the JS-idle liveness token registered in bun_connect.
    unsafe { crate::node_http::unregister_active_app(s as *mut bun_uws_sys::app::App<false>) };

    s
}

/// @trace REQ-BAO-API-017 [api:Bun.connect] on_connect_error callback — fires socket.error JS callback
/// and rejects the pending Promise.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn connect_on_connect_error(
    s: *mut us_socket_t,
    code: ::std::ffi::c_int,
) -> *mut us_socket_t {
    CONNECT_ERROR.with(|e| e.set(true));
    CONNECT_RESULT.with(|r| r.set(Some(0)));

    let ud = &*((*s).group().owner::<ConnectUserData>() as *const ConnectUserData);
    let code_val = Int32Value(code);
    let _ = invoke_js_callback(ud.cx, &ud.error_cb_key, &[code_val]);

    // Reject the pending Promise with an error
    if !ud.promise_settled.get() && !ud.promise.is_null() {
        ud.promise_settled.set(true);
        reject_connect_promise(
            ud.cx,
            ud.promise,
            &format!("Bun.connect: connection error (code {})", code),
        );
    }

    // BCE (connect-error socket spin — same class as HTTPContext's
    // on_connect_error): uSockets hands close responsibility to this
    // handler for the single-address connect fast path; leaving the
    // never-opened socket registered in epoll is a level-triggered EPOLLERR
    // that re-fires this callback every loop pass (CPU spin). close(Failure)
    // unregisters the fd and stops the redispatch. No GcStore identity was
    // ever registered (the socket never opened), but bun_connect MAY have
    // registered this pointer as a JS-idle liveness token (inline-failed
    // connects surface as a "Socket" result) — on_close never dispatches for
    // these sockets, so drop the token here or the loop never idles again.
    unsafe {
        crate::node_http::unregister_active_app(s as *mut bun_uws_sys::app::App<false>);
    }
    (*s).close(CloseCode::failure);

    s
}

// ──────────────────── UDP callbacks ────────────────────

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] on_data callback — fires socket.data JS callback
#[allow(unsafe_op_in_unsafe_fn)]
extern "C" fn udp_on_data(
    socket: *mut UdpSocket,
    packets: *mut PacketBuffer,
    count: ::std::ffi::c_int,
) {
    if count <= 0 || packets.is_null() || socket.is_null() {
        return;
    }

    // Read user data from socket's user_data (set during create)
    let ud = unsafe { &*((*socket).user() as *const UdpUserData) };
    let cx = ud.cx;
    if cx.is_null() {
        return;
    }

    // Enter the persistent realm before the GC allocations — see tcp_on_data.
    let Some(global) = bao_engine::context::thread_realm_global() else {
        return;
    };
    if global.is_null() {
        return;
    }
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    // Iterate packets and invoke JS callback for each
    let pkt_buf = unsafe { &mut *packets };
    for i in 0..count {
        let payload = pkt_buf.get_payload(i);
        if payload.is_empty() {
            continue;
        }
        let js_str = unsafe {
            JS_NewStringCopyN(
                realm_cx.raw_cx(),
                payload.as_ptr() as *const _,
                payload.len(),
            )
        };
        if js_str.is_null() {
            continue;
        }

        rooted!(&in(realm_cx) let data_root = StringValue(unsafe { &*js_str }));
        let data_val = *data_root.handle();
        let _ = unsafe { invoke_js_callback(cx, &ud.data_cb_key, &[data_val]) };
    }
}

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] on_drain callback — fires socket.drain JS callback
#[allow(unsafe_op_in_unsafe_fn)]
extern "C" fn udp_on_drain(socket: *mut UdpSocket) {
    if socket.is_null() {
        return;
    }
    let ud = unsafe { &*((*socket).user() as *const UdpUserData) };
    let _ = unsafe { invoke_js_callback(ud.cx, &ud.drain_cb_key, &[]) };
}

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] on_close callback — fires socket.close JS callback
#[allow(unsafe_op_in_unsafe_fn)]
extern "C" fn udp_on_close(socket: *mut UdpSocket) {
    if socket.is_null() {
        return;
    }
    let ud = unsafe { &*((*socket).user() as *const UdpUserData) };
    let _ = unsafe { invoke_js_callback(ud.cx, &ud.close_cb_key, &[]) };
}

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] on_recv_error callback — fires socket.error JS callback
#[allow(unsafe_op_in_unsafe_fn)]
extern "C" fn udp_on_recv_error(socket: *mut UdpSocket, code: ::std::ffi::c_int) {
    if socket.is_null() {
        return;
    }
    let ud = unsafe { &*((*socket).user() as *const UdpUserData) };
    let code_val = Int32Value(code);
    let _ = unsafe { invoke_js_callback(ud.cx, &ud.error_cb_key, &[code_val]) };
}

// ──────────────────── Shared helpers ────────────────────

/// Write default HTTP response when no fetch handler is registered.
fn write_default_listen_response(res: &mut Response<false>, req: &bun_uws_sys::request::Request) {
    let method = req.method().to_vec();
    let url = req.url().to_vec();
    let body = format!(
        r#"{{"method":"{}","url":"{}"}}"#,
        ::std::str::from_utf8(&method).unwrap_or("GET"),
        ::std::str::from_utf8(&url).unwrap_or("/"),
    );
    res.write_status(b"200 OK");
    res.write_header(b"Content-Type", b"application/json");
    res.end(body.as_bytes(), true);
}

/// Build a JS Request object from a uWS Request (mirrors bun_serve pattern).
unsafe fn build_request_object(
    cx: &mut mozjs::context::JSContext,
    req: &bun_uws_sys::request::Request,
) -> *mut JSObject {
    rooted!(&in(cx) let req_obj = w2::JS_NewPlainObject(cx));
    if req_obj.get().is_null() {
        return ptr::null_mut();
    }

    let req_h = req_obj.handle().into();

    // method
    let method = req.method().to_vec();
    let c_method = ZBox::from_bytes(&method);
    let js_method = JS_NewStringCopyZ(cx.raw_cx(), c_method.as_ptr());
    if !js_method.is_null() {
        rooted!(&in(cx) let mv = StringValue(&*js_method));
        JS_DefineProperty(
            cx.raw_cx(),
            req_h,
            c"method".as_ptr(),
            mv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // url
    let url = req.url().to_vec();
    let c_url = ZBox::from_bytes(&url);
    let js_url = JS_NewStringCopyZ(cx.raw_cx(), c_url.as_ptr());
    if !js_url.is_null() {
        rooted!(&in(cx) let uv = StringValue(&*js_url));
        JS_DefineProperty(
            cx.raw_cx(),
            req_h,
            c"url".as_ptr(),
            uv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    req_obj.get()
}

/// Maximum iterations for Promise resolution spin loop.
const LISTEN_PROMISE_POLL_MAX_ITERS: u32 = 200;

/// Resolve a JS return value that may be a Promise to a Response object.
/// Mirrors `serve_resolve_response_value` pattern from bun_api.rs.
unsafe fn resolve_response_value(cx: &mut mozjs::context::JSContext, rval: JSVal) -> *mut JSObject {
    if !rval.is_object() {
        return ptr::null_mut();
    }
    let obj = rval.to_object();

    let raw_cx = unsafe { cx.raw_cx() };

    // Fast path: not a promise — check if it's Response-like
    if !is_promise(cx, obj) {
        return if is_response_like(cx, obj) {
            obj
        } else {
            ptr::null_mut()
        };
    }

    // Slow path: Promise<Response>. Drain microtasks until the promise settles.
    rooted!(&in(cx) let obj_root = obj);
    let mut iters = 0u32;
    loop {
        if !JS::IsPromiseObject(obj_root.handle().into()) {
            return ptr::null_mut();
        }
        let state = JS::GetPromiseState(obj_root.handle().into());
        match state {
            PromiseState::Fulfilled => {
                let mut result_val = UndefinedValue();
                mozjs::glue::JS_GetPromiseResult(
                    obj_root.handle().into(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut result_val,
                    },
                );
                if !result_val.is_object() {
                    return ptr::null_mut();
                }
                let result_obj = result_val.to_object();
                return if is_response_like(cx, result_obj) {
                    result_obj
                } else {
                    ptr::null_mut()
                };
            }
            PromiseState::Rejected => {
                JS_ClearPendingException(raw_cx);
                return ptr::null_mut();
            }
            _ => {}
        }

        // Still pending — drain microtasks
        mozjs_sys::jsapi::js::RunJobs(raw_cx);
        crate::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(ptr::null_mut());
        });

        iters += 1;
        if iters >= LISTEN_PROMISE_POLL_MAX_ITERS {
            return ptr::null_mut();
        }
    }
}

/// Check if a JS object is a Promise.
fn is_promise(cx: &mut mozjs::context::JSContext, obj: *mut JSObject) -> bool {
    let raw_cx = unsafe { cx.raw_cx() };
    let mut wrapped_cx =
        unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)) };
    let cx_r = &mut wrapped_cx;
    unsafe {
        rooted!(&in(cx_r) let obj_r = obj);
        JS::IsPromiseObject(obj_r.handle().into())
    }
}

/// Duck-type check: does this object look like a Response (has a numeric `status`)?
fn is_response_like(cx: &mut mozjs::context::JSContext, obj: *mut JSObject) -> bool {
    let raw_cx = unsafe { cx.raw_cx() };
    let mut wrapped_cx =
        unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx)) };
    let cx_r = &mut wrapped_cx;
    rooted!(&in(cx_r) let obj_r = obj);
    let mut status_val = UndefinedValue();
    unsafe {
        JS_GetProperty(
            raw_cx,
            obj_r.handle().into(),
            c"status".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut status_val,
            },
        );
    }
    status_val.is_int32() || status_val.is_double()
}

/// Write a JS Response object's content to uWS Response (mirrors bun_serve pattern).
unsafe fn write_response_object(
    cx: *mut JSContext,
    res: &mut Response<false>,
    resp_obj: *mut JSObject,
) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let resp_root = resp_obj);
    let resp_h = resp_root.handle().into();

    // Try to get status (default 200)
    let mut status_val = UndefinedValue();
    JS_GetProperty(
        cx,
        resp_h,
        c"status".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut status_val,
        },
    );
    let status = if status_val.is_int32() {
        status_val.to_int32()
    } else {
        200
    };

    // Try to get body
    let mut body_val = UndefinedValue();
    JS_GetProperty(
        cx,
        resp_h,
        c"body".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut body_val,
        },
    );
    let body = if body_val.is_string() {
        crate::js_to_rust_string(cx, body_val)
    } else if body_val.is_object() {
        // Could be ArrayBuffer — simplified: try toString
        rooted!(&in(cx_ref) let body_obj = body_val.to_object());
        let mut str_val = UndefinedValue();
        JS_CallFunctionName(
            cx,
            body_obj.handle().into(),
            c"toString".as_ptr(),
            &HandleValueArray::empty(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut str_val,
            },
        );
        if str_val.is_string() {
            crate::js_to_rust_string(cx, str_val)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let status_str = format!("{} OK", status);
    res.write_status(status_str.as_bytes());
    res.write_header(b"Content-Type", b"text/plain");
    if !body.is_empty() {
        res.end(body.as_bytes(), true);
    } else {
        res.end(b"", true);
    }
}

/// Build a sockaddr_storage from host:port. Returns 0 on failure.
fn build_sockaddr(host: &str, port: u16, storage: &mut libc::sockaddr_storage) -> usize {
    let addr: ::std::net::SocketAddr = match host.parse() {
        Ok(a) => a,
        Err(_) => match format!("{}:{}", host, port).parse() {
            Ok(a) => a,
            Err(_) => return 0,
        },
    };
    match addr {
        ::std::net::SocketAddr::V4(v4) => {
            let sa = libc::sockaddr_in {
                sin_family: libc::AF_INET as u16,
                sin_port: port.to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from(*v4.ip()).to_be(),
                },
                sin_zero: [0; 8],
            };
            let sa_bytes = unsafe {
                ::std::slice::from_raw_parts(
                    &sa as *const _ as *const u8,
                    ::std::mem::size_of::<libc::sockaddr_in>(),
                )
            };
            let storage_bytes = unsafe {
                ::std::slice::from_raw_parts_mut(
                    storage as *mut _ as *mut u8,
                    ::std::mem::size_of::<libc::sockaddr_storage>(),
                )
            };
            storage_bytes[..sa_bytes.len()].copy_from_slice(sa_bytes);
            sa_bytes.len()
        }
        ::std::net::SocketAddr::V6(v6) => {
            let sa = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as u16,
                sin6_port: port.to_be(),
                sin6_flowinfo: 0,
                sin6_addr: libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                },
                sin6_scope_id: 0,
            };
            let sa_bytes = unsafe {
                ::std::slice::from_raw_parts(
                    &sa as *const _ as *const u8,
                    ::std::mem::size_of::<libc::sockaddr_in6>(),
                )
            };
            let storage_bytes = unsafe {
                ::std::slice::from_raw_parts_mut(
                    storage as *mut _ as *mut u8,
                    ::std::mem::size_of::<libc::sockaddr_storage>(),
                )
            };
            storage_bytes[..sa_bytes.len()].copy_from_slice(sa_bytes);
            sa_bytes.len()
        }
    }
}

/// Clean up a GcStore key stored as a JS property on an object.
unsafe fn cleanup_gc_key(cx: *mut JSContext, obj_h: Handle<*mut JSObject>, prop: *const i8) {
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        prop,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_string() {
        let key = crate::js_to_rust_string(cx, val);
        gc_store_remove(cx, &key);
    }
}

/// Store a GcStore key as a JS string property on an object (for cleanup in destroy).
fn store_gc_key_on_obj(
    cx: *mut JSContext,
    _cx_ref: &mut mozjs::context::JSContext,
    obj_h: Handle<*mut JSObject>,
    prop: *const i8,
    key: &Option<String>,
) {
    if let Some(ref k) = *key {
        let c_k = ZBox::from_bytes(k.as_bytes());
        unsafe {
            let js_str = JS_NewStringCopyZ(cx, c_k.as_ptr());
            if !js_str.is_null() {
                let mut wrapped_cx =
                    mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let v = StringValue(&*js_str));
                JS_DefineProperty(cx, obj_h, prop, v.handle().into(), 0);
            }
        }
    }
}

/// @trace REQ-BAO-API-017 [api:Bun.connect] Reject a connect Promise with an error message.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_connect_promise(cx: *mut JSContext, promise: *mut JSObject, msg: &str) {
    if cx.is_null() || promise.is_null() {
        return;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    // Settle guard: both connect_on_connect_error and bun_connect's
    // socket_key==0 arm reach here — rejecting an already-settled promise
    // fails and leaves a pending exception mid-eval.
    rooted!(&in(cx_ref) let p_early = promise);
    if JS::GetPromiseState(p_early.handle().into()) != PromiseState::Pending {
        return;
    }
    // Rejection value must be a REAL Error instance (`Error(msg)` called as
    // a function performs the constructor steps) — a bare {message} object
    // breaks `catch (e) { e instanceof Error }` / stack-shape consumers.
    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() {
        return;
    }
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global.handle().into(),
        c"Error".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_val,
        },
    );
    if !ctor_val.is_object() {
        return;
    }
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
    if js_str.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let msg_val = StringValue(&*js_str));
    rooted!(&in(cx_ref) let ctor_root = ctor_val);
    let call_args = HandleValueArray {
        length_: 1,
        elements_: &*msg_val.handle(),
    };
    let mut err_val = UndefinedValue();
    let err_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut err_val,
    };
    let called = JS_CallFunctionValue(
        cx,
        global.handle().into(),
        ctor_root.handle().into(),
        &call_args,
        err_h,
    );
    if !called || !err_val.is_object() {
        if !called {
            JS_ClearPendingException(cx);
        }
        return;
    }
    rooted!(&in(cx_ref) let err_root = err_val);
    rooted!(&in(cx_ref) let p = promise);
    mozjs_sys::jsapi::JS::RejectPromise(cx, p.handle().into(), err_root.handle().into());
    mozjs_sys::jsapi::js::RunJobs(cx);
}

/// Get ConnectUserData from a JS socket object's _udPtr private property.
/// Returns None if the property is missing or invalid.
unsafe fn get_connect_ud<'a>(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
) -> Option<&'a ConnectUserData> {
    let mut ud_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        c"_udPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ud_val,
        },
    );
    if ud_val.is_double() && (ud_val.asBits_ & 0xFFFF000000000000) == 0 {
        let ud_ptr = ud_val.to_private() as *mut ConnectUserData;
        if !ud_ptr.is_null() {
            return Some(&*ud_ptr);
        }
    }
    None
}

// ──────────────────── Install entry point ────────────────────

/// Install Bun.listen / Bun.connect / Bun.udpSocket native functions on the Bun object.
/// @trace REQ-BAO-API-017 [api:Bun.listen/connect/udpSocket]
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    let raw_cx = cx.raw_cx();
    let bun_h = bun_obj.into();

    mozjs_sys::jsapi::JS_DefineFunction(
        raw_cx,
        bun_h,
        c"listen".as_ptr(),
        Some(bun_listen),
        1,
        JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        raw_cx,
        bun_h,
        c"connect".as_ptr(),
        Some(bun_connect),
        1,
        JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        raw_cx,
        bun_h,
        c"udpSocket".as_ptr(),
        Some(bun_udp_socket),
        2,
        JSPROP_ENUMERATE as u32,
    );
}
