// @trace REQ-BAO-API-017 [api:Bun.udpSocket] — UDP socket bridge
//! Bun.udpSocket — bridge to bun_uws_sys::udp::Socket.
//!
//! Reuses bun_uws_sys::udp::Socket (uSockets C++ UDP) — no hand-written UDP code.
//! Callbacks: data / drain / close / error.
//! JS object methods: send / sendMany / close / address / ref / unref / connect /
//!   disconnect / setBroadcast / setTTL / setMulticastTTL / setMulticastLoopback /
//!   setMulticastInterface / addMembership / dropMembership / reload.
//! Returns Promise<UDPSocket> (per Bun upstream API).

use ::std::cell::Cell;
use ::std::ptr::{self, NonNull};
use ::std::sync::atomic::{AtomicU64, Ordering};

use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, PrivateValue, StringValue,
    UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::Loop;
use bun_uws_sys::udp::PacketBuffer;
use bun_uws_sys::udp::Socket as UdpSocket;

use crate::gc_store::{gc_store_get, gc_store_insert, gc_store_remove, gc_store_unique_key};

// ──────────────────── ID counter ────────────────────

static UDP_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

// ──────────────────── Event loop helper ────────────────────

/// Get the uSockets event loop, ensuring bao_uloop is initialized.
fn get_loop() -> *mut Loop {
    bao_uloop::force_link();
    bao_uloop::uws_get_loop()
}

// ──────────────────── JS helpers ────────────────────

/// Extract a JS function callback from a property on a JS object.
/// Returns `None` if the property is missing or not a function.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
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

/// Invoke a JS callback stored in GcStore by key.
/// Passes `args` as arguments to the JS function. Returns `true` on success.
/// Silently ignores missing/invalid callbacks (GC'd, null cx, etc.).
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
unsafe fn invoke_js_callback(cx: *mut JSContext, cb_key: &Option<String>, args: &[JSVal]) -> bool {
    let key = match cb_key {
        Some(k) => k,
        None => return false,
    };
    if cx.is_null() {
        return false;
    }

    let Some(cb_obj) = gc_store_get(cx, key) else {
        return false;
    };
    if cb_obj.is_null() {
        return false;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let handler_val = ObjectValue(cb_obj));
    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() {
        return false;
    }

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
        cx,
        global.handle().into(),
        handler_val.handle().into(),
        &call_args,
        rval_h,
    );
    if !ok {
        // The UDP callback threw. Capture the pending exception, clear it, and
        // route it (process.on('uncaughtException') or stderr + exit 1) —
        // Node semantics; NOT silently swallowed (same routing as timer and
        // EventEmitter listener throws).
        let mut exn = UndefinedValue();
        JS_GetPendingException(
            cx,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut exn,
            },
        );
        JS_ClearPendingException(cx);
        rooted!(&in(cx_ref) let reason_root = exn);
        if !exn.is_undefined() {
            crate::uncaught::route_uncaught_exception(cx, exn);
        }
    }
    ok
}

/// Build a libc sockaddr_storage from host:port.
/// Returns the address length, or 0 on parse failure.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
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
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
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

