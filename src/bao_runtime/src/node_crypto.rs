// @trace REQ-ENG-007 [entity:BaoRuntime]
use ::std::cell::RefCell;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use bao_engine::context::RawValueRootGuard;
use bun_core::ZBox;

use bun_sha_hmac;
use bun_sha_hmac::hmac::EVP_MAX_MD_SIZE;
use core::ptr;
use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

thread_local! {
    static HASH_DATA: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static HASH_ALGO: RefCell<String> = const { RefCell::new(String::new()) };
    static HMAC_ALGO: RefCell<String> = const { RefCell::new(String::new()) };
    static HMAC_KEY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static HMAC_DATA: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Define a numeric constant property on a JS object (used for crypto.constants).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_constant_number(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: *const i8,
    val: f64,
) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    rooted!(&in(cx_ref) let v = mozjs::jsval::DoubleValue(val));
    JS_DefineProperty(
        cx,
        obj_root.handle().into(),
        name,
        v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let crypto_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if crypto_obj.get().is_null() {
        return;
    }

    unsafe {
        // --- Core hash / HMAC / random ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createHash".as_ptr(),
            Some(crypto_create_hash),
            1,
            JSPROP_ENUMERATE as u32,
        );
        // @trace REQ-ENG-007 [api:crypto.Hash] — Node also exposes the Hash
        // class form: `new crypto.Hash(algorithm)` is equivalent to
        // createHash(algorithm) (deprecated in Node but load-bearing for
        // upstream code that does `new (require("crypto").Hash)("sha256")`).
        // JSFUN_CONSTRUCTOR so `new Hash(...)` routes here; the instance gets
        // Hash.prototype as its prototype so instanceof holds.
        let hash_ctor_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(crypto_hash_ctor),
            1,
            JSFUN_CONSTRUCTOR,
            c"Hash".as_ptr(),
        );
        if !hash_ctor_fn.is_null() {
            let hash_ctor_obj = JS_GetFunctionObject(hash_ctor_fn);
            rooted!(&in(cx) let hc = hash_ctor_obj);
            // Native constructors need an explicit object `prototype` —
            // `new Hash(...)` resolves `this` from it (same pattern as
            // vm.Script).
            rooted!(&in(cx) let proto = unsafe { w2::JS_NewPlainObject(cx) });
            if !proto.get().is_null() {
                rooted!(&in(cx) let pv = mozjs::jsval::ObjectValue(proto.get()));
                JS_DefineProperty(
                    cx.raw_cx(),
                    hc.handle().into(),
                    c"prototype".as_ptr(),
                    pv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            rooted!(&in(cx) let hv = mozjs::jsval::ObjectValue(hash_ctor_obj));
            JS_DefineProperty(
                cx.raw_cx(),
                crypto_obj.handle().into(),
                c"Hash".as_ptr(),
                hv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createHmac".as_ptr(),
            Some(crypto_create_hmac),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"randomBytes".as_ptr(),
            Some(crypto_random_bytes),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"randomUUID".as_ptr(),
            Some(crypto_random_uuid),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getRandomValues".as_ptr(),
            Some(crypto_get_random_values),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"randomInt".as_ptr(),
            Some(crypto_random_int),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"randomFill".as_ptr(),
            Some(crypto_random_fill),
            4,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"randomFillSync".as_ptr(),
            Some(crypto_random_fill_sync),
            3,
            JSPROP_ENUMERATE as u32,
        );

        // --- KDF ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"pbkdf2Sync".as_ptr(),
            Some(crypto_pbkdf2_sync),
            5,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"pbkdf2".as_ptr(),
            Some(crypto_pbkdf2),
            6,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"scryptSync".as_ptr(),
            Some(crypto_scrypt_sync),
            5,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"scrypt".as_ptr(),
            Some(crypto_scrypt),
            5,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"hkdfSync".as_ptr(),
            Some(crypto_hkdf_sync),
            5,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"hkdf".as_ptr(),
            Some(crypto_hkdf),
            6,
            JSPROP_ENUMERATE as u32,
        );

        // --- Cipher ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createCipheriv".as_ptr(),
            Some(crypto_create_cipher_iv),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createDecipheriv".as_ptr(),
            Some(crypto_create_decipher_iv),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getCiphers".as_ptr(),
            Some(crypto_get_ciphers),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getCipherInfo".as_ptr(),
            Some(crypto_get_cipher_info),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // --- Hash info ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getHashes".as_ptr(),
            Some(crypto_get_hashes),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getCurves".as_ptr(),
            Some(crypto_get_curves),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"hash".as_ptr(),
            Some(crypto_hash),
            3,
            JSPROP_ENUMERATE as u32,
        );

        // --- Sign / Verify ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createSign".as_ptr(),
            Some(crypto_create_sign),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createVerify".as_ptr(),
            Some(crypto_create_verify),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"sign".as_ptr(),
            Some(crypto_sign_sync),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"verify".as_ptr(),
            Some(crypto_verify_sync),
            4,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"timingSafeEqual".as_ptr(),
            Some(crypto_timing_safe_equal),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // --- Key generation / KeyObject ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createSecretKey".as_ptr(),
            Some(crypto_create_secret_key),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createPublicKey".as_ptr(),
            Some(crypto_create_public_key),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createPrivateKey".as_ptr(),
            Some(crypto_create_private_key),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"KeyObject".as_ptr(),
            Some(crypto_key_object),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"generateKeyPairSync".as_ptr(),
            Some(crypto_generate_key_pair_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"generateKeyPair".as_ptr(),
            Some(crypto_generate_key_pair),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"generateKey".as_ptr(),
            Some(crypto_generate_key),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"generateKeySync".as_ptr(),
            Some(crypto_generate_key_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // --- RSA encrypt/decrypt ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"publicEncrypt".as_ptr(),
            Some(crypto_public_encrypt),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"publicDecrypt".as_ptr(),
            Some(crypto_public_decrypt),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"privateEncrypt".as_ptr(),
            Some(crypto_private_encrypt),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"privateDecrypt".as_ptr(),
            Some(crypto_private_decrypt),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // --- ECDH ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createECDH".as_ptr(),
            Some(crypto_create_ecdh),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // --- DH ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createDiffieHellman".as_ptr(),
            Some(crypto_create_dh),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"createDiffieHellmanGroup".as_ptr(),
            Some(crypto_diffie_hellman_group),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getDiffieHellman".as_ptr(),
            Some(crypto_diffie_hellman_group),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"diffieHellman".as_ptr(),
            Some(crypto_diffie_hellman),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // --- X509 ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"X509Certificate".as_ptr(),
            Some(crypto_x509_certificate),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"X509".as_ptr(),
            Some(crypto_x509),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // --- Certificate (SPKAC) ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"Certificate".as_ptr(),
            Some(crypto_certificate_ctor),
            0,
            JSPROP_ENUMERATE as u32,
        );

        // --- Prime ---
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"generatePrimeSync".as_ptr(),
            Some(crypto_generate_prime_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"generatePrime".as_ptr(),
            Some(crypto_generate_prime),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"checkPrime".as_ptr(),
            Some(crypto_check_prime),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"checkPrimeSync".as_ptr(),
            Some(crypto_check_prime_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // --- crypto.constants ---
        {
            let mut wrapped_cx2 =
                mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
            let cx2 = &mut wrapped_cx2;
            rooted!(&in(cx2) let constants_obj = w2::JS_NewPlainObject(cx2));
            if !constants_obj.get().is_null() {
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_ALL".as_ptr(),
                    0x80000404u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_ALLOW_UNSAFE_LEGACY_RENEGOTIATION".as_ptr(),
                    0x00000400u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_NO_SSLv2".as_ptr(),
                    0x0u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_NO_SSLv3".as_ptr(),
                    0x02000000u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_NO_TLSv1".as_ptr(),
                    0x04000000u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_NO_TLSv1_1".as_ptr(),
                    0x08000000u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_NO_TLSv1_2".as_ptr(),
                    0x10000000u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"SSL_OP_NO_TLSv1_3".as_ptr(),
                    0x20000000u32 as f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"RSA_PKCS1_PADDING".as_ptr(),
                    1f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"RSA_PKCS1_OAEP_PADDING".as_ptr(),
                    4f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"RSA_NO_PADDING".as_ptr(),
                    3f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"RSA_PKCS1_PSS_PADDING".as_ptr(),
                    6f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"POINT_CONVERSION_UNCOMPRESSED".as_ptr(),
                    4f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"POINT_CONVERSION_COMPRESSED".as_ptr(),
                    2f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"POINT_CONVERSION_HYBRID".as_ptr(),
                    6f64,
                );
                define_constant_number(
                    cx.raw_cx(),
                    constants_obj.get(),
                    c"OPENSSL_VERSION_NUMBER".as_ptr(),
                    0x1010107fu64 as f64,
                );
                rooted!(&in(cx) let const_val = mozjs::jsval::ObjectValue(constants_obj.get()));
                JS_DefineProperty(
                    cx.raw_cx(),
                    crypto_obj.handle().into(),
                    c"constants".as_ptr(),
                    const_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // --- subtle + webcrypto ---
        let mut subtle = UndefinedValue();
        let global = CurrentGlobalOrNull(cx.raw_cx());
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let mut global_crypto = UndefinedValue();
            JS_GetProperty(
                cx.raw_cx(),
                global_root.handle().into(),
                c"crypto".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut global_crypto,
                },
            );
            if global_crypto.is_object() {
                rooted!(&in(cx) let crypto_global = global_crypto.to_object());
                JS_GetProperty(
                    cx.raw_cx(),
                    crypto_global.handle().into(),
                    c"subtle".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut subtle,
                    },
                );
                // webcrypto = globalThis.crypto
                let mut webcrypto_val = UndefinedValue();
                JS_GetProperty(
                    cx.raw_cx(),
                    global_root.handle().into(),
                    c"crypto".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut webcrypto_val,
                    },
                );
                if webcrypto_val.is_object() {
                    rooted!(&in(cx) let webcrypto_rooted = webcrypto_val);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        crypto_obj.handle().into(),
                        c"webcrypto".as_ptr(),
                        webcrypto_rooted.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
        }
        if subtle.is_object() {
            rooted!(&in(cx) let subtle_rooted = subtle);
            JS_DefineProperty(
                cx.raw_cx(),
                crypto_obj.handle().into(),
                c"subtle".as_ptr(),
                subtle_rooted.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    cache_builtin(cx, "crypto", crypto_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn arg_to_string(cx: *mut JSContext, val: JSVal) -> Option<String> {
    if val.is_undefined() || val.is_null() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let val_root = val);
    let s = mozjs::rust::ToString(&mut wrapped_cx, val_root.handle().into());
    if s.is_null() {
        return None;
    }
    Some(crate::jsstr_to_rust_string(cx, s))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_string(cx: *mut JSContext, args: &CallArgs, s: &str) -> bool {
    let c_str = ZBox::from_bytes(s.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if js_str.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::StringValue(&*js_str));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_type_error(cx: *mut JSContext, msg: &str) -> bool {
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    false
}

// --- createHash ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_hash(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return throw_type_error(cx, "createHash() requires an algorithm name");
    }
    let algo = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "createHash() algorithm must be a string"),
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let hash_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if hash_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    HASH_ALGO.with(|a| *a.borrow_mut() = algo);
    HASH_DATA.with(|d| d.borrow_mut().clear());

    attach_hash_methods(cx_ref, hash_obj.handle());

    args.rval().set(mozjs::jsval::ObjectValue(hash_obj.get()));
    true
}

/// Attach the update/digest/copy surface to a hash instance object.
unsafe fn attach_hash_methods(
    cx: &mut mozjs::context::JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) {
    w2::JS_DefineFunction(
        cx,
        obj,
        c"update".as_ptr(),
        Some(hash_update),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx,
        obj,
        c"digest".as_ptr(),
        Some(hash_digest),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx,
        obj,
        c"copy".as_ptr(),
        Some(hash_copy),
        0,
        JSPROP_ENUMERATE as u32,
    );
}

/// `new crypto.Hash(algorithm)` — class form of createHash. Native
/// constructors receive a MAGIC `thisv` while constructing (never the created
/// object), so the instance is the createHash object re-prototyped from the
/// explicitly-defined `Hash.prototype` (vm.Script pattern), making
/// `h instanceof crypto.Hash` hold.
///
/// NOTE: `CallArgs::callee()` and `rval()` alias the SAME vp slot in this
/// engine's CallArgs layout, so the prototype MUST be read off the callee
/// BEFORE `crypto_create_hash` overwrites rval with the instance.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_hash_ctor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Phase 1 (rval untouched): read Hash.prototype off the constructor.
    let mut proto_val = UndefinedValue();
    {
        let pre = CallArgs::from_vp(vp, argc);
        let callee = pre.callee();
        if !callee.is_null() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let ctor = callee);
            JS_GetProperty(
                cx,
                ctor.handle().into(),
                c"prototype".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut proto_val,
                },
            );
        }
    }

    // Phase 2: validate + initialise the shared hash state (TLS algo/data)
    // and build the plain createHash instance (update/digest/copy attached).
    if !crypto_create_hash(cx, argc, vp) {
        return false;
    }
    let args = CallArgs::from_vp(vp, argc);
    if !(*args.rval().ptr).is_object() || !proto_val.is_object() {
        return true;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let instance = (*args.rval().ptr).to_object());
    rooted!(&in(cx_ref) let proto = proto_val.to_object());
    JS_SetPrototype(cx, instance.handle().into(), proto.handle().into());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn hash_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return throw_type_error(cx, "hash.update() requires data");
    }

    let this = args.thisv();
    let input = *args.get(0).ptr;
    // @trace REQ-ENG-007 [api:crypto.hash.update] — Node.js accepts string,
    // Buffer/Uint8Array/TypedArray, DataView, and ArrayBuffer. Strings are
    // taken as-is (raw bytes); typed arrays are read by their byte view.
    // buffer.test.js "truncation after decode" drives update(Buffer.from(...)).
    // Node.js also honors hash.update(str, inputEncoding) — decode the string
    // per the optional 2nd argument (BUG-ENG-CIPHER-ENC class fix).
    let input_encoding = if input.is_string() && argc >= 2 {
        arg_to_string(cx, *args.get(1).ptr)
            .map(|s| s.to_lowercase())
            .filter(|s| {
                matches!(
                    s.as_str(),
                    "hex"
                        | "base64"
                        | "base64url"
                        | "utf8"
                        | "utf-8"
                        | "utf-16le"
                        | "latin1"
                        | "ascii"
                )
            })
    } else {
        None
    };
    let data = if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        decode_input_string(&s, input_encoding.as_deref())
    } else if input.is_object() {
        let wrapped_cx_obj = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx_obj) let obj_root = input.to_object());
        // Try as Uint8Array / Buffer / TypedArray view.
        let mut length: usize = 0;
        let mut is_shared = false;
        let mut data_ptr: *mut u8 = ptr::null_mut();
        let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
            obj_root.get(),
            &mut length,
            &mut is_shared,
            &mut data_ptr,
        );
        if !unwrapped.is_null() && !data_ptr.is_null() && length > 0 {
            let slice = ::std::slice::from_raw_parts(data_ptr, length);
            slice.to_vec()
        } else if length == 0 && !unwrapped.is_null() {
            Vec::new()
        } else {
            // Try ArrayBufferView (DataView, Int32Array, etc.).
            let mut view_length: usize = 0;
            let mut view_shared = false;
            let mut view_data: *mut u8 = ptr::null_mut();
            let view_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
                obj_root.get(),
                &mut view_length,
                &mut view_shared,
                &mut view_data,
            );
            if !view_unwrapped.is_null() && !view_data.is_null() && view_length > 0 {
                let slice = ::std::slice::from_raw_parts(view_data, view_length);
                slice.to_vec()
            } else if !view_unwrapped.is_null() {
                Vec::new()
            } else {
                // Try plain ArrayBuffer via JS::GetObjectAsArrayBuffer.
                let mut ab_length: usize = 0;
                let mut ab_data: *mut u8 = ptr::null_mut();
                let ab_unwrapped = mozjs_sys::jsapi::JS::GetObjectAsArrayBuffer(
                    obj_root.get(),
                    &mut ab_length,
                    &mut ab_data,
                );
                if !ab_unwrapped.is_null() && !ab_data.is_null() && ab_length > 0 {
                    let slice = ::std::slice::from_raw_parts(ab_data, ab_length);
                    slice.to_vec()
                } else if !ab_unwrapped.is_null() {
                    Vec::new()
                } else {
                    return throw_type_error(
                        cx,
                        "hash.update() data must be a string, Buffer, TypedArray, or DataView",
                    );
                }
            }
        }
    } else {
        return throw_type_error(
            cx,
            "hash.update() data must be a string, Buffer, TypedArray, or DataView",
        );
    };

    HASH_DATA.with(|d| d.borrow_mut().extend_from_slice(&data));
    args.rval().set(*this.ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn hash_digest(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let encoding = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.to_lowercase(),
            None => "hex".to_string(),
        }
    } else {
        "hex".to_string()
    };

    let algo = HASH_ALGO.with(|a| ::std::mem::take(&mut *a.borrow_mut()));
    let data = HASH_DATA.with(|d| ::std::mem::take(&mut *d.borrow_mut()));

    let result = match algo.as_str() {
        "sha256" => {
            let mut h = bun_sha_hmac::SHA256::init();
            h.update(&data);
            let mut out = [0u8; bun_sha_hmac::SHA256::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha512" => {
            let mut h = bun_sha_hmac::SHA512::init();
            h.update(&data);
            let mut out = [0u8; bun_sha_hmac::SHA512::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha384" => {
            let mut h = bun_sha_hmac::SHA384::init();
            h.update(&data);
            let mut out = [0u8; bun_sha_hmac::SHA384::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha224" => {
            let mut h = bun_sha_hmac::SHA224::init();
            h.update(&data);
            let mut out = [0u8; bun_sha_hmac::SHA224::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha1" => {
            let mut h = bun_sha_hmac::SHA1::init();
            h.update(&data);
            let mut out = [0u8; bun_sha_hmac::SHA1::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "md5" => {
            let mut h = bun_sha_hmac::MD5::init();
            h.update(&data);
            let mut out = [0u8; bun_sha_hmac::MD5::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        _ => {
            return throw_type_error(cx, &format!("Unsupported hash algorithm: {}", algo));
        }
    };

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes)
                .unwrap_or("")
                .to_owned();
            return_string(cx, &args, &encoded)
        }
        _ => return_string(cx, &args, &hex::encode(&result)),
    }
}

/// Hash .copy() — creates a new Hash with the same algorithm and current state.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn hash_copy(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // Re-create a hash with the same algo; the internal state is thread-local
    // so the copy will start with the same accumulated data.
    let algo = HASH_ALGO.with(|a| a.borrow().clone());
    let data = HASH_DATA.with(|d| d.borrow().clone());
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let hash_obj = w2::JS_NewPlainObject(cx_ref));
    if hash_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    HASH_ALGO.with(|a| *a.borrow_mut() = algo);
    HASH_DATA.with(|d| *d.borrow_mut() = data);
    w2::JS_DefineFunction(
        cx_ref,
        hash_obj.handle(),
        c"update".as_ptr(),
        Some(hash_update),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        hash_obj.handle(),
        c"digest".as_ptr(),
        Some(hash_digest),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        hash_obj.handle(),
        c"copy".as_ptr(),
        Some(hash_copy),
        0,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(hash_obj.get()));
    true
}

// --- createHmac ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_hmac(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        return throw_type_error(cx, "createHmac() requires algorithm and key");
    }
    let algo = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "createHmac() algorithm must be a string"),
    };
    // Key material as BYTES (BCE: routing a Buffer key through string
    // coercion mangled bytes ≥ 0x80 into UTF-8 replacement chars — a silent
    // WRONG mac). Node shapes: string (UTF-8 bytes) | Buffer/TypedArray |
    // secret KeyObject.
    let key_val = *args.get(1).ptr;
    let key: Vec<u8> = if key_val.is_string() {
        crate::js_to_rust_string(cx, key_val).into_bytes()
    } else if key_val.is_object() {
        let key_obj = key_val.to_object();
        let mut wrapped_key_cx =
            mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let key_cx_ref = &mut wrapped_key_cx;
        rooted!(&in(key_cx_ref) let key_r = key_obj);
        let mut idx_val = UndefinedValue();
        JS_GetProperty(
            cx,
            key_r.handle().into(),
            c"_keyIdx".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut idx_val,
            },
        );
        if idx_val.is_int32() {
            let ktype = get_string_prop(cx, key_obj, c"type".as_ptr()).unwrap_or_default();
            if ktype != "secret" {
                return throw_type_error(cx, "createHmac() key KeyObject must be a secret key");
            }
            let idx = idx_val.to_int32() as usize;
            match KEY_OBJECTS.with(|v| {
                v.borrow()
                    .get(idx)
                    .map(|k| k.as_ref().map(|b| b.clone()))
                    .flatten()
            }) {
                Some(b) => b,
                None => {
                    return throw_type_error(cx, "createHmac() KeyObject key data unavailable");
                }
            }
        } else {
            let b = extract_buffer_bytes(cx, key_val);
            if b.is_empty() {
                return throw_type_error(
                    cx,
                    "createHmac() key must be a string, Buffer/TypedArray, or secret KeyObject",
                );
            }
            b
        }
    } else {
        return throw_type_error(
            cx,
            "createHmac() key must be a string, Buffer/TypedArray, or secret KeyObject",
        );
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let hmac_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if hmac_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    HMAC_ALGO.with(|a| *a.borrow_mut() = algo);
    HMAC_KEY.with(|k| *k.borrow_mut() = key);
    HMAC_DATA.with(|d| d.borrow_mut().clear());

    w2::JS_DefineFunction(
        cx_ref,
        hmac_obj.handle(),
        c"update".as_ptr(),
        Some(hmac_update),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        hmac_obj.handle(),
        c"digest".as_ptr(),
        Some(hmac_digest),
        1,
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(mozjs::jsval::ObjectValue(hmac_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn hmac_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return throw_type_error(cx, "hmac.update() requires data");
    }
    let this = args.thisv();
    let input = *args.get(0).ptr;
    // Node.js: hmac.update(data, inputEncoding). Decode the string per the
    // optional 2nd argument (BUG-ENG-CIPHER-ENC class fix).
    let input_encoding = if input.is_string() && argc >= 2 {
        arg_to_string(cx, *args.get(1).ptr)
            .map(|s| s.to_lowercase())
            .filter(|s| {
                matches!(
                    s.as_str(),
                    "hex"
                        | "base64"
                        | "base64url"
                        | "utf8"
                        | "utf-8"
                        | "utf-16le"
                        | "latin1"
                        | "ascii"
                )
            })
    } else {
        None
    };
    let data = if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        decode_input_string(&s, input_encoding.as_deref())
    } else {
        return throw_type_error(cx, "hmac.update() data must be a string");
    };
    HMAC_DATA.with(|d| d.borrow_mut().extend_from_slice(&data));
    args.rval().set(*this.ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn hmac_digest(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let encoding = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.to_lowercase(),
            None => "hex".to_string(),
        }
    } else {
        "hex".to_string()
    };

    let algo = HMAC_ALGO.with(|a| ::std::mem::take(&mut *a.borrow_mut()));
    let key = HMAC_KEY.with(|k| ::std::mem::take(&mut *k.borrow_mut()));
    let data = HMAC_DATA.with(|d| ::std::mem::take(&mut *d.borrow_mut()));

    let result: Vec<u8> = match algo.as_str() {
        "sha256" => {
            let mut out = [0u8; EVP_MAX_MD_SIZE];
            bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha256, &mut out)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        "sha512" => {
            let mut out = [0u8; EVP_MAX_MD_SIZE];
            bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha512, &mut out)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        // Node supports the full SHA-2 family in createHmac; sha384/sha224
        // were missing (createHmac('sha384') threw "Unsupported").
        "sha384" => {
            let mut out = [0u8; EVP_MAX_MD_SIZE];
            bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha384, &mut out)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        "sha224" => {
            let mut out = [0u8; EVP_MAX_MD_SIZE];
            bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha224, &mut out)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        "sha1" => {
            let mut out = [0u8; EVP_MAX_MD_SIZE];
            bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha1, &mut out)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        "md5" => {
            // Node.js supports HMAC-MD5 (createHmac("md5", key)). Digest is 16
            // bytes → 32 hex chars. Routed through BoringSSL EVP_md5 via the
            // shared bun_sha_hmac::generate path (no algorithm reinvented).
            let mut out = [0u8; EVP_MAX_MD_SIZE];
            bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Md5, &mut out)
                .map(|s| s.to_vec())
                .unwrap_or_default()
        }
        _ => {
            return throw_type_error(cx, &format!("Unsupported HMAC algorithm: {}", algo));
        }
    };

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes)
                .unwrap_or("")
                .to_owned();
            return_string(cx, &args, &encoded)
        }
        _ => return_string(cx, &args, &hex::encode(&result)),
    }
}

