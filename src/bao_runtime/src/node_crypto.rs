// @trace REQ-ENG-007 [entity:BaoRuntime]
use ::std::cell::RefCell;
use bun_core::ZBox;
use ::std::ptr::NonNull;

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

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let crypto_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if crypto_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createHash".as_ptr(), Some(crypto_create_hash), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createHmac".as_ptr(), Some(crypto_create_hmac), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"randomBytes".as_ptr(), Some(crypto_random_bytes), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"pbkdf2Sync".as_ptr(), Some(crypto_pbkdf2_sync), 5, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"scryptSync".as_ptr(), Some(crypto_scrypt_sync), 5, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"randomUUID".as_ptr(), Some(crypto_random_uuid), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"getRandomValues".as_ptr(), Some(crypto_get_random_values), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createCipheriv".as_ptr(), Some(crypto_create_cipher_iv), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createDecipheriv".as_ptr(), Some(crypto_create_decipher_iv), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"timingSafeEqual".as_ptr(), Some(crypto_timing_safe_equal), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"getHashes".as_ptr(), Some(crypto_get_hashes), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"getCiphers".as_ptr(), Some(crypto_get_ciphers), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createSign".as_ptr(), Some(crypto_create_sign), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createVerify".as_ptr(), Some(crypto_create_verify), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createSecretKey".as_ptr(), Some(crypto_create_secret_key), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"generateKeyPairSync".as_ptr(), Some(crypto_generate_key_pair_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createECDH".as_ptr(), Some(crypto_create_ecdh), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"X509Certificate".as_ptr(), Some(crypto_x509_certificate), 1, JSPROP_ENUMERATE as u32);

        let mut subtle = UndefinedValue();
        let global = CurrentGlobalOrNull(cx.raw_cx());
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let mut global_crypto = UndefinedValue();
            JS_GetProperty(cx.raw_cx(), global_root.handle().into(), c"crypto".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut global_crypto,
            });
            if global_crypto.is_object() {
                rooted!(&in(cx) let crypto_global = global_crypto.to_object());
                JS_GetProperty(cx.raw_cx(), crypto_global.handle().into(), c"subtle".as_ptr(), MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut subtle,
                });
            }
        }
        if subtle.is_object() {
            rooted!(&in(cx) let subtle_rooted = subtle);
            JS_DefineProperty(cx.raw_cx(), crypto_obj.handle().into(), c"subtle".as_ptr(), subtle_rooted.handle().into(), JSPROP_ENUMERATE as u32);
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

    w2::JS_DefineFunction(cx_ref, hash_obj.handle(), c"update".as_ptr(), Some(hash_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, hash_obj.handle(), c"digest".as_ptr(), Some(hash_digest), 1, JSPROP_ENUMERATE as u32);

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
            .filter(|s| matches!(
                s.as_str(),
                "hex" | "base64" | "base64url" | "utf8" | "utf-8" | "utf-16le" | "latin1" | "ascii"
            ))
    } else {
        None
    };
    let data = if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        decode_input_string(&s, input_encoding.as_deref())
    } else if input.is_object() {
        let mut wrapped_cx_obj = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
                let ab_unwrapped = mozjs_sys::jsapi::JS::GetObjectAsArrayBuffer(obj_root.get(), &mut ab_length, &mut ab_data);
                if !ab_unwrapped.is_null() && !ab_data.is_null() && ab_length > 0 {
                    let slice = ::std::slice::from_raw_parts(ab_data, ab_length);
                    slice.to_vec()
                } else if !ab_unwrapped.is_null() {
                    Vec::new()
                } else {
                    return throw_type_error(cx, "hash.update() data must be a string, Buffer, TypedArray, or DataView");
                }
            }
        }
    } else {
        return throw_type_error(cx, "hash.update() data must be a string, Buffer, TypedArray, or DataView");
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
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
            return_string(cx, &args, &encoded)
        }
        _ => return_string(cx, &args, &hex::encode(&result)),
    }
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

    w2::JS_DefineFunction(cx_ref, hmac_obj.handle(), c"update".as_ptr(), Some(hmac_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, hmac_obj.handle(), c"digest".as_ptr(), Some(hmac_digest), 1, JSPROP_ENUMERATE as u32);

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
            .filter(|s| matches!(
                s.as_str(),
                "hex" | "base64" | "base64url" | "utf8" | "utf-8" | "utf-16le" | "latin1" | "ascii"
            ))
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
        _ => {
            return throw_type_error(cx, &format!("Unsupported HMAC algorithm: {}", algo));
        }
    };

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
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
        return throw_type_error(cx, "pbkdf2Sync() requires (password, salt, iterations, keylen, digest)");
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
        if v.is_int32() { v.to_int32() as u32 } else { return throw_type_error(cx, "pbkdf2Sync() iterations must be a number"); }
    };
    let key_len = {
        let v = *args.get(3).ptr;
        if v.is_int32() { v.to_int32() as usize } else { return throw_type_error(cx, "pbkdf2Sync() keylen must be a number"); }
    };
    let digest_name = match arg_to_string(cx, *args.get(4).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "pbkdf2Sync() digest must be a string"),
    };

    // @trace REQ-ENG-007 [entity:bao_crypto] DEC-ENG-003: pbkdf2 routed to
    // bao_crypto::kdf (sha_hmac::pbkdf2 removed). Supports sha1/sha256/sha512.
    let pbkdf2_hash = match bao_crypto::kdf::parse_pbkdf2_hash(&digest_name) {
        Ok(h) => h,
        Err(_) => return throw_type_error(cx, &format!("Unsupported PBKDF2 digest: {}", digest_name)),
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
        unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32); }
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
        if v.is_int32() { v.to_int32() as usize } else { return throw_type_error(cx, "scryptSync() keylen must be a number"); }
    };

    let log_n: u8 = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_int32() { (v.to_int32() as f64).log2() as u8 } else { 14 }
    } else { 14 };
    let n = 1u64 << log_n;

    let mut out = vec![0u8; key_len];
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
        unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32); }
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
    format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11],
        bytes[12], bytes[13], bytes[14], bytes[15])
}

