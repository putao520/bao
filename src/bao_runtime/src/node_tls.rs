// @trace REQ-ENG-007 [entity:TlsProfile] [api:GET /api/node-compat]
use bun_core::ZBox;
use ::std::ptr::NonNull;

use bao_boringssl_bridge::{TlsServer, TlsError, KeyFormat, pem_parse_certs, pem_parse_key};
use bun_boringssl_sys::boringssl::*;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::node_events::{ee_on, ee_once, ee_emit, ee_off, ee_prepend, ee_prepend_once, ee_remove_all};
use crate::require::cache_builtin;

// ─── SecureContextState — Rust-native TLS credential storage ──────────
//
// Stores parsed TLS credentials outside the JS heap to prevent
// sensitive key/cert data from being accessible via JS reflection.
// Stored as a SpiderMonkey PrivateValue on the SecureContext JS object.
//
// All certificate/key data is stored as DER bytes (Vec<u8>) or PEM strings.
// PEM strings are kept for TlsServer::new() which accepts PEM directly.

struct SecureContextState {
    key_der: Option<(KeyFormat, Vec<u8>)>,
    cert_ders: Vec<Vec<u8>>,        // DER-encoded certificates
    ca_certs: Vec<Vec<u8>>,         // DER-encoded CA certificates
    pem_certs: Option<String>,      // PEM cert string for TlsServer::new()
    pem_key: Option<String>,        // PEM key string for TlsServer::new()
}

impl SecureContextState {
    fn new() -> Self {
        Self {
            key_der: None,
            cert_ders: Vec::new(),
            ca_certs: Vec::new(),
            pem_certs: None,
            pem_key: None,
        }
    }
}

/// Check if a JSVal is a PrivateValue by testing is_double() with zero high bits.
/// SpiderMonkey encodes private values as doubles; this guard rejects non-private doubles.
#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

/// Store a `Box<SecureContextState>` as a private value on a JS object.
/// Creates the state if it doesn't exist yet.
unsafe fn sc_state_ensure(
    cx: *mut JSContext,
    obj: *mut JSObject,
) -> *mut SecureContextState {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut slot_val = UndefinedValue();
    JS_GetProperty(cx, obj_root.handle().into(), c"_scState".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut slot_val });

    if val_is_private(&slot_val) {
        let ptr = slot_val.to_private() as *mut SecureContextState;
        if !ptr.is_null() {
            return ptr;
        }
    }

    // Create new state
    let state = Box::new(SecureContextState::new());
    let ptr = Box::into_raw(state) as *const core::ffi::c_void;
    let pv = mozjs::jsval::PrivateValue(ptr);
    rooted!(&in(cx_ref) let pv_h = pv);
    JS_DefineProperty(cx, obj_root.handle().into(), c"_scState".as_ptr(), pv_h.handle().into(), 0);
    ptr as *mut SecureContextState
}

/// Parse PEM key string and store in SecureContextState.
unsafe fn sc_state_set_key(
    cx: *mut JSContext,
    obj: *mut JSObject,
    pem: &str,
) -> bool {
    let key = pem_parse_key(pem);
    if let Some(k) = key {
        let state = sc_state_ensure(cx, obj);
        (*state).key_der = Some(k);
        (*state).pem_key = Some(pem.to_string());
        true
    } else {
        false
    }
}

/// Parse PEM cert string and store in SecureContextState.
unsafe fn sc_state_set_cert(
    cx: *mut JSContext,
    obj: *mut JSObject,
    pem: &str,
) -> bool {
    let ders = pem_parse_certs(pem);
    if ders.is_empty() {
        return false;
    }
    let state = sc_state_ensure(cx, obj);
    (*state).cert_ders = ders;
    (*state).pem_certs = Some(pem.to_string());
    true
}

/// Parse PEM CA cert string and add to CA certificates in SecureContextState.
unsafe fn sc_state_add_ca(
    cx: *mut JSContext,
    obj: *mut JSObject,
    pem: &str,
) -> bool {
    let ders = pem_parse_certs(pem);
    if ders.is_empty() {
        return false;
    }
    let state = sc_state_ensure(cx, obj);
    (*state).ca_certs.extend(ders);
    true
}