// --- randomBytes ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_bytes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return throw_type_error(cx, "randomBytes() requires a size");
    }
    let size_val = *args.get(0).ptr;
    let size = if size_val.is_int32() {
        size_val.to_int32() as usize
    } else if size_val.is_double() {
        size_val.to_double() as usize
    } else {
        return throw_type_error(cx, "randomBytes() size must be a number");
    };

    let mut bytes = vec![0u8; size];
    // Use BoringSSL CSPRNG instead of rand
    bao_crypto::random::rand_bytes(&mut bytes).unwrap();

    // @trace REQ-ENG-006 [entity:Buffer]
    // Node.js: crypto.randomBytes returns a Buffer instance, not a plain
    // array/object. Build a real Buffer via the shared globals helper so that
    // `Buffer.isBuffer(crypto.randomBytes(N)) === true`.
    let buf_obj = crate::globals::create_buffer_object(cx, &bytes);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    true
}

// --- pbkdf2Sync ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_pbkdf2_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 5 {
        return throw_type_error(
            cx,
            "pbkdf2Sync() requires (password, salt, iterations, keylen, digest)",
        );
    }

    let password = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.into_bytes(),
        None => return throw_type_error(cx, "pbkdf2Sync() password must be a string"),
    };
    let salt = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => return throw_type_error(cx, "pbkdf2Sync() salt must be a string"),
    };
    let iterations = {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            v.to_int32() as u32
        } else {
            return throw_type_error(cx, "pbkdf2Sync() iterations must be a number");
        }
    };
    let key_len = {
        let v = *args.get(3).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else {
            return throw_type_error(cx, "pbkdf2Sync() keylen must be a number");
        }
    };
    let digest_name = match arg_to_string(cx, *args.get(4).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "pbkdf2Sync() digest must be a string"),
    };

    // @trace REQ-ENG-007 [entity:bao_crypto] DEC-ENG-003: pbkdf2 routed to
    // bao_crypto::kdf (sha_hmac::pbkdf2 removed). Supports sha1/sha256/sha512.
    let pbkdf2_hash = match bao_crypto::kdf::parse_pbkdf2_hash(&digest_name) {
        Ok(h) => h,
        Err(_) => {
            return throw_type_error(cx, &format!("Unsupported PBKDF2 digest: {}", digest_name));
        }
    };
    let result = match bao_crypto::kdf::pbkdf2(&password, &salt, iterations, pbkdf2_hash, key_len) {
        Ok(out) => out,
        Err(e) => return throw_type_error(cx, &format!("pbkdf2Sync() derivation failed: {}", e)),
    };

    // @trace REQ-ENG-007 [api:crypto.pbkdf2Sync] — Node returns a Buffer, not
    // an Array. The previous Array-of-ints return silently broke every Buffer
    // consumer (`.toString("hex")` missing, `Buffer.isBuffer()` false).
    // Same surface as the async pbkdf2() path (create_buffer_object).
    let buf_obj = crate::globals::create_buffer_object(cx, &result);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    true
}

// --- scryptSync ---

/// Read a numeric property off a JS options object. Returns `default` when the
/// object or the property is absent. Accepts int32/double per Node semantics.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_num_prop(
    cx: *mut JSContext,
    obj: Handle<*mut JSObject>,
    name: *const ::std::os::raw::c_char,
    default: u64,
) -> u64 {
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.into(),
        name,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_int32() {
        let n = v.to_int32();
        if n >= 0 {
            return n as u64;
        }
    } else if v.is_double() {
        let d = v.to_double();
        if d >= 0.0 && d.is_finite() {
            return d as u64;
        }
    }
    default
}

/// Parse scrypt options (Node `crypto.scryptSync(pw, salt, keylen[, options])`).
/// Recognises `N`/`cost`, `r`/`blocksize`, `p`/`parallelization`, and `maxmem`
/// (accepted for API compatibility; BoringSSL enforces its own memory bound).
/// Defaults follow Node: N=16384, r=8, p=1.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_scrypt_options(cx: *mut JSContext, val: JSVal) -> (u64, u64, u64) {
    const DEFAULTS: (u64, u64, u64) = (16384, 8, 1);
    if !val.is_object() {
        return DEFAULTS;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = val.to_object());

    let n = read_num_prop(cx_ref.raw_cx(), obj.handle().into(), c"N".as_ptr(), 0);
    let n = if n == 0 {
        read_num_prop(cx_ref.raw_cx(), obj.handle().into(), c"cost".as_ptr(), 16384)
    } else {
        n
    };
    let r = read_num_prop(cx_ref.raw_cx(), obj.handle().into(), c"r".as_ptr(), 0);
    let r = if r == 0 {
        read_num_prop(cx_ref.raw_cx(), obj.handle().into(), c"blocksize".as_ptr(), 8)
    } else {
        r
    };
    let p = read_num_prop(cx_ref.raw_cx(), obj.handle().into(), c"p".as_ptr(), 0);
    let p = if p == 0 {
        read_num_prop(cx_ref.raw_cx(), obj.handle().into(), c"parallelization".as_ptr(), 1)
    } else {
        p
    };
    (n, r, p)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_scrypt_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 {
        return throw_type_error(cx, "scryptSync() requires (password, salt, keylen)");
    }

    // Node accepts string | ArrayBuffer | TypedArray | DataView for both
    // password and salt. Strings are UTF-8 encoded.
    let password = if (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr).into_bytes()
    } else if (*args.get(0).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        return throw_type_error(cx, "scryptSync() password must be a string or Buffer");
    };
    let salt = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr).into_bytes()
    } else if (*args.get(1).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        return throw_type_error(cx, "scryptSync() salt must be a string or Buffer");
    };
    let key_len = {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            let n = v.to_int32();
            if n <= 0 {
                return throw_type_error(cx, "scryptSync() keylen must be > 0");
            }
            n as usize
        } else if v.is_double() {
            let d = v.to_double();
            if !(d > 0.0 && d.is_finite()) {
                return throw_type_error(cx, "scryptSync() keylen must be > 0");
            }
            d as usize
        } else {
            return throw_type_error(cx, "scryptSync() keylen must be a number");
        }
    };

    let (n, r, p) = if argc > 3 {
        parse_scrypt_options(cx, *args.get(3).ptr)
    } else {
        (16384, 8, 1)
    };

    // @trace REQ-ENG-007 [api:node:crypto scryptSync] [entity:bao_crypto]
    // BCE (v-surface P0-1): the Ok(Vec<u8>) from bao_crypto::kdf::scrypt was
    // discarded and a pre-zeroed `vec![0u8; key_len]` returned — every key was
    // all-zero bytes. The derivation output IS the return value; use it.
    let derived = match bao_crypto::kdf::scrypt(&password, &salt, n, r, p, key_len) {
        Ok(out) => out,
        Err(e) => return throw_type_error(cx, &format!("scryptSync() failed: {}", e)),
    };

    // Node returns a Buffer instance.
    let buf_obj = crate::globals::create_buffer_object(cx, &derived);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    true
}

// --- randomUUID ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_uuid(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let uuid = uuid_v4();
    return_string(cx, &args, &uuid)
}

fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    // Use BoringSSL CSPRNG instead of rand
    bao_crypto::random::rand_bytes(&mut bytes).unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

// --- getRandomValues ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_random_values(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return throw_type_error(cx, "getRandomValues() requires a typed array");
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = (*args.get(0).ptr).to_object());

    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        arr.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let len = if len_val.is_int32() {
        len_val.to_int32() as usize
    } else {
        return throw_type_error(cx, "getRandomValues() invalid array");
    };

    let mut random_bytes = vec![0u8; len];
    // Use BoringSSL CSPRNG instead of rand
    bao_crypto::random::rand_bytes(&mut random_bytes).unwrap();

    for (i, &byte) in random_bytes.iter().enumerate() {
        rooted!(&in(cx_ref) let v = mozjs::jsval::Int32Value(byte as i32));
        JS_SetElement(cx, arr.handle().into(), i as u32, v.handle().into());
    }

    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

// --- createCipheriv / createDecipheriv ---
// @trace REQ-ENG-007 [entity:BaoRuntime] [api:node:crypto createCipheriv/createDecipheriv]
// Real BoringSSL ciphers via bao_crypto::cipher. Per-instance state stored in a
// thread-local registry keyed by a serial-number hidden on the JS object, so two
// concurrent cipher objects have independent state (required by test_crypto_cipher.js).
//
// Node.js encoding contract (BUG-ENG-CIPHER-ENC fix):
//   cipher.update(data)                                -> Buffer
//   cipher.update(data, inputEncoding)                 -> Buffer   (string data decoded)
//   cipher.update(data, inputEncoding, outputEncoding) -> string   (output encoded)
//   cipher.final()                                     -> Buffer
//   cipher.final(outputEncoding)                       -> string
// When no output encoding is given, a real Buffer instance is returned (so
// Buffer.isBuffer works); with an output encoding the result is a string.
// AEAD (AES-GCM/ChaCha20-Poly1305) exposes getAuthTag()/setAuthTag()/setAAD().

/// Decode a JS string argument into bytes per `input_encoding`.
/// - None / "utf8" / "utf-8" / "buffer": raw UTF-8 bytes.
/// - "hex": hex-decode (invalid chars become a decode error -> raw bytes fallback).
/// - "base64": base64-decode.
/// Mirrors Node.js `Buffer.from(str, encoding)` semantics for cipher inputs.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn decode_input_string(s: &str, input_encoding: Option<&str>) -> Vec<u8> {
    match input_encoding {
        Some("hex") => hex::decode(s).unwrap_or_else(|_| s.as_bytes().to_vec()),
        Some("base64") => {
            bun_base64::decode_alloc(s.as_bytes()).unwrap_or_else(|_| s.as_bytes().to_vec())
        }
        // utf8 / utf-8 / buffer / latin1 / ascii: raw bytes (latin1/ascii map 1:1).
        _ => s.as_bytes().to_vec(),
    }
}

/// Produce the JS return value for cipher output bytes per `output_encoding`.
/// - None: a real Buffer instance (Buffer.isBuffer === true).
/// - "hex"/"base64"/"utf8"/...: an encoded string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn encode_output_bytes(
    cx: *mut JSContext,
    args: &CallArgs,
    bytes: &[u8],
    output_encoding: Option<&str>,
) -> bool {
    match output_encoding {
        Some("hex") => return_string(cx, args, &hex::encode(bytes)),
        Some("base64") => {
            let encoded_bytes = bun_base64::encode_alloc(bytes);
            let encoded = ::std::str::from_utf8(&encoded_bytes)
                .unwrap_or("")
                .to_owned();
            return_string(cx, args, &encoded)
        }
        Some("utf8") | Some("utf-8") | Some("utf-16le") | Some("latin1") | Some("ascii") => {
            // For string-like encodings, decode the bytes as UTF-8 lossily
            // (Node returns a string for these output encodings).
            let s = String::from_utf8_lossy(bytes);
            return_string(cx, args, &s)
        }
        Some(_) => {
            // Unknown encoding: default to hex (Node throws, but we are lenient).
            return_string(cx, args, &hex::encode(bytes))
        }
        None => {
            // No output encoding: return a real Buffer (Node.js contract).
            let buf_obj = crate::globals::create_buffer_object(cx, bytes);
            if buf_obj.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
            }
            true
        }
    }
}

thread_local! {
    static CIPHER_REGISTRY: RefCell<Vec<Option<bao_crypto::cipher::CipherCtx>>> =
        const { RefCell::new(Vec::new()) };
    static CIPHER_NEXT_ID: RefCell<u32> = const { RefCell::new(1) };
}

fn cipher_registry_insert(ctx: bao_crypto::cipher::CipherCtx) -> u32 {
    let id = CIPHER_NEXT_ID.with(|next| {
        let id = *next.borrow();
        *next.borrow_mut() = id.wrapping_add(1);
        id
    });
    let idx = id as usize;
    CIPHER_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        if idx >= reg.len() {
            let extra = idx + 1 - reg.len();
            reg.reserve(extra);
            while reg.len() <= idx {
                reg.push(None);
            }
        }
        reg[idx] = Some(ctx);
    });
    id
}

#[allow(dead_code)]
fn cipher_registry_take(id: u32) -> Option<bao_crypto::cipher::CipherCtx> {
    CIPHER_REGISTRY.with(|reg| reg.borrow_mut().get_mut(id as usize).and_then(|s| s.take()))
}

fn cipher_registry_with_mut<R>(
    id: u32,
    f: &mut dyn FnMut(&mut bao_crypto::cipher::CipherCtx) -> R,
) -> Option<R> {
    CIPHER_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        match reg.get_mut(id as usize).and_then(|s| s.as_mut()) {
            Some(ctx) => Some(f(ctx)),
            None => None,
        }
    })
}

fn cipher_registry_remove(id: u32) {
    CIPHER_REGISTRY.with(|reg| {
        if let Some(slot) = reg.borrow_mut().get_mut(id as usize) {
            *slot = None;
        }
    });
}

/// Extract key/iv bytes from a JS value: a JS string yields its UTF-8 bytes;
/// everything else (Uint8Array/Buffer/TypedArray/number[]) yields the raw
/// element bytes via extract_buffer_bytes. This avoids coercing a number[]
/// key to a comma-joined decimal string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_key_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    if val.is_string() {
        crate::js_to_rust_string(cx, val).into_bytes()
    } else {
        extract_buffer_bytes(cx, val)
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn extract_buffer_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    if !val.is_object() {
        return Vec::new();
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = val.to_object());
    // Prefer Uint8Array/TypedArray/ArrayBuffer fast paths (Buffer/Uint8Array).
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ptr::null_mut();
    let u8_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        obj_root.get(),
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if !u8_unwrapped.is_null() && !data_ptr.is_null() && length > 0 {
        let slice = ::std::slice::from_raw_parts(data_ptr, length);
        return slice.to_vec();
    } else if !u8_unwrapped.is_null() {
        return Vec::new();
    }
    let mut view_length: usize = 0;
    let mut view_shared = false;
    let mut view_data: *mut u8 = ptr::null_mut();
    let view_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
        obj_root.get(),
        &mut view_length,
        &mut view_shared,
        &mut view_data,
    );
    if !view_unwrapped.is_null() && !view_data.is_null() && view_length > 0 {
        let slice = ::std::slice::from_raw_parts(view_data, view_length);
        return slice.to_vec();
    } else if !view_unwrapped.is_null() {
        return Vec::new();
    }
    // Plain number[] array fallback.
    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let len = if len_val.is_int32() {
        len_val.to_int32() as usize
    } else {
        return Vec::new();
    };
    let mut bytes = Vec::with_capacity(len);
    for i in 0u32..len as u32 {
        let mut byte_val = UndefinedValue();
        JS_GetElement(
            cx,
            obj_root.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut byte_val,
            },
        );
        bytes.push(if byte_val.is_int32() {
            byte_val.to_int32() as u8
        } else {
            0
        });
    }
    bytes
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bytes_to_js_array(cx: *mut JSContext, bytes: &[u8]) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, bytes.len()));
    if arr.get().is_null() {
        return ptr::null_mut();
    }
    for (i, &byte) in bytes.iter().enumerate() {
        let val = mozjs::jsval::Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v = val);
        JS_DefineElement(
            cx,
            arr.handle().into(),
            i as u32,
            v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    arr.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn store_cipher_id(cx: *mut JSContext, obj: *mut JSObject, id: u32) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let id_val = mozjs::jsval::Int32Value(id as i32);
    rooted!(&in(cx_ref) let idv = id_val);
    JS_DefineProperty(
        cx,
        obj_rooted.handle().into(),
        c"__bao_cipher_id".as_ptr(),
        idv.handle().into(),
        0, // not enumerable, not writable, not configurable
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_cipher_id(cx: *mut JSContext, obj: *mut JSObject) -> Option<u32> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let mut id_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_rooted.handle().into(),
        c"__bao_cipher_id".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut id_val,
        },
    );
    if id_val.is_int32() {
        Some(id_val.to_int32() as u32)
    } else {
        None
    }
}

unsafe fn read_cipher_id_from_this(cx: *mut JSContext, args: &CallArgs) -> Option<u32> {
    let this = args.thisv();
    if !this.is_object() {
        return None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_root = this.to_object());
    read_cipher_id(cx, this_root.get())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_cipher_iv(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 {
        return throw_type_error(cx, "createCipheriv() requires (algorithm, key, iv)");
    }
    let algo_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "createCipheriv() algorithm must be a string"),
    };
    let key = extract_key_bytes(cx, *args.get(1).ptr);
    let iv = extract_key_bytes(cx, *args.get(2).ptr);

    let algo = match bao_crypto::cipher::parse_algorithm(&algo_name) {
        Ok(a) => a,
        Err(_) => return throw_type_error(cx, &format!("Unsupported cipher: {}", algo_name)),
    };
    let ctx = match bao_crypto::cipher::CipherCtx::new(
        algo,
        &key,
        &iv,
        bao_crypto::cipher::Direction::Encrypt,
    ) {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("createCipheriv() init failed: {}", e)),
    };
    let id = cipher_registry_insert(ctx);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if obj.get().is_null() {
        cipher_registry_remove(id);
        args.rval().set(UndefinedValue());
        return true;
    }
    store_cipher_id(cx, obj.get(), id);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"update".as_ptr(),
        Some(cipher_update),
        3,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"final".as_ptr(),
        Some(cipher_final),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getAuthTag".as_ptr(),
        Some(cipher_get_auth_tag),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"setAuthTag".as_ptr(),
        Some(cipher_set_auth_tag),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"setAAD".as_ptr(),
        Some(cipher_set_aad),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_decipher_iv(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 {
        return throw_type_error(cx, "createDecipheriv() requires (algorithm, key, iv)");
    }
    let algo_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "createDecipheriv() algorithm must be a string"),
    };
    let key = extract_key_bytes(cx, *args.get(1).ptr);
    let iv = extract_key_bytes(cx, *args.get(2).ptr);

    let algo = match bao_crypto::cipher::parse_algorithm(&algo_name) {
        Ok(a) => a,
        Err(_) => return throw_type_error(cx, &format!("Unsupported cipher: {}", algo_name)),
    };
    let ctx = match bao_crypto::cipher::CipherCtx::new(
        algo,
        &key,
        &iv,
        bao_crypto::cipher::Direction::Decrypt,
    ) {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("createDecipheriv() init failed: {}", e)),
    };
    let id = cipher_registry_insert(ctx);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if obj.get().is_null() {
        cipher_registry_remove(id);
        args.rval().set(UndefinedValue());
        return true;
    }
    store_cipher_id(cx, obj.get(), id);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"update".as_ptr(),
        Some(cipher_update),
        3,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"final".as_ptr(),
        Some(cipher_final),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getAuthTag".as_ptr(),
        Some(cipher_get_auth_tag),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"setAuthTag".as_ptr(),
        Some(cipher_set_auth_tag),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"setAAD".as_ptr(),
        Some(cipher_set_aad),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_update_args(cx: *mut JSContext, args: &CallArgs, argc: u32) -> Option<Vec<u8>> {
    if argc < 1 {
        throw_type_error(cx, "update() requires data");
        return None;
    }
    let input = *args.get(0).ptr;
    // Second argument is the input encoding (Node.js: update(data, inputEncoding, outputEncoding)).
    // Only meaningful when `data` is a string; ignored for Buffer/TypedArray inputs.
    let input_encoding = if input.is_string() && argc >= 2 {
        arg_to_string(cx, *args.get(1).ptr)
            .map(|s| s.to_lowercase())
            .filter(|s| {
                matches!(
                    s.as_str(),
                    "hex"
                        | "base64"
                        | "base64url"
                        | "utf8"
                        | "utf-8"
                        | "utf-16le"
                        | "latin1"
                        | "ascii"
                )
            })
    } else {
        None
    };
    let data = if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        decode_input_string(&s, input_encoding.as_deref())
    } else if input.is_object() {
        extract_buffer_bytes(cx, input)
    } else {
        Vec::new()
    };
    Some(data)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_cipher_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "cipher.update() invalid receiver"),
    };
    let data = match parse_update_args(cx, &args, argc) {
        Some(d) => d,
        None => return false,
    };
    let out = match cipher_registry_with_mut(id, &mut |ctx| ctx.update(&data)) {
        Some(Ok(bytes)) => bytes,
        Some(Err(e)) => return throw_type_error(cx, &format!("cipher.update() failed: {}", e)),
        None => return throw_type_error(cx, "cipher.update() stale context"),
    };
    // Third argument is the output encoding (Node.js: update(data, inputEnc, outputEnc)).
    let data_val = *args.get(0).ptr;
    let data_is_string = data_val.is_string();
    let output_encoding = if argc >= 3 {
        arg_to_string(cx, *args.get(2).ptr).map(|s| s.to_lowercase())
    } else if argc == 2 && !data_is_string {
        // update(Buffer, outputEncoding): the 2nd arg is the output encoding.
        arg_to_string(cx, *args.get(1).ptr).map(|s| s.to_lowercase())
    } else {
        None
    };
    encode_output_bytes(cx, &args, &out, output_encoding.as_deref())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_final(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_cipher_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "cipher.final() invalid receiver"),
    };
    // Finalize in place; the context stays in the registry so AEAD encrypt can
    // still expose getAuthTag() afterwards. A second final() call hits the
    // already-finalized guard inside CipherCtx::final_ex().
    let result = match cipher_registry_with_mut(id, &mut |ctx| ctx.final_ex()) {
        Some(r) => r,
        None => return throw_type_error(cx, "cipher.final() stale context"),
    };
    let out = match result {
        Ok(bytes) => bytes,
        Err(e) => return throw_type_error(cx, &format!("cipher.final() failed: {}", e)),
    };
    // Optional output encoding (Node.js: final(outputEncoding)).
    let output_encoding = if argc >= 1 {
        arg_to_string(cx, *args.get(0).ptr).map(|s| s.to_lowercase())
    } else {
        None
    };
    encode_output_bytes(cx, &args, &out, output_encoding.as_deref())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_get_auth_tag(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let id = match read_cipher_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "cipher.getAuthTag() invalid receiver"),
    };
    let tag = cipher_registry_with_mut(id, &mut |ctx| ctx.take_auth_tag());
    let tag = match tag {
        Some(Some(t)) => t,
        _ => Vec::new(),
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let _cx_ref = &mut wrapped_cx;
    let arr = bytes_to_js_array(cx, &tag);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_set_auth_tag(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_cipher_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "cipher.setAuthTag() invalid receiver"),
    };
    if argc < 1 {
        return throw_type_error(cx, "cipher.setAuthTag() requires a tag");
    }
    let tag = extract_buffer_bytes(cx, *args.get(0).ptr);
    let res = cipher_registry_with_mut(id, &mut |ctx| ctx.set_auth_tag(&tag));
    match res {
        Some(Ok(())) => {
            args.rval().set(*args.thisv().ptr);
            true
        }
        Some(Err(e)) => throw_type_error(cx, &format!("cipher.setAuthTag() failed: {}", e)),
        None => throw_type_error(cx, "cipher.setAuthTag() stale context"),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_set_aad(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_cipher_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "cipher.setAAD() invalid receiver"),
    };
    if argc < 1 {
        return throw_type_error(cx, "cipher.setAAD() requires data");
    }
    let aad = extract_buffer_bytes(cx, *args.get(0).ptr);
    let res = cipher_registry_with_mut(id, &mut |ctx| ctx.update_aad(&aad));
    match res {
        Some(Ok(())) => {
            args.rval().set(*args.thisv().ptr);
            true
        }
        Some(Err(e)) => throw_type_error(cx, &format!("cipher.setAAD() failed: {}", e)),
        None => throw_type_error(cx, "cipher.setAAD() stale context"),
    }
}