// --- getRandomValues ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_random_values(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return throw_type_error(cx, "getRandomValues() requires a typed array");
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = (*args.get(0).ptr).to_object());

    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr.handle().into(), c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { return throw_type_error(cx, "getRandomValues() invalid array") };

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
        Some("base64") => bun_base64::decode_alloc(s.as_bytes())
            .unwrap_or_else(|_| s.as_bytes().to_vec()),
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
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
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
    if !val.is_object() { return Vec::new(); }
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
    JS_GetProperty(cx, obj_root.handle().into(), c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { return Vec::new() };
    let mut bytes = Vec::with_capacity(len);
    for i in 0u32..len as u32 {
        let mut byte_val = UndefinedValue();
        JS_GetElement(cx, obj_root.handle().into(), i, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
        });
        bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
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
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
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
    JS_GetProperty(cx, obj_rooted.handle().into(), c"__bao_cipher_id".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut id_val,
    });
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_root = this.to_object());
    read_cipher_id(cx, this_root.get())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_cipher_iv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(cipher_update), 3, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"final".as_ptr(), Some(cipher_final), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"getAuthTag".as_ptr(), Some(cipher_get_auth_tag), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"setAuthTag".as_ptr(), Some(cipher_set_auth_tag), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"setAAD".as_ptr(), Some(cipher_set_aad), 1, JSPROP_ENUMERATE as u32);
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_decipher_iv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(cipher_update), 3, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"final".as_ptr(), Some(cipher_final), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"getAuthTag".as_ptr(), Some(cipher_get_auth_tag), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"setAuthTag".as_ptr(), Some(cipher_set_auth_tag), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"setAAD".as_ptr(), Some(cipher_set_aad), 1, JSPROP_ENUMERATE as u32);
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
            .filter(|s| matches!(
                s.as_str(),
                "hex" | "base64" | "base64url" | "utf8" | "utf-8" | "utf-16le" | "latin1" | "ascii"
            ))
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
    let cx_ref = &mut wrapped_cx;
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
unsafe extern "C" fn crypto_timing_safe_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    let hashes = ["sha1", "sha224", "sha256", "sha384", "sha512", "md5", "md4", "md2", "ripemd160"];
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, hashes.len()));
    if !arr.get().is_null() {
        for (i, name) in hashes.iter().enumerate() {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
                JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
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
        "aes-128-cbc", "aes-128-ecb", "aes-128-gcm",
        "aes-192-cbc", "aes-192-ecb", "aes-192-gcm",
        "aes-256-cbc", "aes-256-ecb", "aes-256-gcm",
        "chacha20-poly1305", "aes-128-cfb", "aes-256-cfb",
        "aes-128-ctr", "aes-256-ctr", "des-ede3-cbc",
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
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(sign_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"sign".as_ptr(), Some(sign_sign), 2, JSPROP_ENUMERATE as u32);
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sign_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "sign.update() requires data"); }
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
        if name.contains("256") { Some(RsaHash::Sha256) }
        else if name.contains("384") { Some(RsaHash::Sha384) }
        else if name.contains("512") { Some(RsaHash::Sha512) }
        else { Some(RsaHash::Sha256) }
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
        match arg_to_string(cx, *args.get(1).ptr) { Some(s) => s, None => "hex".to_string() }
    } else { "hex".to_string() };
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
    } else { Vec::new() };

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
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
            return_string(cx, &args, &encoded)
        }
        "buffer" => {
            // Return the raw signature as a number[] (Node buffer encoding).
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
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
        match arg_to_string(cx, *args.get(0).ptr) { Some(s) => s.to_lowercase(), None => "sha256".to_string() }
    } else { "sha256".to_string() };
    HASH_ALGO.with(|a| *a.borrow_mut() = algo);
    HASH_DATA.with(|d| d.borrow_mut().clear());
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(sign_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"verify".as_ptr(), Some(verify_verify), 3, JSPROP_ENUMERATE as u32);
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
    if argc < 2 { return throw_type_error(cx, "verify.verify() requires (key, signature)"); }
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
unsafe extern "C" fn crypto_create_secret_key(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }

    rooted!(&in(cx_ref) let kv = mozjs::jsval::StringValue(&*JS_NewStringCopyZ(cx, c"secret".as_ptr())));
    JS_DefineProperty(cx, obj.handle().into(), c"type".as_ptr(), kv.handle().into(), JSPROP_ENUMERATE as u32);
    if argc > 0 {
        let bytes = if (*args.get(0).ptr).is_object() {
            extract_buffer_bytes(cx, *args.get(0).ptr)
        } else if (*args.get(0).ptr).is_string() {
            crate::js_to_rust_string(cx, *args.get(0).ptr).into_bytes()
        } else {
            Vec::new()
        };
        let exported = hex::encode(&bytes);
        let exp_str = JS_NewStringCopyN(cx, exported.as_ptr() as *const ::std::os::raw::c_char, exported.len());
        if !exp_str.is_null() {
            rooted!(&in(cx_ref) let ev = mozjs::jsval::StringValue(&*exp_str));
            JS_DefineProperty(cx, obj.handle().into(), c"export".as_ptr(), ev.handle().into(), 0);
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

// --- generateKeyPairSync ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_key_pair_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
            bao_crypto::keypair::KeyPairType::Rsa { bits: bits as usize }
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
            JS_GetProperty(cx, obj.handle().into(), prop_c.as_ptr() as *const _, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut v,
            });
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
            JS_GetProperty(cx, obj.handle().into(), prop_c.as_ptr() as *const _, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut v,
            });
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
unsafe fn set_string_prop(cx: *mut JSContext, obj: *mut JSObject, name: *const core::ffi::c_char, value: &str) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let js_str = JS_NewStringCopyN(cx, value.as_ptr() as *const core::ffi::c_char, value.len());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let v = mozjs::jsval::StringValue(&*js_str));
        JS_DefineProperty(cx, obj_rooted.handle().into(), name, v.handle().into(), JSPROP_ENUMERATE as u32);
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
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"getPublicKey".as_ptr(), Some(ecdh_get_public_key), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"computeSecret".as_ptr(), Some(ecdh_compute_secret), 1, JSPROP_ENUMERATE as u32);
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
    JS_DefineProperty(cx, obj_rooted.handle().into(), c"__bao_ecdh_id".as_ptr(), idv.handle().into(), 0);
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
    JS_GetProperty(cx, obj.handle().into(), c"__bao_ecdh_id".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut id_val,
    });
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
        reg.borrow().get(id as usize).and_then(|s| s.as_ref()).map(|kp| kp.public_key_bytes())
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
        reg.borrow().get(id as usize).and_then(|s| s.as_ref()).and_then(|kp| {
            kp.compute_shared_secret(&peer_pub).ok()
        })
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

// --- X509Certificate ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_x509_certificate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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
    set_string_prop(cx, obj.get(), c"serialNumber".as_ptr(), &cert.serial_number());
    set_string_prop(cx, obj.get(), c"validFrom".as_ptr(), &cert.valid_from());
    set_string_prop(cx, obj.get(), c"validTo".as_ptr(), &cert.valid_to());
    set_string_prop(cx, obj.get(), c"fingerprint256".as_ptr(), &cert.fingerprint_sha256());
    set_string_prop(cx, obj.get(), c"fingerprint".as_ptr(), &cert.fingerprint_sha1());
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
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
        assert!(matches!(v, b'8' | b'9' | b'a' | b'b'), "variant must be 8/9/a/b, got {}", v as char);
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
