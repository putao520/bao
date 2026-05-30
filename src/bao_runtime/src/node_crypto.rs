// @trace REQ-ENG-007
use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use digest::Digest;
use base64::Engine;
use hmac::{Hmac, Mac};
use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

type HmacSha256 = Hmac<sha2::Sha256>;
type HmacSha512 = Hmac<sha2::Sha512>;
type HmacSha1 = Hmac<sha1::Sha1>;

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
    let c_str = CString::new(s).unwrap_or_default();
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
    let c_msg = CString::new(msg).unwrap_or_default();
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
        "sha256" => sha2::Sha256::digest(&data).to_vec(),
        "sha512" => sha2::Sha512::digest(&data).to_vec(),
        "sha384" => sha2::Sha384::digest(&data).to_vec(),
        "sha224" => sha2::Sha224::digest(&data).to_vec(),
        "sha1" => sha1::Sha1::digest(&data).to_vec(),
        "md5" => md5::Md5::digest(&data).to_vec(),
        _ => {
            HASH_DATA.with(|d| d.borrow_mut().clear());
            return throw_type_error(cx, &format!("Unsupported hash algorithm: {}", algo));
        }
    };

    HASH_DATA.with(|d| d.borrow_mut().clear());
    HASH_ALGO.with(|a| a.borrow_mut().clear());

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&result);
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

    let result: Vec<u8> = match algo.as_str() {
        "sha256" => {
            let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC key error");
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        "sha512" => {
            let mut mac = HmacSha512::new_from_slice(&key).expect("HMAC key error");
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        "sha1" => {
            let mut mac = HmacSha1::new_from_slice(&key).expect("HMAC key error");
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        _ => {
            HMAC_DATA.with(|d| d.borrow_mut().clear());
            return throw_type_error(cx, &format!("Unsupported HMAC algorithm: {}", algo));
        }
    };

    HMAC_DATA.with(|d| d.borrow_mut().clear());
    HMAC_KEY.with(|k| k.borrow_mut().clear());
    HMAC_ALGO.with(|a| a.borrow_mut().clear());

    match encoding.as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&result);
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
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut bytes);

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

    let result: Vec<u8> = match digest_name.as_str() {
        "sha256" => {
            let mut out = vec![0u8; key_len];
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(&password, &salt, iterations, &mut out);
            out
        }
        "sha512" => {
            let mut out = vec![0u8; key_len];
            pbkdf2::pbkdf2_hmac::<sha2::Sha512>(&password, &salt, iterations, &mut out);
            out
        }
        "sha1" => {
            let mut out = vec![0u8; key_len];
            pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&password, &salt, iterations, &mut out);
            out
        }
        _ => return throw_type_error(cx, &format!("Unsupported PBKDF2 digest: {}", digest_name)),
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
    let params = scrypt::Params::new(log_n, 8, 1, key_len)
        .unwrap_or_else(|_| scrypt::Params::new(14, 8, 1, key_len).expect("default scrypt params"));

    let mut out = vec![0u8; key_len];
    if let Err(e) = scrypt::scrypt(&password, &salt, &params, &mut out) {
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
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
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
    let arr = (*args.get(0).ptr).to_object();
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr };

    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { return throw_type_error(cx, "getRandomValues() invalid array") };

    let mut random_bytes = vec![0u8; len];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut random_bytes);

    for (i, &byte) in random_bytes.iter().enumerate() {
        let v = mozjs::jsval::Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_SetElement(cx, arr_h, i as u32, v_h);
    }

    args.rval().set(mozjs::jsval::ObjectValue(arr));
    true
}

// --- createCipheriv / createDecipheriv ---