// --- timingSafeEqual ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_timing_safe_equal(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        return throw_type_error(cx, "timingSafeEqual() requires two buffer arguments");
    }
    let a = extract_buffer_bytes(cx, *args.get(0).ptr);
    let b = extract_buffer_bytes(cx, *args.get(1).ptr);
    if a.len() != b.len() {
        return throw_type_error(cx, "timingSafeEqual() inputs must have the same length");
    }
    // @trace REQ-ENG-007 [api:node:crypto timingSafeEqual] real constant-time
    // compare via BoringSSL CRYPTO_memcmp (routed through bun_boringssl_sys).
    let equal = bun_boringssl_sys::constant_time_eq(&a, &b);
    args.rval().set(mozjs::jsval::BooleanValue(equal));
    true
}

// --- getHashes ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_hashes(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let hashes = [
        "sha1",
        "sha224",
        "sha256",
        "sha384",
        "sha512",
        "md5",
        "md4",
        "md2",
        "ripemd160",
    ];
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, hashes.len()));
    if !arr.get().is_null() {
        for (i, name) in hashes.iter().enumerate() {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
                JS_DefineElement(
                    cx,
                    arr.handle().into(),
                    i as u32,
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// --- getCiphers ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_ciphers(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let ciphers = [
        "aes-128-cbc",
        "aes-128-ecb",
        "aes-128-gcm",
        "aes-192-cbc",
        "aes-192-ecb",
        "aes-192-gcm",
        "aes-256-cbc",
        "aes-256-ecb",
        "aes-256-gcm",
        "chacha20-poly1305",
        "aes-128-cfb",
        "aes-256-cfb",
        "aes-128-ctr",
        "aes-256-ctr",
        "des-ede3-cbc",
    ];
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, ciphers.len()));
    if !arr.get().is_null() {
        for (i, name) in ciphers.iter().enumerate() {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
                JS_DefineElement(
                    cx,
                    arr.handle().into(),
                    i as u32,
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// --- createSign / createVerify ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_sign(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let algo = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.to_lowercase(),
            None => "sha256".to_string(),
        }
    } else {
        "sha256".to_string()
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // Per-instance algorithm + accumulated data. BCE (v-surface P0-3): the
    // shared HASH_ALGO/HASH_DATA thread-locals let two interleaved Sign/Verify
    // instances corrupt each other's state; stashing on the instance makes
    // `s1.update(); s2.update(); s1.sign()` correct.
    set_hidden_string_prop(cx, obj.get(), c"_baoAlgo".as_ptr(), &algo);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"update".as_ptr(),
        Some(sign_update),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"sign".as_ptr(),
        Some(sign_sign),
        2,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

/// Store a non-enumerable string property on a JS object (hidden state slot).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_hidden_string_prop(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: *const ::std::os::raw::c_char,
    value: &str,
) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let c_str = ZBox::from_bytes(value.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
        JS_DefineProperty(
            cx,
            obj_root.handle().into(),
            name,
            v.handle().into(),
            0, // non-enumerable, configurable (so take_ can delete it)
        );
    }
}

/// Read a string property off a JS object. Returns None when absent/not a string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: *const ::std::os::raw::c_char,
) -> Option<String> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        name,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_string() {
        Some(crate::js_to_rust_string(cx, v))
    } else {
        None
    }
}

/// Append one chunk of bytes to the instance's accumulated update() data,
/// stored as a non-enumerable array of Buffers on `this` (GC-safe).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn push_instance_data(cx: *mut JSContext, obj: *mut JSObject, bytes: &[u8]) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);

    let chunk = crate::globals::create_buffer_object(cx, bytes);
    if chunk.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let chunk_root = chunk);

    let mut arr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_baoData".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut arr_val,
        },
    );
    let arr_ptr: *mut JSObject = if arr_val.is_object() {
        arr_val.to_object()
    } else {
        let a = w2::NewArrayObject1(cx_ref, 0);
        if a.is_null() {
            return;
        }
        rooted!(&in(cx_ref) let av = mozjs::jsval::ObjectValue(a));
        JS_DefineProperty(
            cx,
            obj_root.handle().into(),
            c"_baoData".as_ptr(),
            av.handle().into(),
            0,
        );
        a
    };
    rooted!(&in(cx_ref) let arr_root = arr_ptr);

    let mut len: u32 = 0;
    if w2::GetArrayLength(cx_ref, arr_root.handle().into(), &mut len) {
        rooted!(&in(cx_ref) let cv = mozjs::jsval::ObjectValue(chunk_root.get()));
        JS_DefineElement(
            cx,
            arr_root.handle().into(),
            len,
            cv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

/// Consume the instance's accumulated update() data and clear it.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn take_instance_data(cx: *mut JSContext, obj: *mut JSObject) -> Vec<u8> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);

    let mut arr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_baoData".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut arr_val,
        },
    );
    // Clear consumed state regardless of how the read goes.
    JS_DeleteProperty1(cx, obj_root.handle().into(), c"_baoData".as_ptr());
    if !arr_val.is_object() {
        return Vec::new();
    }
    rooted!(&in(cx_ref) let arr_root = arr_val.to_object());
    let mut len: u32 = 0;
    if !w2::GetArrayLength(cx_ref, arr_root.handle().into(), &mut len) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..len {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            arr_root.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if elem.is_object() {
            out.extend_from_slice(&extract_buffer_bytes(cx, elem));
        }
    }
    out
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sign_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return throw_type_error(cx, "sign.update() requires data");
    }
    let this_v = *args.thisv().ptr;
    if !this_v.is_object() {
        return throw_type_error(cx, "sign.update() requires a Sign/Verify instance receiver");
    }
    let input = *args.get(0).ptr;
    let data = if input.is_string() {
        crate::js_to_rust_string(cx, input).into_bytes()
    } else if input.is_object() {
        extract_buffer_bytes(cx, input)
    } else {
        Vec::new()
    };
    push_instance_data(cx, this_v.to_object(), &data);
    args.rval().set(this_v);
    true
}

/// Resolve a Node `createSign`/`createVerify` algorithm string into a
/// `bao_crypto` SignAlgorithm. Returns None for HMAC-style names (which keep
/// the HMAC path) or when the algorithm is ambiguous.
/// @trace REQ-ENG-007 [api:node:crypto createSign] [entity:bao_crypto]
fn resolve_sign_algorithm(algo: &str) -> Option<bao_crypto::sign::SignAlgorithm> {
    use bao_crypto::sign::{RsaHash, SignAlgorithm};
    let lower = algo.to_lowercase();
    let rsa_hash = |name: &str| -> Option<RsaHash> {
        if name.contains("256") {
            Some(RsaHash::Sha256)
        } else if name.contains("384") {
            Some(RsaHash::Sha384)
        } else if name.contains("512") {
            Some(RsaHash::Sha512)
        } else {
            Some(RsaHash::Sha256)
        }
    };
    // RSA-PSS family.
    if lower.contains("rsa-pss") || lower.contains("pss") {
        return rsa_hash(&lower).map(|h| SignAlgorithm::RsaPss { hash: h });
    }
    // RSA-PKCS1v15 family.
    if lower.starts_with("rsa") || lower.contains("rsa-sha") || lower.contains("rsa_pkcs1") {
        return rsa_hash(&lower).map(|h| SignAlgorithm::RsaPkcs1v15 { hash: h });
    }
    // ECDSA family.
    if lower.contains("ecdsa") || lower.contains("p256") || lower.contains("prime256v1") {
        return Some(SignAlgorithm::EcdsaP256);
    }
    if lower.contains("p384") || lower.contains("secp384r1") {
        return Some(SignAlgorithm::EcdsaP384);
    }
    // Ed25519.
    if lower == "ed25519" || lower.contains("ed25519") {
        return Some(SignAlgorithm::Ed25519);
    }
    None
}

/// Asymmetric key kinds detectable from PEM/DER bytes via BoringSSL.
enum AsymKeyKind {
    Rsa,
    Ec,
    Ed25519,
}

/// BCE (v-surface P0-3): `createSign('sha256')` + RSA key silently fell to the
/// HMAC path because bare digest names match no family pattern — the signature
/// family in Node is chosen by the KEY TYPE, the digest only picks the hash.
/// Parse the key (PEM private/public, DER PKCS#8/SPKI) and report its kind.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn detect_asym_key_kind(key_bytes: &[u8]) -> Option<AsymKeyKind> {
    use bun_boringssl_sys as bssl;

    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn pem_to_pkey(key_bytes: &[u8], public: bool) -> *mut bssl::EVP_PKEY {
        let bio = bssl::BIO_new_mem_buf(
            key_bytes.as_ptr() as *const core::ffi::c_void,
            key_bytes.len() as isize,
        );
        if bio.is_null() {
            return core::ptr::null_mut();
        }
        let pkey = if public {
            bssl::PEM_read_bio_PUBKEY(
                bio,
                core::ptr::null_mut(),
                None::<bssl::pem_password_cb>,
                core::ptr::null_mut(),
            )
        } else {
            bssl::PEM_read_bio_PrivateKey(
                bio,
                core::ptr::null_mut(),
                None::<bssl::pem_password_cb>,
                core::ptr::null_mut(),
            )
        };
        bssl::BIO_free(bio);
        pkey
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn der_to_pkey(
        key_bytes: &[u8],
        public: bool,
    ) -> (*mut bssl::EVP_PKEY, *const u8) {
        let mut inp = key_bytes.as_ptr();
        let pkey = if public {
            bssl::d2i_PUBKEY(core::ptr::null_mut(), &mut inp, key_bytes.len() as core::ffi::c_long)
        } else {
            bssl::d2i_AutoPrivateKey(
                core::ptr::null_mut(),
                &mut inp,
                key_bytes.len() as core::ffi::c_long,
            )
        };
        (pkey, inp)
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn kind_of(pkey: *mut bssl::EVP_PKEY) -> Option<AsymKeyKind> {
        if pkey.is_null() {
            return None;
        }
        // BoringSSL's EVP_PKEY_id returns the key's NID. In the vendored
        // BoringSSL build that NID is 949 for Ed25519 — NOT the
        // EVP_PKEY_ED25519 type constant (1087, the OpenSSL numbering);
        // comparing against EVP_PKEY_ED25519 alone never matched and Ed25519
        // keys silently fell to the HMAC path. (Probing the canonical static
        // via EVP_PKEY_id(EVP_pkey_ed25519()) segfaults in this build, so
        // both spellings are accepted. RSA=6 / EC=408 coincide in both
        // namespaces.)
        const ED25519_NID_VENDORED_BORINGSSL: core::ffi::c_int = 949;
        let id = bssl::EVP_PKEY_id(pkey);
        let kind = if id == bssl::EVP_PKEY_RSA {
            Some(AsymKeyKind::Rsa)
        } else if id == bssl::EVP_PKEY_EC {
            Some(AsymKeyKind::Ec)
        } else if id == bssl::EVP_PKEY_ED25519 || id == ED25519_NID_VENDORED_BORINGSSL {
            Some(AsymKeyKind::Ed25519)
        } else {
            None
        };
        bssl::EVP_PKEY_free(pkey);
        kind
    }

    if looks_like_pem_key(key_bytes) {
        let pkey = pem_to_pkey(key_bytes, false);
        let pkey = if pkey.is_null() {
            pem_to_pkey(key_bytes, true)
        } else {
            pkey
        };
        kind_of(pkey)
    } else if !key_bytes.is_empty() {
        let (pkey, _) = der_to_pkey(key_bytes, false);
        let (pkey, _) = if pkey.is_null() {
            der_to_pkey(key_bytes, true)
        } else {
            (pkey, core::ptr::null())
        };
        kind_of(pkey)
    } else {
        None
    }
}

/// Combined resolution: explicit family names win; otherwise the key type
/// picks the family and the algorithm string supplies the digest (Node
/// semantics for `createSign('sha256')` with an asymmetric key).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_sign_algorithm_for_key(
    algo: &str,
    key: &[u8],
) -> Option<bao_crypto::sign::SignAlgorithm> {
    use bao_crypto::sign::{RsaHash, SignAlgorithm};
    if let Some(explicit) = resolve_sign_algorithm(algo) {
        return Some(explicit);
    }
    let kind = detect_asym_key_kind(key)?;
    let hash = if algo.contains("384") {
        RsaHash::Sha384
    } else if algo.contains("512") {
        RsaHash::Sha512
    } else {
        RsaHash::Sha256
    };
    match kind {
        AsymKeyKind::Rsa => Some(SignAlgorithm::RsaPkcs1v15 { hash }),
        AsymKeyKind::Ec => {
            // The curve comes from the key itself; the digest picks the md.
            if algo.contains("384") || algo.contains("512") {
                Some(SignAlgorithm::EcdsaP384)
            } else {
                Some(SignAlgorithm::EcdsaP256)
            }
        }
        AsymKeyKind::Ed25519 => Some(SignAlgorithm::Ed25519),
    }
}

/// Detect whether `key_bytes` is a PEM-encoded asymmetric private/public key.
fn looks_like_pem_key(key_bytes: &[u8]) -> bool {
    if key_bytes.len() < 11 {
        return false;
    }
    key_bytes.starts_with(b"-----BEGIN ")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sign_sign(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Node: sign.sign(privateKey[, outputEncoding]) — a Buffer when no
    // output encoding is given, a string otherwise.
    let encoding: Option<String> = if argc > 1 {
        match arg_to_string(cx, *args.get(1).ptr) {
            Some(s) => Some(s),
            None => None,
        }
    } else {
        None
    };
    // @trace REQ-ENG-007 [api:node:crypto sign.sign] [entity:bao_crypto]
    // Real asymmetric signing via bao_crypto::sign::Signer for RSA-PKCS1v15/PSS,
    // ECDSA P256/P384, Ed25519. HMAC remains for HMAC algorithms / raw keys.
    let this_v = *args.thisv().ptr;
    let (algo, data) = if this_v.is_object() {
        let this_obj = this_v.to_object();
        (
            get_string_prop(cx, this_obj, c"_baoAlgo".as_ptr())
                .unwrap_or_else(|| "sha256".to_string()),
            take_instance_data(cx, this_obj),
        )
    } else {
        (HASH_ALGO.with(|a| ::std::mem::take(&mut *a.borrow_mut())), Vec::new())
    };
    let key = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.into_bytes(),
            None => extract_buffer_bytes(cx, *args.get(0).ptr),
        }
    } else {
        Vec::new()
    };

    let result: Vec<u8> = if let Some(sign_algo) = resolve_sign_algorithm_for_key(&algo, &key) {
        // Asymmetric path. Key must be a PEM or DER private key.
        let signer_res = if looks_like_pem_key(&key) {
            let pem = String::from_utf8_lossy(&key).into_owned();
            bao_crypto::sign::Signer::from_pkcs8_pem(&sign_algo, &pem)
        } else {
            bao_crypto::sign::Signer::from_pkcs8_der(&sign_algo, &key)
        };
        let format = match sign_algo {
            bao_crypto::sign::SignAlgorithm::Ed25519 => bao_crypto::sign::SignatureFormat::Raw,
            _ => bao_crypto::sign::SignatureFormat::Der,
        };
        match signer_res {
            Ok(signer) => match signer.sign(&data, format) {
                Ok(sig) => sig,
                Err(e) => return throw_type_error(cx, &format!("sign.sign() failed: {}", e)),
            },
            Err(e) => return throw_type_error(cx, &format!("sign.sign() key load failed: {}", e)),
        }
    } else {
        // HMAC path (sha256/sha512/sha1 with a raw secret key).
        let alg = match algo.as_str() {
            "sha512" => bun_sha_hmac::Algorithm::Sha512,
            "sha1" => bun_sha_hmac::Algorithm::Sha1,
            _ => bun_sha_hmac::Algorithm::Sha256,
        };
        let mut out = [0u8; EVP_MAX_MD_SIZE];
        bun_sha_hmac::generate(&key, &data, alg, &mut out)
            .map(|s| s.to_vec())
            .unwrap_or_default()
    };

    match encoding.as_deref().map(|s| s.to_lowercase()) {
        None => {
            // No output encoding: Node returns a Buffer instance.
            let buf_obj = crate::globals::create_buffer_object(cx, &result);
            if buf_obj.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
            }
            true
        }
        Some(enc) => match enc.as_str() {
            "base64" => {
                let encoded_bytes = bun_base64::encode_alloc(&result);
                let encoded = ::std::str::from_utf8(&encoded_bytes)
                    .unwrap_or("")
                    .to_owned();
                return_string(cx, &args, &encoded)
            }
            _ => return_string(cx, &args, &hex::encode(&result)),
        },
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_verify(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let algo = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.to_lowercase(),
            None => "sha256".to_string(),
        }
    } else {
        "sha256".to_string()
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // Per-instance algorithm + accumulated data (see crypto_create_sign).
    set_hidden_string_prop(cx, obj.get(), c"_baoAlgo".as_ptr(), &algo);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"update".as_ptr(),
        Some(sign_update),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"verify".as_ptr(),
        Some(verify_verify),
        3,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn verify_verify(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // @trace REQ-ENG-007 [api:node:crypto verify.verify] [entity:bao_crypto]
    // Real asymmetric verification via bao_crypto::verify::Verifier for
    // RSA-PKCS1v15/PSS, ECDSA P256/P384, Ed25519. HMAC path retained for
    // HMAC algorithms / raw keys (compared constant-time).
    if argc < 2 {
        return throw_type_error(cx, "verify.verify() requires (key, signature)");
    }
    let key = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(0).ptr),
    };
    // Signature may be a hex string, base64 string, or a byte array.
    let sig_bytes = if (*args.get(1).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        let sig_str = arg_to_string(cx, *args.get(1).ptr).unwrap_or_default();
        // Try hex first, fall back to raw bytes.
        hex::decode(&sig_str).unwrap_or_else(|_| sig_str.into_bytes())
    };
    let this_v = *args.thisv().ptr;
    let (algo, data) = if this_v.is_object() {
        let this_obj = this_v.to_object();
        (
            get_string_prop(cx, this_obj, c"_baoAlgo".as_ptr())
                .unwrap_or_else(|| "sha256".to_string()),
            take_instance_data(cx, this_obj),
        )
    } else {
        (HASH_ALGO.with(|a| ::std::mem::take(&mut *a.borrow_mut())), Vec::new())
    };

    let verified: bool = if let Some(sign_algo) = resolve_sign_algorithm_for_key(&algo, &key) {
        let verifier_res = if looks_like_pem_key(&key) {
            let pem = String::from_utf8_lossy(&key).into_owned();
            bao_crypto::verify::Verifier::from_public_pem(&sign_algo, &pem)
        } else {
            bao_crypto::verify::Verifier::from_public_der(&sign_algo, &key)
        };
        let format = match sign_algo {
            bao_crypto::sign::SignAlgorithm::Ed25519 => bao_crypto::sign::SignatureFormat::Raw,
            _ => bao_crypto::sign::SignatureFormat::Der,
        };
        match verifier_res {
            Ok(verifier) => match verifier.verify(&data, &sig_bytes, format) {
                Ok(ok) => ok,
                Err(_) => false,
            },
            Err(_) => false,
        }
    } else {
        // HMAC path: recompute and compare constant-time.
        let alg = match algo.as_str() {
            "sha512" => bun_sha_hmac::Algorithm::Sha512,
            "sha1" => bun_sha_hmac::Algorithm::Sha1,
            _ => bun_sha_hmac::Algorithm::Sha256,
        };
        let mut out = [0u8; EVP_MAX_MD_SIZE];
        let computed = bun_sha_hmac::generate(&key, &data, alg, &mut out)
            .map(|s| s.to_vec())
            .unwrap_or_default();
        bun_boringssl_sys::constant_time_eq(&computed, &sig_bytes)
    };
    args.rval().set(mozjs::jsval::BooleanValue(verified));
    true
}