/// Reject a Promise with an error message object.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
unsafe fn reject_udp_promise(cx: *mut JSContext, promise: *mut JSObject, msg: &str) {
    if cx.is_null() || promise.is_null() {
        return;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
    if !err_obj.get().is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let msg_val = StringValue(&*js_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"message".as_ptr(),
                msg_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        rooted!(&in(cx_ref) let err_val = ObjectValue(err_obj.get()));
        rooted!(&in(cx_ref) let p = promise);
        mozjs_sys::jsapi::JS::RejectPromise(cx, p.handle().into(), err_val.handle().into());
    }
    mozjs_sys::jsapi::js::RunJobs(cx);
}

/// Extract a sockaddr_storage from a peer pointer (from PacketBuffer::get_peer).
/// Returns (hostname_string, port) or None on failure.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
unsafe fn parse_peer_addr(peer: &libc::sockaddr_storage) -> Option<(String, u16)> {
    let family = peer.ss_family as i32;
    if family == libc::AF_INET {
        let peer4 = peer as *const _ as *const libc::sockaddr_in;
        let port = u16::from_be((*peer4).sin_port);
        let ip = ::std::net::Ipv4Addr::from(u32::from_be((*peer4).sin_addr.s_addr));
        Some((format!("{}", ip), port))
    } else if family == libc::AF_INET6 {
        let peer6 = peer as *const _ as *const libc::sockaddr_in6;
        let port = u16::from_be((*peer6).sin6_port);
        let ip = ::std::net::Ipv6Addr::from((*peer6).sin6_addr.s6_addr);
        Some((format!("{}", ip), port))
    } else {
        None
    }
}

// ──────────────────── UdpUserData ────────────────────

/// User data for Bun.udpSocket.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
#[allow(dead_code)]
struct UdpUserData {
    data_cb_key: Option<String>,
    drain_cb_key: Option<String>,
    close_cb_key: Option<String>,
    error_cb_key: Option<String>,
    cx: *mut JSContext,
    /// Promise to resolve with the UDPSocket object on successful creation.
    promise: *mut JSObject,
    /// Whether the promise has been settled (resolved or rejected).
    promise_settled: Cell<bool>,
    /// Whether the socket has been closed.
    closed: Cell<bool>,
    /// Connected mode: stores the connected port (0 = not connected).
    connect_port: Cell<u16>,
}

// ──────────────────── Bun.udpSocket host_fn ────────────────────

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] Bun.udpSocket(options) -> Promise<UDPSocket>
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
    let mut flags: i32 = 0;
    let mut connect_hostname: Option<String> = None;
    let mut connect_port: u16 = 0;

    if argc > 0 {
        let opts_val = *args.get(0).ptr;
        // Support both Bun.udpSocket(port, hostname) and Bun.udpSocket(options)
        if opts_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let opts_obj = opts_val.to_object());
            let opts_h = opts_obj.handle().into();

            // port
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

            // hostname
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

            // flags (IPV6_V6ONLY etc.)
            let mut fv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"flags".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut fv,
                },
            );
            if fv.is_int32() {
                flags = fv.to_int32();
            }

            // connect: { hostname, port } — connected UDP mode
            let mut cv = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_h,
                c"connect".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut cv,
                },
            );
            if cv.is_object() {
                rooted!(&in(cx_ref) let conn_obj = cv.to_object());
                let conn_h = conn_obj.handle().into();

                let mut chv = UndefinedValue();
                JS_GetProperty(
                    cx,
                    conn_h,
                    c"hostname".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut chv,
                    },
                );
                if chv.is_string() {
                    connect_hostname = Some(crate::js_to_rust_string(cx, chv));
                }

                let mut cpv = UndefinedValue();
                JS_GetProperty(
                    cx,
                    conn_h,
                    c"port".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut cpv,
                    },
                );
                if cpv.is_int32() {
                    connect_port = cpv.to_int32().max(0) as u16;
                } else if cpv.is_double() {
                    connect_port = cpv.to_double().max(0.0) as u16;
                }
            }

            // socket: { data, drain, close, error, binaryType }
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
                rooted!(&in(cx_ref) let sock_obj = sv.to_object());
                let sock_h = sock_obj.handle().into();
                on_data = extract_js_callback(cx, sock_h, "data");
                on_drain = extract_js_callback(cx, sock_h, "drain");
                on_close = extract_js_callback(cx, sock_h, "close");
                on_error = extract_js_callback(cx, sock_h, "error");
            } else {
                // Backward compat: callbacks on top-level options object
                on_data = extract_js_callback(cx, opts_h, "data");
                on_drain = extract_js_callback(cx, opts_h, "drain");
                on_close = extract_js_callback(cx, opts_h, "close");
                on_error = extract_js_callback(cx, opts_h, "error");
            }
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

    // Create Promise — SPEC requires Promise<UDPSocket>
    crate::timers::with_event_loop(|_| {});

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let null_global = ::std::ptr::null_mut::<JSObject>());
    let promise =
        unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_global.handle().into()) };
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let loop_ = get_loop();
    if loop_.is_null() {
        // Reject the promise — no event loop
        unsafe {
            reject_udp_promise(cx, promise, "Bun.udpSocket: event loop not available");
        }
        rooted!(&in(cx_ref) let p = promise);
        args.rval().set(ObjectValue(p.get()));
        return true;
    }

    // Build user data (stored as udp socket user_data)
    let ud = Box::new(UdpUserData {
        data_cb_key,
        drain_cb_key,
        close_cb_key,
        error_cb_key,
        cx,
        promise,
        promise_settled: Cell::new(false),
        closed: Cell::new(false),
        connect_port: Cell::new(connect_port),
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
        flags,
        Some(&mut err),
        ud_ptr,
    );

    if udp_socket.is_null() || err != 0 {
        let _ = unsafe { Box::from_raw(ud_ptr as *mut UdpUserData) };
        // Reject the promise with error info
        let err_msg = if err != 0 {
            format!("Bun.udpSocket: bind failed (errno {})", err)
        } else {
            "Bun.udpSocket: failed to create socket".to_string()
        };
        unsafe {
            reject_udp_promise(cx, promise, &err_msg);
        }
        rooted!(&in(cx_ref) let p = promise);
        args.rval().set(ObjectValue(p.get()));
        return true;
    }

    // Handle connect option — connected UDP mode
    if let Some(ref conn_host) = connect_hostname {
        if !conn_host.is_empty() && connect_port > 0 {
            let conn_cstr = ZBox::from_bytes(conn_host.as_bytes());
            let ret = unsafe {
                (*udp_socket).connect((*conn_cstr).as_cstr().as_ptr(), connect_port as u32)
            };
            if ret != 0 {
                unsafe {
                    (*udp_socket).close();
                }
                let _ = Box::from_raw(ud_ptr as *mut UdpUserData);
                unsafe {
                    reject_udp_promise(cx, promise, "Bun.udpSocket: connect failed");
                }
                rooted!(&in(cx_ref) let p = promise);
                args.rval().set(ObjectValue(p.get()));
                return true;
            }
        }
    }

    // Read actual bound port
    let bound_port = unsafe { (*udp_socket).bound_port() };

    // Build JS UDPSocket object
    rooted!(&in(cx_ref) let udp_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if udp_obj.get().is_null() {
        unsafe {
            (*udp_socket).close();
        }
        let _ = Box::from_raw(ud_ptr as *mut UdpUserData);
        unsafe {
            reject_udp_promise(cx, promise, "Bun.udpSocket: failed to create JS object");
        }
        rooted!(&in(cx_ref) let p = promise);
        args.rval().set(ObjectValue(p.get()));
        return true;
    }
    let udp_h = udp_obj.handle().into();

    // Store socket pointer as private value
    let sock_val = PrivateValue(udp_socket as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let sv = sock_val);
    unsafe {
        JS_DefineProperty(cx, udp_h, c"_socketPtr".as_ptr(), sv.handle().into(), 0);
    }

    // Store ud_ptr for cleanup
    let ud_jsval = PrivateValue(ud_ptr as *const core::ffi::c_void);
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

    // ──── Define JS methods on the UDPSocket object ────

    // udp.send(data, port, address) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_send(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        // Check closed
        let ud = get_udp_ud(cx, this_h);
        if let Some(ud_ref) = ud {
            if ud_ref.closed.get() {
                // Throw error for closed socket
                let mut ex = UndefinedValue();
                JS_ReportErrorASCII(cx, c"Socket is closed".as_ptr());
                JS_GetPendingException(
                    cx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut ex,
                    },
                );
                if !ex.is_undefined() {
                    JS_ClearPendingException(cx);
                }
                args.rval().set(BooleanValue(false));
                return true;
            }
        }

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(BooleanValue(false));
            return true;
        }

        // Connected mode: send(data) — 1 arg
        let is_connected = ud.map_or(false, |u| u.connect_port.get() > 0);

        // Parse arguments: send(data) or send(data, port, address)
        let data = if argc > 0 && (*args.get(0).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(0).ptr)
        } else {
            String::new()
        };

        let (target_port, target_addr) = if is_connected {
            // Connected socket: no destination needed
            (0u16, String::new())
        } else {
            let tp: u16 = if argc > 1 {
                (*args.get(1).ptr).to_int32().max(0) as u16
            } else {
                0
            };
            let ta = if argc > 2 && (*args.get(2).ptr).is_string() {
                crate::js_to_rust_string(cx, *args.get(2).ptr)
            } else {
                "127.0.0.1".to_string()
            };
            (tp, ta)
        };

        // Build sockaddr for target address (only for unconnected)
        let mut addr_storage: libc::sockaddr_storage = unsafe { ::std::mem::zeroed() };
        let addr_ptr: *const ::std::ffi::c_void = if is_connected {
            ::std::ptr::null()
        } else {
            let addr_len = build_sockaddr(&target_addr, target_port, &mut addr_storage);
            if addr_len == 0 {
                args.rval().set(BooleanValue(false));
                return true;
            }
            &addr_storage as *const _ as *const ::std::ffi::c_void
        };

        let payloads: [*const u8; 1] = [data.as_ptr()];
        let lengths: [usize; 1] = [data.len()];
        let addresses: [*const ::std::ffi::c_void; 1] = [addr_ptr];

        let sent = unsafe { (*socket_ptr).send(&payloads, &lengths, &addresses) };
        // Bun upstream returns boolean: true if sent > 0
        args.rval().set(BooleanValue(sent > 0));
        true
    }

    // udp.sendMany(packets) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_send_many(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let ud = get_udp_ud(cx, this_h);
        if let Some(ud_ref) = ud {
            if ud_ref.closed.get() {
                args.rval().set(Int32Value(-1));
                return true;
            }
        }

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(Int32Value(-1));
            return true;
        }

        let is_connected = ud.map_or(false, |u| u.connect_port.get() > 0);

        // Expect first arg to be an array: [data, port, addr, data, port, addr, ...]
        // For connected sockets: [data, data, ...]
        if argc == 0 || !(*args.get(0).ptr).is_object() {
            args.rval().set(Int32Value(-1));
            return true;
        }

        rooted!(&in(cx_ref) let arr_obj = (*args.get(0).ptr).to_object());
        let arr_h = arr_obj.handle().into();

        let mut arr_len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            arr_h,
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut arr_len_val,
            },
        );
        let arr_len = if arr_len_val.is_int32() {
            arr_len_val.to_int32().max(0) as usize
        } else {
            0
        };

        if arr_len == 0 {
            args.rval().set(Int32Value(0));
            return true;
        }

        // Collect payloads, lengths, addresses
        let mut payloads: Vec<*const u8> = Vec::with_capacity(arr_len);
        let mut lengths: Vec<usize> = Vec::with_capacity(arr_len);
        let mut addrs: Vec<libc::sockaddr_storage> = Vec::with_capacity(arr_len);
        let mut addr_ptrs: Vec<*const ::std::ffi::c_void> = Vec::with_capacity(arr_len);

        let stride = if is_connected { 1 } else { 3 };
        let num_packets = arr_len / stride;

        for i in 0..num_packets {
            let data_idx = i * stride;

            // Get data element
            let mut elem = UndefinedValue();
            let idx_cstr = ZBox::from_bytes(format!("{}", data_idx).as_bytes());
            JS_GetProperty(
                cx,
                arr_h,
                idx_cstr.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );

            let data_str = if elem.is_string() {
                crate::js_to_rust_string(cx, elem)
            } else {
                String::new()
            };
            payloads.push(data_str.as_ptr());
            lengths.push(data_str.len());

            if is_connected {
                addr_ptrs.push(::std::ptr::null());
            } else {
                // Get port
                let mut port_val = UndefinedValue();
                let port_cstr = ZBox::from_bytes(format!("{}", data_idx + 1).as_bytes());
                JS_GetProperty(
                    cx,
                    arr_h,
                    port_cstr.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut port_val,
                    },
                );
                let pkt_port: u16 = if port_val.is_int32() {
                    port_val.to_int32().max(0) as u16
                } else {
                    0
                };

                // Get address
                let mut addr_val = UndefinedValue();
                let addr_cstr = ZBox::from_bytes(format!("{}", data_idx + 2).as_bytes());
                JS_GetProperty(
                    cx,
                    arr_h,
                    addr_cstr.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut addr_val,
                    },
                );
                let pkt_addr = if addr_val.is_string() {
                    crate::js_to_rust_string(cx, addr_val)
                } else {
                    "127.0.0.1".to_string()
                };

                let mut storage: libc::sockaddr_storage = ::std::mem::zeroed();
                let alen = build_sockaddr(&pkt_addr, pkt_port, &mut storage);
                if alen == 0 {
                    addr_ptrs.push(::std::ptr::null());
                } else {
                    addrs.push(storage);
                    addr_ptrs.push(addrs.last().unwrap() as *const _ as *const ::std::ffi::c_void);
                }
            }

            // Leak data_str to keep payload pointer valid through send()
            ::std::mem::forget(data_str);
        }

        let sent = unsafe { (*socket_ptr).send(&payloads, &lengths, &addr_ptrs) };
        args.rval().set(Int32Value(sent));
        true
    }

    // udp.close() — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_close_fn(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if !socket_ptr.is_null() {
            // Mark closed before closing
            let ud = get_udp_ud(cx, this_h);
            if let Some(ud_ref) = ud {
                ud_ref.closed.set(true);
            }
            unsafe {
                (*socket_ptr).close();
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

    // udp.address() — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_address(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
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

            // Also expose family
            let family_str = if ip_str.contains(':') { "IPv6" } else { "IPv4" };
            let c_fam = ZBox::from_bytes(family_str.as_bytes());
            let js_fam = JS_NewStringCopyZ(cx, c_fam.as_ptr());
            if !js_fam.is_null() {
                rooted!(&in(cx_ref) let fam_v = StringValue(&*js_fam));
                JS_DefineProperty(
                    cx,
                    ret_root.handle().into(),
                    c"family".as_ptr(),
                    fam_v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }

            args.rval().set(ObjectValue(ret_root.get()));
        } else {
            args.rval().set(UndefinedValue());
        }
        true
    }

    // udp.ref() / udp.unref() — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    // These interact with the event loop's keep-alive mechanism.
    // For now, they are functional no-ops that return `this` for chaining
    // (matching Node.js/Bun behavior where ref/unref on a non-poll-ref'd
    // socket is a no-op). Full poll_ref integration requires bao_uloop
    // exposing a per-socket refcount — tracked as future enhancement.
    unsafe extern "C" fn udp_ref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        // Return this for chaining — args.thisv() is Handle<Value>, get() extracts Value
        let this = args.thisv();
        args.rval().set(*this.ptr);
        true
    }
    unsafe extern "C" fn udp_unref(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let this = args.thisv();
        args.rval().set(*this.ptr);
        true
    }

    // udp.connect(hostname, port) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let ud = get_udp_ud(cx, this_h);
        if let Some(ud_ref) = ud {
            if ud_ref.closed.get() {
                args.rval().set(Int32Value(-1));
                return true;
            }
        }

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(Int32Value(-1));
            return true;
        }

        let host = if argc > 0 && (*args.get(0).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(0).ptr)
        } else {
            "127.0.0.1".to_string()
        };
        let port: u32 = if argc > 1 {
            let v = *args.get(1).ptr;
            if v.is_int32() {
                v.to_int32().max(0) as u32
            } else if v.is_double() {
                v.to_double().max(0.0) as u32
            } else {
                0
            }
        } else {
            0
        };

        let host_cstr = ZBox::from_bytes(host.as_bytes());
        let ret = unsafe { (*socket_ptr).connect((*host_cstr).as_cstr().as_ptr(), port) };

        if ret == 0 {
            if let Some(ud_ref) = ud {
                ud_ref.connect_port.set(port as u16);
            }
        }

        args.rval().set(Int32Value(ret));
        true
    }

    // udp.disconnect() — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_disconnect(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(Int32Value(-1));
            return true;
        }

        let ret = unsafe { (*socket_ptr).disconnect() };

        let ud = get_udp_ud(cx, this_h);
        if let Some(ud_ref) = ud {
            ud_ref.connect_port.set(0);
        }

        args.rval().set(Int32Value(ret));
        true
    }

    // udp.setBroadcast(enable) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_set_broadcast(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(BooleanValue(false));
            return true;
        }

        let enabled = if argc > 0 {
            (*args.get(0).ptr).to_boolean()
        } else {
            false
        };
        let _res = unsafe { (*socket_ptr).set_broadcast(enabled) };
        args.rval().set(BooleanValue(enabled));
        true
    }

    // udp.setTTL(ttl) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_set_ttl(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(Int32Value(-1));
            return true;
        }

        let ttl: i32 = if argc > 0 {
            let v = *args.get(0).ptr;
            if v.is_int32() { v.to_int32() } else { 64 }
        } else {
            64
        };

        let _res = unsafe { (*socket_ptr).set_unicast_ttl(ttl) };
        args.rval().set(Int32Value(ttl));
        true
    }

    // udp.setMulticastTTL(ttl) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_set_multicast_ttl(
        cx: *mut JSContext,
        argc: u32,
        vp: *mut JSVal,
    ) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(Int32Value(-1));
            return true;
        }

        let ttl: i32 = if argc > 0 {
            let v = *args.get(0).ptr;
            if v.is_int32() { v.to_int32() } else { 1 }
        } else {
            1
        };

        let _res = unsafe { (*socket_ptr).set_multicast_ttl(ttl) };
        args.rval().set(Int32Value(ttl));
        true
    }

    // udp.setMulticastLoopback(enable) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_set_multicast_loopback(
        cx: *mut JSContext,
        argc: u32,
        vp: *mut JSVal,
    ) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(BooleanValue(false));
            return true;
        }

        let enabled = if argc > 0 {
            (*args.get(0).ptr).to_boolean()
        } else {
            false
        };
        let _res = unsafe { (*socket_ptr).set_multicast_loopback(enabled) };
        args.rval().set(BooleanValue(enabled));
        true
    }

    // udp.setMulticastInterface(address) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_set_multicast_interface(
        cx: *mut JSContext,
        argc: u32,
        vp: *mut JSVal,
    ) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(BooleanValue(false));
            return true;
        }

        let mut iface_storage: libc::sockaddr_storage = ::std::mem::zeroed();
        if argc > 0 && (*args.get(0).ptr).is_string() {
            let addr_str = crate::js_to_rust_string(cx, *args.get(0).ptr);
            let alen = build_sockaddr(&addr_str, 0, &mut iface_storage);
            if alen == 0 {
                args.rval().set(BooleanValue(false));
                return true;
            }
        }

        let _res = unsafe { (*socket_ptr).set_multicast_interface(&iface_storage) };
        args.rval().set(BooleanValue(true));
        true
    }

    // udp.addMembership(address, interface?) / dropMembership — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_add_membership(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        udp_set_membership_impl(cx, argc, vp, false)
    }
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_drop_membership(
        cx: *mut JSContext,
        argc: u32,
        vp: *mut JSVal,
    ) -> bool {
        udp_set_membership_impl(cx, argc, vp, true)
    }

    /// Shared implementation for addMembership / dropMembership.
    /// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn udp_set_membership_impl(
        cx: *mut JSContext,
        argc: u32,
        vp: *mut JSVal,
        drop: bool,
    ) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let socket_ptr = get_socket_ptr(cx, this_h);
        if socket_ptr.is_null() {
            args.rval().set(BooleanValue(false));
            return true;
        }

        // Parse address
        let mut addr_storage: libc::sockaddr_storage = ::std::mem::zeroed();
        if argc > 0 && (*args.get(0).ptr).is_string() {
            let addr_str = crate::js_to_rust_string(cx, *args.get(0).ptr);
            let alen = build_sockaddr(&addr_str, 0, &mut addr_storage);
            if alen == 0 {
                args.rval().set(BooleanValue(false));
                return true;
            }
        } else {
            args.rval().set(BooleanValue(false));
            return true;
        }

        // Optional interface
        let mut iface_storage: libc::sockaddr_storage = ::std::mem::zeroed();
        let iface_opt: Option<&libc::sockaddr_storage> =
            if argc > 1 && (*args.get(1).ptr).is_string() {
                let iface_str = crate::js_to_rust_string(cx, *args.get(1).ptr);
                let ilen = build_sockaddr(&iface_str, 0, &mut iface_storage);
                if ilen > 0 { Some(&iface_storage) } else { None }
            } else {
                None
            };

        let _res = unsafe { (*socket_ptr).set_membership(&addr_storage, iface_opt, drop) };
        args.rval().set(BooleanValue(true));
        true
    }

    // udp.reload(options) — @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    // Reconfigures the socket's callback handlers at runtime.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn udp_reload(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        // Parse new options to extract callbacks
        if argc > 0 && (*args.get(0).ptr).is_object() {
            rooted!(&in(cx_ref) let opts_obj = (*args.get(0).ptr).to_object());
            let opts_h = opts_obj.handle().into();

            // Look for socket: { data, drain, close, error }
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

            let sock_h = if sv.is_object() {
                rooted!(&in(cx_ref) let sock_obj = sv.to_object());
                sock_obj.handle().into()
            } else {
                opts_h
            };

            // Update callbacks in user data
            let ud = get_udp_ud_mut(cx, this_h);
            if let Some(ud_ref) = ud {
                if let Some(cb) = extract_js_callback(cx, sock_h, "data") {
                    if let Some(ref old_key) = ud_ref.data_cb_key {
                        gc_store_remove(cx, old_key);
                    }
                    let k = gc_store_unique_key(&format!("udp_data_reload"));
                    gc_store_insert(cx, &k, cb);
                    ud_ref.data_cb_key = Some(k);
                }
                if let Some(cb) = extract_js_callback(cx, sock_h, "drain") {
                    if let Some(ref old_key) = ud_ref.drain_cb_key {
                        gc_store_remove(cx, old_key);
                    }
                    let k = gc_store_unique_key(&format!("udp_drain_reload"));
                    gc_store_insert(cx, &k, cb);
                    ud_ref.drain_cb_key = Some(k);
                }
                if let Some(cb) = extract_js_callback(cx, sock_h, "close") {
                    if let Some(ref old_key) = ud_ref.close_cb_key {
                        gc_store_remove(cx, old_key);
                    }
                    let k = gc_store_unique_key(&format!("udp_close_reload"));
                    gc_store_insert(cx, &k, cb);
                    ud_ref.close_cb_key = Some(k);
                }
                if let Some(cb) = extract_js_callback(cx, sock_h, "error") {
                    if let Some(ref old_key) = ud_ref.error_cb_key {
                        gc_store_remove(cx, old_key);
                    }
                    let k = gc_store_unique_key(&format!("udp_error_reload"));
                    gc_store_insert(cx, &k, cb);
                    ud_ref.error_cb_key = Some(k);
                }
            }
        }

        args.rval().set(UndefinedValue());
        true
    }

    // ──── closed getter ────
    // @trace REQ-BAO-API-017 [api:Bun.udpSocket]
    unsafe extern "C" fn udp_get_closed(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
        let args = CallArgs::from_vp(vp, _argc);
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(_cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
        let this_h = this_obj.handle().into();

        let ud = get_udp_ud(_cx, this_h);
        let closed = ud.map_or(true, |u| u.closed.get());
        args.rval().set(BooleanValue(closed));
        true
    }

    // ──── Register all methods ────
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
            c"sendMany".as_ptr(),
            Some(udp_send_many),
            1,
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
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"connect".as_ptr(),
            Some(udp_connect),
            2,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"disconnect".as_ptr(),
            Some(udp_disconnect),
            0,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"setBroadcast".as_ptr(),
            Some(udp_set_broadcast),
            1,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"setTTL".as_ptr(),
            Some(udp_set_ttl),
            1,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"setMulticastTTL".as_ptr(),
            Some(udp_set_multicast_ttl),
            1,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"setMulticastLoopback".as_ptr(),
            Some(udp_set_multicast_loopback),
            1,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"setMulticastInterface".as_ptr(),
            Some(udp_set_multicast_interface),
            1,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"addMembership".as_ptr(),
            Some(udp_add_membership),
            2,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"dropMembership".as_ptr(),
            Some(udp_drop_membership),
            2,
            JSPROP_ENUMERATE as u32,
        );
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            udp_h,
            c"reload".as_ptr(),
            Some(udp_reload),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // closed getter property — use JS_DefineProperty1 with getter native
        mozjs_sys::jsapi::JS_DefineProperty1(
            cx,
            udp_h,
            c"closed".as_ptr(),
            Some(udp_get_closed),
            None,
            (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
        );
    }

    // Resolve the Promise with the UDPSocket object
    rooted!(&in(cx_ref) let val = ObjectValue(udp_obj.get()));
    rooted!(&in(cx_ref) let p = promise);
    unsafe {
        mozjs_sys::jsapi::JS::ResolvePromise(cx, p.handle().into(), val.handle().into());
        mozjs_sys::jsapi::js::RunJobs(cx);
    }

    // Mark promise as settled
    {
        let ud = unsafe { &*(ud_ptr as *const UdpUserData) };
        ud.promise_settled.set(true);
    }

    rooted!(&in(cx_ref) let p = promise);
    args.rval().set(ObjectValue(p.get()));
    true
}

