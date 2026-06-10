// @trace REQ-ENG-007
// @trace REQ-ENG-008 [bao_crypto real sign/verify/cipher/ECDH/keypair/certificate]
use ::std::cell::RefCell;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

fn decode_hex(s: &str) -> Vec<u8> {
    let src = s.as_bytes();
    let len = src.len() / 2;
    let mut dst = vec![0u8; len];
    let decoded = bun_core::string::decode_hex_to_bytes_truncate(&mut dst, src);
    dst.truncate(decoded);
    dst
}

thread_local! {
    static HASH_DATA: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static HASH_ALGO: RefCell<String> = const { RefCell::new(String::new()) };
    static HMAC_ALGO: RefCell<String> = const { RefCell::new(String::new()) };
    static HMAC_KEY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static HMAC_DATA: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static SIGN_ALGO: RefCell<String> = const { RefCell::new(String::new()) };
    static SIGN_DATA: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static CIPHER_ALGO: RefCell<String> = const { RefCell::new(String::new()) };
    static CIPHER_KEY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static CIPHER_IV: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static CIPHER_AAD: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static CIPHER_TAG: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
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
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"createECDH".as_ptr(), Some(crypto_create_ecdh), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"generateKeyPairSync".as_ptr(), Some(crypto_generate_key_pair_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"hkdfSync".as_ptr(), Some(crypto_hkdf_sync), 5, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, crypto_obj.handle(), c"X509Certificate".as_ptr(), Some(crypto_x509_certificate), 1, JSPROP_ENUMERATE as u32);

        let mut subtle = UndefinedValue();
        let global = CurrentGlobalOrNull(cx.raw_cx());
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let mut global_crypto = UndefinedValue();
            JS_GetProperty(cx.raw_cx(), global_h, c"crypto".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut global_crypto,
            });
            if global_crypto.is_object() {
                let crypto_global_h = Handle::<*mut JSObject> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &global_crypto.to_object(),
                };
                JS_GetProperty(cx.raw_cx(), crypto_global_h, c"subtle".as_ptr(), MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut subtle,
                });
            }
        }
        if subtle.is_object() {
            let subtle_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &subtle };
            JS_DefineProperty(cx.raw_cx(), crypto_obj.handle().into(), c"subtle".as_ptr(), subtle_h, JSPROP_ENUMERATE as u32);
        }
    }

    cache_builtin(cx, "crypto", crypto_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn arg_to_string(cx: *mut JSContext, val: JSVal) -> Option<String> {
    if val.is_undefined() || val.is_null() {
        return None;
    }
    let raw_handle = mozjs::rust::HandleValue::from_marked_location(&val);
    let s = mozjs::rust::ToString(cx, raw_handle);
    if s.is_null() {
        return None;
    }
    Some(crate::jsstr_to_rust_string(cx, s))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_string(cx: *mut JSContext, args: &CallArgs, s: &str) -> bool {
    let c_str = bun_core::ZBox::from_bytes(s.as_bytes());
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
    let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
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
    let data = if input.is_string() {
        crate::js_to_rust_string(cx, input).into_bytes()
    } else {
        return throw_type_error(cx, "hash.update() data must be a string");
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

    let algo = HASH_ALGO.with(|a| a.borrow().clone());
    let data = HASH_DATA.with(|d| d.borrow().clone());

    let result = match algo.as_str() {
        "sha256" => { let mut out = [0u8; 32]; bun_sha_hmac::sha::hashers::SHA256::hash(&data, &mut out); out.to_vec() }
        "sha512" => { let mut out = [0u8; 64]; bun_sha_hmac::sha::hashers::SHA512::hash(&data, &mut out); out.to_vec() }
        "sha384" => { let mut out = [0u8; 48]; bun_sha_hmac::sha::hashers::SHA384::hash(&data, &mut out); out.to_vec() }
        "sha224" => { let mut h = bun_sha_hmac::SHA224::init(); h.update(&data); let mut out = [0u8; 28]; h.r#final(&mut out); out.to_vec() }
        "sha1" => { let mut out = [0u8; 20]; bun_sha_hmac::sha::hashers::SHA1::hash(&data, &mut out); out.to_vec() }
        "md5" => { let mut h = bun_sha_hmac::MD5::init(); h.update(&data); let mut out = [0u8; 16]; h.r#final(&mut out); out.to_vec() }
        _ => {
            HASH_DATA.with(|d| d.borrow_mut().clear());
            return throw_type_error(cx, &format!("Unsupported hash algorithm: {}", algo));
        }
    };

    HASH_DATA.with(|d| d.borrow_mut().clear());
    HASH_ALGO.with(|a| a.borrow_mut().clear());

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
            return_string(cx, &args, &encoded)
        }
        _ => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result)),
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
    let data = if input.is_string() {
        crate::js_to_rust_string(cx, input).into_bytes()
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

    let algo = HMAC_ALGO.with(|a| a.borrow().clone());
    let key = HMAC_KEY.with(|k| k.borrow().clone());
    let data = HMAC_DATA.with(|d| d.borrow().clone());

    let mut buf = [0u8; bun_sha_hmac::hmac::EVP_MAX_MD_SIZE];
    let result: Vec<u8> = match algo.as_str() {
        "sha256" => bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha256, &mut buf).map(|s| s.to_vec()).unwrap_or_default(),
        "sha512" => bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha512, &mut buf).map(|s| s.to_vec()).unwrap_or_default(),
        "sha1" => bun_sha_hmac::generate(&key, &data, bun_sha_hmac::Algorithm::Sha1, &mut buf).map(|s| s.to_vec()).unwrap_or_default(),
        _ => {
            HMAC_DATA.with(|d| d.borrow_mut().clear());
            return throw_type_error(cx, &format!("Unsupported HMAC algorithm: {}", algo));
        }
    };

    HMAC_DATA.with(|d| d.borrow_mut().clear());
    HMAC_KEY.with(|k| k.borrow_mut().clear());
    HMAC_ALGO.with(|a| a.borrow_mut().clear());

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
            return_string(cx, &args, &encoded)
        }
        _ => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result)),
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
    let _ = getrandom::fill(&mut bytes);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, size) });
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    for (i, &byte) in bytes.iter().enumerate() {
        let val = mozjs::jsval::Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v = val);
        unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32); }
    }

    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
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

    let algorithm = match digest_name.as_str() {
        "sha256" => bun_sha_hmac::Algorithm::Sha256,
        "sha512" => bun_sha_hmac::Algorithm::Sha512,
        "sha1" => bun_sha_hmac::Algorithm::Sha1,
        _ => return throw_type_error(cx, &format!("Unsupported PBKDF2 digest: {}", digest_name)),
    };
    let mut out = vec![0u8; key_len];
    let result = if bun_sha_hmac::pbkdf2::derive(&password, &salt, iterations, algorithm, &mut out) {
        out
    } else {
        return throw_type_error(cx, "PBKDF2 derivation failed");
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
    let n: u64 = 1u64 << log_n;
    let r: u64 = 8;
    let p: u64 = 1;
    let max_mem: usize = 32 * 1024 * 1024; // 32 MiB default

    let mut out = vec![0u8; key_len];
    if !bun_sha_hmac::scrypt::derive(&password, &salt, n, r, p, max_mem, &mut out) {
        return throw_type_error(cx, "scryptSync() failed");
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
pub unsafe extern "C" fn crypto_random_uuid(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let uuid = uuid_v4();
    return_string(cx, &args, &uuid)
}

fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = bun_core::fmt::bytes_to_hex_lower_string(&bytes);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

// --- getRandomValues ---

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn crypto_get_random_values(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return throw_type_error(cx, "getRandomValues() requires a typed array");
    }
    let arr = (*args.get(0).ptr).to_object();
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr };

    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { return throw_type_error(cx, "getRandomValues() invalid array") };

    let mut random_bytes = vec![0u8; len];
    let _ = getrandom::fill(&mut random_bytes);

    for (i, &byte) in random_bytes.iter().enumerate() {
        let v = mozjs::jsval::Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_SetElement(cx, arr_h, i as u32, v_h);
    }

    args.rval().set(mozjs::jsval::ObjectValue(arr));
    true
}