// --- createSecretKey ---

/// createSecretKey(buffer[, encoding]) — a REAL KeyObject of type "secret"
/// storing the raw bytes. (The old implementation returned a plain object
/// whose `export` was a hex STRING property — not callable, and it leaked
/// the secret as an enumerable property.)
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_secret_key(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "createSecretKey() requires key material");
    }
    let bytes = if (*args.get(0).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else if (*args.get(0).ptr).is_string() {
        // createSecretKey('ascii-str', 'hex'|'base64'|'base64url') decodes
        // per the encoding; without one, the string IS the raw key (UTF-8).
        let s = crate::js_to_rust_string(cx, *args.get(0).ptr);
        if argc > 1 && (*args.get(1).ptr).is_string() {
            let enc = crate::js_to_rust_string(cx, *args.get(1).ptr).to_lowercase();
            match enc.as_str() {
                "hex" => match hex::decode(&s) {
                    Ok(b) => b,
                    Err(e) => {
                        return throw_type_error(cx, &format!("createSecretKey: hex decode: {}", e));
                    }
                },
                "base64" | "base64url" => {
                    let src = s.as_bytes();
                    let upper = bun_base64::decode_lenient_len(src.len());
                    let mut out = vec![0u8; upper];
                    let n = bun_base64::decode_lenient(&mut out, src, enc == "base64url");
                    out.truncate(n);
                    out
                }
                "utf8" | "utf-8" | "ascii" | "latin1" | "binary" => s.into_bytes(),
                other => {
                    return throw_type_error(
                        cx,
                        &format!("createSecretKey: unsupported encoding {:?}", other),
                    );
                }
            }
        } else {
            s.into_bytes()
        }
    } else {
        return throw_type_error(cx, "createSecretKey() requires a Buffer or string");
    };
    if bytes.is_empty() {
        return throw_type_error(cx, "createSecretKey() requires non-empty key material");
    }
    let idx = alloc_key_object(bytes);
    let obj = make_key_object_js(cx, idx, "secret", None);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

// --- generateKeyPairSync ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_key_pair_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // @trace REQ-ENG-007 [api:node:crypto generateKeyPairSync] [entity:bao_crypto]
    // Node signature: generateKeyPairSync(type, options) where type is
    // 'rsa' | 'ec' | 'ed25519' | 'x25519'. Returns {publicKey, privateKey} as
    // KeyObjects (Node contract: .type/.asymmetricKeyType/.export() on both;
    // sign/verify accept them through the KeyObject slot path). RSA default
    // bits=2048; ec default curve=P256.
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "generateKeyPairSync() requires a key type");
    }
    let kp_type_str = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "generateKeyPairSync() type must be a string"),
    };
    let kp_type = match kp_type_str.as_str() {
        "rsa" => {
            let bits = read_option_number(cx, &args, argc, 1, "modulusLength", 2048);
            bao_crypto::keypair::KeyPairType::Rsa {
                bits: bits as usize,
            }
        }
        "ec" => {
            let curve_name = read_option_string(cx, &args, argc, 1, "namedCurve", "P-256");
            let curve = match curve_name.to_uppercase().as_str() {
                "P-256" | "PRIME256V1" | "SECP256R1" => bao_crypto::keypair::EcCurve::P256,
                "P-384" | "SECP384R1" => bao_crypto::keypair::EcCurve::P384,
                _ => return throw_type_error(cx, &format!("unsupported EC curve: {}", curve_name)),
            };
            bao_crypto::keypair::KeyPairType::Ec { curve }
        }
        "ed25519" => bao_crypto::keypair::KeyPairType::Ed25519,
        "x25519" => bao_crypto::keypair::KeyPairType::X25519,
        other => return throw_type_error(cx, &format!("unsupported key type: {}", other)),
    };
    let result = match bao_crypto::keypair::generate_key_pair(&kp_type) {
        Ok(r) => r,
        Err(e) => return throw_type_error(cx, &format!("generateKeyPairSync() failed: {}", e)),
    };
    // KeyObject construction from a generated PEM — the SAME canonical
    // pipeline createPublicKey/createPrivateKey run (parse → canonical DER
    // slot → KeyObject), so the returned keys carry the full surface:
    // .export({type, format}) (PEM and DER), .type, .asymmetricKeyType, and
    // KeyObject-slot acceptance in sign/verify. The previous raw-PEM-string
    // return was a silent shape downgrade: `.export` was not a function on
    // the generated keys.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn pem_to_key_object(
        cx: *mut JSContext,
        pem: &str,
        half: KeyHalf,
    ) -> ::std::result::Result<*mut JSObject, String> {
        let pkey = parse_key_to_pkey(pem.as_bytes(), Some("pem"), half)?;
        let canonical = pkey_canonical_der(pkey, half);
        let kind = pkey_kind_name(pkey);
        bun_boringssl_sys::EVP_PKEY_free(pkey);
        let canonical = canonical?;
        let idx = alloc_key_object(canonical);
        let key_type = if half == KeyHalf::Public {
            "public"
        } else {
            "private"
        };
        let obj = make_key_object_js(cx, idx, key_type, Some(kind));
        if obj.is_null() {
            return Err("KeyObject allocation failed".to_string());
        }
        Ok(obj)
    }
    let pub_pem = result
        .public_key_pem
        .ok_or_else(|| "generateKeyPairSync() produced no public PEM".to_string());
    let pub_obj = match pub_pem.and_then(|p| pem_to_key_object(cx, &p, KeyHalf::Public)) {
        Ok(o) => o,
        Err(e) => return throw_type_error(cx, &format!("generateKeyPairSync(): {}", e)),
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    // Root the public KeyObject before building the private one — the second
    // allocation can GC and move the first.
    rooted!(&in(cx_ref) let pub_root = pub_obj);
    let priv_pem = result
        .private_key_pem
        .ok_or_else(|| "generateKeyPairSync() produced no private PEM".to_string());
    let priv_obj = match priv_pem.and_then(|p| pem_to_key_object(cx, &p, KeyHalf::Private)) {
        Ok(o) => o,
        Err(e) => return throw_type_error(cx, &format!("generateKeyPairSync(): {}", e)),
    };
    rooted!(&in(cx_ref) let priv_root = priv_obj);
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let pub_v = mozjs::jsval::ObjectValue(pub_root.get());
    rooted!(&in(cx_ref) let pub_v_root = pub_v);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"publicKey".as_ptr(),
        pub_v_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    let priv_v = mozjs::jsval::ObjectValue(priv_root.get());
    rooted!(&in(cx_ref) let priv_v_root = priv_v);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"privateKey".as_ptr(),
        priv_v_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_option_string(
    cx: *mut JSContext,
    args: &CallArgs,
    argc: u32,
    arg_index: usize,
    prop: &str,
    default: &str,
) -> String {
    if arg_index < argc as usize {
        let opts_val = *args.get(arg_index as u32).ptr;
        if opts_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let obj = opts_val.to_object());
            let mut v = UndefinedValue();
            let prop_c: &[u8] = match prop {
                "namedCurve" => b"namedCurve\0",
                "modulusLength" => b"modulusLength\0",
                _ => b"\0",
            };
            JS_GetProperty(
                cx,
                obj.handle().into(),
                prop_c.as_ptr() as *const _,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            );
            if v.is_string() {
                return crate::js_to_rust_string(cx, v);
            }
        }
    }
    default.to_string()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_option_number(
    cx: *mut JSContext,
    args: &CallArgs,
    argc: u32,
    arg_index: usize,
    prop: &str,
    default: i64,
) -> i64 {
    if arg_index < argc as usize {
        let opts_val = *args.get(arg_index as u32).ptr;
        if opts_val.is_object() {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let obj = opts_val.to_object());
            let mut v = UndefinedValue();
            let prop_c: &[u8] = match prop {
                "modulusLength" => b"modulusLength\0",
                _ => b"\0",
            };
            JS_GetProperty(
                cx,
                obj.handle().into(),
                prop_c.as_ptr() as *const _,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            );
            if v.is_int32() {
                return v.to_int32() as i64;
            } else if v.is_double() {
                return v.to_double() as i64;
            }
        }
    }
    default
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_string_prop(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: *const core::ffi::c_char,
    value: &str,
) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const core::ffi::c_char, value.len());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
        JS_DefineProperty(
            cx,
            obj_rooted.handle().into(),
            name,
            v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

// --- createECDH ---

thread_local! {
    static ECDH_REGISTRY: RefCell<Vec<Option<bao_crypto::key_exchange::EcdhKeyPair>>> =
        const { RefCell::new(Vec::new()) };
    static ECDH_NEXT_ID: RefCell<u32> = const { RefCell::new(1) };
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_ecdh(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // @trace REQ-ENG-007 [api:node:crypto createECDH] [entity:bao_crypto]
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "createECDH() requires a curve name");
    }
    let curve_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s,
        None => return throw_type_error(cx, "createECDH() curve must be a string"),
    };
    let curve = match bao_crypto::key_exchange::parse_curve(&curve_name) {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("createECDH() failed: {}", e)),
    };
    let kp = match bao_crypto::key_exchange::EcdhKeyPair::generate(curve) {
        Ok(k) => k,
        Err(e) => return throw_type_error(cx, &format!("createECDH() generate failed: {}", e)),
    };
    let id = ECDH_NEXT_ID.with(|n| {
        let id = *n.borrow();
        *n.borrow_mut() = id.wrapping_add(1);
        id
    });
    ECDH_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        let idx = id as usize;
        if idx >= reg.len() {
            let extra = idx + 1 - reg.len();
            reg.reserve(extra);
            while reg.len() <= idx {
                reg.push(None);
            }
        }
        reg[idx] = Some(kp);
    });

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    store_ecdh_id(cx, obj.get(), id);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPublicKey".as_ptr(),
        Some(ecdh_get_public_key),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"computeSecret".as_ptr(),
        Some(ecdh_compute_secret),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"convertKey".as_ptr(),
        Some(ecdh_convert_key),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn store_ecdh_id(cx: *mut JSContext, obj: *mut JSObject, id: u32) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let id_val = mozjs::jsval::Int32Value(id as i32);
    rooted!(&in(cx_ref) let idv = id_val);
    JS_DefineProperty(
        cx,
        obj_rooted.handle().into(),
        c"__bao_ecdh_id".as_ptr(),
        idv.handle().into(),
        0,
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_ecdh_id_from_this(cx: *mut JSContext, args: &CallArgs) -> Option<u32> {
    let this = args.thisv();
    if !this.is_object() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());
    let mut id_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"__bao_ecdh_id".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut id_val,
        },
    );
    if id_val.is_int32() {
        Some(id_val.to_int32() as u32)
    } else {
        None
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ecdh_get_public_key(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let id = match read_ecdh_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "ecdh.getPublicKey() invalid receiver"),
    };
    let pub_bytes = ECDH_REGISTRY.with(|reg| {
        reg.borrow()
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .map(|kp| kp.public_key_bytes())
    });
    let pub_bytes = match pub_bytes {
        Some(b) => b,
        None => return throw_type_error(cx, "ecdh.getPublicKey() stale context"),
    };
    let arr = bytes_to_js_array(cx, &pub_bytes);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ecdh_compute_secret(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_ecdh_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "ecdh.computeSecret() invalid receiver"),
    };
    if argc < 1 {
        return throw_type_error(cx, "ecdh.computeSecret() requires peer public key");
    }
    let peer_pub = extract_buffer_bytes(cx, *args.get(0).ptr);
    let secret = ECDH_REGISTRY.with(|reg| {
        reg.borrow()
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .and_then(|kp| kp.compute_shared_secret(&peer_pub).ok())
    });
    let secret = match secret {
        Some(s) => s,
        None => return throw_type_error(cx, "ecdh.computeSecret() failed"),
    };
    let arr = bytes_to_js_array(cx, &secret);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

/// ECDH .convertKey() — converts the key to the specified format.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ecdh_convert_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_ecdh_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "ecdh.convertKey() invalid receiver"),
    };
    if argc < 1 {
        return throw_type_error(cx, "ecdh.convertKey() requires key data");
    }
    let key_data = extract_buffer_bytes(cx, *args.get(0).ptr);
    let curve = ECDH_REGISTRY.with(|reg| {
        reg.borrow()
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .map(|kp| kp.curve())
    });
    let curve = match curve {
        Some(c) => c,
        None => return throw_type_error(cx, "ecdh.convertKey() stale context"),
    };
    let reconstructed =
        match bao_crypto::key_exchange::EcdhKeyPair::reconstruct_keypair(curve, &key_data) {
            Ok(kp) => kp,
            Err(e) => return throw_type_error(cx, &format!("ecdh.convertKey() failed: {}", e)),
        };
    let pub_bytes = reconstructed.public_key_bytes();
    let arr = bytes_to_js_array(cx, &pub_bytes);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

// --- X509Certificate ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_x509_certificate(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // @trace REQ-ENG-007 [api:node:crypto X509Certificate] [entity:bao_crypto]
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "X509Certificate() requires a PEM/DER buffer");
    }
    let pem = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s,
        None => return throw_type_error(cx, "X509Certificate() input must be a PEM string"),
    };
    let cert = match bao_crypto::certificate::X509Certificate::from_pem(&pem) {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("X509Certificate() parse failed: {}", e)),
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    set_string_prop(cx, obj.get(), c"subject".as_ptr(), &cert.subject());
    set_string_prop(cx, obj.get(), c"issuer".as_ptr(), &cert.issuer());
    set_string_prop(
        cx,
        obj.get(),
        c"serialNumber".as_ptr(),
        &cert.serial_number(),
    );
    set_string_prop(cx, obj.get(), c"validFrom".as_ptr(), &cert.valid_from());
    set_string_prop(cx, obj.get(), c"validTo".as_ptr(), &cert.valid_to());
    set_string_prop(
        cx,
        obj.get(),
        c"fingerprint256".as_ptr(),
        &cert.fingerprint_sha256(),
    );
    set_string_prop(
        cx,
        obj.get(),
        c"fingerprint".as_ptr(),
        &cert.fingerprint_sha1(),
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

// --- X509 (Node crypto.X509 certificate parser) ---
// @trace REQ-ENG-007 [api:node:crypto X509] [entity:bao_crypto]
// Node.js exposes both X509Certificate and X509 as certificate parser
// constructors. X509 accepts a PEM/DER buffer and exposes subject/issuer/raw.
// Here we reuse bao_crypto::certificate::X509Certificate to do the real parse
// (PEM_read_bio_X509 / d2i_X509 under the hood), then surface the same
// properties on the returned object so the constructor is real, not a stub.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_x509(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "X509() requires a PEM/DER buffer");
    }
    // Accept a PEM string or a Buffer/TypedArray carrying DER bytes.
    let arg0 = *args.get(0).ptr;
    let cert = if arg0.is_string() {
        let pem = crate::js_to_rust_string(cx, arg0);
        bao_crypto::certificate::X509Certificate::from_pem(&pem)
    } else if arg0.is_object() {
        let der = extract_buffer_bytes(cx, arg0);
        bao_crypto::certificate::X509Certificate::from_der(&der)
    } else {
        return throw_type_error(cx, "X509() input must be a PEM string or DER buffer");
    };
    let cert = match cert {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("X509() parse failed: {}", e)),
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    set_string_prop(cx, obj.get(), c"subject".as_ptr(), &cert.subject());
    set_string_prop(cx, obj.get(), c"issuer".as_ptr(), &cert.issuer());
    set_string_prop(
        cx,
        obj.get(),
        c"serialNumber".as_ptr(),
        &cert.serial_number(),
    );
    set_string_prop(cx, obj.get(), c"validFrom".as_ptr(), &cert.valid_from());
    set_string_prop(cx, obj.get(), c"validTo".as_ptr(), &cert.valid_to());
    set_string_prop(
        cx,
        obj.get(),
        c"fingerprint256".as_ptr(),
        &cert.fingerprint_sha256(),
    );
    set_string_prop(
        cx,
        obj.get(),
        c"fingerprint".as_ptr(),
        &cert.fingerprint_sha1(),
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

// --- hkdfSync ---
// @trace REQ-ENG-007 [api:node:crypto hkdfSync] [entity:bao_crypto]
// Node: crypto.hkdfSync(digest, key, salt, info, length) -> ArrayBuffer.
// Real HKDF-Extract+Expand via BoringSSL HKDF() (bao_crypto::kdf::hkdf).

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_hkdf_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 5 {
        return throw_type_error(cx, "hkdfSync() requires (digest, key, salt, info, length)");
    }
    let digest_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "hkdfSync() digest must be a string"),
    };
    let key = extract_buffer_bytes(cx, *args.get(1).ptr);
    let salt = extract_buffer_bytes(cx, *args.get(2).ptr);
    let info = extract_buffer_bytes(cx, *args.get(3).ptr);
    let length = {
        let v = *args.get(4).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else if v.is_double() {
            v.to_double() as usize
        } else {
            return throw_type_error(cx, "hkdfSync() length must be a number");
        }
    };

    let hash = match digest_name.as_str() {
        "sha256" => bao_crypto::kdf::HkdfHash::Sha256,
        "sha1" => bao_crypto::kdf::HkdfHash::Sha1,
        other => {
            return throw_type_error(cx, &format!("Unsupported HKDF digest: {}", other));
        }
    };
    let out = match bao_crypto::kdf::hkdf(hash, &salt, &key, &info, length) {
        Ok(o) => o,
        Err(e) => return throw_type_error(cx, &format!("hkdfSync() failed: {}", e)),
    };

    // Node returns an ArrayBuffer; we materialise it as a Uint8Array-backed
    // Buffer so Buffer.isBuffer(...) and .length work uniformly with the rest
    // of our crypto surface.
    let buf_obj = crate::globals::create_buffer_object(cx, &out);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    }
    true
}

// --- createDiffieHellman ---
// @trace REQ-ENG-007 [api:node:crypto createDiffieHellman] [entity:bao_crypto]
// Node: createDiffieHellman(prime | primeLength[, generator]) -> DH object.
// Real MODP DH via bao_crypto::dh::DiffieHellman (BoringSSL DH_* underneath).

thread_local! {
    static DH_REGISTRY: RefCell<Vec<Option<bao_crypto::dh::DiffieHellman>>> =
        const { RefCell::new(Vec::new()) };
    static DH_NEXT_ID: RefCell<u32> = const { RefCell::new(1) };
}

fn dh_registry_insert(dh: bao_crypto::dh::DiffieHellman) -> u32 {
    let id = DH_NEXT_ID.with(|next| {
        let id = *next.borrow();
        *next.borrow_mut() = id.wrapping_add(1);
        id
    });
    let idx = id as usize;
    DH_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        if idx >= reg.len() {
            let extra = idx + 1 - reg.len();
            reg.reserve(extra);
            while reg.len() <= idx {
                reg.push(None);
            }
        }
        reg[idx] = Some(dh);
    });
    id
}