// ──────────────────── Socket pointer / UD helpers ────────────────────

/// Extract the UdpSocket pointer from a JS object's _socketPtr private property.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
unsafe fn get_socket_ptr(cx: *mut JSContext, obj_h: Handle<*mut JSObject>) -> *mut UdpSocket {
    let mut sv = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        c"_socketPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut sv,
        },
    );
    if sv.is_double() && (sv.asBits_ & 0xFFFF000000000000) == 0 {
        sv.to_private() as *mut UdpSocket
    } else {
        ::std::ptr::null_mut()
    }
}

/// Get a shared reference to UdpUserData from a JS object's _udPtr private property.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
unsafe fn get_udp_ud<'a>(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
) -> Option<&'a UdpUserData> {
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
        let ud_ptr = ud_val.to_private() as *mut UdpUserData;
        if !ud_ptr.is_null() {
            Some(&*ud_ptr)
        } else {
            None
        }
    } else {
        None
    }
}

/// Get a mutable reference to UdpUserData from a JS object's _udPtr private property.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
unsafe fn get_udp_ud_mut<'a>(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
) -> Option<&'a mut UdpUserData> {
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
        let ud_ptr = ud_val.to_private() as *mut UdpUserData;
        if !ud_ptr.is_null() {
            Some(&mut *ud_ptr)
        } else {
            None
        }
    } else {
        None
    }
}