// --- createCipheriv / createDecipheriv ---

#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn extract_buffer_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    if !val.is_object() { return Vec::new(); }
    let obj = val.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { return Vec::new() };
    let mut bytes = Vec::with_capacity(len);
    for i in 0..len {
        let mut byte_val = UndefinedValue();
        JS_GetElement(cx, obj_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
        });
        bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
    }
    bytes
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_cipher_iv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 { return throw_type_error(cx, "createCipheriv() requires (algorithm, key, iv)"); }
    let algo_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "createCipheriv() algorithm must be a string"),
    };
    let key = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(1).ptr),
    };
    let iv = match arg_to_string(cx, *args.get(2).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(2).ptr),
    };
    let aad = if argc > 3 {
        extract_buffer_bytes(cx, *args.get(3).ptr)
    } else {
        Vec::new()
    };

    CIPHER_ALGO.with(|a| *a.borrow_mut() = algo_name);
    CIPHER_KEY.with(|k| *k.borrow_mut() = key);
    CIPHER_IV.with(|v| *v.borrow_mut() = iv);
    CIPHER_AAD.with(|a| *a.borrow_mut() = aad);
    CIPHER_TAG.with(|t| t.borrow_mut().clear());

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(cipher_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"final".as_ptr(), Some(cipher_final), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"getAuthTag".as_ptr(), Some(cipher_get_auth_tag), 0, JSPROP_ENUMERATE as u32);
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_decipher_iv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 { return throw_type_error(cx, "createDecipheriv() requires (algorithm, key, iv)"); }
    let algo_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "createDecipheriv() algorithm must be a string"),
    };
    let key = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(1).ptr),
    };
    let iv = match arg_to_string(cx, *args.get(2).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(2).ptr),
    };
    let aad = if argc > 3 {
        extract_buffer_bytes(cx, *args.get(3).ptr)
    } else {
        Vec::new()
    };

    CIPHER_ALGO.with(|a| *a.borrow_mut() = algo_name);
    CIPHER_KEY.with(|k| *k.borrow_mut() = key);
    CIPHER_IV.with(|v| *v.borrow_mut() = iv);
    CIPHER_AAD.with(|a| *a.borrow_mut() = aad);
    CIPHER_TAG.with(|t| t.borrow_mut().clear());

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(decipher_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"final".as_ptr(), Some(decipher_final), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"setAuthTag".as_ptr(), Some(decipher_set_auth_tag), 1, JSPROP_ENUMERATE as u32);
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "cipher.update() requires data"); }
    let input = *args.get(0).ptr;
    let data = if input.is_string() {
        crate::js_to_rust_string(cx, input).into_bytes()
    } else if input.is_object() {
        extract_buffer_bytes(cx, input)
    } else {
        Vec::new()
    };

    let algo_name = CIPHER_ALGO.with(|a| a.borrow().clone());
    let key = CIPHER_KEY.with(|k| k.borrow().clone());
    let iv = CIPHER_IV.with(|v| v.borrow().clone());
    let aad = CIPHER_AAD.with(|a| a.borrow().clone());

    let algo = match bao_crypto::cipher::parse_algorithm(&algo_name) {
        Ok(a) => a,
        Err(_) => return throw_type_error(cx, &format!("Unsupported cipher algorithm: {}", algo_name)),
    };

    let aad_opt = if aad.is_empty() { None } else { Some(aad.as_slice()) };
    match bao_crypto::cipher::encrypt(algo, &key, &iv, aad_opt, &data) {
        Ok(result) => {
            CIPHER_TAG.with(|t| *t.borrow_mut() = result.auth_tag.clone());
            return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result.ciphertext))
        }
        Err(e) => throw_type_error(cx, &format!("cipher.update() encryption failed: {}", e)),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_final(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    return_string(cx, &args, "")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_get_auth_tag(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let tag = CIPHER_TAG.with(|t| t.borrow().clone());
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, tag.len()) });
    if arr.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    for (i, &byte) in tag.iter().enumerate() {
        let val = mozjs::jsval::Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v = val);
        unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32); }
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn decipher_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "decipher.update() requires data"); }
    let input = *args.get(0).ptr;
    let data = if input.is_string() {
        let hex_str = crate::js_to_rust_string(cx, input);
        decode_hex(&hex_str)
    } else if input.is_object() {
        extract_buffer_bytes(cx, input)
    } else {
        Vec::new()
    };

    let algo_name = CIPHER_ALGO.with(|a| a.borrow().clone());
    let key = CIPHER_KEY.with(|k| k.borrow().clone());
    let iv = CIPHER_IV.with(|v| v.borrow().clone());
    let aad = CIPHER_AAD.with(|a| a.borrow().clone());
    let tag = CIPHER_TAG.with(|t| t.borrow().clone());

    let algo = match bao_crypto::cipher::parse_algorithm(&algo_name) {
        Ok(a) => a,
        Err(_) => return throw_type_error(cx, &format!("Unsupported cipher algorithm: {}", algo_name)),
    };

    let aad_opt = if aad.is_empty() { None } else { Some(aad.as_slice()) };
    if tag.is_empty() {
        return throw_type_error(cx, "decipher.update() requires auth tag — call setAuthTag() first");
    }

    match bao_crypto::cipher::decrypt(algo, &key, &iv, aad_opt, &data, &tag) {
        Ok(plaintext) => return_string(cx, &args, &String::from_utf8_lossy(&plaintext)),
        Err(e) => throw_type_error(cx, &format!("decipher.update() decryption failed: {}", e)),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn decipher_set_auth_tag(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "setAuthTag() requires tag buffer"); }
    let tag = if (*args.get(0).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else if (*args.get(0).ptr).is_string() {
        decode_hex(&crate::js_to_rust_string(cx, *args.get(0).ptr))
    } else {
        Vec::new()
    };
    CIPHER_TAG.with(|t| *t.borrow_mut() = tag);
    args.rval().set(*args.thisv().ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn decipher_final(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    return_string(cx, &args, "")
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
    let mut result = 0u8;
    for i in 0..a.len() {
        result |= a[i] ^ b[i];
    }
    args.rval().set(mozjs::jsval::BooleanValue(result == 0));
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
            let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
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
        "aes-128-gcm",
        "aes-256-gcm",
        "chacha20-poly1305",
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
        args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// --- createSign / createVerify ---

/// Parse a Node.js sign/verify algorithm name to bao_crypto::SignAlgorithm.
fn parse_sign_algorithm(name: &str) -> ::std::result::Result<bao_crypto::sign::SignAlgorithm, String> {
    match name.to_lowercase().as_str() {
        "rsa-sha256" | "rs256" | "sha256withrsa" | "sha256withrsaencryption" =>
            Ok(bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 { hash: bao_crypto::sign::RsaHash::Sha256 }),
        "rsa-sha384" | "rs384" | "sha384withrsa" | "sha384withrsaencryption" =>
            Ok(bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 { hash: bao_crypto::sign::RsaHash::Sha384 }),
        "rsa-sha512" | "rs512" | "sha512withrsa" | "sha512withrsaencryption" =>
            Ok(bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 { hash: bao_crypto::sign::RsaHash::Sha512 }),
        "rsa-pss-sha256" | "rsa-pss" | "sha256withrsa-pss" =>
            Ok(bao_crypto::sign::SignAlgorithm::RsaPss { hash: bao_crypto::sign::RsaHash::Sha256 }),
        "rsa-pss-sha384" | "sha384withrsa-pss" =>
            Ok(bao_crypto::sign::SignAlgorithm::RsaPss { hash: bao_crypto::sign::RsaHash::Sha384 }),
        "rsa-pss-sha512" | "sha512withrsa-pss" =>
            Ok(bao_crypto::sign::SignAlgorithm::RsaPss { hash: bao_crypto::sign::RsaHash::Sha512 }),
        "ecdsa-sha256" | "es256" | "sha256withecdsa" =>
            Ok(bao_crypto::sign::SignAlgorithm::EcdsaP256),
        "ecdsa-sha384" | "es384" | "sha384withecdsa" =>
            Ok(bao_crypto::sign::SignAlgorithm::EcdsaP384),
        "ed25519" | "eddsa" =>
            Ok(bao_crypto::sign::SignAlgorithm::Ed25519),
        other => Err(format!("Unsupported sign algorithm: {}", other)),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_sign(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let algo = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s,
            None => return throw_type_error(cx, "createSign() algorithm must be a string"),
        }
    } else {
        return throw_type_error(cx, "createSign() requires an algorithm name");
    };

    SIGN_ALGO.with(|a| *a.borrow_mut() = algo);
    SIGN_DATA.with(|d| d.borrow_mut().clear());

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
    let data = if (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr).into_bytes()
    } else if (*args.get(0).ptr).is_object() {
        extract_buffer_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    SIGN_DATA.with(|d| d.borrow_mut().extend_from_slice(&data));
    args.rval().set(*args.thisv().ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sign_sign(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let encoding = if argc > 1 {
        match arg_to_string(cx, *args.get(1).ptr) { Some(s) => s, None => "hex".to_string() }
    } else { "hex".to_string() };

    let algo_str = SIGN_ALGO.with(|a| a.borrow().clone());
    let data = SIGN_DATA.with(|d| d.borrow().clone());
    let key_bytes = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.into_bytes(),
            None => extract_buffer_bytes(cx, *args.get(0).ptr),
        }
    } else { Vec::new() };

    let algo = match parse_sign_algorithm(&algo_str) {
        Ok(a) => a,
        Err(e) => return throw_type_error(cx, &e),
    };

    // Try PEM first (contains "-----BEGIN"), then DER
    let key_str = ::std::str::from_utf8(&key_bytes).ok();
    let signer = if let Some(pem) = key_str {
        if pem.contains("-----BEGIN") {
            bao_crypto::sign::Signer::from_pkcs8_pem(&algo, pem)
        } else {
            bao_crypto::sign::Signer::from_pkcs8_der(&algo, &key_bytes)
        }
    } else {
        bao_crypto::sign::Signer::from_pkcs8_der(&algo, &key_bytes)
    };

    let signer = match signer {
        Ok(s) => s,
        Err(e) => return throw_type_error(cx, &format!("sign.sign() key error: {}", e)),
    };

    let sig_format = bao_crypto::sign::SignatureFormat::Der;
    let result = match signer.sign(&data, sig_format) {
        Ok(r) => r,
        Err(e) => return throw_type_error(cx, &format!("sign.sign() failed: {}", e)),
    };

    SIGN_DATA.with(|d| d.borrow_mut().clear());
    SIGN_ALGO.with(|a| a.borrow_mut().clear());

    match encoding.to_lowercase().as_str() {
        "hex" => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result)),
        "base64" => {
            let encoded_bytes = bun_base64::encode_alloc(&result);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("").to_owned();
            return_string(cx, &args, &encoded)
        }
        _ => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&result)),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_verify(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let algo = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s,
            None => return throw_type_error(cx, "createVerify() algorithm must be a string"),
        }
    } else {
        return throw_type_error(cx, "createVerify() requires an algorithm name");
    };
    SIGN_ALGO.with(|a| *a.borrow_mut() = algo);
    SIGN_DATA.with(|d| d.borrow_mut().clear());
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
    if argc < 2 { return throw_type_error(cx, "verify.verify() requires (key, signature)"); }

    let key_bytes = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(0).ptr),
    };
    let sig_bytes = if (*args.get(1).ptr).is_string() {
        decode_hex(&crate::js_to_rust_string(cx, *args.get(1).ptr))
    } else {
        extract_buffer_bytes(cx, *args.get(1).ptr)
    };

    let algo_str = SIGN_ALGO.with(|a| a.borrow().clone());
    let data = SIGN_DATA.with(|d| d.borrow().clone());

    let algo = match parse_sign_algorithm(&algo_str) {
        Ok(a) => a,
        Err(e) => return throw_type_error(cx, &e),
    };

    let key_str = ::std::str::from_utf8(&key_bytes).ok();
    let verifier = if let Some(pem) = key_str {
        if pem.contains("-----BEGIN") {
            bao_crypto::verify::Verifier::from_pkcs8_pem(&algo, pem)
        } else {
            bao_crypto::verify::Verifier::from_pkcs8_der(&algo, &key_bytes)
        }
    } else {
        bao_crypto::verify::Verifier::from_pkcs8_der(&algo, &key_bytes)
    };

    let verifier = match verifier {
        Ok(v) => v,
        Err(e) => return throw_type_error(cx, &format!("verify.verify() key error: {}", e)),
    };

    let sig_format = bao_crypto::sign::SignatureFormat::Der;
    let result = match verifier.verify(&data, &sig_bytes, sig_format) {
        Ok(r) => r,
        Err(_) => false,
    };

    SIGN_DATA.with(|d| d.borrow_mut().clear());
    SIGN_ALGO.with(|a| a.borrow_mut().clear());
    args.rval().set(mozjs::jsval::BooleanValue(result));
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
        let exported = bun_core::fmt::bytes_to_hex_lower_string(&bytes);
        let exp_str = JS_NewStringCopyN(cx, exported.as_ptr() as *const ::std::os::raw::c_char, exported.len());
        if !exp_str.is_null() {
            rooted!(&in(cx_ref) let ev = mozjs::jsval::StringValue(&*exp_str));
            JS_DefineProperty(cx, obj.handle().into(), c"export".as_ptr(), ev.handle().into(), 0);
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

// --- createECDH ---

thread_local! {
    static ECDH_PRIVATE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ECDH_PUBLIC: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ECDH_CURVE: RefCell<String> = const { RefCell::new(String::new()) };
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_ecdh(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "createECDH() requires a curve name"); }
    let curve_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s,
        None => return throw_type_error(cx, "createECDH() curve must be a string"),
    };

    let curve = match bao_crypto::key_exchange::parse_curve(&curve_name) {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("createECDH() {}", e)),
    };

    let keypair = match bao_crypto::key_exchange::EcdhKeyPair::generate(curve) {
        Ok(kp) => kp,
        Err(e) => return throw_type_error(cx, &format!("createECDH() generation failed: {}", e)),
    };

    let public_bytes = keypair.public_key_bytes();
    let private_bytes = keypair.private_key_bytes();

    ECDH_PRIVATE.with(|p| *p.borrow_mut() = private_bytes.clone());
    ECDH_PUBLIC.with(|p| *p.borrow_mut() = public_bytes.clone());
    ECDH_CURVE.with(|c| *c.borrow_mut() = curve_name);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }

    let pub_hex = bun_core::fmt::bytes_to_hex_lower_string(&public_bytes);
    let pub_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(pub_hex.as_bytes()).as_ptr());
    if !pub_js.is_null() {
        rooted!(&in(cx_ref) let pv = mozjs::jsval::StringValue(&*pub_js));
        JS_DefineProperty(cx, obj.handle().into(), c"publicKey".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let priv_hex = bun_core::fmt::bytes_to_hex_lower_string(&private_bytes);
    let priv_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(priv_hex.as_bytes()).as_ptr());
    if !priv_js.is_null() {
        rooted!(&in(cx_ref) let pv = mozjs::jsval::StringValue(&*priv_js));
        JS_DefineProperty(cx, obj.handle().into(), c"privateKey".as_ptr(), pv.handle().into(), 0);
    }

    w2::JS_DefineFunction(cx_ref, obj.handle(), c"computeSecret".as_ptr(), Some(ecdh_compute_secret), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"getPublicKey".as_ptr(), Some(ecdh_get_public_key), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"getPrivateKey".as_ptr(), Some(ecdh_get_private_key), 0, JSPROP_ENUMERATE as u32);

    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ecdh_compute_secret(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "computeSecret() requires otherPublicKey"); }
    let other_pub_hex = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s,
        None => return throw_type_error(cx, "computeSecret() otherPublicKey must be a string"),
    };
    let other_pub = decode_hex(&other_pub_hex);

    let curve_name = ECDH_CURVE.with(|c| c.borrow().clone());
    let private_bytes = ECDH_PRIVATE.with(|p| p.borrow().clone());

    let curve = match bao_crypto::key_exchange::parse_curve(&curve_name) {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("computeSecret() {}", e)),
    };

    let keypair = match bao_crypto::key_exchange::reconstruct_keypair(curve, &private_bytes) {
        Ok(kp) => kp,
        Err(e) => return throw_type_error(cx, &format!("computeSecret() key error: {}", e)),
    };

    match keypair.compute_shared_secret(&other_pub) {
        Ok(secret) => return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&secret)),
        Err(e) => throw_type_error(cx, &format!("computeSecret() failed: {}", e)),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ecdh_get_public_key(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let pub_bytes = ECDH_PUBLIC.with(|p| p.borrow().clone());
    return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&pub_bytes))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ecdh_get_private_key(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let priv_bytes = ECDH_PRIVATE.with(|p| p.borrow().clone());
    return_string(cx, &args, &bun_core::fmt::bytes_to_hex_lower_string(&priv_bytes))
}

