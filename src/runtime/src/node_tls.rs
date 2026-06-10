// @trace REQ-ENG-007, REQ-ENG-009
use ::std::ptr::NonNull;
use ::std::sync::Arc;

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, PrivateValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use mozjs::glue::JS_GetReservedSlot;

use bao_tls::{TlsClient, TlsConnection, TlsError, TlsProfile, TlsServer, TlsState, bao_crypto_provider};

use crate::require::cache_builtin;

// ─── SecureContext state ─────────────────────────────────────────────
// Stored as a Box<SecureContextState> attached to the JS SecureContext
// object via a reserved slot. Built incrementally (setKey/setCert/addCACert),
// then resolved into an Arc<ServerConfig> for tls.createServer.

struct SecureContextState {
    certs: Vec<rustls::pki_types::CertificateDer<'static>>,
    key: Option<rustls::pki_types::PrivateKeyDer<'static>>,
    ca_certs: Vec<rustls::pki_types::CertificateDer<'static>>,
}

impl SecureContextState {
    fn new() -> Self {
        Self {
            certs: Vec::new(),
            key: None,
            ca_certs: Vec::new(),
        }
    }

    fn build_server_config(&mut self) -> Option<Arc<rustls::ServerConfig>> {
        let key = self.key.take()?;
        if self.certs.is_empty() {
            return None;
        }
        let provider = bao_crypto_provider();
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .ok()?
            .with_no_client_auth()
            .with_single_cert(self.certs.clone(), key)
            .ok()?;
        Some(Arc::new(config))
    }
}

// ─── TLS connection state ────────────────────────────────────────────
// Stored as Box<TlsConnState> on TLSSocket objects. Holds the
// rustls TlsConnection and provides method dispatch.

struct TlsConnState {
    conn: TlsConnection,
    event_handlers: Vec<(*mut JSObject, *mut JSObject)>, // (event_name, callback)
}

// ─── Module install ──────────────────────────────────────────────────

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
                w2::JS_DefineFunction(cx, proto.handle(), c"on".as_ptr(), Some(tls_socket_on), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"once".as_ptr(), Some(tls_socket_on), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"emit".as_ptr(), Some(tls_socket_emit), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"removeListener".as_ptr(), Some(tls_socket_remove_listener), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getProtocol".as_ptr(), Some(tls_get_protocol), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getCipher".as_ptr(), Some(tls_get_cipher), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getPeerCertificate".as_ptr(), Some(tls_get_peer_cert), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getFinished".as_ptr(), Some(tls_get_finished), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getPeerFinished".as_ptr(), Some(tls_get_peer_finished), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getSession".as_ptr(), Some(tls_get_session), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"setEncoding".as_ptr(), Some(tls_set_encoding), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"ref".as_ptr(), Some(tls_socket_ref), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"unref".as_ptr(), Some(tls_socket_unref), 0, 0);

                let proto_val = ObjectValue(proto.get());
                rooted!(&in(cx) let pv = proto_val);
                rooted!(&in(cx) let ctor_h = ctor_obj);
                JS_SetPrototype(raw, ctor_h.handle().into(), proto.handle().into());
            }
        }

        // Static methods
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"connect".as_ptr(), Some(tls_connect), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"createServer".as_ptr(), Some(tls_create_server), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"createSecureContext".as_ptr(), Some(tls_create_secure_context), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"getCiphers".as_ptr(), Some(tls_get_ciphers), 0, JSPROP_ENUMERATE as u32);

        // Constants
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

// ─── Slot index for private data ─────────────────────────────────────

const SLOT_TLS_CONN: u32 = 0;
const SLOT_SECURE_CTX: u32 = 0;

// ─── Helpers ─────────────────────────────────────────────────────────

unsafe fn get_tls_conn_ptr(obj: *mut JSObject) -> *mut TlsConnState {
    let mut val = UndefinedValue();
    JS_GetReservedSlot(obj, SLOT_TLS_CONN, &mut val);
    if val.is_double() {
        let ptr = val.to_private() as *mut TlsConnState;
        if !ptr.is_null() {
            return ptr;
        }
    }
    core::ptr::null_mut()
}

unsafe fn set_tls_conn(obj: *mut JSObject, state: Box<TlsConnState>) {
    let val = PrivateValue(Box::into_raw(state) as *const core::ffi::c_void);
    JS_SetReservedSlot(obj, SLOT_TLS_CONN, &val);
}