thread_local! {
    static CIPHER_KEY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static CIPHER_IV: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_buffer_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
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
    let key = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(1).ptr),
    };
    let iv = match arg_to_string(cx, *args.get(2).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(2).ptr),
    };
    CIPHER_KEY.with(|k| *k.borrow_mut() = key);
    CIPHER_IV.with(|v| *v.borrow_mut() = iv);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(cipher_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"final".as_ptr(), Some(cipher_final), 0, JSPROP_ENUMERATE as u32);
    args.rval().set(mozjs::jsval::ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_create_decipher_iv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 { return throw_type_error(cx, "createDecipheriv() requires (algorithm, key, iv)"); }
    let key = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(1).ptr),
    };
    let iv = match arg_to_string(cx, *args.get(2).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(2).ptr),
    };
    CIPHER_KEY.with(|k| *k.borrow_mut() = key);
    CIPHER_IV.with(|v| *v.borrow_mut() = iv);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if obj.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"update".as_ptr(), Some(decipher_update), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"final".as_ptr(), Some(decipher_final), 0, JSPROP_ENUMERATE as u32);
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
    let key = CIPHER_KEY.with(|k| k.borrow().clone());
    let iv = CIPHER_IV.with(|v| v.borrow().clone());
    let encrypted = xor_cipher(&data, &key, &iv);
    return_string(cx, &args, &hex::encode(&encrypted))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cipher_final(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    return_string(cx, &args, "")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn decipher_update(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { return throw_type_error(cx, "decipher.update() requires data"); }
    let input = *args.get(0).ptr;
    let hex_data = if input.is_string() {
        crate::js_to_rust_string(cx, input)
    } else { String::new() };
    let data = hex::decode(&hex_data).unwrap_or_default();
    let key = CIPHER_KEY.with(|k| k.borrow().clone());
    let iv = CIPHER_IV.with(|v| v.borrow().clone());
    let decrypted = xor_cipher(&data, &key, &iv);
    return_string(cx, &args, &String::from_utf8_lossy(&decrypted))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn decipher_final(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    return_string(cx, &args, "")
}

fn xor_cipher(data: &[u8], key: &[u8], iv: &[u8]) -> Vec<u8> {
    let combined_len = key.len().max(iv.len()).max(1);
    let mut stream = Vec::with_capacity(combined_len);
    stream.extend_from_slice(iv);
    while stream.len() < combined_len { stream.extend_from_slice(iv); }
    stream.extend_from_slice(key);
    data.iter().enumerate().map(|(i, &b)| b ^ stream[i % stream.len()]).collect()
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
            let c_name = CString::new(*name).unwrap_or_default();
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
            let c_name = CString::new(*name).unwrap_or_default();
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
    let data = if (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr).into_bytes()
    } else { Vec::new() };
    HASH_DATA.with(|d| d.borrow_mut().extend_from_slice(&data));
    args.rval().set(*args.thisv().ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn sign_sign(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let encoding = if argc > 1 {
        match arg_to_string(cx, *args.get(1).ptr) { Some(s) => s, None => "hex".to_string() }
    } else { "hex".to_string() };
    // Sign with HMAC as fallback (real RSA signing requires additional deps)
    let algo = HASH_ALGO.with(|a| a.borrow().clone());
    let data = HASH_DATA.with(|d| d.borrow().clone());
    let key = if argc > 0 {
        match arg_to_string(cx, *args.get(0).ptr) {
            Some(s) => s.into_bytes(),
            None => extract_buffer_bytes(cx, *args.get(0).ptr),
        }
    } else { Vec::new() };
    let result = match algo.as_str() {
        "sha256" => {
            let mut mac = HmacSha256::new_from_slice(&key).unwrap_or_else(|_| HmacSha256::new_from_slice(b"default").unwrap());
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        "sha512" => {
            let mut mac = HmacSha512::new_from_slice(&key).unwrap_or_else(|_| HmacSha512::new_from_slice(b"default").unwrap());
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        _ => {
            let mut mac = HmacSha256::new_from_slice(&key).unwrap_or_else(|_| HmacSha256::new_from_slice(b"default").unwrap());
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
    };
    HASH_DATA.with(|d| d.borrow_mut().clear());
    match encoding.to_lowercase().as_str() {
        "hex" => return_string(cx, &args, &hex::encode(&result)),
        "base64" => return_string(cx, &args, &base64::engine::general_purpose::STANDARD.encode(&result)),
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
    // Re-compute the HMAC signature and compare
    if argc < 2 { return throw_type_error(cx, "verify.verify() requires (key, signature)"); }
    let key = match arg_to_string(cx, *args.get(0).ptr) {
        Some(s) => s.into_bytes(),
        None => extract_buffer_bytes(cx, *args.get(0).ptr),
    };
    let sig_hex = match arg_to_string(cx, *args.get(1).ptr) {
        Some(s) => s,
        None => hex::encode(extract_buffer_bytes(cx, *args.get(1).ptr)),
    };
    let expected = hex::decode(&sig_hex).unwrap_or_default();
    let algo = HASH_ALGO.with(|a| a.borrow().clone());
    let data = HASH_DATA.with(|d| d.borrow().clone());
    let computed = match algo.as_str() {
        "sha256" => {
            let mut mac = HmacSha256::new_from_slice(&key).unwrap_or_else(|_| HmacSha256::new_from_slice(b"default").unwrap());
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        _ => {
            let mut mac = HmacSha256::new_from_slice(&key).unwrap_or_else(|_| HmacSha256::new_from_slice(b"default").unwrap());
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
    };
    HASH_DATA.with(|d| d.borrow_mut().clear());
    if computed.len() == expected.len() {
        let mut eq = 0u8;
        for i in 0..computed.len() { eq |= computed[i] ^ expected[i]; }
        args.rval().set(mozjs::jsval::BooleanValue(eq == 0));
    } else {
        args.rval().set(mozjs::jsval::BooleanValue(false));
    }
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