// --- generateKeyPairSync ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_generate_key_pair_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 { return throw_type_error(cx, "generateKeyPairSync() requires a type"); }
    let type_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "generateKeyPairSync() type must be a string"),
    };

    let key_type = match type_name.as_str() {
        "rsa" => {
            let bits = if argc > 1 {
                let v = *args.get(1).ptr;
                if v.is_int32() { v.to_int32() as usize } else { 2048 }
            } else { 2048 };
            bao_crypto::keypair::KeyPairType::Rsa { bits }
        }
        "ec" => {
            let curve = if argc > 1 {
                match arg_to_string(cx, *args.get(1).ptr) {
                    Some(s) if s.to_lowercase() == "p384" || s == "secp384r1" => bao_crypto::keypair::EcCurve::P384,
                    _ => bao_crypto::keypair::EcCurve::P256,
                }
            } else { bao_crypto::keypair::EcCurve::P256 };
            bao_crypto::keypair::KeyPairType::Ec { curve }
        }
        "ed25519" => bao_crypto::keypair::KeyPairType::Ed25519,
        "x25519" => bao_crypto::keypair::KeyPairType::X25519,
        other => return throw_type_error(cx, &format!("generateKeyPairSync() unsupported type: {}", other)),
    };

    let result = match bao_crypto::keypair::generate_key_pair(&key_type) {
        Ok(r) => r,
        Err(e) => return throw_type_error(cx, &format!("generateKeyPairSync() failed: {}", e)),
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }

    let pub_hex = bun_core::fmt::bytes_to_hex_lower_string(&result.public_key_der);
    let pub_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(pub_hex.as_bytes()).as_ptr());
    if !pub_js.is_null() {
        rooted!(&in(cx_ref) let pv = mozjs::jsval::StringValue(&*pub_js));
        JS_DefineProperty(cx, obj.handle().into(), c"publicKey".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let priv_hex = bun_core::fmt::bytes_to_hex_lower_string(&result.private_key_der);
    let priv_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(priv_hex.as_bytes()).as_ptr());
    if !priv_js.is_null() {
        rooted!(&in(cx_ref) let pv = mozjs::jsval::StringValue(&*priv_js));
        JS_DefineProperty(cx, obj.handle().into(), c"privateKey".as_ptr(), pv.handle().into(), 0);
    }

    if let Some(ref pem) = result.public_key_pem {
        let pem_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(pem.as_bytes()).as_ptr());
        if !pem_js.is_null() {
            rooted!(&in(cx_ref) let pv = mozjs::jsval::StringValue(&*pem_js));
            JS_DefineProperty(cx, obj.handle().into(), c"publicKeyPem".as_ptr(), pv.handle().into(), 0);
        }
    }
    if let Some(ref pem) = result.private_key_pem {
        let pem_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(pem.as_bytes()).as_ptr());
        if !pem_js.is_null() {
            rooted!(&in(cx_ref) let pv = mozjs::jsval::StringValue(&*pem_js));
            JS_DefineProperty(cx, obj.handle().into(), c"privateKeyPem".as_ptr(), pv.handle().into(), 0);
        }
    }

    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