unsafe fn get_secure_ctx_ptr(obj: *mut JSObject) -> *mut SecureContextState {
    let mut val = UndefinedValue();
    JS_GetReservedSlot(obj, SLOT_SECURE_CTX, &mut val);
    if val.is_double() {
        let ptr = val.to_private() as *mut SecureContextState;
        if !ptr.is_null() {
            return ptr;
        }
    }
    core::ptr::null_mut()
}

unsafe fn set_secure_ctx(obj: *mut JSObject, state: Box<SecureContextState>) {
    let val = PrivateValue(Box::into_raw(state) as *const core::ffi::c_void);
    JS_SetReservedSlot(obj, SLOT_SECURE_CTX, &val);
}

/// Parse PEM-encoded certificates into rustls CertificateDer list.
fn parse_certs_pem(pem: &[u8]) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    let mut certs = Vec::new();
    for cert in rustls_pemfile::certs(&mut &pem[..]) {
        if let Ok(c) = cert {
            certs.push(c);
        }
    }
    certs
}

/// Parse PEM-encoded private key.
fn parse_key_pem(pem: &[u8]) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    for item in rustls_pemfile::read_all(&mut &pem[..]) {
        if let Ok(rustls_pemfile::Item::Pkcs8Key(key)) = item {
            return Some(rustls::pki_types::PrivateKeyDer::Pkcs8(key));
        }
        if let Ok(rustls_pemfile::Item::Pkcs1Key(key)) = item {
            return Some(rustls::pki_types::PrivateKeyDer::Pkcs1(key));
        }
        if let Ok(rustls_pemfile::Item::Sec1Key(key)) = item {
            return Some(rustls::pki_types::PrivateKeyDer::Sec1(key));
        }
    }
    None
}