fn dh_registry_with_mut<R>(
    id: u32,
    f: &mut dyn FnMut(&mut bao_crypto::dh::DiffieHellman) -> R,
) -> Option<R> {
    DH_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        match reg.get_mut(id as usize).and_then(|s| s.as_mut()) {
            Some(dh) => Some(f(dh)),
            None => None,
        }
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn store_dh_id(cx: *mut JSContext, obj: *mut JSObject, id: u32) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let id_val = mozjs::jsval::Int32Value(id as i32);
    rooted!(&in(cx_ref) let idv = id_val);
    JS_DefineProperty(
        cx,
        obj_rooted.handle().into(),
        c"__bao_dh_id".as_ptr(),
        idv.handle().into(),
        0,
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_dh_id_from_this(cx: *mut JSContext, args: &CallArgs) -> Option<u32> {
    let this = args.thisv();
    if !this.is_object() {
        return None;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());
    let mut id_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"__bao_dh_id".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut id_val,
        },
    );
    if id_val.is_int32() {
        Some(id_val.to_int32() as u32)
    } else {
        None
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_dh(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "createDiffieHellman() requires a prime or prime length");
    }
    let arg0 = *args.get(0).ptr;
    let generator = if argc >= 2 {
        let g = *args.get(1).ptr;
        if g.is_int32() { g.to_int32() } else { 2 }
    } else {
        2
    };
    // Node overloads: number → generate group of that bit length;
    // Buffer/string → use as the explicit prime.
    let dh = if arg0.is_int32() || arg0.is_double() {
        let bits = if arg0.is_int32() {
            arg0.to_int32() as u32
        } else {
            arg0.to_double() as u32
        };
        bao_crypto::dh::DiffieHellman::generate(bits, generator)
    } else if arg0.is_string() {
        // Hex string? Node accepts prime as Buffer or base64/hex string;
        // we treat any string as raw bytes (utf8) to keep semantics simple
        // and predictable for the common Buffer-from-string path.
        let prime = crate::js_to_rust_string(cx, arg0).into_bytes();
        bao_crypto::dh::DiffieHellman::from_prime(&prime, generator)
    } else if arg0.is_object() {
        let prime = extract_buffer_bytes(cx, arg0);
        bao_crypto::dh::DiffieHellman::from_prime(&prime, generator)
    } else {
        return throw_type_error(
            cx,
            "createDiffieHellman() prime must be a number, Buffer, or string",
        );
    };
    let dh = match dh {
        Ok(d) => d,
        Err(e) => return throw_type_error(cx, &format!("createDiffieHellman() failed: {}", e)),
    };
    let id = dh_registry_insert(dh);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    store_dh_id(cx, obj.get(), id);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"generateKeys".as_ptr(),
        Some(dh_generate_keys),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"computeSecret".as_ptr(),
        Some(dh_compute_secret),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPrime".as_ptr(),
        Some(dh_get_prime),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getGenerator".as_ptr(),
        Some(dh_get_generator),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPublicKey".as_ptr(),
        Some(dh_get_public_key),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPrivateKey".as_ptr(),
        Some(dh_get_private_key),
        0,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dh_generate_keys(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let id = match read_dh_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "diffiehellman.generateKeys() invalid receiver"),
    };
    let pub_bytes = match dh_registry_with_mut(id, &mut |dh| dh.generate_keys()) {
        Some(Ok(b)) => b,
        Some(Err(e)) => return throw_type_error(cx, &format!("generateKeys() failed: {}", e)),
        None => return throw_type_error(cx, "generateKeys() stale context"),
    };
    let arr = bytes_to_js_array(cx, &pub_bytes);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dh_compute_secret(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let id = match read_dh_id_from_this(cx, &args) {
        Some(id) => id,
        None => return throw_type_error(cx, "diffiehellman.computeSecret() invalid receiver"),
    };
    if argc < 1 {
        return throw_type_error(cx, "computeSecret() requires peer public key");
    }
    let peer_pub = extract_buffer_bytes(cx, *args.get(0).ptr);
    let secret = DH_REGISTRY.with(|reg| {
        reg.borrow()
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .and_then(|dh| dh.compute_secret(&peer_pub).ok())
    });
    let secret = match secret {
        Some(s) => s,
        None => return throw_type_error(cx, "computeSecret() failed"),
    };
    let arr = bytes_to_js_array(cx, &secret);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dh_read_bytes_prop(
    cx: *mut JSContext,
    args: &CallArgs,
    f: &dyn Fn(&bao_crypto::dh::DiffieHellman) -> Vec<u8>,
) -> bool {
    let id = match read_dh_id_from_this(cx, args) {
        Some(id) => id,
        None => return throw_type_error(cx, "invalid diffiehellman receiver"),
    };
    let bytes = DH_REGISTRY.with(|reg| {
        reg.borrow()
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .map(f)
    });
    let bytes = match bytes {
        Some(b) => b,
        None => return throw_type_error(cx, "stale diffiehellman context"),
    };
    let arr = bytes_to_js_array(cx, &bytes);
    if arr.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(arr));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dh_get_prime(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    dh_read_bytes_prop(cx, &args, &|dh| dh.prime().to_vec())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dh_get_generator(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    dh_read_bytes_prop(cx, &args, &|dh| dh.generator().to_vec())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dh_get_public_key(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    dh_read_bytes_prop(cx, &args, &|dh| dh.public_key())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dh_get_private_key(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    dh_read_bytes_prop(cx, &args, &|dh| dh.private_key())
}

// ============================================================
// KeyObject class — lightweight JS wrapper for key material
// Stores key bytes + type in thread-local Vec; JS object holds index
// ============================================================
//
// Storage normalization (BCE: export() used to return the stored bytes
// verbatim AND consumed them — a second export returned undefined, and the
// options argument was ignored entirely):
//   type "secret"  → raw key bytes
//   type "public"  → canonical SPKI DER   (SubjectPublicKeyInfo)
//   type "private" → canonical PKCS#8 DER (PrivateKeyInfo)
// Canonical DER keeps the existing sign/verify KeyObject consumers
// (Signer::from_pkcs8_der / Verifier::from_public_der) working against the
// SAME slots, and gives export() a single parse point for every output
// encoding (spki/pkcs8/pkcs1/sec1 × pem/der).

thread_local! {
    static KEY_OBJECTS: RefCell<Vec<Option<Vec<u8>>>> = const { RefCell::new(Vec::new()) };
}

fn alloc_key_object(key_bytes: Vec<u8>) -> usize {
    KEY_OBJECTS.with(|v| {
        let mut vec = v.borrow_mut();
        let idx = vec.len();
        vec.push(Some(key_bytes));
        idx
    })
}

// ── BoringSSL key serialization surface ────────────────────────────────────
// bun_boringssl_sys exposes a hand-rolled subset of libcrypto; the KeyObject
// import/export matrix additionally needs the (de)serializers below, declared
// locally against the SAME linked library (all present in
// vendor/boringssl/include/openssl/{pem,rsa,ec_key}.h — verified against the
// vendored headers, including the DECLARE_PEM macro expansions).

unsafe extern "C" {
    /// pem.h — DECLARE_PEM_rw_const(RSAPublicKey, RSA): "BEGIN RSA PUBLIC KEY".
    fn PEM_write_bio_RSAPublicKey(
        bp: *mut bun_boringssl_sys::BIO,
        rsa: *const bun_boringssl_sys::RSA,
    ) -> core::ffi::c_int;
    /// pem.h — DECLARE_PEM_rw_cb(RSAPrivateKey, RSA): "BEGIN RSA PRIVATE KEY"
    /// (enc=NULL → unencrypted).
    fn PEM_write_bio_RSAPrivateKey(
        bp: *mut bun_boringssl_sys::BIO,
        rsa: *const bun_boringssl_sys::RSA,
        enc: *const bun_boringssl_sys::EVP_CIPHER,
        kstr: *const core::ffi::c_char,
        klen: core::ffi::c_int,
        cb: Option<bun_boringssl_sys::pem_password_cb>,
        u: *mut core::ffi::c_void,
    ) -> core::ffi::c_int;
    /// pem.h — DECLARE_PEM_rw_cb(ECPrivateKey, EC_KEY): "BEGIN EC PRIVATE KEY".
    fn PEM_write_bio_ECPrivateKey(
        bp: *mut bun_boringssl_sys::BIO,
        eckey: *const bun_boringssl_sys::EC_KEY,
        enc: *const bun_boringssl_sys::EVP_CIPHER,
        kstr: *const core::ffi::c_char,
        klen: core::ffi::c_int,
        cb: Option<bun_boringssl_sys::pem_password_cb>,
        u: *mut core::ffi::c_void,
    ) -> core::ffi::c_int;
    /// pem.h — PKCS#8 DER writer ("PRIVATE KEY" content, DER encoding).
    fn i2d_PKCS8PrivateKey_bio(
        bp: *mut bun_boringssl_sys::BIO,
        x: *const bun_boringssl_sys::EVP_PKEY,
        enc: *const bun_boringssl_sys::EVP_CIPHER,
        pass: *const core::ffi::c_char,
        pass_len: core::ffi::c_int,
        cb: Option<bun_boringssl_sys::pem_password_cb>,
        u: *mut core::ffi::c_void,
    ) -> core::ffi::c_int;
    /// rsa.h — PKCS#1 RSAPublicKey DER.
    fn i2d_RSAPublicKey(
        rsa: *const bun_boringssl_sys::RSA,
        outp: *mut *mut u8,
    ) -> core::ffi::c_int;
    /// rsa.h — PKCS#1 RSAPrivateKey DER.
    fn i2d_RSAPrivateKey(
        rsa: *const bun_boringssl_sys::RSA,
        outp: *mut *mut u8,
    ) -> core::ffi::c_int;
    /// ec_key.h — RFC 5915 ECPrivateKey DER.
    fn i2d_ECPrivateKey(
        key: *const bun_boringssl_sys::EC_KEY,
        outp: *mut *mut u8,
    ) -> core::ffi::c_int;
    /// rsa.h — PKCS#1 RSAPublicKey DER parser (public DER `type: 'pkcs1'`).
    fn d2i_RSAPublicKey(
        out: *mut *mut bun_boringssl_sys::RSA,
        inp: *mut *const u8,
        len: core::ffi::c_long,
    ) -> *mut bun_boringssl_sys::RSA;
}

/// Which half of a keypair a create*/export call is about.
#[derive(Clone, Copy, PartialEq)]
enum KeyHalf {
    Public,
    Private,
}

/// Slurp a memory BIO's accumulated output.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn key_bio_contents(bio: *mut bun_boringssl_sys::BIO) -> Vec<u8> {
    let pending = bun_boringssl_sys::BIO_ctrl_pending(bio);
    if pending == 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; pending];
    let n = bun_boringssl_sys::BIO_read(
        bio,
        out.as_mut_ptr() as *mut core::ffi::c_void,
        pending as core::ffi::c_int,
    );
    if n <= 0 {
        return Vec::new();
    }
    out.truncate(n as usize);
    out
}

/// Fresh memory BIO, or an error string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn key_mem_bio() -> ::std::result::Result<*mut bun_boringssl_sys::BIO, String> {
    let bio = bun_boringssl_sys::BIO_new(bun_boringssl_sys::BIO_s_mem());
    if bio.is_null() {
        Err("BIO_new failed".to_string())
    } else {
        Ok(bio)
    }
}

/// Node-visible key kind name for `asymmetricKeyType` (from the EVP_PKEY NID;
/// same vendored-BoringSSL Ed25519-NID quirk as kind_of above).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn pkey_kind_name(pkey: *const bun_boringssl_sys::EVP_PKEY) -> &'static str {
    const ED25519_NID_VENDORED_BORINGSSL: core::ffi::c_int = 949;
    let id = bun_boringssl_sys::EVP_PKEY_id(pkey);
    if id == bun_boringssl_sys::EVP_PKEY_RSA {
        "rsa"
    } else if id == bun_boringssl_sys::EVP_PKEY_EC {
        "ec"
    } else if id == bun_boringssl_sys::EVP_PKEY_ED25519 || id == ED25519_NID_VENDORED_BORINGSSL {
        "ed25519"
    } else if id == bun_boringssl_sys::EVP_PKEY_X25519 {
        "x25519"
    } else {
        "unknown"
    }
}

/// Parse key material into an EVP_PKEY. PEM is sniffed by the leading
/// "-----BEGIN " (or forced by format hint); everything else is DER.
///
/// `half == Public` also accepts a PRIVATE key form — Node semantics:
/// `createPublicKey(privateKey)` derives the public half (the SPKI
/// serialization simply drops the private components).
///
/// Caller owns (and must free) the returned EVP_PKEY.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_key_to_pkey(
    bytes: &[u8],
    format_hint: Option<&str>,
    half: KeyHalf,
) -> ::std::result::Result<*mut bun_boringssl_sys::EVP_PKEY, String> {
    use bun_boringssl_sys as bssl;
    if bytes.is_empty() {
        return Err("key data is empty".to_string());
    }
    let read_pem = |public: bool| -> *mut bssl::EVP_PKEY {
        let bio = bssl::BIO_new_mem_buf(
            bytes.as_ptr() as *const core::ffi::c_void,
            bytes.len() as isize,
        );
        if bio.is_null() {
            return core::ptr::null_mut();
        }
        let pkey = if public {
            bssl::PEM_read_bio_PUBKEY(
                bio,
                core::ptr::null_mut(),
                None::<bssl::pem_password_cb>,
                core::ptr::null_mut(),
            )
        } else {
            // Sniffs every PEM private-key spelling: PRIVATE KEY (PKCS#8),
            // RSA PRIVATE KEY (PKCS#1), EC PRIVATE KEY (SEC1).
            bssl::PEM_read_bio_PrivateKey(
                bio,
                core::ptr::null_mut(),
                None::<bssl::pem_password_cb>,
                core::ptr::null_mut(),
            )
        };
        bssl::BIO_free(bio);
        pkey
    };
    let read_der = |public: bool| -> *mut bssl::EVP_PKEY {
        let mut inp = bytes.as_ptr();
        if public {
            let pkey = bssl::d2i_PUBKEY(
                core::ptr::null_mut(),
                &mut inp,
                bytes.len() as core::ffi::c_long,
            );
            if !pkey.is_null() {
                return pkey;
            }
            // DER pkcs1 public ("RSA PUBLIC KEY" DER) — not SPKI: lift the
            // bare RSA key into an EVP_PKEY.
            inp = bytes.as_ptr();
            let rsa = d2i_RSAPublicKey(
                core::ptr::null_mut(),
                &mut inp,
                bytes.len() as core::ffi::c_long,
            );
            if !rsa.is_null() {
                let pkey = bssl::EVP_PKEY_new();
                if !pkey.is_null() && bssl::EVP_PKEY_set1_RSA(pkey, rsa) == 1 {
                    bssl::RSA_free(rsa);
                    return pkey;
                }
                bssl::EVP_PKEY_free(pkey);
                bssl::RSA_free(rsa);
            }
            core::ptr::null_mut()
        } else {
            // d2i_AutoPrivateKey: PKCS#8 + traditional PKCS#1/SEC1 (see
            // vendor crypto/evp/evp_asn1.cc — element count picks the form).
            bssl::d2i_AutoPrivateKey(
                core::ptr::null_mut(),
                &mut inp,
                bytes.len() as core::ffi::c_long,
            )
        }
    };

    let is_pem = looks_like_pem_key(bytes) || format_hint == Some("pem");
    let pkey = if is_pem {
        match half {
            KeyHalf::Public => read_pem(true),
            KeyHalf::Private => read_pem(false),
        }
    } else {
        match half {
            KeyHalf::Public => read_der(true),
            KeyHalf::Private => read_der(false),
        }
    };
    // A private-key form handed to the public side — Node's
    // createPublicKey(privateKey): parse as private, derive the public half
    // (the SPKI serialization drops the private components).
    let pkey = if pkey.is_null() && half == KeyHalf::Public {
        if is_pem { read_pem(false) } else { read_der(false) }
    } else {
        pkey
    };
    if pkey.is_null() {
        Err("Failed to parse key material (expected PEM or DER key)".to_string())
    } else {
        Ok(pkey)
    }
}

/// Canonical storage DER for a parsed key.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn pkey_canonical_der(
    pkey: *mut bun_boringssl_sys::EVP_PKEY,
    half: KeyHalf,
) -> ::std::result::Result<Vec<u8>, String> {
    use bun_boringssl_sys as bssl;
    if half == KeyHalf::Public {
        let len = bssl::i2d_PUBKEY(pkey, core::ptr::null_mut());
        if len <= 0 {
            return Err("i2d_PUBKEY failed".to_string());
        }
        let mut buf = vec![0u8; len as usize];
        let mut outp = buf.as_mut_ptr();
        let n = bssl::i2d_PUBKEY(pkey, &mut outp);
        if n <= 0 {
            return Err("i2d_PUBKEY failed".to_string());
        }
        buf.truncate(n as usize);
        Ok(buf)
    } else {
        let bio = key_mem_bio()?;
        let ok = i2d_PKCS8PrivateKey_bio(
            bio,
            pkey,
            core::ptr::null(),
            core::ptr::null(),
            0,
            None,
            core::ptr::null_mut(),
        );
        let der = if ok == 1 { key_bio_contents(bio) } else { Vec::new() };
        bssl::BIO_free(bio);
        if ok == 1 && !der.is_empty() {
            Ok(der)
        } else {
            Err("i2d_PKCS8PrivateKey_bio failed".to_string())
        }
    }
}

/// KeyObject.export() output — PEM (JS string) or DER (Buffer).
enum KeyExportOut {
    Pem(String),
    Der(Vec<u8>),
}

/// The export({type, format}) matrix over a parsed key.
///
///   public  + spki (default) → "PUBLIC KEY"        PEM / SPKI DER
///   public  + pkcs1 (RSA)    → "RSA PUBLIC KEY"    PEM / PKCS#1 DER
///   private + pkcs8 (default)→ "PRIVATE KEY"       PEM / PKCS#8 DER
///   private + pkcs1 (RSA)    → "RSA PRIVATE KEY"   PEM / PKCS#1 DER
///   private + sec1 (EC)      → "EC PRIVATE KEY"    PEM / SEC1 DER
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn export_pkey_as(
    pkey: *mut bun_boringssl_sys::EVP_PKEY,
    half: KeyHalf,
    type_opt: Option<&str>,
    format_opt: Option<&str>,
) -> ::std::result::Result<KeyExportOut, String> {
    use bun_boringssl_sys as bssl;
    let format = format_opt.unwrap_or("pem");
    if format != "pem" && format != "der" {
        return Err(format!("invalid export format {:?} (expected \"pem\" or \"der\")", format));
    }
    let kind = pkey_kind_name(pkey);
    let typ = type_opt.unwrap_or(if half == KeyHalf::Public { "spki" } else { "pkcs8" });

    // PEM writer into a mem BIO → String.
    let pem = |write: &dyn Fn(*mut bssl::BIO) -> core::ffi::c_int,
               what: &str|
     -> ::std::result::Result<String, String> {
        let bio = key_mem_bio()?;
        let ok = write(bio);
        let bytes = key_bio_contents(bio);
        bssl::BIO_free(bio);
        if ok != 1 {
            return Err(format!("{} export failed", what));
        }
        String::from_utf8(bytes).map_err(|_| format!("{} export produced non-UTF-8 PEM", what))
    };
    // DER writer via the two-call i2d pattern → Vec<u8>.
    let der = |i2d: &dyn Fn(*mut *mut u8) -> core::ffi::c_int,
               what: &str|
     -> ::std::result::Result<Vec<u8>, String> {
        let len = i2d(core::ptr::null_mut());
        if len <= 0 {
            return Err(format!("{} export failed", what));
        }
        let mut buf = vec![0u8; len as usize];
        let mut outp = buf.as_mut_ptr();
        let n = i2d(&mut outp);
        if n <= 0 {
            return Err(format!("{} export failed", what));
        }
        buf.truncate(n as usize);
        Ok(buf)
    };

    match (half, typ) {
        (KeyHalf::Public, "spki") => {
            if format == "pem" {
                pem(&|bio| bssl::PEM_write_bio_PUBKEY(bio, pkey), "spki").map(KeyExportOut::Pem)
            } else {
                der(
                    &|outp| bssl::i2d_PUBKEY(pkey, outp),
                    "spki",
                )
                .map(KeyExportOut::Der)
            }
        }
        (KeyHalf::Public, "pkcs1") => {
            if kind != "rsa" {
                return Err(format!(
                    "invalid export type \"pkcs1\" for {} key (RSA only)",
                    kind
                ));
            }
            let rsa = bssl::EVP_PKEY_get0_RSA(pkey);
            if rsa.is_null() {
                return Err("RSA key components unavailable".to_string());
            }
            if format == "pem" {
                pem(&|bio| PEM_write_bio_RSAPublicKey(bio, rsa), "pkcs1")
                    .map(KeyExportOut::Pem)
            } else {
                der(&|outp| i2d_RSAPublicKey(rsa, outp), "pkcs1").map(KeyExportOut::Der)
            }
        }
        (KeyHalf::Private, "pkcs8") => {
            if format == "pem" {
                pem(
                    &|bio| {
                        bssl::PEM_write_bio_PKCS8PrivateKey(
                            bio,
                            pkey,
                            core::ptr::null(),
                            core::ptr::null_mut(),
                            0,
                            None,
                            core::ptr::null_mut(),
                        )
                    },
                    "pkcs8",
                )
                .map(KeyExportOut::Pem)
            } else {
                let bio = key_mem_bio()?;
                let ok = i2d_PKCS8PrivateKey_bio(
                    bio,
                    pkey,
                    core::ptr::null(),
                    core::ptr::null(),
                    0,
                    None,
                    core::ptr::null_mut(),
                );
                let bytes = key_bio_contents(bio);
                bssl::BIO_free(bio);
                if ok == 1 && !bytes.is_empty() {
                    Ok(KeyExportOut::Der(bytes))
                } else {
                    Err("pkcs8 export failed".to_string())
                }
            }
        }
        (KeyHalf::Private, "pkcs1") => {
            if kind != "rsa" {
                return Err(format!(
                    "invalid export type \"pkcs1\" for {} key (RSA only)",
                    kind
                ));
            }
            let rsa = bssl::EVP_PKEY_get0_RSA(pkey);
            if rsa.is_null() {
                return Err("RSA key components unavailable".to_string());
            }
            if format == "pem" {
                pem(
                    &|bio| {
                        PEM_write_bio_RSAPrivateKey(
                            bio,
                            rsa,
                            core::ptr::null(),
                            core::ptr::null(),
                            0,
                            None,
                            core::ptr::null_mut(),
                        )
                    },
                    "pkcs1",
                )
                .map(KeyExportOut::Pem)
            } else {
                der(&|outp| i2d_RSAPrivateKey(rsa, outp), "pkcs1").map(KeyExportOut::Der)
            }
        }
        (KeyHalf::Private, "sec1") => {
            if kind != "ec" {
                return Err(format!(
                    "invalid export type \"sec1\" for {} key (EC only)",
                    kind
                ));
            }
            let ec = bssl::EVP_PKEY_get0_EC_KEY(pkey);
            if ec.is_null() {
                return Err("EC key components unavailable".to_string());
            }
            if format == "pem" {
                pem(
                    &|bio| {
                        PEM_write_bio_ECPrivateKey(
                            bio,
                            ec,
                            core::ptr::null(),
                            core::ptr::null(),
                            0,
                            None,
                            core::ptr::null_mut(),
                        )
                    },
                    "sec1",
                )
                .map(KeyExportOut::Pem)
            } else {
                der(&|outp| i2d_ECPrivateKey(ec, outp), "sec1").map(KeyExportOut::Der)
            }
        }
        (_, other) => Err(format!(
            "invalid export type {:?} (expected \"spki\", \"pkcs8\", \"pkcs1\" or \"sec1\")",
            other
        )),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn make_key_object_js(
    cx: *mut JSContext,
    idx: usize,
    key_type: &str,
    asym_kind: Option<&str>,
) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = JS_NewPlainObject(cx));
    if obj.get().is_null() {
        return ::std::ptr::null_mut();
    }

    let idx_val = mozjs::jsval::Int32Value(idx as i32);
    rooted!(&in(cx_ref) let idx_rooted = idx_val);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"_keyIdx".as_ptr(),
        idx_rooted.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    let c_type = ZBox::from_bytes(key_type.as_bytes());
    let js_type = JS_NewStringCopyZ(cx, c_type.as_ptr());
    if !js_type.is_null() {
        rooted!(&in(cx_ref) let type_val = mozjs::jsval::StringValue(&*js_type));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"type".as_ptr(),
            type_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    // asymmetricKeyType: "rsa" | "ec" | "ed25519" | "x25519" (Node shape;
    // absent for secret keys).
    if let Some(kind) = asym_kind {
        let c_kind = ZBox::from_bytes(kind.as_bytes());
        let js_kind = JS_NewStringCopyZ(cx, c_kind.as_ptr());
        if !js_kind.is_null() {
            rooted!(&in(cx_ref) let kind_val = mozjs::jsval::StringValue(&*js_kind));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"asymmetricKeyType".as_ptr(),
                kind_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"export".as_ptr(),
        Some(key_object_export),
        1,
        JSPROP_ENUMERATE as u32,
    );
    // symmetric property: true for "secret" keys, false otherwise
    let symmetric_val = mozjs::jsval::BooleanValue(key_type == "secret");
    rooted!(&in(cx_ref) let sym_rooted = symmetric_val);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"symmetric".as_ptr(),
        sym_rooted.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    obj.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_key_object(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // KeyObject(key, type) — internal constructor
    let key_bytes = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let key_type = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(1).ptr).to_string())
    } else {
        "secret".to_string()
    };

    let idx = alloc_key_object(key_bytes);
    let obj = make_key_object_js(cx, idx, &key_type, None);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

/// keyObject.export([options]) — real serialization, non-destructive.
///
///   secret  → Buffer of the raw key bytes (options.format may only be
///             "buffer"/undefined, Node shape)
///   public  → {type: "spki"(default)|"pkcs1", format: "pem"(default)|"der"}
///   private → {type: "pkcs8"(default)|"pkcs1"|"sec1", format: "pem"|"der"}
///
/// PEM → string, DER → Buffer. Storage is CLONED, never consumed — the old
/// implementation took the bytes out of the slot (second export = undefined).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn key_object_export(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    use bun_boringssl_sys as bssl;
    let args = CallArgs::from_vp(vp, argc);
    if !args.thisv().is_object() {
        return false;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());

    let mut idx_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this.handle().into(),
        c"_keyIdx".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut idx_val,
        },
    );
    if !idx_val.is_int32() {
        return throw_type_error(cx, "export: not a KeyObject (missing _keyIdx)");
    }
    let idx = idx_val.to_int32() as usize;
    let key_type = get_string_prop(cx, this.get(), c"type".as_ptr()).unwrap_or_default();
    let stored = KEY_OBJECTS.with(|v| {
        v.borrow()
            .get(idx)
            .map(|k| k.as_ref().map(|b| b.clone()))
            .flatten()
    });
    let bytes = match stored {
        Some(b) => b,
        None => {
            return throw_type_error(cx, "export: key material is no longer available");
        }
    };

    // options {type, format} (optional for secret keys).
    let (type_opt, format_opt) = if argc > 0 && (*args.get(0).ptr).is_object() {
        let opts = (*args.get(0).ptr).to_object();
        (
            get_string_prop(cx, opts, c"type".as_ptr()),
            get_string_prop(cx, opts, c"format".as_ptr()),
        )
    } else {
        (None, None)
    };

    match key_type.as_str() {
        "secret" => {
            if let Some(f) = format_opt.as_deref() {
                if f != "buffer" {
                    return throw_type_error(cx, &format!(
                        "export: invalid format {:?} for a secret key (expected \"buffer\")",
                        f
                    ));
                }
            }
            let buf_obj = crate::globals::create_buffer_object(cx, &bytes);
            if buf_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
            true
        }
        "public" | "private" => {
            let half = if key_type == "public" {
                KeyHalf::Public
            } else {
                KeyHalf::Private
            };
            // Canonical (or constructor-provided PEM/DER) storage → parse →
            // serialize into the requested encoding.
            let parse_result = parse_key_to_pkey(&bytes, None, half);
            let pkey = match parse_result {
                Ok(p) => p,
                Err(e) => {
                    return throw_type_error(cx, &format!("export: {}", e));
                }
            };
            let out = export_pkey_as(pkey, half, type_opt.as_deref(), format_opt.as_deref());
            bssl::EVP_PKEY_free(pkey);
            match out {
                Ok(KeyExportOut::Pem(pem)) => {
                    let c_pem = ZBox::from_bytes(pem.as_bytes());
                    let js_str = JS_NewStringCopyN(
                        cx,
                        c_pem.as_ptr() as *const ::std::os::raw::c_char,
                        pem.len(),
                    );
                    if js_str.is_null() {
                        args.rval().set(UndefinedValue());
                        return true;
                    }
                    rooted!(&in(cx_ref) let sv = mozjs::jsval::StringValue(&*js_str));
                    args.rval().set(sv.get());
                    true
                }
                Ok(KeyExportOut::Der(der)) => {
                    let buf_obj = crate::globals::create_buffer_object(cx, &der);
                    if buf_obj.is_null() {
                        args.rval().set(UndefinedValue());
                        return true;
                    }
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    true
                }
                Err(e) => throw_type_error(cx, &format!("export: {}", e)),
            }
        }
        other => throw_type_error(cx, &format!("export: unknown key type {:?}", other)),
    }
}

