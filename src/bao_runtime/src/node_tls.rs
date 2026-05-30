// @trace REQ-ENG-007
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

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
                w2::JS_DefineFunction(cx, proto.handle(), c"once".as_ptr(), Some(tls_socket_noop), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"emit".as_ptr(), Some(tls_socket_noop), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"removeListener".as_ptr(), Some(tls_socket_noop), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getProtocol".as_ptr(), Some(tls_get_protocol), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getCipher".as_ptr(), Some(tls_get_cipher), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getPeerCertificate".as_ptr(), Some(tls_get_peer_cert), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"getFinished".as_ptr(), Some(tls_socket_noop), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getPeerFinished".as_ptr(), Some(tls_socket_noop), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"getSession".as_ptr(), Some(tls_socket_noop), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"setEncoding".as_ptr(), Some(tls_socket_noop), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"ref".as_ptr(), Some(tls_socket_noop), 0, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"unref".as_ptr(), Some(tls_socket_noop), 0, 0);

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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_connect(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let (host, port) = if argc > 0 && (*args.get(0).ptr).is_object() {
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

    let _cb: Option<*mut JSObject> = None;

    // Attempt TLS connection via ureq (verifies handshake, socket I/O is stubbed)
    if let Ok(_tcp_stream) = ::std::net::TcpStream::connect((&*host, port)) {
        // TCP reachable — verify TLS handshake via ureq HEAD request
        let test_url = format!("https://{}:{}", host, port);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(::std::time::Duration::from_secs(5)))
            .build()
            .into();
        let tls_ok = agent.head(&test_url).call().is_ok();

        if tls_ok {
            rooted!(&in(cx_ref) let tls_obj = w2::JS_NewPlainObject(cx_ref));
            if !tls_obj.get().is_null() {
                rooted!(&in(cx_ref) let auth = mozjs::jsval::BooleanValue(true));
                JS_DefineProperty(cx, tls_obj.handle().into(), c"authorized".as_ptr(), auth.handle().into(), JSPROP_ENUMERATE as u32);
                rooted!(&in(cx_ref) let enc = mozjs::jsval::BooleanValue(true));
                JS_DefineProperty(cx, tls_obj.handle().into(), c"encrypted".as_ptr(), enc.handle().into(), JSPROP_ENUMERATE as u32);

                let host_str = JS_NewStringCopyN(cx, host.as_ptr() as *const ::std::os::raw::c_char, host.len());
                if !host_str.is_null() {
                    rooted!(&in(cx_ref) let hv = mozjs::jsval::StringValue(&*host_str));
                    JS_DefineProperty(cx, tls_obj.handle().into(), c"servername".as_ptr(), hv.handle().into(), JSPROP_ENUMERATE as u32);
                }

                args.rval().set(ObjectValue(tls_obj.get()));
                return true;
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_create_server(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // Server requires cert/key — return a mock server object
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(_cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let server = w2::JS_NewPlainObject(cx_ref));
    if !server.get().is_null() {
        w2::JS_DefineFunction(cx_ref, server.handle(), c"listen".as_ptr(), Some(tls_socket_noop), 2, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"close".as_ptr(), Some(tls_socket_noop), 1, 0);
        w2::JS_DefineFunction(cx_ref, server.handle(), c"on".as_ptr(), Some(tls_socket_noop), 2, 0);
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
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setKey".as_ptr(), Some(tls_socket_noop), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setCert".as_ptr(), Some(tls_socket_noop), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"addCACert".as_ptr(), Some(tls_socket_noop), 1, 0);
        w2::JS_DefineFunction(cx_ref, ctx.handle(), c"setCA".as_ptr(), Some(tls_socket_noop), 1, 0);
        args.rval().set(ObjectValue(ctx.get()));
        return true;
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
            let c_name = CString::new(*name).unwrap_or_default();
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tls_socket_on(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
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

unsafe extern "C" fn tls_socket_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}