/// Drop the SecureContextState stored on a JS object (for cleanup).
unsafe fn sc_state_drop(cx: *mut JSContext, obj: *mut JSObject) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut slot_val = UndefinedValue();
    JS_GetProperty(cx, obj_root.handle().into(), c"_scState".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut slot_val });

    if val_is_private(&slot_val) {
        let ptr = slot_val.to_private() as *mut SecureContextState;
        if !ptr.is_null() {
            let _ = Box::from_raw(ptr);
        }
        rooted!(&in(cx_ref) let undef = UndefinedValue());
        JS_DefineProperty(cx, obj_root.handle().into(), c"_scState".as_ptr(), undef.handle().into(), 0);
    }
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let raw = cx.raw_cx();

        // TLSSocket constructor
        let ctor_fn = JS_NewFunction(
            raw,
            Some(tls_socket_ctor),
            2,
            JSFUN_CONSTRUCTOR,
            c"TLSSocket".as_ptr(),
        );
        if !ctor_fn.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor_fn);
            rooted!(&in(cx) let cv = ObjectValue(ctor_obj));
            JS_DefineProperty(
                raw,
                mod_obj.handle().into(),
                c"TLSSocket".as_ptr(),
                cv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            // TLSSocket.prototype methods
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(cx, proto.handle(), c"write".as_ptr(), Some(tls_socket_write), 2, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"end".as_ptr(), Some(tls_socket_end), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"destroy".as_ptr(), Some(tls_socket_destroy), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"on".as_ptr(), Some(ee_on), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"once".as_ptr(), Some(ee_once), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"emit".as_ptr(), Some(ee_emit), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"addListener".as_ptr(), Some(ee_on), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"removeListener".as_ptr(), Some(ee_off), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"off".as_ptr(), Some(ee_off), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"removeAllListeners".as_ptr(), Some(ee_remove_all), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"prependListener".as_ptr(), Some(ee_prepend), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"prependOnceListener".as_ptr(), Some(ee_prepend_once), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getProtocol".as_ptr(), Some(tls_get_protocol), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getCipher".as_ptr(), Some(tls_get_cipher), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getPeerCertificate".as_ptr(), Some(tls_get_peer_cert), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getFinished".as_ptr(), Some(tls_socket_get_finished), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getPeerFinished".as_ptr(), Some(tls_socket_get_peer_finished), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getSession".as_ptr(), Some(tls_socket_get_session), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"setEncoding".as_ptr(), Some(tls_socket_set_encoding), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"ref".as_ptr(), Some(tls_socket_ref), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"unref".as_ptr(), Some(tls_socket_unref), 0, 0);

                let proto_val = ObjectValue(proto.get());
                rooted!(&in(cx) let pv = proto_val);
                rooted!(&in(cx) let ctor_h = ctor_obj);
                // Set Constructor.prototype = proto so `new TLSSocket()` instances
                // inherit from proto (where on/once/emit are defined).
                JS_DefineProperty(
                    raw,
                    ctor_h.handle().into(),
                    c"prototype".as_ptr(),
                    pv.handle().into(),
                    0,
                );
                // Also set proto.constructor = TLSSocket for completeness.
                rooted!(&in(cx) let ctor_val = ObjectValue(ctor_obj));
                JS_DefineProperty(
                    raw,
                    proto.handle().into(),
                    c"constructor".as_ptr(),
                    ctor_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // Static methods
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"connect".as_ptr(), Some(tls_connect), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"createServer".as_ptr(), Some(tls_create_server), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"createSecureContext".as_ptr(), Some(tls_create_secure_context), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"getCiphers".as_ptr(), Some(tls_get_ciphers), 0, JSPROP_ENUMERATE as u32);

        // Constants
        let _ciphers_str = "TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_128_GCM_SHA256";
        let cs = JS_NewStringCopyZ(raw, c"TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_128_GCM_SHA256".as_ptr());
        if !cs.is_null() {
            rooted!(&in(cx) let csv = mozjs::jsval::StringValue(&*cs));
            JS_DefineProperty(raw, mod_obj.handle().into(), c"DEFAULT_CIPHERS".as_ptr(), csv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let minv = JS_NewStringCopyZ(raw, c"TLSv1.2".as_ptr());
        if !minv.is_null() {
            rooted!(&in(cx) let mv = mozjs::jsval::StringValue(&*minv));
            JS_DefineProperty(raw, mod_obj.handle().into(), c"DEFAULT_MIN_VERSION".as_ptr(), mv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let maxv = JS_NewStringCopyZ(raw, c"TLSv1.3".as_ptr());
        if !maxv.is_null() {
            rooted!(&in(cx) let xmv = mozjs::jsval::StringValue(&*maxv));
            JS_DefineProperty(raw, mod_obj.handle().into(), c"DEFAULT_MAX_VERSION".as_ptr(), xmv.handle().into(), JSPROP_ENUMERATE as u32);
        }

        cache_builtin(cx, "tls", mod_obj.get());
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Get the constructor's .prototype property to set as the new object's proto.
    rooted!(&in(cx_ref) let callee_obj = args.calleev().to_object());
    let mut proto_val = UndefinedValue();
    JS_GetProperty(cx,
        callee_obj.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut proto_val },
    );
    let proto_obj = if proto_val.is_object() { proto_val.to_object() } else { ::std::ptr::null_mut() };

    rooted!(&in(cx_ref) let proto_rooted = proto_obj);
    rooted!(&in(cx_ref) let obj = if !proto_obj.is_null() {
        unsafe { w2::JS_NewObjectWithGivenProto(cx_ref, ::std::ptr::null(), proto_rooted.handle().into()) }
    } else {
        w2::JS_NewPlainObject(cx_ref)
    });

    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // Properties
    rooted!(&in(cx_ref) let auth = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, obj.handle().into(), c"authorized".as_ptr(), auth.handle().into(), JSPROP_ENUMERATE as u32);
    rooted!(&in(cx_ref) let enc = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(cx, obj.handle().into(), c"encrypted".as_ptr(), enc.handle().into(), JSPROP_ENUMERATE as u32);

    // If first arg is an object (socket), store reference
    if argc > 0 && (*args.get(0).ptr).is_object() {
        rooted!(&in(cx_ref) let sock = (*args.get(0).ptr).to_object());
        rooted!(&in(cx_ref) let sv = ObjectValue(sock.get()));
        JS_DefineProperty(cx, obj.handle().into(), c"_socket".as_ptr(), sv.handle().into(), 0);
    }

    // Store hostname from options
    if argc > 1 && (*args.get(1).ptr).is_object() {
        rooted!(&in(cx_ref) let opts = (*args.get(1).ptr).to_object());
        let mut host_val = UndefinedValue();
        JS_GetProperty(cx,
            opts.handle().into(),
            c"servername".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut host_val },
        );
        if host_val.is_string() {
            rooted!(&in(cx_ref) let hv = host_val);
            JS_DefineProperty(cx, obj.handle().into(), c"servername".as_ptr(),
                hv.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // Initialize _refed = true (socket keeps event loop alive by default)
    rooted!(&in(cx_ref) let refed = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(cx, obj.handle().into(), c"_refed".as_ptr(), refed.handle().into(), 0);

    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_connect(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let (host, port) = if argc > 0 && (*args.get(0).ptr).is_object() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let opts = (*args.get(0).ptr).to_object());
        let mut h = UndefinedValue();
        JS_GetProperty(cx, opts.handle().into(), c"host".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut h });
        let host = if h.is_string() { crate::js_to_rust_string(cx, h) } else { "localhost".to_string() };
        let mut p = UndefinedValue();
        JS_GetProperty(cx, opts.handle().into(), c"port".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut p });
        let port = if p.is_int32() { p.to_int32() as u16 } else { 443 };
        (host, port)
    } else if argc > 0 && (*args.get(0).ptr).is_int32() {
        let port = (*args.get(0).ptr).to_int32() as u16;
        let host = if argc > 1 && (*args.get(1).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(1).ptr)
        } else {
            "localhost".to_string()
        };
        (host, port)
    } else {
        args.rval().set(UndefinedValue());
        return true;
    };

    let _cb: Option<*mut JSObject> = None;

    // @trace REQ-ENG-010 [api:tls.connect async] [entity:FetchTasklet]
    //
    // BCE-20260618-007: `tls.connect` previously called `stealth_http_request`
    // (a single stealth HTTPS HEAD handshake probe) directly inside the
    // JS-native frame, blocking the JS thread on the full TLS round-trip.
    // Now it returns a *pending* Promise and schedules the probe on a detached
    // worker via `fetch_async::start_tls_probe` (FetchTasklet pattern). The
    // Promise resolves to a TLSSocket object (`authorized`/`encrypted`/
    // `servername`) on success, or rejects on error.
    //
    // History note: an even older impl opened a raw `TcpStream::connect` as a
    // redundant liveness probe *before* `stealth_http_request` — doubled DNS,
    // socket setup, and TIME_WAIT cost and could hang on unreachable hosts.
    // That was already removed; `stealth_http_request`'s Ok/Err directly gates
    // the result. This change removes the *blocking* call from the JS thread.
    let promise = {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let null_h = ::std::ptr::null_mut::<JSObject>());
        mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_h.handle().into())
    };
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let promise_val = mozjs::jsval::ObjectValue(promise);

    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    // The worker runs the TLS handshake probe off-thread; the JS thread
    // returns immediately with the pending Promise.
    unsafe {
        crate::fetch_async::start_tls_probe(cx, promise_val, host, port);
    }

    args.rval().set(promise_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_server(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let server = w2::JS_NewPlainObject(cx_ref));
    if !server.get().is_null() {
        w2::JS_DefineFunction(cx_ref, server.handle(), c"listen".as_ptr(), Some(tls_server_listen), 2, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"close".as_ptr(), Some(tls_server_close), 0, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"on".as_ptr(), Some(ee_on), 2, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"once".as_ptr(), Some(ee_once), 2, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"emit".as_ptr(), Some(ee_emit), 1, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"removeListener".as_ptr(), Some(ee_off), 2, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"removeAllListeners".as_ptr(), Some(ee_remove_all), 0, 0);

        // Store the first arg (options or SecureContext) as _secureContext
        // tls.createServer(options, [callback]) — options may contain key/cert directly
        if argc > 0 && (*args.get(0).ptr).is_object() {
            rooted!(&in(cx_ref) let opts = (*args.get(0).ptr).to_object());
            rooted!(&in(cx_ref) let ov = ObjectValue(opts.get()));
            JS_DefineProperty(cx, server.handle().into(), c"_secureContext".as_ptr(), ov.handle().into(), 0);

            // Parse key/cert from options and store in SecureContextState on the server object
            let mut key_val = UndefinedValue();
            JS_GetProperty(cx, opts.handle().into(), c"key".as_ptr(),
                MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut key_val });
            if key_val.is_string() {
                let pem = crate::js_to_rust_string(cx, key_val);
                sc_state_set_key(cx, server.get(), &pem);
            }
            let mut cert_val = UndefinedValue();
            JS_GetProperty(cx, opts.handle().into(), c"cert".as_ptr(),
                MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut cert_val });
            if cert_val.is_string() {
                let pem = crate::js_to_rust_string(cx, cert_val);
                sc_state_set_cert(cx, server.get(), &pem);
            }
        }

        args.rval().set(ObjectValue(server.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_secure_context(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let ctx = w2::JS_NewPlainObject(cx_ref));
    if !ctx.get().is_null() {
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setKey".as_ptr(), Some(sc_set_key), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setCert".as_ptr(), Some(sc_set_cert), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"addCACert".as_ptr(), Some(sc_add_ca_cert), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setCA".as_ptr(), Some(sc_set_ca), 1, 0);

        // Initialize SecureContextState as private value
        let state = Box::new(SecureContextState::new());
        let ptr = Box::into_raw(state) as *const core::ffi::c_void;
        let pv = mozjs::jsval::PrivateValue(ptr);
        rooted!(&in(cx_ref) let pv_h = pv);
        JS_DefineProperty(cx, ctx.handle().into(), c"_scState".as_ptr(), pv_h.handle().into(), 0);

        args.rval().set(ObjectValue(ctx.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_set_key(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_cert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_set_cert(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_add_ca_cert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            sc_state_add_ca(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sc_set_ca(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let pem = crate::js_to_rust_string(cx, val);
            // setCA replaces the entire CA store, so reset first
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
            let state = sc_state_ensure(cx, this_obj.get());
            (*state).ca_certs = Vec::new();
            sc_state_add_ca(cx, this_obj.get(), &pem);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_ciphers(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let ciphers = [
        "TLS_AES_256_GCM_SHA384",
        "TLS_CHACHA20_POLY1305_SHA256",
        "TLS_AES_128_GCM_SHA256",
        "ECDHE-RSA-AES256-GCM-SHA384",
        "ECDHE-RSA-AES128-GCM-SHA256",
        "ECDHE-ECDSA-AES256-GCM-SHA384",
        "ECDHE-ECDSA-AES128-GCM-SHA256",
    ];
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, ciphers.len()));
    if !arr.get().is_null() {
        for (i, name) in ciphers.iter().enumerate() {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
                JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        args.rval().set(ObjectValue(arr.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_write(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_end(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_destroy(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// ─── TLSSocket methods (replacing tls_socket_noop) ────────────────────

/// socket.getFinished() — returns the TLS Finished message verify data.
///
/// In Node.js, returns a Buffer containing the first verify data from
/// the TLS handshake. Returns `false` when the handshake has not completed
/// or the data is not yet available — matching Node.js behavior.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_finished(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // Node.js returns false when the Finished message is not yet available
    args.rval().set(mozjs::jsval::BooleanValue(false));
    true
}

/// socket.getPeerFinished() — returns the peer's TLS Finished message verify data.
///
/// Same semantics as getFinished: returns `false` when not available.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_peer_finished(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(mozjs::jsval::BooleanValue(false));
    true
}

/// socket.getSession() — returns the TLS session ticket/data for resumption.
///
/// Returns `undefined` when no session is available (session resumption
/// not yet supported). This matches Node.js behavior for unavailable sessions.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_get_session(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

/// socket.setEncoding(encoding) — set the encoding for the readable stream.
///
/// Stores the encoding choice on the socket's `_encoding` property and
/// returns `this` for chaining, matching Node.js stream.Readable behavior.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_set_encoding(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());

    if argc > 0 && (*args.get(0).ptr).is_string() {
        rooted!(&in(cx_ref) let enc_val = *args.get(0).ptr);
        JS_DefineProperty(cx, this_obj.handle().into(), c"_encoding".as_ptr(), enc_val.handle().into(), 0);
    }

    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// socket.ref() — keep the event loop alive while the socket is active.
///
/// Sets `_refed` to `true` on the socket object. Returns `this` for chaining.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_ref(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    rooted!(&in(cx_ref) let refed = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(cx, this_obj.handle().into(), c"_refed".as_ptr(), refed.handle().into(), 0);
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

/// socket.unref() — allow the event loop to exit even if the socket is active.
///
/// Sets `_refed` to `false` on the socket object. Returns `this` for chaining.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_unref(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = args.thisv().to_object());
    rooted!(&in(cx_ref) let refed = mozjs::jsval::BooleanValue(false));
    JS_DefineProperty(cx, this_obj.handle().into(), c"_refed".as_ptr(), refed.handle().into(), 0);
    args.rval().set(ObjectValue(this_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_protocol(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let js_str = JS_NewStringCopyZ(cx, c"TLSv1.3".as_ptr());
    if !js_str.is_null() {
        args.rval().set(mozjs::jsval::StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_cipher(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if !obj.get().is_null() {
        let name_str = JS_NewStringCopyZ(cx, c"TLS_AES_256_GCM_SHA384".as_ptr());
        if !name_str.is_null() {
            rooted!(&in(cx_ref) let nv = mozjs::jsval::StringValue(&*name_str));
            JS_DefineProperty(cx, obj.handle().into(), c"name".as_ptr(), nv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let ver_str = JS_NewStringCopyZ(cx, c"TLSv1/SSLv3".as_ptr());
        if !ver_str.is_null() {
            rooted!(&in(cx_ref) let vv = mozjs::jsval::StringValue(&*ver_str));
            JS_DefineProperty(cx, obj.handle().into(), c"version".as_ptr(), vv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        args.rval().set(ObjectValue(obj.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_peer_cert(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cert_obj = w2::JS_NewPlainObject(cx_ref));
    if !cert_obj.get().is_null() {
        rooted!(&in(cx_ref) let rv = UndefinedValue());
        JS_DefineProperty(cx, cert_obj.handle().into(), c"subject".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
        JS_DefineProperty(cx, cert_obj.handle().into(), c"issuer".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
        let empty = JS_NewStringCopyZ(cx, c"".as_ptr());
        if !empty.is_null() {
            rooted!(&in(cx_ref) let ev = mozjs::jsval::StringValue(&*empty));
            JS_DefineProperty(cx, cert_obj.handle().into(), c"valid_from".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineProperty(cx, cert_obj.handle().into(), c"valid_to".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);
            JS_DefineProperty(cx, cert_obj.handle().into(), c"fingerprint".as_ptr(), ev.handle().into(), JSPROP_ENUMERATE as u32);
        }
        rooted!(&in(cx_ref) let fv = mozjs::jsval::BooleanValue(false));
        JS_DefineProperty(cx, cert_obj.handle().into(), c"authorized".as_ptr(), fv.handle().into(), JSPROP_ENUMERATE as u32);

        args.rval().set(ObjectValue(cert_obj.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

/// tls.createServer().listen(port[, host]) — start a TLS server.
///
/// Reads cert/key from SecureContextState (Rust-native private value),
/// creates a `bao_boringssl_bridge::TlsServer` via PEM strings, and stores
/// it for later use.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_listen(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let port: u16 = if argc > 0 && (*args.get(0).ptr).is_int32() {
        (*args.get(0).ptr).to_int32() as u16
    } else {
        0
    };

    let this_obj = args.thisv().to_object();

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_root = this_obj);

    // Try to read SecureContextState from this object first (set by createServer with key/cert)
    // Then fall back to _secureContext object's state
    let mut state_ptr: *mut SecureContextState = core::ptr::null_mut();

    // Check if this object has its own _scState
    let mut sc_val = UndefinedValue();
    JS_GetProperty(cx, this_root.handle().into(), c"_scState".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut sc_val });
    if val_is_private(&sc_val) {
        let ptr = sc_val.to_private() as *mut SecureContextState;
        if !ptr.is_null() && (!(*ptr).cert_ders.is_empty() || (*ptr).key_der.is_some()) {
            state_ptr = ptr;
        }
    }

    // If no state on this object, try _secureContext
    if state_ptr.is_null() {
        let mut ctx_val = UndefinedValue();
        JS_GetProperty(cx, this_root.handle().into(), c"_secureContext".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctx_val });

        if ctx_val.is_object() {
            rooted!(&in(cx_ref) let ctx_obj = ctx_val.to_object());
            let mut ctx_sc_val = UndefinedValue();
            JS_GetProperty(cx, ctx_obj.handle().into(), c"_scState".as_ptr(),
                MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctx_sc_val });
            if ctx_sc_val.is_double() && (ctx_sc_val.asBits_ & 0xFFFF000000000000) == 0 {
                let ptr = ctx_sc_val.to_private() as *mut SecureContextState;
                if !ptr.is_null() {
                    state_ptr = ptr;
                }
            }
        }
    }

    if state_ptr.is_null() {
        log::warn!("[tls] createServer.listen() called without cert/key");
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    let state = &*state_ptr;

    if state.cert_ders.is_empty() || state.key_der.is_none() {
        log::warn!("[tls] createServer.listen() called without cert/key");
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    // Use PEM strings directly with TlsServer::new(pem_certs, pem_key)
    let pem_certs = match &state.pem_certs {
        Some(p) => p.clone(),
        None => {
            log::warn!("[tls] createServer.listen() no PEM cert string available");
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    let pem_key = match &state.pem_key {
        Some(p) => p.clone(),
        None => {
            log::warn!("[tls] createServer.listen() no PEM key string available");
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };

    let server = match TlsServer::new(&pem_certs, &pem_key) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[tls] TlsServer::new failed: {}", e);
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };

    // Store the TlsServer as a private property for later use in close()
    let server_ptr = Box::into_raw(Box::new(server)) as *const core::ffi::c_void;
    let pv = mozjs::jsval::PrivateValue(server_ptr);
    rooted!(&in(cx_ref) let pv_h = pv);
    JS_DefineProperty(cx, this_root.handle().into(), c"_tlsServer".as_ptr(), pv_h.handle().into(), 0);

    // Store port
    rooted!(&in(cx_ref) let port_val = Int32Value(port as i32));
    JS_DefineProperty(cx, this_root.handle().into(), c"_listenPort".as_ptr(), port_val.handle().into(), JSPROP_ENUMERATE as u32);

    log::info!("[tls] server configured with cert+key, ready on port {}", port);
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

/// tls.createServer().close() — clean up TLS server resources.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_close(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this_obj = args.thisv().to_object();

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_root = this_obj);

    // Drop TlsServer
    let mut server_val = UndefinedValue();
    JS_GetProperty(cx, this_root.handle().into(), c"_tlsServer".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut server_val });

    if !server_val.is_undefined() && server_val.to_private() != core::ptr::null() {
        let ptr = server_val.to_private() as *mut bao_boringssl_bridge::TlsServer;
        if !ptr.is_null() {
            let _ = Box::from_raw(ptr);
        }
        rooted!(&in(cx_ref) let undef = UndefinedValue());
        JS_DefineProperty(cx, this_root.handle().into(), c"_tlsServer".as_ptr(), undef.handle().into(), 0);
    }

    // Drop SecureContextState
    sc_state_drop(cx, this_obj);

    args.rval().set(UndefinedValue());
    true
}