/// Resolve the key material input of createPublicKey/createPrivateKey.
///
/// Accepted shapes (Node):
///   - string             → PEM (or DER) bytes
///   - Buffer/TypedArray  → PEM (or DER) bytes
///   - KeyObject          → clone of its stored (canonical DER) material
///   - options object     → {key: string|Buffer, format?: "pem"|"der",
///                           type?: "pkcs8"|"spki"|"pkcs1"|"sec1"}
/// Encrypted keys (passphrase option present) fail closed.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_key_input(
    cx: *mut JSContext,
    val: JSVal,
) -> ::std::result::Result<(Vec<u8>, Option<String>, Option<String>), String> {
    if val.is_string() {
        let s = crate::jsstr_to_rust_string(cx, val.to_string());
        if s.is_empty() {
            return Err("key is empty".to_string());
        }
        return Ok((s.into_bytes(), None, None));
    }
    if !val.is_object() {
        return Err("key must be a string, Buffer, KeyObject or options object".to_string());
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = val.to_object());

    // KeyObject → clone stored material.
    let mut idx_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"_keyIdx".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut idx_val,
        },
    );
    if idx_val.is_int32() {
        let idx = idx_val.to_int32() as usize;
        let stored = KEY_OBJECTS.with(|v| {
            v.borrow()
                .get(idx)
                .map(|k| k.as_ref().map(|b| b.clone()))
                .flatten()
        });
        return stored.ok_or_else(|| "KeyObject key material is no longer available".to_string())
            .map(|b| (b, None, None));
    }

    // Options object {key, format, type, passphrase}.
    let mut key_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"key".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut key_val,
        },
    );
    if !key_val.is_undefined() {
        // Encrypted imports are not supported — fail closed, never silently
        // drop the passphrase.
        let mut pass_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"passphrase".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut pass_val,
            },
        );
        if !pass_val.is_undefined() && !pass_val.is_null() {
            return Err("encrypted keys are not supported (passphrase given)".to_string());
        }
        let bytes = if key_val.is_string() {
            crate::jsstr_to_rust_string(cx, key_val.to_string()).into_bytes()
        } else {
            extract_buffer_bytes(cx, key_val)
        };
        if bytes.is_empty() {
            return Err("options.key is empty".to_string());
        }
        let format = get_string_prop(cx, obj.get(), c"format".as_ptr());
        let typ = get_string_prop(cx, obj.get(), c"type".as_ptr());
        return Ok((bytes, format, typ));
    }

    // Plain Buffer/TypedArray.
    let bytes = extract_buffer_bytes(cx, val);
    if bytes.is_empty() {
        return Err("key must be a string, Buffer, KeyObject or options object".to_string());
    }
    Ok((bytes, None, None))
}

