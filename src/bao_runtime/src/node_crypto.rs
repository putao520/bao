// @trace REQ-ENG-007 [entity:BaoRuntime]
use ::std::cell::RefCell;
use ::std::ptr::NonNull;
use bun_core::ZBox;

use bun_sha_hmac;
use bun_sha_hmac::hmac::EVP_MAX_MD_SIZE;
use core::ptr;
use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
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
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let val_root = val);
    let s = mozjs::rust::ToString(cx, val_root.handle().into());
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
    let key = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => return throw_type_error(cx, "createHmac() key must be a string"),
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

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, result.len()) });
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    for (i, &byte) in result.iter().enumerate() {
        let val = mozjs::jsval::Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v = val);
        unsafe {
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

// --- scryptSync ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_scrypt_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 {
        return throw_type_error(cx, "scryptSync() requires (password, salt, keylen)");
    }

    let password = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.into_bytes(),
        None => return throw_type_error(cx, "scryptSync() password must be a string"),
    };
    let salt = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => return throw_type_error(cx, "scryptSync() salt must be a string"),
    };
    let key_len = {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else {
            return throw_type_error(cx, "scryptSync() keylen must be a number");
        }
    };

    let log_n: u8 = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_int32() {
            (v.to_int32() as f64).log2() as u8
        } else {
            14
        }
    } else {
        14
    };
    let n = 1u64 << log_n;

    let out = vec![0u8; key_len];
    if let Err(e) = bao_crypto::kdf::scrypt(&password, &salt, n, 8, 1, key_len) {
        return throw_type_error(cx, &format!("scryptSync() failed: {}", e));
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, out.len()) });
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    for (i, &byte) in out.iter().enumerate() {
        let val = mozjs::jsval::Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v = val);
        unsafe {
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

    HASH_ALGO.with(|a| *a.borrow_mut() = algo);
    HASH_DATA.with(|d| d.borrow_mut().clear());

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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sign_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return throw_type_error(cx, "sign.update() requires data");
    }
    let input = *args.get(0).ptr;
    let data = if input.is_string() {
        crate::js_to_rust_string(cx, input).into_bytes()
    } else if input.is_object() {
        extract_buffer_bytes(cx, input)
    } else {
        Vec::new()
    };
    HASH_DATA.with(|d| d.borrow_mut().extend_from_slice(&data));
    args.rval().set(*args.thisv().ptr);
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
    let encoding = if argc > 1 {
        match arg_to_string(cx, *args.get(1).ptr) {
            Some(s) => s,
            None => "hex".to_string(),
        }
    } else {
        "hex".to_string()
    };
    // @trace REQ-ENG-007 [api:node:crypto sign.sign] [entity:bao_crypto]
    // Real asymmetric signing via bao_crypto::sign::Signer for RSA-PKCS1v15/PSS,
    // ECDSA P256/P384, Ed25519. HMAC remains for HMAC algorithms / raw keys.
    let algo = HASH_ALGO.with(|a| ::std::mem::take(&mut *a.borrow_mut()));
    let data = HASH_DATA.with(|d| ::std::mem::take(&mut *d.borrow_mut()));
    let key = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.into_bytes(),
            None => extract_buffer_bytes(cx, *args.get(0).ptr),
        }
    } else {
        Vec::new()
    };

    let result: Vec<u8> = if let Some(sign_algo) = resolve_sign_algorithm(&algo) {
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

    match encoding.to_lowercase().as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes)
                .unwrap_or("")
                .to_owned();
            return_string(cx, &args, &encoded)
        }
        "buffer" => {
            // Return the raw signature as a number[] (Node buffer encoding).
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let _cx_ref = &mut wrapped_cx;
            let arr = bytes_to_js_array(cx, &result);
            if arr.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::ObjectValue(arr));
            }
            true
        }
        _ => return_string(cx, &args, &hex::encode(&result)),
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
    HASH_ALGO.with(|a| *a.borrow_mut() = algo);
    HASH_DATA.with(|d| d.borrow_mut().clear());
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
    let algo = HASH_ALGO.with(|a| ::std::mem::take(&mut *a.borrow_mut()));
    let data = HASH_DATA.with(|d| ::std::mem::take(&mut *d.borrow_mut()));

    let verified: bool = if let Some(sign_algo) = resolve_sign_algorithm(&algo) {
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_secret_key(
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
        return true;
    }

    rooted!(&in(cx_ref) let kv = mozjs::jsval::StringValue(&*JS_NewStringCopyZ(cx, c"secret".as_ptr())));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"type".as_ptr(),
        kv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    if argc > 0 {
        let bytes = if (*args.get(0).ptr).is_object() {
            extract_buffer_bytes(cx, *args.get(0).ptr)
        } else if (*args.get(0).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(0).ptr).into_bytes()
        } else {
            Vec::new()
        };
        let exported = hex::encode(&bytes);
        let exp_str = JS_NewStringCopyN(
            cx,
            exported.as_ptr() as *const ::std::os::raw::c_char,
            exported.len(),
        );
        if !exp_str.is_null() {
            rooted!(&in(cx_ref) let ev = mozjs::jsval::StringValue(&*exp_str));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"export".as_ptr(),
                ev.handle().into(),
                0,
            );
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
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
    // PEM strings. RSA default bits=2048; ec default curve=P256.
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
    let pub_pem = result.public_key_pem.unwrap_or_default();
    let priv_pem = result.private_key_pem.unwrap_or_default();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    set_string_prop(cx, obj.get(), c"publicKey".as_ptr(), &pub_pem);
    set_string_prop(cx, obj.get(), c"privateKey".as_ptr(), &priv_pem);
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn make_key_object_js(cx: *mut JSContext, idx: usize, key_type: &str) -> *mut JSObject {
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
    w2::JS_DefineFunction(
        cx_ref,
        obj.handle(),
        c"export".as_ptr(),
        Some(key_object_export),
        0,
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
    let obj = make_key_object_js(cx, idx, &key_type);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn key_object_export(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
        return false;
    }
    let idx = idx_val.to_int32() as usize;

    let key_bytes = KEY_OBJECTS.with(|v| v.borrow_mut().get_mut(idx).and_then(|k| k.take()));

    if let Some(bytes) = key_bytes {
        let buf_obj = crate::globals::create_buffer_object(cx, &bytes);
        if !buf_obj.is_null() {
            args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_public_key(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let key_bytes = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let idx = alloc_key_object(key_bytes);
    let obj = make_key_object_js(cx, idx, "public");
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_private_key(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let key_bytes = if argc > 0 {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let idx = alloc_key_object(key_bytes);
    let obj = make_key_object_js(cx, idx, "private");
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
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
// Uses same pattern as fs_async: spawn thread + uws_loop_defer
// ============================================================

struct CryptoAsyncCtx {
    cx: *mut JSContext,
    callback: *mut JSObject,
    result: ::std::sync::Arc<::std::sync::Mutex<Option<::std::result::Result<Vec<u8>, String>>>>,
    #[allow(dead_code)]
    op_name: String,
    rooted: bool,
}

unsafe fn schedule_crypto_defer(ctx_ptr: usize) {
    bao_uloop::force_link();
    let loop_ = bao_uloop::uws_get_loop();
    if loop_.is_null() {
        let _ = Box::from_raw(ctx_ptr as *mut CryptoAsyncCtx);
        return;
    }
    bao_uloop::uws_loop_defer(
        loop_,
        ctx_ptr as *mut ::std::ffi::c_void,
        crypto_async_defer_callback,
    );
}

unsafe extern "C" fn crypto_async_defer_callback(raw_ctx: *mut ::std::ffi::c_void) {
    let ctx = Box::from_raw(raw_ctx as *mut CryptoAsyncCtx);
    let cx = ctx.cx;
    let callback = ctx.callback;

    let mut result_guard = ctx.result.lock().unwrap();
    let result_opt = result_guard.take();
    ::std::mem::drop(result_guard);

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cb = callback);
    rooted!(&in(cx_ref) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let global_rooted = global);

    match result_opt {
        Some(Ok(data)) => {
            let buf_obj = crate::globals::create_buffer_object(cx, &data);
            let val = if buf_obj.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::ObjectValue(buf_obj)
            };
            rooted!(&in(cx_ref) let val_rooted = val);
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
            rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
            if !err_obj.get().is_null() {
                let c_msg = ZBox::from_bytes(msg.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx_ref) let msg_val = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineProperty(
                        cx,
                        err_obj.handle().into(),
                        c"message".as_ptr(),
                        msg_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
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
    if ctx.rooted {
        let mut cb_val = mozjs::jsval::ObjectValue(callback);
        RemoveRawValueRoot(cx, &mut cb_val);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn spawn_crypto_async<F>(cx: *mut JSContext, op_name: &str, callback: *mut JSObject, work: F)
where
    F: FnOnce() -> ::std::result::Result<Vec<u8>, String> + Send + 'static,
{
    let mut cb_val = mozjs::jsval::ObjectValue(callback);
    let rooted = AddRawValueRoot(
        cx,
        &mut cb_val,
        b"crypto_async_cb\0".as_ptr() as *const ::std::os::raw::c_char,
    );

    let result_slot: ::std::sync::Arc<
        ::std::sync::Mutex<Option<::std::result::Result<Vec<u8>, String>>>,
    > = ::std::sync::Arc::new(::std::sync::Mutex::new(None));
    let result_clone = result_slot.clone();

    let op_name_owned = op_name.to_string();

    let ctx = Box::new(CryptoAsyncCtx {
        cx,
        callback,
        result: result_slot,
        op_name: op_name_owned,
        rooted,
    });
    let ctx_ptr = Box::into_raw(ctx) as usize;

    ::std::thread::spawn(move || {
        let result = work();
        {
            let mut slot = result_clone.lock().unwrap();
            *slot = Some(result);
        }
        unsafe {
            schedule_crypto_defer(ctx_ptr);
        }
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

    let has_callback = argc > 5 && (*args.get(5).ptr).is_object();
    if has_callback {
        let callback = (*args.get(5).ptr).to_object();
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

#[allow(dead_code, unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_bytes_async(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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

    let has_callback = argc > 1 && (*args.get(1).ptr).is_object();
    if has_callback {
        let callback = (*args.get(1).ptr).to_object();
        spawn_crypto_async(cx, "randomBytes", callback, move || {
            let mut buf = vec![0u8; size];
            bao_crypto::random::rand_bytes(&mut buf)
                .map(|_| buf)
                .map_err(|e| format!("randomBytes: {}", e))
        });
        args.rval().set(UndefinedValue());
        true
    } else {
        // Sync path (same as original crypto_random_bytes)
        let mut bytes = vec![0u8; size];
        bao_crypto::random::rand_bytes(&mut bytes).unwrap();
        let buf_obj = crate::globals::create_buffer_object(cx, &bytes);
        if buf_obj.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
        true
    }
}

// ============================================================
// Single-shot sign/verify (sync)
// ============================================================

fn parse_sign_algorithm(algo: &str) -> bao_crypto::sign::SignAlgorithm {
    match algo.to_uppercase().as_str() {
        "SHA256" | "RS256" => bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 {
            hash: bao_crypto::sign::RsaHash::Sha256,
        },
        "SHA384" | "RS384" => bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 {
            hash: bao_crypto::sign::RsaHash::Sha384,
        },
        "SHA512" | "RS512" => bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 {
            hash: bao_crypto::sign::RsaHash::Sha512,
        },
        "PS256" => bao_crypto::sign::SignAlgorithm::RsaPss {
            hash: bao_crypto::sign::RsaHash::Sha256,
        },
        "PS384" => bao_crypto::sign::SignAlgorithm::RsaPss {
            hash: bao_crypto::sign::RsaHash::Sha384,
        },
        "PS512" => bao_crypto::sign::SignAlgorithm::RsaPss {
            hash: bao_crypto::sign::RsaHash::Sha512,
        },
        "ECDSA" | "ES256" => bao_crypto::sign::SignAlgorithm::EcdsaP256,
        "ES384" => bao_crypto::sign::SignAlgorithm::EcdsaP384,
        "ED25519" => bao_crypto::sign::SignAlgorithm::Ed25519,
        _ => bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 {
            hash: bao_crypto::sign::RsaHash::Sha256,
        },
    }
}

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

    let sign_algo = parse_sign_algorithm(&algo);

    // Try to create Signer from key argument
    let signer = if key_val.is_string() {
        let pem = crate::jsstr_to_rust_string(cx, key_val.to_string());
        bao_crypto::sign::Signer::from_pkcs8_pem(&sign_algo, &pem)
    } else if key_val.is_object() {
        // Check for _keyIdx (KeyObject) or try as buffer (DER key)
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
            let key_bytes = KEY_OBJECTS.with(|v| {
                v.borrow()
                    .get(idx)
                    .map(|k| k.as_ref().map(|b| b.clone()))
                    .flatten()
            });
            if let Some(der) = key_bytes {
                bao_crypto::sign::Signer::from_pkcs8_der(&sign_algo, &der)
            } else {
                Err(bao_crypto::CryptoError::InvalidKey(
                    "KeyObject key data not available".into(),
                ))
            }
        } else {
            let der = extract_buffer_bytes(cx, key_val);
            bao_crypto::sign::Signer::from_pkcs8_der(&sign_algo, &der)
        }
    } else {
        Err(bao_crypto::CryptoError::InvalidKey(
            "sign: key argument required".into(),
        ))
    };

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

    let sign_algo = parse_sign_algorithm(&algo);

    let verifier = if key_val.is_string() {
        let pem = crate::jsstr_to_rust_string(cx, key_val.to_string());
        // Try as public key PEM first, then private
        bao_crypto::verify::Verifier::from_public_pem(&sign_algo, &pem)
            .or_else(|_| bao_crypto::verify::Verifier::from_pkcs8_pem(&sign_algo, &pem))
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
            let key_bytes = KEY_OBJECTS.with(|v| {
                v.borrow()
                    .get(idx)
                    .map(|k| k.as_ref().map(|b| b.clone()))
                    .flatten()
            });
            if let Some(der) = key_bytes {
                bao_crypto::verify::Verifier::from_public_der(&sign_algo, &der)
                    .or_else(|_| bao_crypto::verify::Verifier::from_pkcs8_der(&sign_algo, &der))
            } else {
                Err(bao_crypto::CryptoError::InvalidKey(
                    "KeyObject key data not available".into(),
                ))
            }
        } else {
            let der = extract_buffer_bytes(cx, key_val);
            bao_crypto::verify::Verifier::from_public_der(&sign_algo, &der)
                .or_else(|_| bao_crypto::verify::Verifier::from_pkcs8_der(&sign_algo, &der))
        }
    } else {
        Err(bao_crypto::CryptoError::InvalidKey(
            "verify: key argument required".into(),
        ))
    };

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
    let obj = make_key_object_js(cx, idx, "secret");
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