// --- hkdfSync ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_hkdf_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 5 { return throw_type_error(cx, "hkdfSync() requires (digest, ikm, salt, info, keylen)"); }

    let digest_name = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.to_lowercase(),
        None => return throw_type_error(cx, "hkdfSync() digest must be a string"),
    };
    let ikm = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(1).ptr),
    };
    let salt = match arg_to_string(cx, *args.get(2).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(2).ptr),
    };
    let info = match arg_to_string(cx, *args.get(3).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(3).ptr),
    };
    let key_len = {
        let v = *args.get(4).ptr;
        if v.is_int32() { v.to_int32() as usize } else { return throw_type_error(cx, "hkdfSync() keylen must be a number"); }
    };

    let hash = match digest_name.as_str() {
        "sha256" => bao_crypto::kdf::HkdfHash::Sha256,
        "sha1" => bao_crypto::kdf::HkdfHash::Sha1,
        other => return throw_type_error(cx, &format!("hkdfSync() unsupported digest: {}", other)),
    };

    let result = match bao_crypto::kdf::hkdf(hash, &salt, &ikm, &info, key_len) {
        Ok(r) => r,
        Err(e) => return throw_type_error(cx, &format!("hkdfSync() failed: {}", e)),
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, result.len()) });
    if arr.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    for (i, &byte) in result.iter().enumerate() {
        let val = mozjs::jsval::Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v = val);
        unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32); }
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