/// Shared createPublicKey/createPrivateKey body: parse input → canonical DER
/// slot → KeyObject with type + asymmetricKeyType.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_asym_key_object(
    cx: *mut JSContext,
    args: &CallArgs,
    argc: u32,
    half: KeyHalf,
) -> bool {
    if argc < 1 {
        return throw_type_error(
            cx,
            if half == KeyHalf::Public {
                "createPublicKey() requires a key"
            } else {
                "createPrivateKey() requires a key"
            },
        );
    }
    let (bytes, format, _type) = match resolve_key_input(cx, *args.get(0).ptr) {
        Ok(t) => t,
        Err(e) => {
            let what = if half == KeyHalf::Public {
                "createPublicKey"
            } else {
                "createPrivateKey"
            };
            return throw_type_error(cx, &format!("{}: {}", what, e));
        }
    };
    let pkey = match parse_key_to_pkey(&bytes, format.as_deref(), half) {
        Ok(p) => p,
        Err(e) => {
            let what = if half == KeyHalf::Public {
                "createPublicKey"
            } else {
                "createPrivateKey"
            };
            return throw_type_error(cx, &format!("{}: {}", what, e));
        }
    };
    let canonical = pkey_canonical_der(pkey, half);
    let kind = pkey_kind_name(pkey);
    bun_boringssl_sys::EVP_PKEY_free(pkey);
    let canonical = match canonical {
        Ok(c) => c,
        Err(e) => {
            let what = if half == KeyHalf::Public {
                "createPublicKey"
            } else {
                "createPrivateKey"
            };
            return throw_type_error(cx, &format!("{}: {}", what, e));
        }
    };
    let idx = alloc_key_object(canonical);
    let key_type = if half == KeyHalf::Public { "public" } else { "private" };
    let obj = make_key_object_js(cx, idx, key_type, Some(kind));
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_public_key(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    create_asym_key_object(cx, &args, argc, KeyHalf::Public)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_private_key(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    create_asym_key_object(cx, &args, argc, KeyHalf::Private)
}

// ============================================================
// RSA publicEncrypt / publicDecrypt / privateEncrypt / privateDecrypt
// Uses bao_crypto::sign::Signer / verify::Verifier for RSA operations
// since bao_crypto::cipher only has symmetric ciphers.
// For RSA encrypt/decrypt we use BoringSSL EVP_PKEY_encrypt/decrypt directly.
// ============================================================

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_public_encrypt(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Args: (key, buffer) — key can be KeyObject, PEM string, or options object
    let data = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        if argc > 0 {
            extract_buffer_bytes(cx, *args.get(0).ptr)
        } else {
            Vec::new()
        }
    };

    // Try to get key PEM from first arg
    let key_pem = if argc > 0 && (*args.get(0).ptr).is_string() {
        Some(crate::jsstr_to_rust_string(
            cx,
            (*args.get(0).ptr).to_string(),
        ))
    } else {
        None
    };

    if let Some(pem) = key_pem {
        // Use BoringSSL RSA_public_encrypt via EVP_PKEY
        let result = rsa_public_encrypt_pem(&data, &pem);
        match result {
            Ok(encrypted) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &encrypted);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    return true;
                }
                args.rval().set(UndefinedValue());
                true
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("publicEncrypt: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"publicEncrypt: key argument required".as_ptr());
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_public_decrypt(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        if argc > 0 {
            extract_buffer_bytes(cx, *args.get(0).ptr)
        } else {
            Vec::new()
        }
    };
    let key_pem = if argc > 0 && (*args.get(0).ptr).is_string() {
        Some(crate::jsstr_to_rust_string(
            cx,
            (*args.get(0).ptr).to_string(),
        ))
    } else {
        None
    };

    if let Some(pem) = key_pem {
        let result = rsa_public_decrypt_pem(&data, &pem);
        match result {
            Ok(decrypted) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &decrypted);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    return true;
                }
                args.rval().set(UndefinedValue());
                true
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("publicDecrypt: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"publicDecrypt: key argument required".as_ptr());
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_private_encrypt(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        if argc > 0 {
            extract_buffer_bytes(cx, *args.get(0).ptr)
        } else {
            Vec::new()
        }
    };
    let key_pem = if argc > 0 && (*args.get(0).ptr).is_string() {
        Some(crate::jsstr_to_rust_string(
            cx,
            (*args.get(0).ptr).to_string(),
        ))
    } else {
        None
    };

    if let Some(pem) = key_pem {
        // privateEncrypt = RSA signing with PKCS1 padding (no digest)
        let result = rsa_private_encrypt_pem(&data, &pem);
        match result {
            Ok(signed) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &signed);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    return true;
                }
                args.rval().set(UndefinedValue());
                true
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("privateEncrypt: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"privateEncrypt: key argument required".as_ptr());
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_private_decrypt(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        if argc > 0 {
            extract_buffer_bytes(cx, *args.get(0).ptr)
        } else {
            Vec::new()
        }
    };
    let key_pem = if argc > 0 && (*args.get(0).ptr).is_string() {
        Some(crate::jsstr_to_rust_string(
            cx,
            (*args.get(0).ptr).to_string(),
        ))
    } else {
        None
    };

    if let Some(pem) = key_pem {
        let result = rsa_private_decrypt_pem(&data, &pem);
        match result {
            Ok(decrypted) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &decrypted);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    return true;
                }
                args.rval().set(UndefinedValue());
                true
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("privateDecrypt: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"privateDecrypt: key argument required".as_ptr());
        false
    }
}

// --- BoringSSL RSA raw operations ---
// Uses RSA_public_encrypt/decrypt + RSA_private_encrypt/decrypt directly
// (these are in bun_boringssl_sys), extracting the RSA key from EVP_PKEY
// with EVP_PKEY_get0_RSA.

fn rsa_public_encrypt_pem(data: &[u8], pem: &str) -> ::std::result::Result<Vec<u8>, String> {
    unsafe {
        let bio = bun_boringssl_sys::BIO_new_mem_buf(
            pem.as_ptr() as *const ::std::ffi::c_void,
            pem.len() as isize,
        );
        if bio.is_null() {
            return Err("BIO_new_mem_buf failed".into());
        }
        let pkey = bun_boringssl_sys::PEM_read_bio_PUBKEY(
            bio,
            ::std::ptr::null_mut(),
            None::<bun_boringssl_sys::pem_password_cb>,
            ::std::ptr::null_mut(),
        );
        bun_boringssl_sys::BIO_free(bio);
        if pkey.is_null() {
            return Err("PEM_read_bio_PUBKEY failed".into());
        }
        let rsa = bun_boringssl_sys::EVP_PKEY_get0_RSA(pkey);
        if rsa.is_null() {
            bun_boringssl_sys::EVP_PKEY_free(pkey);
            return Err("Not an RSA key".into());
        }
        let result = rsa_encrypt_inner(rsa, data);
        bun_boringssl_sys::EVP_PKEY_free(pkey);
        result
    }
}

fn rsa_public_decrypt_pem(data: &[u8], pem: &str) -> ::std::result::Result<Vec<u8>, String> {
    unsafe {
        let bio = bun_boringssl_sys::BIO_new_mem_buf(
            pem.as_ptr() as *const ::std::ffi::c_void,
            pem.len() as isize,
        );
        if bio.is_null() {
            return Err("BIO_new_mem_buf failed".into());
        }
        let pkey = bun_boringssl_sys::PEM_read_bio_PUBKEY(
            bio,
            ::std::ptr::null_mut(),
            None::<bun_boringssl_sys::pem_password_cb>,
            ::std::ptr::null_mut(),
        );
        bun_boringssl_sys::BIO_free(bio);
        if pkey.is_null() {
            return Err("PEM_read_bio_PUBKEY failed".into());
        }
        let rsa = bun_boringssl_sys::EVP_PKEY_get0_RSA(pkey);
        if rsa.is_null() {
            bun_boringssl_sys::EVP_PKEY_free(pkey);
            return Err("Not an RSA key".into());
        }
        let result = rsa_public_decrypt_inner(rsa, data);
        bun_boringssl_sys::EVP_PKEY_free(pkey);
        result
    }
}

fn rsa_private_encrypt_pem(data: &[u8], pem: &str) -> ::std::result::Result<Vec<u8>, String> {
    unsafe {
        let bio = bun_boringssl_sys::BIO_new_mem_buf(
            pem.as_ptr() as *const ::std::ffi::c_void,
            pem.len() as isize,
        );
        if bio.is_null() {
            return Err("BIO_new_mem_buf failed".into());
        }
        let pkey = bun_boringssl_sys::PEM_read_bio_PrivateKey(
            bio,
            ::std::ptr::null_mut(),
            None::<bun_boringssl_sys::pem_password_cb>,
            ::std::ptr::null_mut(),
        );
        bun_boringssl_sys::BIO_free(bio);
        if pkey.is_null() {
            return Err("PEM_read_bio_PrivateKey failed".into());
        }
        let rsa = bun_boringssl_sys::EVP_PKEY_get0_RSA(pkey);
        if rsa.is_null() {
            bun_boringssl_sys::EVP_PKEY_free(pkey);
            return Err("Not an RSA key".into());
        }
        let result = rsa_private_encrypt_inner(rsa, data);
        bun_boringssl_sys::EVP_PKEY_free(pkey);
        result
    }
}

fn rsa_private_decrypt_pem(data: &[u8], pem: &str) -> ::std::result::Result<Vec<u8>, String> {
    unsafe {
        let bio = bun_boringssl_sys::BIO_new_mem_buf(
            pem.as_ptr() as *const ::std::ffi::c_void,
            pem.len() as isize,
        );
        if bio.is_null() {
            return Err("BIO_new_mem_buf failed".into());
        }
        let pkey = bun_boringssl_sys::PEM_read_bio_PrivateKey(
            bio,
            ::std::ptr::null_mut(),
            None::<bun_boringssl_sys::pem_password_cb>,
            ::std::ptr::null_mut(),
        );
        bun_boringssl_sys::BIO_free(bio);
        if pkey.is_null() {
            return Err("PEM_read_bio_PrivateKey failed".into());
        }
        let rsa = bun_boringssl_sys::EVP_PKEY_get0_RSA(pkey);
        if rsa.is_null() {
            bun_boringssl_sys::EVP_PKEY_free(pkey);
            return Err("Not an RSA key".into());
        }
        let result = rsa_private_decrypt_inner(rsa, data);
        bun_boringssl_sys::EVP_PKEY_free(pkey);
        result
    }
}

unsafe fn rsa_encrypt_inner(
    rsa: *mut bun_boringssl_sys::RSA,
    data: &[u8],
) -> ::std::result::Result<Vec<u8>, String> {
    let key_size = bun_boringssl_sys::RSA_size(rsa) as usize;
    let mut out = vec![0u8; key_size];
    let len = bun_boringssl_sys::RSA_public_encrypt(
        data.len(),
        data.as_ptr(),
        out.as_mut_ptr(),
        rsa,
        bun_boringssl_sys::RSA_PKCS1_PADDING,
    );
    if len < 0 {
        return Err("RSA_public_encrypt failed".into());
    }
    out.truncate(len as usize);
    Ok(out)
}

unsafe fn rsa_public_decrypt_inner(
    rsa: *mut bun_boringssl_sys::RSA,
    data: &[u8],
) -> ::std::result::Result<Vec<u8>, String> {
    let key_size = bun_boringssl_sys::RSA_size(rsa) as usize;
    let mut out = vec![0u8; key_size];
    let len = bun_boringssl_sys::RSA_public_decrypt(
        data.len(),
        data.as_ptr(),
        out.as_mut_ptr(),
        rsa,
        bun_boringssl_sys::RSA_PKCS1_PADDING,
    );
    if len < 0 {
        return Err("RSA_public_decrypt failed".into());
    }
    out.truncate(len as usize);
    Ok(out)
}

unsafe fn rsa_private_encrypt_inner(
    rsa: *mut bun_boringssl_sys::RSA,
    data: &[u8],
) -> ::std::result::Result<Vec<u8>, String> {
    let key_size = bun_boringssl_sys::RSA_size(rsa) as usize;
    let mut out = vec![0u8; key_size];
    let len = bun_boringssl_sys::RSA_private_encrypt(
        data.len(),
        data.as_ptr(),
        out.as_mut_ptr(),
        rsa,
        bun_boringssl_sys::RSA_PKCS1_PADDING,
    );
    if len < 0 {
        return Err("RSA_private_encrypt failed".into());
    }
    out.truncate(len as usize);
    Ok(out)
}

unsafe fn rsa_private_decrypt_inner(
    rsa: *mut bun_boringssl_sys::RSA,
    data: &[u8],
) -> ::std::result::Result<Vec<u8>, String> {
    let key_size = bun_boringssl_sys::RSA_size(rsa) as usize;
    let mut out = vec![0u8; key_size];
    let len = bun_boringssl_sys::RSA_private_decrypt(
        data.len(),
        data.as_ptr(),
        out.as_mut_ptr(),
        rsa,
        bun_boringssl_sys::RSA_PKCS1_PADDING,
    );
    if len < 0 {
        return Err("RSA_private_decrypt failed".into());
    }
    out.truncate(len as usize);
    Ok(out)
}

// ============================================================
// Async crypto infrastructure (callback-based)
// Same ConcurrentTask carrier as fs_async (A' route, user-adjudicated
// 2026-08-21): worker completes → complete_post enqueues the JS-thread
// tasklet on the pump's MiniEventLoop (ConcurrentWakeup-registered). The
// former uws_loop_defer path deferred onto the WORKER thread's private
// uWS::Loop — a loop no pump ever ticks, so callbacks never delivered.
// ============================================================

struct CryptoAsyncCtx {
    cx: *mut JSContext,
    /// Raw callback pointer captured at spawn. Prefer `cb_root.get(0)` —
    /// the guard's slot is updated in place by a moving GC; this pointer is
    /// only the fallback for the rooting-failed path.
    callback: *mut JSObject,
    /// RAII heap root for the callback value, spanning the worker-thread
    /// window. Released when this Box drops (tasklet run or the worker-side
    /// fail-closed release), liveness-guarded.
    cb_root: Option<RawValueRootGuard>,
    result: ::std::sync::Arc<::std::sync::Mutex<Option<::std::result::Result<Vec<u8>, String>>>>,
    #[allow(dead_code)]
    op_name: String,
    /// JS thread's pump MiniEventLoop (timers::with_event_loop — the
    /// ConcurrentWakeup-registered instance; NOT dispatch_sm's independent
    /// BaoEventLoop).
    mini_loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
    /// ConcurrentTask carrier embedded in this Box (address-stable).
    concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext,
    /// Prevents duplicate tasklet scheduling.
    has_scheduled: AtomicBool,
}

/// Process-global count of in-flight async crypto ops (spawn-side ++,
/// retired by `CryptoAsyncCtx::drop`) — pump-liveness view across the
/// worker window (same wedge class as fs_async_pending).
static CRYPTO_ASYNC_PENDING: AtomicUsize = AtomicUsize::new(0);

/// In-flight async crypto ops on any thread (spawn-side view for pump
/// liveness).
pub fn crypto_async_pending() -> usize {
    CRYPTO_ASYNC_PENDING.load(AtomicOrdering::Acquire)
}

impl Drop for CryptoAsyncCtx {
    fn drop(&mut self) {
        // Terminal event on EVERY exit path — tasklet ran, or the worker-side
        // fail-closed release — so the pending counter can never hang.
        CRYPTO_ASYNC_PENDING.fetch_sub(1, AtomicOrdering::AcqRel);
    }
}

/// ConcurrentTask shim: runs `crypto_async_defer_callback` on the JS thread
/// when the pump's MiniEventLoop tick pops the carrier.
fn crypto_async_tasklet_shim(ctx: *mut CryptoAsyncCtx, _parent: *mut ()) {
    // SAFETY: ctx is the Box::into_raw pointer bound to the carrier at
    // spawn; the tasklet runs exactly once (sole consumer takes the Box).
    unsafe { crypto_async_defer_callback(ctx) };
}

/// Worker-thread completion (mirror of node_fs::complete_post): CAS-guarded
/// enqueue of the JS-thread tasklet on the captured pump loop; a `false`
/// return (JS thread exited / never registered) releases the carrier Box
/// fail-closed. The RAII root release on this foreign thread leaks the
/// rooted slots by design (bounded — see RawValueRootGuard::drop).
unsafe fn crypto_complete_post(ctx: *mut CryptoAsyncCtx) {
    unsafe {
        if (*ctx)
            .has_scheduled
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_ok()
        {
            let loop_ptr = (*ctx).mini_loop_ptr;
            if !loop_ptr.is_null() {
                let carrier = ::std::ptr::addr_of_mut!((*ctx).concurrent_task);
                // SAFETY: carrier is the embedded task in a live Box; loop_ptr
                // was captured on the JS thread at spawn.
                let delivered =
                    bun_event_loop::ConcurrentWakeup::enqueue_task_concurrent_cross_thread(
                        loop_ptr as *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
                        ::std::ptr::NonNull::new_unchecked(carrier),
                    );
                if !delivered {
                    // JS thread gone: nothing will ever drain the queue.
                    drop(Box::from_raw(ctx));
                }
            } else {
                // Pump loop never captured: same fail-closed release.
                drop(Box::from_raw(ctx));
            }
        }
    }
}

unsafe fn crypto_async_defer_callback(ctx_ptr: *mut CryptoAsyncCtx) {
    let ctx = Box::from_raw(ctx_ptr);
    let cx = ctx.cx;
    // Live callback value: prefer the RAII root's slot (updated in place by
    // a moving GC) over the raw pointer captured at spawn time.
    let cb_value = ctx.cb_root.as_ref().map_or_else(
        || mozjs::jsval::ObjectValue(ctx.callback),
        |g| g.get(0),
    );

    let mut result_guard = ctx.result.lock().unwrap();
    let result_opt = result_guard.take();
    ::std::mem::drop(result_guard);

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cb_val = cb_value);
    // BCE-BUG-ENG-370 (same class as fetch_async/bun_build/fs tasklets):
    // runs from the MiniEventLoop tick OUTSIDE any JS activation — enter the
    // callback object's realm for the dispatch window; realm-derived SM API
    // below (JS_NewPlainObject, string creation, create_buffer_object) would
    // otherwise NULL-deref on cx->realm_ == NULL.
    rooted!(&in(cx_ref) let cb_obj = cb_value.to_object());
    let mut realm = AutoRealm::new_from_handle(cx_ref, cb_obj.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let global = CurrentGlobalOrNull(cx);
    // Drain-time dispatch may run outside any entered realm; fall back to
    // the thread's persistent realm global instead of silently dropping the
    // callback (same convention as timers::fire_js_callback_raw).
    let global = if global.is_null() {
        match bao_engine::context::thread_realm_global() {
            ::std::option::Option::Some(g) if !g.is_null() => g,
            _ => return,
        }
    } else {
        global
    };
    rooted!(&in(realm_cx) let global_rooted = global);

    match result_opt {
        Some(Ok(data)) => {
            let buf_obj = crate::globals::create_buffer_object(cx, &data);
            let val = if buf_obj.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::ObjectValue(buf_obj)
            };
            rooted!(&in(realm_cx) let val_rooted = val);
            let args_arr = [UndefinedValue(), val_rooted.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Err(msg)) => {
            rooted!(&in(realm_cx) let err_obj = JS_NewPlainObject(cx));
            if !err_obj.get().is_null() {
                let c_msg = ZBox::from_bytes(msg.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(realm_cx) let msg_val = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineProperty(
                        cx,
                        err_obj.handle().into(),
                        c"message".as_ptr(),
                        msg_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(realm_cx) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
            let args_arr = [err_val.get()];
            let cb_args = HandleValueArray {
                length_: 1,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        None => {}
    }
    // Terminal unroot is RAII: `ctx` (Box<CryptoAsyncCtx>) drops at the end
    // of this callback, releasing the `cb_root` heap root with the correct
    // registered address on every exit path (including the null-global
    // early return above).
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn spawn_crypto_async<F>(cx: *mut JSContext, op_name: &str, callback: *mut JSObject, work: F)
where
    F: FnOnce() -> ::std::result::Result<Vec<u8>, String> + Send + 'static,
{
    // Heap-root the callback value for the async window via the RAII guard
    // (stable heap slot the GC updates in place; unrooted when the
    // CryptoAsyncCtx Box drops, with the correct registered address).
    let cb_val = mozjs::jsval::ObjectValue(callback);
    let cb_root = unsafe {
        RawValueRootGuard::new(cx, ::std::slice::from_ref(&cb_val), c"crypto_async_cb")
    };

    let result_slot: ::std::sync::Arc<
        ::std::sync::Mutex<Option<::std::result::Result<Vec<u8>, String>>>,
    > = ::std::sync::Arc::new(::std::sync::Mutex::new(None));
    let result_clone = result_slot.clone();

    let op_name_owned = op_name.to_string();

    let ctx = Box::new(CryptoAsyncCtx {
        cx,
        callback,
        cb_root,
        result: result_slot,
        op_name: op_name_owned,
        mini_loop_ptr: ::std::ptr::null(),
        concurrent_task:
            bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext::default(),
        has_scheduled: AtomicBool::new(false),
    });
    // Increment BEFORE the Box can ever drop (Drop retires it) so the pump
    // liveness view can never miss an in-flight op.
    CRYPTO_ASYNC_PENDING.fetch_add(1, AtomicOrdering::Release);
    let ctx_ptr = Box::into_raw(ctx);

    // Capture the JS thread's pump MiniEventLoop — the ConcurrentWakeup-
    // registered instance behind timers::with_event_loop (materializes +
    // registers on first call). NOT dispatch_sm's BaoEventLoop instance.
    // SAFETY: the closure borrows the loop for the call only; the pointer
    // stays valid for the thread's lifetime (leaked at materialization).
    let loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
        crate::timers::with_event_loop(|loop_| loop_ as *const _);
    // SAFETY: ctx_ptr is a live heap allocation we just created; the carrier
    // is embedded, so its address is stable for the Box's lifetime.
    unsafe {
        (*ctx_ptr).mini_loop_ptr = loop_ptr;
        (*ctx_ptr).concurrent_task.from(ctx_ptr, crypto_async_tasklet_shim);
    }

    // Raw pointers are not Send — the worker takes the ctx as an integer
    // token; off-thread it is touched solely via crypto_complete_post's
    // atomics/queue push.
    let ctx_token = ctx_ptr as usize;

    ::std::thread::spawn(move || {
        let result = work();
        {
            let mut slot = result_clone.lock().unwrap();
            *slot = Some(result);
        }
        // SAFETY: ctx_token reconstitutes the live spawn-side Box; sole
        // completion path (CAS-guarded).
        unsafe { crypto_complete_post(ctx_token as *mut CryptoAsyncCtx) };
    });
}

// ============================================================
// Async pbkdf2 — callback variant
// ============================================================

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_pbkdf2(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Args: password, salt, iterations, keylen, digest[, callback]
    let password = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let salt = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        Vec::new()
    };
    let iterations = if argc > 2 && (*args.get(2).ptr).is_int32() {
        (*args.get(2).ptr).to_int32() as u32
    } else {
        100000
    };
    let keylen = if argc > 3 && (*args.get(3).ptr).is_int32() {
        (*args.get(3).ptr).to_int32() as usize
    } else {
        32
    };
    let digest_name = if argc > 4 && (*args.get(4).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(4).ptr).to_string())
    } else {
        "sha256".to_string()
    };

    // PORT NOTE(upstream 614d19fe9): pbkdf2(password, salt, iterations,
    // keylen, callback) — digest omitted, the callback sits in the 5th
    // position. Upstream shuffles `arg4.is_function()` into the callback
    // slot inside `PBKDF2::from_js`; the digest keeps its default then (the
    // `digest_name` reader above already skips a function value).
    let digest_slot_fn = if argc > 4 && (*args.get(4).ptr).is_object() {
        let obj = (*args.get(4).ptr).to_object();
        if JS_ObjectIsFunction(obj) {
            Some(obj)
        } else {
            None
        }
    } else {
        None
    };

    let callback = match digest_slot_fn {
        Some(cb) => Some(cb),
        None if argc > 5 && (*args.get(5).ptr).is_object() => {
            Some((*args.get(5).ptr).to_object())
        }
        None => None,
    };
    if let Some(callback) = callback {
        spawn_crypto_async(cx, "pbkdf2", callback, move || {
            let hash = bao_crypto::kdf::parse_pbkdf2_hash(&digest_name)
                .map_err(|e| format!("pbkdf2: {}", e))?;
            bao_crypto::kdf::pbkdf2(&password, &salt, iterations, hash, keylen)
                .map_err(|e| format!("pbkdf2: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        // Sync fallback (no callback provided)
        match bao_crypto::kdf::parse_pbkdf2_hash(&digest_name) {
            Ok(hash) => match bao_crypto::kdf::pbkdf2(&password, &salt, iterations, hash, keylen) {
                Ok(key) => {
                    let buf_obj = crate::globals::create_buffer_object(cx, &key);
                    if !buf_obj.is_null() {
                        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                        true
                    } else {
                        args.rval().set(UndefinedValue());
                        true
                    }
                }
                Err(e) => {
                    let c_msg = ZBox::from_bytes(format!("pbkdf2: {}", e).as_bytes());
                    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                    false
                }
            },
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("pbkdf2: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    }
}

// ============================================================
// Async scrypt — callback variant
// ============================================================

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_scrypt(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let password = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let salt = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        Vec::new()
    };
    let keylen = if argc > 2 && (*args.get(2).ptr).is_int32() {
        (*args.get(2).ptr).to_int32() as usize
    } else {
        32
    };
    let options_val = if argc > 3 {
        *args.get(3).ptr
    } else {
        UndefinedValue()
    };

    let (n, r, p) = if options_val.is_object() {
        let mut wrapped_cx2 =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let opts_obj = options_val.to_object());
        let mut n_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"N".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut n_val,
            },
        );
        let n = if n_val.is_int32() {
            n_val.to_int32() as u64
        } else if n_val.is_double() {
            n_val.to_double() as u64
        } else {
            16384
        };
        let mut r_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"r".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut r_val,
            },
        );
        let r = if r_val.is_int32() {
            r_val.to_int32() as u64
        } else if r_val.is_double() {
            r_val.to_double() as u64
        } else {
            8
        };
        let mut p_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"p".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut p_val,
            },
        );
        let p = if p_val.is_int32() {
            p_val.to_int32() as u64
        } else if p_val.is_double() {
            p_val.to_double() as u64
        } else {
            1
        };
        (n, r, p)
    } else {
        (16384, 8, 1)
    };

    let has_callback = argc > 4 && (*args.get(4).ptr).is_object();
    if has_callback {
        let callback = (*args.get(4).ptr).to_object();
        spawn_crypto_async(cx, "scrypt", callback, move || {
            bao_crypto::kdf::scrypt(&password, &salt, n, r, p, keylen)
                .map_err(|e| format!("scrypt: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        match bao_crypto::kdf::scrypt(&password, &salt, n, r, p, keylen) {
            Ok(key) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &key);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    true
                } else {
                    args.rval().set(UndefinedValue());
                    true
                }
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("scrypt: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    }
}

// ============================================================
// Async generateKeyPair — callback variant
// ============================================================

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_key_pair(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let key_type = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(0).ptr).to_string())
    } else {
        "rsa".to_string()
    };

    // Parse options from arg 1
    let options_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mut rsa_bits = 2048usize;
    let mut ec_curve = "P-256".to_string();
    if options_val.is_object() {
        let mut wrapped_cx2 =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let opts_obj = options_val.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"modulusLength".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        if len_val.is_int32() {
            rsa_bits = len_val.to_int32() as usize;
        }
        let mut curve_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"namedCurve".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut curve_val,
            },
        );
        if curve_val.is_string() {
            ec_curve = crate::jsstr_to_rust_string(cx, curve_val.to_string());
        }
    }

    let has_callback = argc > 2 && (*args.get(2).ptr).is_object();
    if has_callback {
        let callback = (*args.get(2).ptr).to_object();
        let kt = key_type.clone();
        spawn_crypto_async(cx, "generateKeyPair", callback, move || {
            let kp_type = match kt.to_lowercase().as_str() {
                "rsa" => bao_crypto::keypair::KeyPairType::Rsa { bits: rsa_bits },
                "ec" => {
                    let curve = match ec_curve.as_str() {
                        "P-384" | "secp384r1" => bao_crypto::keypair::EcCurve::P384,
                        _ => bao_crypto::keypair::EcCurve::P256,
                    };
                    bao_crypto::keypair::KeyPairType::Ec { curve }
                }
                "ed25519" => bao_crypto::keypair::KeyPairType::Ed25519,
                "x25519" => bao_crypto::keypair::KeyPairType::X25519,
                _ => return Err(format!("generateKeyPair: unsupported type '{}'", kt)),
            };
            bao_crypto::keypair::generate_key_pair(&kp_type)
                .map(|_result| Vec::new()) // KeyPairResult is serialized separately
                .map_err(|e| format!("generateKeyPair: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        JS_ReportErrorUTF8(cx, c"generateKeyPair requires a callback".as_ptr());
        false
    }
}

// ============================================================
// randomBytes — async-capable (replaces the sync-only version)
// ============================================================


#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_sign_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Args: algorithm, data, key
    let algo = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(0).ptr).to_string())
    } else {
        "SHA256".to_string()
    };
    let data = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        Vec::new()
    };
    let key_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };

    // Key material first (string PEM | KeyObject slot | DER buffer): the
    // signature family is resolved from the KEY KIND when the algorithm
    // string is a bare digest — Node semantics (crypto.sign('SHA256', data,
    // ecPrivateKey) is ECDSA, not RSA-PKCS1v15; same class as the
    // createSign fix routed through resolve_sign_algorithm_for_key).
    let key_bytes: ::std::result::Result<Vec<u8>, bao_crypto::CryptoError> = if key_val.is_string()
    {
        Ok(crate::jsstr_to_rust_string(cx, key_val.to_string()).into_bytes())
    } else if key_val.is_object() {
        let mut wrapped_cx2 =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let obj = key_val.to_object());
        let mut idx_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_keyIdx".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut idx_val,
            },
        );
        if idx_val.is_int32() {
            let idx = idx_val.to_int32() as usize;
            let stored = KEY_OBJECTS.with(|v| {
                v.borrow()
                    .get(idx)
                    .map(|k| k.as_ref().map(|b| b.clone()))
                    .flatten()
            });
            stored.ok_or_else(|| {
                bao_crypto::CryptoError::InvalidKey(
                    "KeyObject key data not available".into(),
                )
            })
        } else {
            Ok(extract_buffer_bytes(cx, key_val))
        }
    } else {
        Err(bao_crypto::CryptoError::InvalidKey(
            "sign: key argument required".into(),
        ))
    };

    let signer = key_bytes.and_then(|key| {
        let sign_algo = resolve_sign_algorithm_for_key(&algo, &key).ok_or_else(|| {
            bao_crypto::CryptoError::InvalidKey(format!(
                "sign: unrecognized algorithm {:?} for the given key",
                algo
            ))
        })?;
        if looks_like_pem_key(&key) {
            let pem = String::from_utf8_lossy(&key).into_owned();
            bao_crypto::sign::Signer::from_pkcs8_pem(&sign_algo, &pem)
        } else {
            bao_crypto::sign::Signer::from_pkcs8_der(&sign_algo, &key)
        }
    });

    match signer {
        Ok(s) => match s.sign(&data, bao_crypto::sign::SignatureFormat::Der) {
            Ok(sig) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &sig);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    true
                } else {
                    args.rval().set(UndefinedValue());
                    true
                }
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("sign: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        },
        Err(e) => {
            let c_msg = ZBox::from_bytes(format!("sign: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_verify_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Args: algorithm, data, key, signature
    let algo = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(0).ptr).to_string())
    } else {
        "SHA256".to_string()
    };
    let data = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        Vec::new()
    };
    let key_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let signature = if argc > 3 {
        extract_buffer_bytes(cx, *args.get(3).ptr)
    } else {
        Vec::new()
    };

    // Key material first (same discipline as crypto_sign_sync): bare digest
    // names resolve the family from the KEY KIND, not an RSA default.
    let key_bytes: ::std::result::Result<Vec<u8>, bao_crypto::CryptoError> = if key_val.is_string()
    {
        Ok(crate::jsstr_to_rust_string(cx, key_val.to_string()).into_bytes())
    } else if key_val.is_object() {
        let mut wrapped_cx2 =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let obj = key_val.to_object());
        let mut idx_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_keyIdx".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut idx_val,
            },
        );
        if idx_val.is_int32() {
            let idx = idx_val.to_int32() as usize;
            let stored = KEY_OBJECTS.with(|v| {
                v.borrow()
                    .get(idx)
                    .map(|k| k.as_ref().map(|b| b.clone()))
                    .flatten()
            });
            stored.ok_or_else(|| {
                bao_crypto::CryptoError::InvalidKey(
                    "KeyObject key data not available".into(),
                )
            })
        } else {
            Ok(extract_buffer_bytes(cx, key_val))
        }
    } else {
        Err(bao_crypto::CryptoError::InvalidKey(
            "verify: key argument required".into(),
        ))
    };

    let verifier = key_bytes.and_then(|key| {
        let sign_algo = resolve_sign_algorithm_for_key(&algo, &key).ok_or_else(|| {
            bao_crypto::CryptoError::InvalidKey(format!(
                "verify: unrecognized algorithm {:?} for the given key",
                algo
            ))
        })?;
        // Public form first, then private (Node allows verifying with a
        // private KeyObject).
        if looks_like_pem_key(&key) {
            let pem = String::from_utf8_lossy(&key).into_owned();
            bao_crypto::verify::Verifier::from_public_pem(&sign_algo, &pem)
                .or_else(|_| bao_crypto::verify::Verifier::from_pkcs8_pem(&sign_algo, &pem))
        } else {
            bao_crypto::verify::Verifier::from_public_der(&sign_algo, &key)
                .or_else(|_| bao_crypto::verify::Verifier::from_pkcs8_der(&sign_algo, &key))
        }
    });

    match verifier {
        Ok(v) => match v.verify(&data, &signature, bao_crypto::sign::SignatureFormat::Der) {
            Ok(result) => {
                args.rval().set(mozjs::jsval::BooleanValue(result));
                true
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("verify: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        },
        Err(e) => {
            let c_msg = ZBox::from_bytes(format!("verify: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

// ============================================================
// generateKey / generateKeySync — secret key generation
// ============================================================

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let _type = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(0).ptr).to_string())
    } else {
        "hmac".to_string()
    };
    let options_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let mut length = 32usize;
    if options_val.is_object() {
        let mut wrapped_cx2 =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let opts_obj = options_val.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        if len_val.is_int32() {
            length = len_val.to_int32() as usize;
        } else if len_val.is_double() {
            length = len_val.to_double() as usize;
        }
    }

    let has_callback = argc > 2 && (*args.get(2).ptr).is_object();
    if has_callback {
        let callback = (*args.get(2).ptr).to_object();
        spawn_crypto_async(cx, "generateKey", callback, move || {
            let mut buf = vec![0u8; length];
            bao_crypto::random::rand_bytes(&mut buf)
                .map(|_| buf)
                .map_err(|e| format!("generateKey: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        JS_ReportErrorUTF8(cx, c"generateKey requires a callback".as_ptr());
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_key_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let _type = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(0).ptr).to_string())
    } else {
        "hmac".to_string()
    };
    let options_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let mut length = 32usize;
    if options_val.is_object() {
        let mut wrapped_cx2 =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref2 = &mut wrapped_cx2;
        rooted!(&in(cx_ref2) let opts_obj = options_val.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_obj.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        if len_val.is_int32() {
            length = len_val.to_int32() as usize;
        } else if len_val.is_double() {
            length = len_val.to_double() as usize;
        }
    }

    let mut buf = vec![0u8; length];
    bao_crypto::random::rand_bytes(&mut buf).unwrap();
    let idx = alloc_key_object(buf);
    let obj = make_key_object_js(cx, idx, "secret", None);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

// ============================================================
// hkdf (async callback variant)
// ============================================================

fn parse_hkdf_hash(name: &str) -> bao_crypto::kdf::HkdfHash {
    match name.to_lowercase().as_str() {
        "sha1" => bao_crypto::kdf::HkdfHash::Sha1,
        _ => bao_crypto::kdf::HkdfHash::Sha256,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_hkdf(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let digest = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::jsstr_to_rust_string(cx, (*args.get(0).ptr).to_string())
    } else {
        "SHA256".to_string()
    };
    let ikm = if argc > 1 {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        Vec::new()
    };
    let salt = if argc > 2 {
        extract_buffer_bytes(cx, *args.get(2).ptr)
    } else {
        Vec::new()
    };
    let info = if argc > 3 {
        extract_buffer_bytes(cx, *args.get(3).ptr)
    } else {
        Vec::new()
    };
    let keylen = if argc > 4 && (*args.get(4).ptr).is_int32() {
        (*args.get(4).ptr).to_int32() as usize
    } else {
        32
    };

    let has_callback = argc > 5 && (*args.get(5).ptr).is_object();
    if has_callback {
        let callback = (*args.get(5).ptr).to_object();
        let hash = parse_hkdf_hash(&digest);
        spawn_crypto_async(cx, "hkdf", callback, move || {
            bao_crypto::kdf::hkdf(hash, &salt, &ikm, &info, keylen)
                .map_err(|e| format!("hkdf: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        let hash = parse_hkdf_hash(&digest);
        match bao_crypto::kdf::hkdf(hash, &salt, &ikm, &info, keylen) {
            Ok(key) => {
                let buf_obj = crate::globals::create_buffer_object(cx, &key);
                if !buf_obj.is_null() {
                    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
                    true
                } else {
                    args.rval().set(UndefinedValue());
                    true
                }
            }
            Err(e) => {
                let c_msg = ZBox::from_bytes(format!("hkdf: {}", e).as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                false
            }
        }
    }
}

// ============================================================
// checkPrime / checkPrimeSync — probabilistic primality test
// Uses BoringSSL BN_is_prime_ex for Miller-Rabin test
// ============================================================

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_check_prime(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let candidate = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };

    let has_callback = argc > 1 && (*args.get(1).ptr).is_object();
    if has_callback {
        let callback = (*args.get(1).ptr).to_object();
        spawn_crypto_async(cx, "checkPrime", callback, move || {
            let is_prime = check_prime_boringssl(&candidate);
            Ok(vec![if is_prime { 1u8 } else { 0u8 }])
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        JS_ReportErrorUTF8(cx, c"checkPrime requires a callback".as_ptr());
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_check_prime_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let candidate = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };

    let is_prime = check_prime_boringssl(&candidate);
    args.rval().set(mozjs::jsval::BooleanValue(is_prime));
    true
}

fn check_prime_boringssl(bytes: &[u8]) -> bool {
    unsafe {
        let bn = bun_boringssl_sys::BN_bin2bn(bytes.as_ptr(), bytes.len(), ::std::ptr::null_mut());
        if bn.is_null() {
            return false;
        }
        // BN_is_prime_fasttest_ex checks with 64 rounds of Miller-Rabin
        let result = bun_boringssl_sys::BN_is_prime_fasttest_ex(
            bn,
            64,
            ::std::ptr::null_mut(),
            0,
            ::std::ptr::null_mut(),
        );
        bun_boringssl_sys::BN_free(bn);
        result == 1
    }
}

// ---- Local BoringSSL FFI declarations (symbols present in linked libboringssl.a) ----

unsafe extern "C" {
    fn BN_generate_prime_ex(
        ret: *mut bun_boringssl_sys::BIGNUM,
        bits: core::ffi::c_int,
        safe: core::ffi::c_int,
        add: *const bun_boringssl_sys::BIGNUM,
        rem: *const bun_boringssl_sys::BIGNUM,
        cb: *mut core::ffi::c_void,
    ) -> core::ffi::c_int;

    fn d2i_NETSCAPE_SPKAC(
        out: *mut *mut NETSCAPE_SPKAC,
        inp: *mut *const u8,
        len: core::ffi::c_long,
    ) -> *mut NETSCAPE_SPKAC;

    fn NETSCAPE_SPKAC_free(spac: *mut NETSCAPE_SPKAC);
}

/// Opaque type for BoringSSL NETSCAPE_SPKAC structure.
#[repr(C)]
struct NETSCAPE_SPKAC {
    _private: [u8; 0],
}

// ---- Certificate class (SPKAC) ----

unsafe fn parse_spkac(der: &[u8]) -> Option<*mut NETSCAPE_SPKAC> {
    let mut p = der.as_ptr();
    let spkac = d2i_NETSCAPE_SPKAC(::std::ptr::null_mut(), &mut p, der.len() as libc::c_long);
    if spkac.is_null() { None } else { Some(spkac) }
}

fn extract_spkac_challenge(der: &[u8]) -> String {
    if der.len() < 4 {
        return String::new();
    }
    let mut pos = 0;
    if der[pos] != 0x30 {
        return String::new();
    }
    pos += 1;
    pos += asn1_length_size(&der[pos..]);
    if pos >= der.len() || der[pos] != 0x16 {
        return String::new();
    }
    pos += 1;
    if pos >= der.len() {
        return String::new();
    }
    let str_len = der[pos] as usize;
    pos += 1;
    if pos + str_len > der.len() {
        return String::new();
    }
    String::from_utf8_lossy(&der[pos..pos + str_len]).into_owned()
}

fn asn1_length_size(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 1;
    }
    if buf[0] & 0x80 == 0 {
        1
    } else {
        1 + (buf[0] & 0x7f) as usize
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_certificate_ctor(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"verifySpkac".as_ptr(),
        Some(cert_verify_spkac),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"exportPublicKey".as_ptr(),
        Some(cert_export_public_key),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"exportChallenge".as_ptr(),
        Some(cert_export_challenge),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"exportSpkac".as_ptr(),
        Some(cert_export_spkac),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"verifyPublicKey".as_ptr(),
        Some(cert_verify_public_key),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cert_verify_spkac(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "verifySpkac() requires a buffer");
    }
    let buf = extract_buffer_bytes(cx, *args.get(0).ptr);
    let valid = if let Some(spkac) = parse_spkac(&buf) {
        NETSCAPE_SPKAC_free(spkac);
        true
    } else {
        false
    };
    args.rval().set(mozjs::jsval::BooleanValue(valid));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cert_export_public_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "exportPublicKey() requires a buffer");
    }
    let buf = extract_buffer_bytes(cx, *args.get(0).ptr);
    let buf_obj = crate::globals::create_buffer_object(cx, &buf);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cert_export_challenge(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "exportChallenge() requires a buffer");
    }
    let buf = extract_buffer_bytes(cx, *args.get(0).ptr);
    let challenge = extract_spkac_challenge(&buf);
    return_string(cx, &args, &challenge)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cert_export_spkac(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "exportSpkac() requires a buffer");
    }
    let buf = extract_buffer_bytes(cx, *args.get(0).ptr);
    let buf_obj = crate::globals::create_buffer_object(cx, &buf);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cert_verify_public_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "verifyPublicKey() requires a key buffer");
    }
    let key_bytes = extract_buffer_bytes(cx, *args.get(0).ptr);
    let valid = {
        let mut p = key_bytes.as_ptr();
        let pkey = bun_boringssl_sys::d2i_PUBKEY(
            ::std::ptr::null_mut(),
            &mut p,
            key_bytes.len() as libc::c_long,
        );
        if pkey.is_null() {
            false
        } else {
            bun_boringssl_sys::EVP_PKEY_free(pkey);
            true
        }
    };
    args.rval().set(mozjs::jsval::BooleanValue(valid));
    true
}

// ---- getCurves ----

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_curves(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let curves = [
        "P-256",
        "prime256v1",
        "secp256r1",
        "P-384",
        "secp384r1",
        "P-521",
        "secp521r1",
        "X25519",
        "Ed25519",
        "X448",
        "Ed448",
        "secp256k1",
    ];
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, curves.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (i, name) in curves.iter().enumerate() {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
            JS_DefineElement(
                cx,
                arr.handle().into(),
                i as u32,
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

// ---- getCipherInfo ----

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_cipher_info(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "getCipherInfo() requires a cipher name");
    }
    let name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "getCipherInfo() name must be a string"),
    };
    let algo = match bao_crypto::cipher::parse_algorithm(&name) {
        Ok(a) => a,
        Err(_) => {
            args.rval().set(UndefinedValue());
            return true;
        }
    };
    let (key_len, iv_len, mode, block_size) = match algo {
        bao_crypto::cipher::CipherAlgorithm::Aes128Cbc => (16, 16, "cbc", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes192Cbc => (24, 16, "cbc", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes256Cbc => (32, 16, "cbc", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes128Ctr => (16, 16, "ctr", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes192Ctr => (24, 16, "ctr", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes256Ctr => (32, 16, "ctr", 16),
        bao_crypto::cipher::CipherAlgorithm::DesEde3Cbc => (24, 8, "cbc", 8),
        bao_crypto::cipher::CipherAlgorithm::Aes128Gcm => (16, 12, "gcm", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes192Gcm => (24, 12, "gcm", 16),
        bao_crypto::cipher::CipherAlgorithm::Aes256Gcm => (32, 12, "gcm", 16),
        bao_crypto::cipher::CipherAlgorithm::ChaCha20Poly1305 => (32, 12, "ccm", 1),
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    set_string_prop(cx, obj.get(), c"name".as_ptr(), &name);
    let v = mozjs::jsval::Int32Value(key_len as i32);
    rooted!(&in(cx_ref) let kv = v);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"keyLength".as_ptr(),
        kv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    let v2 = mozjs::jsval::Int32Value(iv_len as i32);
    rooted!(&in(cx_ref) let ivv = v2);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"ivLength".as_ptr(),
        ivv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    set_string_prop(cx, obj.get(), c"mode".as_ptr(), mode);
    let v3 = mozjs::jsval::Int32Value(block_size as i32);
    rooted!(&in(cx_ref) let bsv = v3);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"blockSize".as_ptr(),
        bsv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

// ---- randomInt ----

fn next_pow2(v: u64) -> u64 {
    if v == 0 {
        return 1;
    }
    let mut n = v - 1;
    n |= n >> 1;
    n |= n >> 2;
    n |= n >> 4;
    n |= n >> 8;
    n |= n >> 16;
    n |= n >> 32;
    n + 1
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_int(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (min, max) = if argc == 1 {
        let v = *args.get(0).ptr;
        let m = if v.is_int32() {
            v.to_int32() as i64
        } else if v.is_double() {
            v.to_double() as i64
        } else {
            return throw_type_error(cx, "randomInt() argument must be a number");
        };
        (0i64, m)
    } else if argc >= 2 {
        let v0 = *args.get(0).ptr;
        let v1 = *args.get(1).ptr;
        let lo = if v0.is_int32() {
            v0.to_int32() as i64
        } else if v0.is_double() {
            v0.to_double() as i64
        } else {
            return throw_type_error(cx, "randomInt() min must be a number");
        };
        let hi = if v1.is_int32() {
            v1.to_int32() as i64
        } else if v1.is_double() {
            v1.to_double() as i64
        } else {
            return throw_type_error(cx, "randomInt() max must be a number");
        };
        (lo, hi)
    } else {
        return throw_type_error(cx, "randomInt() requires at least one argument");
    };
    if min >= max {
        return throw_type_error(cx, "randomInt() min must be less than max");
    }
    let range = (max - min) as u64;
    let mask = if range.is_power_of_two() {
        range - 1
    } else {
        next_pow2(range) - 1
    };
    let num_bytes = ((64 - mask.leading_zeros() + 7) / 8) as usize;
    let mut buf = [0u8; 8];
    let result = loop {
        bao_crypto::random::rand_bytes(&mut buf[..num_bytes]).unwrap();
        let mut r = 0u64;
        for &b in &buf[..num_bytes] {
            r = (r << 8) | b as u64;
        }
        r &= mask;
        if r < range {
            break min + r as i64;
        }
    };
    args.rval().set(mozjs::jsval::DoubleValue(result as f64));
    true
}

// ---- randomFillSync / randomFill ----

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_fill_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 || !(*args.get(0).ptr).is_object() {
        return throw_type_error(cx, "randomFillSync() requires a buffer");
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let buf_obj = (*args.get(0).ptr).to_object());
    let offset = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else if v.is_double() {
            v.to_double() as usize
        } else {
            0
        }
    } else {
        0
    };
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ptr::null_mut();
    let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        buf_obj.get(),
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if unwrapped.is_null() {
        let mut vl: usize = 0;
        let mut vs = false;
        let mut vd: *mut u8 = ptr::null_mut();
        let vu = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
            buf_obj.get(),
            &mut vl,
            &mut vs,
            &mut vd,
        );
        if vu.is_null() {
            return throw_type_error(cx, "randomFillSync() requires a TypedArray");
        }
        length = vl;
        data_ptr = vd;
    }
    let size = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else if v.is_double() {
            v.to_double() as usize
        } else {
            length - offset
        }
    } else {
        length - offset
    };
    if offset + size > length {
        return throw_type_error(cx, "randomFillSync() offset + size exceeds buffer length");
    }
    if !data_ptr.is_null() && size > 0 {
        let slice = ::std::slice::from_raw_parts_mut(data_ptr.add(offset), size);
        bao_crypto::random::rand_bytes(slice).unwrap();
    }
    args.rval().set(mozjs::jsval::ObjectValue(buf_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_fill(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 || !(*args.get(0).ptr).is_object() {
        return throw_type_error(cx, "randomFill() requires a buffer");
    }
    let callback_idx = if argc > 3 && (*args.get(3).ptr).is_object() {
        Some(3u32)
    } else if argc > 2 && (*args.get(2).ptr).is_object() && !(*args.get(2).ptr).is_number() {
        Some(2u32)
    } else if argc > 1 && (*args.get(1).ptr).is_object() && !(*args.get(1).ptr).is_number() {
        Some(1u32)
    } else {
        None
    };
    if let Some(idx) = callback_idx {
        let buf_data = extract_buffer_bytes(cx, *args.get(0).ptr);
        let callback = (*args.get(idx).ptr).to_object();
        spawn_crypto_async(cx, "randomFill", callback, move || {
            let mut filled = buf_data;
            bao_crypto::random::rand_bytes(&mut filled)
                .map(|_| filled)
                .map_err(|e| format!("randomFill: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        crypto_random_fill_sync(cx, argc, vp)
    }
}

// ---- generatePrimeSync / generatePrime ----

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_prime_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "generatePrimeSync() requires a bit size");
    }
    let size_val = *args.get(0).ptr;
    let bits = if size_val.is_int32() {
        size_val.to_int32()
    } else if size_val.is_double() {
        size_val.to_double() as i32
    } else {
        return throw_type_error(cx, "generatePrimeSync() size must be a number");
    };
    if bits < 2 {
        return throw_type_error(cx, "generatePrimeSync() size must be at least 2");
    }
    let safe = if argc > 1 && (*args.get(1).ptr).is_object() {
        let mut wcx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cr = &mut wcx;
        rooted!(&in(cr) let opts = (*args.get(1).ptr).to_object());
        let mut sv = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"safe".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut sv,
            },
        );
        if sv.is_boolean() {
            sv.to_boolean()
        } else {
            true
        }
    } else {
        true
    };
    let prime_bytes = {
        let bn = bun_boringssl_sys::BN_new();
        if bn.is_null() {
            return throw_type_error(cx, "generatePrimeSync() BN_new failed");
        }
        let result = BN_generate_prime_ex(
            bn,
            bits,
            if safe { 1 } else { 0 },
            ::std::ptr::null(),
            ::std::ptr::null(),
            ::std::ptr::null_mut(),
        );
        if result != 1 {
            bun_boringssl_sys::BN_free(bn);
            return throw_type_error(cx, "generatePrimeSync() generation failed");
        }
        let num_bytes = ((bun_boringssl_sys::BN_num_bits(bn) + 7) / 8) as usize;
        let mut out = vec![0u8; num_bytes];
        bun_boringssl_sys::BN_bn2bin(bn, out.as_mut_ptr());
        bun_boringssl_sys::BN_free(bn);
        out
    };
    let buf_obj = crate::globals::create_buffer_object(cx, &prime_bytes);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_prime(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let bits = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32()
        } else if v.is_double() {
            v.to_double() as i32
        } else {
            256
        }
    } else {
        256
    };
    let safe = if argc > 1 && (*args.get(1).ptr).is_object() {
        let mut wcx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cr = &mut wcx;
        rooted!(&in(cr) let opts = (*args.get(1).ptr).to_object());
        let mut sv = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"safe".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut sv,
            },
        );
        if sv.is_boolean() {
            sv.to_boolean()
        } else {
            true
        }
    } else {
        true
    };
    let has_callback = argc > 2 && (*args.get(2).ptr).is_object();
    if has_callback {
        let callback = (*args.get(2).ptr).to_object();
        spawn_crypto_async(cx, "generatePrime", callback, move || {
            let bn = bun_boringssl_sys::BN_new();
            if bn.is_null() {
                return Err("generatePrime: BN_new failed".into());
            }
            let result = BN_generate_prime_ex(
                bn,
                bits,
                if safe { 1 } else { 0 },
                ::std::ptr::null(),
                ::std::ptr::null(),
                ::std::ptr::null_mut(),
            );
            if result != 1 {
                bun_boringssl_sys::BN_free(bn);
                return Err("generatePrime: generation failed".into());
            }
            let num_bytes = ((bun_boringssl_sys::BN_num_bits(bn) + 7) / 8) as usize;
            let mut out = vec![0u8; num_bytes];
            bun_boringssl_sys::BN_bn2bin(bn, out.as_mut_ptr());
            bun_boringssl_sys::BN_free(bn);
            Ok(out)
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        crypto_generate_prime_sync(cx, argc, vp)
    }
}

// ---- crypto.hash (one-shot) ----

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_hash(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        return throw_type_error(cx, "crypto.hash() requires algorithm and input");
    }
    let algo = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "crypto.hash() algorithm must be a string"),
    };
    let input = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr).into_bytes()
    } else if (*args.get(1).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    } else {
        return throw_type_error(cx, "crypto.hash() input must be a string or Buffer");
    };
    let output_encoding = if argc > 2 {
        arg_to_string(cx, *args.get(2).ptr).map(|s| s.to_lowercase())
    } else {
        Some("hex".to_string())
    };
    let result = match algo.as_str() {
        "sha256" => {
            let mut h = bun_sha_hmac::SHA256::init();
            h.update(&input);
            let mut out = [0u8; 32];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha512" => {
            let mut h = bun_sha_hmac::SHA512::init();
            h.update(&input);
            let mut out = [0u8; 64];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha384" => {
            let mut h = bun_sha_hmac::SHA384::init();
            h.update(&input);
            let mut out = [0u8; 48];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha224" => {
            let mut h = bun_sha_hmac::SHA224::init();
            h.update(&input);
            let mut out = [0u8; 28];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha1" => {
            let mut h = bun_sha_hmac::SHA1::init();
            h.update(&input);
            let mut out = [0u8; 20];
            h.r#final(&mut out);
            out.to_vec()
        }
        "md5" => {
            let mut h = bun_sha_hmac::MD5::init();
            h.update(&input);
            let mut out = [0u8; 16];
            h.r#final(&mut out);
            out.to_vec()
        }
        _ => {
            return throw_type_error(
                cx,
                &format!("crypto.hash() unsupported algorithm: {}", algo),
            );
        }
    };
    match output_encoding.as_deref() {
        Some("hex") => return_string(cx, &args, &hex::encode(&result)),
        Some("base64") => {
            let eb = bun_base64::encode_alloc(&result);
            let s = ::std::str::from_utf8(&eb).unwrap_or("").to_owned();
            return_string(cx, &args, &s)
        }
        Some("buffer") => {
            let bo = crate::globals::create_buffer_object(cx, &result);
            if bo.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::ObjectValue(bo));
            }
            true
        }
        _ => return_string(cx, &args, &hex::encode(&result)),
    }
}

// ---- DiffieHellmanGroup / getDiffieHellman / diffieHellman ----

fn modp_prime(group: &str) -> Option<Vec<u8>> {
    let hex = match group {
        "modp1" => {
            "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF"
        }
        "modp2" => {
            "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AACAA68FFFFFFFFFFFFFFFF"
        }
        "modp5" => {
            "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AA9420000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
        }
        "modp14" => {
            "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AACAA68FFFFFFFFFFFFFFFF"
        }
        _ => return None,
    };
    Some(hex::decode(hex).unwrap_or_default())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_diffie_hellman_group(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 {
        return throw_type_error(cx, "createDiffieHellmanGroup() requires a group name");
    }
    let group_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "group must be a string"),
    };
    let prime = match modp_prime(&group_name) {
        Some(p) => p,
        None => return throw_type_error(cx, &format!("Unsupported DH group: {}", group_name)),
    };
    let dh = match bao_crypto::dh::DiffieHellman::from_prime(&prime, 2) {
        Ok(d) => d,
        Err(e) => {
            return throw_type_error(cx, &format!("createDiffieHellmanGroup() failed: {}", e));
        }
    };
    let id = dh_registry_insert(dh);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    store_dh_id(cx, obj.get(), id);
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"generateKeys".as_ptr(),
        Some(dh_generate_keys),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"computeSecret".as_ptr(),
        Some(dh_compute_secret),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPrime".as_ptr(),
        Some(dh_get_prime),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getGenerator".as_ptr(),
        Some(dh_get_generator),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPublicKey".as_ptr(),
        Some(dh_get_public_key),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"getPrivateKey".as_ptr(),
        Some(dh_get_private_key),
        0,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_diffie_hellman(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        return throw_type_error(cx, "diffieHellman() requires two key arguments");
    }
    let key1 = extract_buffer_bytes(cx, *args.get(0).ptr);
    let key2 = extract_buffer_bytes(cx, *args.get(1).ptr);
    let dh = match bao_crypto::dh::DiffieHellman::from_prime(&key1, 2) {
        Ok(d) => d,
        Err(e) => return throw_type_error(cx, &format!("diffieHellman() failed: {}", e)),
    };
    let mut dh_obj = dh;
    let _pub_key = match dh_obj.generate_keys() {
        Ok(k) => k,
        Err(e) => {
            return throw_type_error(cx, &format!("diffieHellman() generateKeys failed: {}", e));
        }
    };
    let secret = match dh_obj.compute_secret(&key2) {
        Ok(s) => s,
        Err(e) => {
            return throw_type_error(cx, &format!("diffieHellman() computeSecret failed: {}", e));
        }
    };
    let buf_obj = crate::globals::create_buffer_object(cx, &secret);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v4_format() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[8..9], "-");
        assert_eq!(&u[13..14], "-");
        assert_eq!(&u[18..19], "-");
        assert_eq!(&u[23..24], "-");
    }

    #[test]
    fn uuid_v4_version_and_variant() {
        let u = uuid_v4();
        assert_eq!(&u[14..15], "4", "version nibble must be 4");
        let v = u.as_bytes()[19];
        assert!(
            matches!(v, b'8' | b'9' | b'a' | b'b'),
            "variant must be 8/9/a/b, got {}",
            v as char
        );
    }

    #[test]
    fn uuid_v4_all_hex() {
        let u = uuid_v4();
        for (i, c) in u.chars().enumerate() {
            if i == 8 || i == 13 || i == 18 || i == 23 {
                assert_eq!(c, '-');
            } else {
                assert!(c.is_ascii_hexdigit(), "pos {} must be hex, got {}", i, c);
            }
        }
    }

    #[test]
    fn uuid_v4_unique() {
        assert_ne!(uuid_v4(), uuid_v4());
    }

    #[test]
    fn uuid_v4_length() {
        let id = uuid_v4();
        assert_eq!(id.len(), 36); // 32 hex + 4 dashes
    }

    #[test]
    fn uuid_v4_dash_positions() {
        let id = uuid_v4();
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
        assert_eq!(&id[8..9], "-");
        assert_eq!(&id[13..14], "-");
        assert_eq!(&id[18..19], "-");
        assert_eq!(&id[23..24], "-");
    }

    #[test]
    fn uuid_v4_multiple_unique() {
        let ids: Vec<String> = (0..100).map(|_| uuid_v4()).collect();
        let unique: ::std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 100);
    }
}