// ──────────────────── UDP callbacks ────────────────────

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] on_data callback — fires socket.data JS callback
/// Bun upstream passes (this, data, port, address, flags) to the data callback.
/// We construct a Uint8Array-like view from the packet payload and extract
/// peer address/port from the sockaddr_storage.
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

    // Iterate packets and invoke JS callback for each
    let pkt_buf = unsafe { &mut *packets };
    for i in 0..count {
        // Extract payload as Vec<u8> (copy) to avoid holding mutable borrow
        let payload: Vec<u8> = {
            let slice = pkt_buf.get_payload(i);
            slice.to_vec()
        };
        if payload.is_empty() {
            continue;
        }

        // Extract peer address and port from sockaddr_storage (also copy)
        let (peer_addr, peer_port) = {
            let peer = pkt_buf.get_peer(i);
            unsafe { parse_peer_addr(peer).unwrap_or_else(|| ("0.0.0.0".to_string(), 0)) }
        };

        // Check for truncation (immutable borrow, safe after above mutable borrows ended)
        let truncated = pkt_buf.get_truncated(i);

        // Create JS values for the callback arguments:
        // data callback signature: (data, port, address, flags)
        // - data: Uint8Array (or Buffer) — we create a JS string for now
        //   (full ArrayBuffer support requires JS_NewArrayBufferWithContents)
        // - port: number
        // - address: string
        // - flags: { truncated: boolean }
        let js_str = unsafe { JS_NewStringCopyN(cx, payload.as_ptr() as *const _, payload.len()) };
        if js_str.is_null() {
            continue;
        }

        let data_val = unsafe { StringValue(&*js_str) };
        let port_val = Int32Value(peer_port as i32);

        let c_addr = ZBox::from_bytes(peer_addr.as_bytes());
        let js_addr = unsafe { JS_NewStringCopyZ(cx, c_addr.as_ptr()) };
        let addr_val = if !js_addr.is_null() {
            unsafe { StringValue(&*js_addr) }
        } else {
            UndefinedValue()
        };

        // Build flags object { truncated: boolean }
        let mut wrapped_cx =
            unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
        let cx_ref = &mut wrapped_cx;
        let flags_obj = unsafe { w2::JS_NewPlainObject(cx_ref) };
        let flags_val = if !flags_obj.is_null() {
            rooted!(&in(cx_ref) let fo = flags_obj);
            let fo_h = fo.handle().into();
            rooted!(&in(cx_ref) let trunc_v = BooleanValue(truncated));
            unsafe {
                JS_DefineProperty(
                    cx,
                    fo_h,
                    c"truncated".as_ptr(),
                    trunc_v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            ObjectValue(flags_obj)
        } else {
            UndefinedValue()
        };

        let _ = unsafe {
            invoke_js_callback(
                cx,
                &ud.data_cb_key,
                &[data_val, port_val, addr_val, flags_val],
            )
        };
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

    // Mark as closed
    ud.closed.set(true);

    let _ = unsafe { invoke_js_callback(ud.cx, &ud.close_cb_key, &[]) };
}

/// @trace REQ-BAO-API-017 [api:Bun.udpSocket] on_recv_error callback — fires socket.error JS callback
#[allow(unsafe_op_in_unsafe_fn)]
extern "C" fn udp_on_recv_error(socket: *mut UdpSocket, code: ::std::ffi::c_int) {
    if socket.is_null() {
        return;
    }
    let ud = unsafe { &*((*socket).user() as *const UdpUserData) };

    // Build a SystemError-like object with errno and message
    let cx = ud.cx;
    if cx.is_null() {
        return;
    }

    // Create error object: { errno: code, message: "recv error", code: "ERR_UDP_RECV" }
    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;
    let err_obj = unsafe { w2::JS_NewPlainObject(cx_ref) };
    if !err_obj.is_null() {
        rooted!(&in(cx_ref) let eo = err_obj);
        let eo_h = eo.handle().into();

        rooted!(&in(cx_ref) let errno_v = Int32Value(code));
        unsafe {
            JS_DefineProperty(
                cx,
                eo_h,
                c"errno".as_ptr(),
                errno_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        let c_msg = ZBox::from_bytes(b"recv error".as_slice());
        let js_msg = unsafe { JS_NewStringCopyZ(cx, c_msg.as_ptr()) };
        if !js_msg.is_null() {
            rooted!(&in(cx_ref) let msg_v = unsafe { StringValue(&*js_msg) });
            unsafe {
                JS_DefineProperty(
                    cx,
                    eo_h,
                    c"message".as_ptr(),
                    msg_v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        let c_code = ZBox::from_bytes(b"ERR_UDP_RECV".as_slice());
        let js_code = unsafe { JS_NewStringCopyZ(cx, c_code.as_ptr()) };
        if !js_code.is_null() {
            rooted!(&in(cx_ref) let code_v = unsafe { StringValue(&*js_code) });
            unsafe {
                JS_DefineProperty(
                    cx,
                    eo_h,
                    c"code".as_ptr(),
                    code_v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        let err_val = ObjectValue(err_obj);
        let _ = unsafe { invoke_js_callback(cx, &ud.error_cb_key, &[err_val]) };
    } else {
        // Fallback: just pass the error code
        let code_val = Int32Value(code);
        let _ = unsafe { invoke_js_callback(cx, &ud.error_cb_key, &[code_val]) };
    }
}

// ──────────────────── Install entry point ────────────────────

/// Install Bun.udpSocket native function on the Bun object.
/// @trace REQ-BAO-API-017 [api:Bun.udpSocket]
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    let raw_cx = cx.raw_cx();
    let bun_h = bun_obj.into();

    mozjs_sys::jsapi::JS_DefineFunction(
        raw_cx,
        bun_h,
        c"udpSocket".as_ptr(),
        Some(bun_udp_socket),
        2,
        JSPROP_ENUMERATE as u32,
    );
}