// --- X509Certificate ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_x509_certificate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "X509Certificate() requires a buffer"); }

    let input = *args.get(0).ptr;
    let cert = if input.is_string() {
        let pem_str = crate::js_to_rust_string(cx, input);
        bao_crypto::certificate::X509Certificate::from_pem(&pem_str)
    } else {
        let der_bytes = extract_buffer_bytes(cx, input);
        if der_bytes.is_empty() {
            return throw_type_error(cx, "X509Certificate() invalid buffer");
        }
        bao_crypto::certificate::X509Certificate::from_der(&der_bytes)
    };

    let cert = match cert {
        Ok(c) => c,
        Err(e) => return throw_type_error(cx, &format!("X509Certificate() parse failed: {}", e)),
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }

    let subject = cert.subject();
    let subject_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(subject.as_bytes()).as_ptr());
    if !subject_js.is_null() {
        rooted!(&in(cx_ref) let sv = mozjs::jsval::StringValue(&*subject_js));
        JS_DefineProperty(cx, obj.handle().into(), c"subject".as_ptr(), sv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let issuer = cert.issuer();
    let issuer_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(issuer.as_bytes()).as_ptr());
    if !issuer_js.is_null() {
        rooted!(&in(cx_ref) let iv = mozjs::jsval::StringValue(&*issuer_js));
        JS_DefineProperty(cx, obj.handle().into(), c"issuer".as_ptr(), iv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let fp256 = cert.fingerprint_sha256();
    let fp256_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(fp256.as_bytes()).as_ptr());
    if !fp256_js.is_null() {
        rooted!(&in(cx_ref) let fv = mozjs::jsval::StringValue(&*fp256_js));
        JS_DefineProperty(cx, obj.handle().into(), c"fingerprintSHA256".as_ptr(), fv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let fp1 = cert.fingerprint_sha1();
    let fp1_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(fp1.as_bytes()).as_ptr());
    if !fp1_js.is_null() {
        rooted!(&in(cx_ref) let fv = mozjs::jsval::StringValue(&*fp1_js));
        JS_DefineProperty(cx, obj.handle().into(), c"fingerprintSHA1".as_ptr(), fv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let valid_from = cert.valid_from();
    let vf_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(valid_from.as_bytes()).as_ptr());
    if !vf_js.is_null() {
        rooted!(&in(cx_ref) let vfv = mozjs::jsval::StringValue(&*vf_js));
        JS_DefineProperty(cx, obj.handle().into(), c"validFrom".as_ptr(), vfv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let valid_to = cert.valid_to();
    let vt_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(valid_to.as_bytes()).as_ptr());
    if !vt_js.is_null() {
        rooted!(&in(cx_ref) let vtv = mozjs::jsval::StringValue(&*vt_js));
        JS_DefineProperty(cx, obj.handle().into(), c"validTo".as_ptr(), vtv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let serial = cert.serial_number();
    let serial_js = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(serial.as_bytes()).as_ptr());
    if !serial_js.is_null() {
        rooted!(&in(cx_ref) let sev = mozjs::jsval::StringValue(&*serial_js));
        JS_DefineProperty(cx, obj.handle().into(), c"serialNumber".as_ptr(), sev.handle().into(), JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- UUID tests (retained) ---

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
    fn uuid_v4_unique() {
        assert_ne!(uuid_v4(), uuid_v4());
    }

    #[test]
    fn uuid_v4_multiple_unique() {
        let ids: Vec<String> = (0..100).map(|_| uuid_v4()).collect();
        let unique: ::std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 100);
    }

    // --- Sign algorithm parsing tests ---

    #[test]
    fn parse_sign_algorithm_rsa_pkcs1v15() {
        assert!(matches!(
            parse_sign_algorithm("RSA-SHA256"),
            Ok(bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 { hash: bao_crypto::sign::RsaHash::Sha256 })
        ));
        assert!(matches!(
            parse_sign_algorithm("rsa-sha512"),
            Ok(bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 { hash: bao_crypto::sign::RsaHash::Sha512 })
        ));
        assert!(matches!(
            parse_sign_algorithm("SHA384WithRSA"),
            Ok(bao_crypto::sign::SignAlgorithm::RsaPkcs1v15 { hash: bao_crypto::sign::RsaHash::Sha384 })
        ));
    }

    #[test]
    fn parse_sign_algorithm_rsa_pss() {
        assert!(matches!(
            parse_sign_algorithm("RSA-PSS-SHA256"),
            Ok(bao_crypto::sign::SignAlgorithm::RsaPss { hash: bao_crypto::sign::RsaHash::Sha256 })
        ));
        assert!(matches!(
            parse_sign_algorithm("rsa-pss"),
            Ok(bao_crypto::sign::SignAlgorithm::RsaPss { hash: bao_crypto::sign::RsaHash::Sha256 })
        ));
    }

    #[test]
    fn parse_sign_algorithm_ecdsa() {
        assert!(matches!(
            parse_sign_algorithm("ECDSA-SHA256"),
            Ok(bao_crypto::sign::SignAlgorithm::EcdsaP256)
        ));
        assert!(matches!(
            parse_sign_algorithm("ES384"),
            Ok(bao_crypto::sign::SignAlgorithm::EcdsaP384)
        ));
    }

    #[test]
    fn parse_sign_algorithm_ed25519() {
        assert!(matches!(
            parse_sign_algorithm("Ed25519"),
            Ok(bao_crypto::sign::SignAlgorithm::Ed25519)
        ));
    }

    #[test]
    fn parse_sign_algorithm_unsupported() {
        assert!(parse_sign_algorithm("unknown").is_err());
    }

    // --- AEAD cipher roundtrip tests (replacing xor_cipher tests) ---

    #[test]
    fn aes_256_gcm_encrypt_decrypt_roundtrip() {
        let key = b"0123456789abcdef0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"hello bao crypto real cipher";
        let aad = b"additional data";

        let algo = bao_crypto::cipher::CipherAlgorithm::Aes256Gcm;
        let result = bao_crypto::cipher::encrypt(algo, key, iv, Some(aad), plaintext).unwrap();
        assert!(!result.ciphertext.is_empty());
        assert_eq!(result.auth_tag.len(), 16);

        let decrypted = bao_crypto::cipher::decrypt(algo, key, iv, Some(aad), &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_128_gcm_encrypt_decrypt_roundtrip() {
        let key = b"0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"hello aes-128-gcm";

        let algo = bao_crypto::cipher::CipherAlgorithm::Aes128Gcm;
        let result = bao_crypto::cipher::encrypt(algo, key, iv, None, plaintext).unwrap();
        let decrypted = bao_crypto::cipher::decrypt(algo, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn chacha20_poly1305_encrypt_decrypt_roundtrip() {
        let key = b"0123456789abcdef0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"hello chacha20-poly1305";

        let algo = bao_crypto::cipher::CipherAlgorithm::ChaCha20Poly1305;
        let result = bao_crypto::cipher::encrypt(algo, key, iv, None, plaintext).unwrap();
        let decrypted = bao_crypto::cipher::decrypt(algo, key, iv, None, &result.ciphertext, &result.auth_tag).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_256_gcm_wrong_key_fails() {
        let key = b"0123456789abcdef0123456789abcdef";
        let wrong_key = b"fedcba9876543210fedcba9876543210";
        let iv = b"0123456789ab";
        let plaintext = b"secret message";

        let algo = bao_crypto::cipher::CipherAlgorithm::Aes256Gcm;
        let result = bao_crypto::cipher::encrypt(algo, key, iv, None, plaintext).unwrap();
        assert!(bao_crypto::cipher::decrypt(algo, wrong_key, iv, None, &result.ciphertext, &result.auth_tag).is_err());
    }

    #[test]
    fn aes_256_gcm_wrong_tag_fails() {
        let key = b"0123456789abcdef0123456789abcdef";
        let iv = b"0123456789ab";
        let plaintext = b"secret message";

        let algo = bao_crypto::cipher::CipherAlgorithm::Aes256Gcm;
        let result = bao_crypto::cipher::encrypt(algo, key, iv, None, plaintext).unwrap();
        let mut bad_tag = result.auth_tag.clone();
        bad_tag[0] ^= 0xff;
        assert!(bao_crypto::cipher::decrypt(algo, key, iv, None, &result.ciphertext, &bad_tag).is_err());
    }

    // --- ECDH shared secret tests ---

    #[test]
    fn ecdh_p256_shared_secret_matches() {
        let alice = bao_crypto::key_exchange::EcdhKeyPair::generate(bao_crypto::key_exchange::EcdhCurve::P256).unwrap();
        let bob = bao_crypto::key_exchange::EcdhKeyPair::generate(bao_crypto::key_exchange::EcdhCurve::P256).unwrap();
        let shared_a = alice.compute_shared_secret(&bob.public_key_bytes()).unwrap();
        let shared_b = bob.compute_shared_secret(&alice.public_key_bytes()).unwrap();
        assert_eq!(shared_a, shared_b);
    }

    #[test]
    fn ecdh_x25519_shared_secret_matches() {
        let alice = bao_crypto::key_exchange::EcdhKeyPair::generate(bao_crypto::key_exchange::EcdhCurve::X25519).unwrap();
        let bob = bao_crypto::key_exchange::EcdhKeyPair::generate(bao_crypto::key_exchange::EcdhCurve::X25519).unwrap();
        let shared_a = alice.compute_shared_secret(&bob.public_key_bytes()).unwrap();
        let shared_b = bob.compute_shared_secret(&alice.public_key_bytes()).unwrap();
        assert_eq!(shared_a, shared_b);
    }

    // --- Key pair generation tests ---

    #[test]
    fn generate_ec_p256_key_pair() {
        let result = bao_crypto::keypair::generate_key_pair(
            &bao_crypto::keypair::KeyPairType::Ec { curve: bao_crypto::keypair::EcCurve::P256 }
        ).unwrap();
        assert!(!result.private_key_der.is_empty());
        assert!(!result.public_key_der.is_empty());
    }

    #[test]
    fn generate_ed25519_key_pair() {
        let result = bao_crypto::keypair::generate_key_pair(
            &bao_crypto::keypair::KeyPairType::Ed25519
        ).unwrap();
        assert!(!result.private_key_der.is_empty());
        assert!(!result.public_key_der.is_empty());
    }

    // --- HKDF tests ---

    #[test]
    fn hkdf_sha256_deterministic() {
        let salt = b"salt";
        let ikm = b"input key material";
        let info = b"info";
        let okm1 = bao_crypto::kdf::hkdf(bao_crypto::kdf::HkdfHash::Sha256, salt, ikm, info, 32).unwrap();
        let okm2 = bao_crypto::kdf::hkdf(bao_crypto::kdf::HkdfHash::Sha256, salt, ikm, info, 32).unwrap();
        assert_eq!(okm1, okm2);
        assert_eq!(okm1.len(), 32);
    }

    // --- Decode hex tests ---

    #[test]
    fn decode_hex_valid() {
        assert_eq!(decode_hex("48656c6c6f"), b"Hello");
        assert_eq!(decode_hex(""), b"");
    }

    #[test]
    fn decode_hex_odd_length_truncates() {
        assert_eq!(decode_hex("abc"), b"\xab");
    }
}