// ─── TLSSocket constructor ───────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
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
        let sock = (*args.get(0).ptr).to_object();
        rooted!(&in(cx_ref) let sv = ObjectValue(sock));
        JS_DefineProperty(cx, obj.handle().into(), c"_socket".as_ptr(), sv.handle().into(), 0);
    }

    // Store hostname from options
    if argc > 1 && (*args.get(1).ptr).is_object() {
        let opts = (*args.get(1).ptr).to_object();
        let mut host_val = UndefinedValue();
        JS_GetProperty(cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts },
            c"servername".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut host_val },
        );
        if host_val.is_string() {
            JS_DefineProperty(cx, obj.handle().into(), c"servername".as_ptr(),
                Handle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &host_val },
                JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

// ─── tls.connect ─────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_connect(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let (host, _port) = if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts };
        let mut h = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"host".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut h });
        let host = if h.is_string() { crate::js_to_rust_string(cx, h) } else { "localhost".to_string() };
        let mut p = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"port".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut p });
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

    // Build TLS client with the Bao CryptoProvider
    let name = match rustls::pki_types::ServerName::try_from(host) {
        Ok(n) => n,
        Err(_) => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };

    // Extract servername string before name is moved into connect()
    let servername_str = name.to_str().into_owned();

    let client = TlsClient::new();
    let conn = match client.connect(name) {
        Ok(c) => c,
        Err(_) => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };

    rooted!(&in(cx_ref) let tls_obj = w2::JS_NewPlainObject(cx_ref));
    if !tls_obj.get().is_null() {
        rooted!(&in(cx_ref) let auth = mozjs::jsval::BooleanValue(true));
        JS_DefineProperty(cx, tls_obj.handle().into(), c"authorized".as_ptr(), auth.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(cx_ref) let enc = mozjs::jsval::BooleanValue(true));
        JS_DefineProperty(cx, tls_obj.handle().into(), c"encrypted".as_ptr(), enc.handle().into(), JSPROP_ENUMERATE as u32);

        let host_z = bun_core::ZBox::from_bytes(servername_str.as_bytes());
        let host_str = JS_NewStringCopyZ(cx, host_z.as_ptr());
        if !host_str.is_null() {
            rooted!(&in(cx_ref) let hv = mozjs::jsval::StringValue(&*host_str));
            JS_DefineProperty(cx, tls_obj.handle().into(), c"servername".as_ptr(), hv.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // Store the TlsConnection in a reserved slot
        let state = Box::new(TlsConnState {
            conn,
            event_handlers: Vec::new(),
        });
        set_tls_conn(tls_obj.get(), state);

        args.rval().set(ObjectValue(tls_obj.get()));
        return true;
    }

    args.rval().set(UndefinedValue());
    true
}

// ─── tls.createServer ────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_server(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // First arg may be a SecureContext or options with key/cert
    let mut secure_state = SecureContextState::new();

    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts };

        // Try to read key/cert from options
        let mut key_val = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"key".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut key_val });
        if key_val.is_string() {
            let key_str = crate::js_to_rust_string(cx, key_val);
            if let Some(pk) = parse_key_pem(key_str.as_bytes()) {
                secure_state.key = Some(pk);
            }
        }

        let mut cert_val = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"cert".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut cert_val });
        if cert_val.is_string() {
            let cert_str = crate::js_to_rust_string(cx, cert_val);
            secure_state.certs = parse_certs_pem(cert_str.as_bytes());
        }
    }

    rooted!(&in(cx_ref) let server = w2::JS_NewPlainObject(cx_ref));
    if !server.get().is_null() {
        // Store the SecureContextState
        set_secure_ctx(server.get(), Box::new(secure_state));

        w2::JS_DefineFunction(cx_ref, server.handle(), c"listen".as_ptr(), Some(tls_server_listen), 2, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"close".as_ptr(), Some(tls_server_close), 1, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"on".as_ptr(), Some(tls_socket_on), 2, 0);
        args.rval().set(ObjectValue(server.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── tls.createSecureContext ─────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_secure_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let mut state = SecureContextState::new();

    // Parse options from first argument
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts };

        let mut key_val = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"key".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut key_val });
        if key_val.is_string() {
            let key_str = crate::js_to_rust_string(cx, key_val);
            if let Some(pk) = parse_key_pem(key_str.as_bytes()) {
                state.key = Some(pk);
            }
        }

        let mut cert_val = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"cert".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut cert_val });
        if cert_val.is_string() {
            let cert_str = crate::js_to_rust_string(cx, cert_val);
            state.certs = parse_certs_pem(cert_str.as_bytes());
        }

        let mut ca_val = UndefinedValue();
        JS_GetProperty(cx, opts_h, c"ca".as_ptr(), MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ca_val });
        if ca_val.is_string() {
            let ca_str = crate::js_to_rust_string(cx, ca_val);
            state.ca_certs = parse_certs_pem(ca_str.as_bytes());
        }
    }

    rooted!(&in(cx_ref) let ctx = w2::JS_NewPlainObject(cx_ref));
    if !ctx.get().is_null() {
        set_secure_ctx(ctx.get(), Box::new(state));

        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setKey".as_ptr(), Some(secure_context_set_key), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setCert".as_ptr(), Some(secure_context_set_cert), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"addCACert".as_ptr(), Some(secure_context_add_ca_cert), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setCA".as_ptr(), Some(secure_context_set_ca), 1, 0);
        args.rval().set(ObjectValue(ctx.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── SecureContext methods ───────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn secure_context_set_key(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    if let Some(state) = get_secure_ctx_ptr(obj).as_mut() {
        if argc > 0 && (*args.get(0).ptr).is_string() {
            let key_str = crate::js_to_rust_string(cx, *args.get(0).ptr);
            if let Some(pk) = parse_key_pem(key_str.as_bytes()) {
                state.key = Some(pk);
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn secure_context_set_cert(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    if let Some(state) = get_secure_ctx_ptr(obj).as_mut() {
        if argc > 0 && (*args.get(0).ptr).is_string() {
            let cert_str = crate::js_to_rust_string(cx, *args.get(0).ptr);
            state.certs = parse_certs_pem(cert_str.as_bytes());
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn secure_context_add_ca_cert(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    if let Some(state) = get_secure_ctx_ptr(obj).as_mut() {
        if argc > 0 && (*args.get(0).ptr).is_string() {
            let ca_str = crate::js_to_rust_string(cx, *args.get(0).ptr);
            let certs = parse_certs_pem(ca_str.as_bytes());
            state.ca_certs.extend(certs);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn secure_context_set_ca(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    if let Some(state) = get_secure_ctx_ptr(obj).as_mut() {
        if argc > 0 && (*args.get(0).ptr).is_string() {
            let ca_str = crate::js_to_rust_string(cx, *args.get(0).ptr);
            state.ca_certs = parse_certs_pem(ca_str.as_bytes());
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── Server methods ──────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_listen(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // TODO: Wire to bun_uws::App<true> when TLS socket path is integrated.
    // For now, the SecureContext is built and ready; the actual listen
    // requires the UpgradedDuplex → TlsConnection integration (A4-4).
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_server_close(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// ─── TLSSocket methods ───────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = this.to_object();
    if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
        if argc > 0 {
            let data_val = *args.get(0).ptr;
            if data_val.is_string() {
                let s = crate::js_to_rust_string(cx, data_val);
                match state.conn.write(s.as_bytes()) {
                    Ok(n) => {
                        args.rval().set(Int32Value(n as i32));
                        return true;
                    }
                    Err(TlsError::NotReady) => {
                        args.rval().set(Int32Value(0));
                        return true;
                    }
                    Err(_) => {
                        args.rval().set(mozjs::jsval::BooleanValue(false));
                        return true;
                    }
                }
            }
        }
    }
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
    let this = args.thisv();
    if this.is_object() {
        let obj = this.to_object();
        if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
            let _ = state.conn.queue_close_notify();
        }
    }
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
    let this = args.thisv();
    if this.is_object() {
        let obj = this.to_object();
        if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
            let _ = state.conn.queue_close_notify();
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_on(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if this.is_object() {
        let obj = this.to_object();
        if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
            if argc >= 2 && (*args.get(0).ptr).is_string() && (*args.get(1).ptr).is_object() {
                let ev_name = (*args.get(0).ptr).to_object();
                let cb = (*args.get(1).ptr).to_object();
                state.event_handlers.push((ev_name, cb));
            }
        }
        args.rval().set(ObjectValue(obj));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_emit(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    // Emit is a no-op in this implementation — event dispatch is handled
    // by the TLS state machine in the event loop.
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if this.is_object() {
        args.rval().set(ObjectValue(this.to_object()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_remove_listener(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if this.is_object() {
        let obj = this.to_object();
        if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
            if argc >= 2 && (*args.get(0).ptr).is_string() && (*args.get(1).ptr).is_object() {
                let ev_name = (*args.get(0).ptr).to_object();
                let cb = (*args.get(1).ptr).to_object();
                state.event_handlers.retain(|(n, c)| n != &ev_name || c != &cb);
            }
        }
        args.rval().set(ObjectValue(obj));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_protocol(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    let version_str = if this.is_object() {
        let obj = this.to_object();
        if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
            match state.conn.protocol_version() {
                Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3",
                Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2",
                Some(_) => "unknown",
                None => "unknown",
            }
        } else {
            "unknown"
        }
    } else {
        "unknown"
    };

    let c_str = bun_core::ZBox::from_bytes(version_str.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
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

    let (name, version) = {
        let this = args.thisv();
        if this.is_object() {
            let obj = this.to_object();
            if let Some(state) = get_tls_conn_ptr(obj).as_mut() {
                let cs = state.conn.negotiated_cipher_suite();
                let pv = state.conn.protocol_version();
                let name = cs.map(|s| s.suite().as_str().unwrap_or("unknown")).unwrap_or("unknown");
                let ver = match pv {
                    Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3",
                    Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1/SSLv3",
                    Some(_) => "unknown",
                    None => "unknown",
                };
                (name.to_string(), ver.to_string())
            } else {
                ("unknown".to_string(), "unknown".to_string())
            }
        } else {
            ("unknown".to_string(), "unknown".to_string())
        }
    };

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if !obj.get().is_null() {
        let name_z = bun_core::ZBox::from_bytes(name.as_bytes());
        let name_str = JS_NewStringCopyZ(cx, name_z.as_ptr());
        if !name_str.is_null() {
            rooted!(&in(cx_ref) let nv = mozjs::jsval::StringValue(&*name_str));
            JS_DefineProperty(cx, obj.handle().into(), c"name".as_ptr(), nv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let ver_z = bun_core::ZBox::from_bytes(version.as_bytes());
        let ver_str = JS_NewStringCopyZ(cx, ver_z.as_ptr());
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

// ─── TLS session data methods ────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_finished(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    // rustls does not expose finished messages directly.
    // Return empty Buffer (Node.js returns null if not available).
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_peer_finished(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_get_session(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    // rustls does not support TLS session resumption via getSession().
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_set_encoding(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    // No-op: encoding is always UTF-8 in Bao
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_ref(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_unref(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// ─── tls.getCiphers ──────────────────────────────────────────────────

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
            let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
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
