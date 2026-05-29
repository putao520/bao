// Global object installation entry point + Buffer + Crypto
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1,
};
use mozjs::conversions::jsstr_to_string;

use digest::Digest;

pub unsafe fn install_all(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    crate::bun_api::install_bun_global(cx, global);
    crate::bun_api::install_process_global(cx, global);
    install_buffer_global(cx, global);
    crate::fetch_api::install_fetch_global(cx, global);
    crate::fetch_api::install_response_constructor(cx, global);
    crate::fetch_api::install_headers_constructor(cx, global);
    crate::require::install_require(cx, global);
    crate::timers::install_timer_globals(cx, global);
    crate::web_api::install_performance(cx, global);
    crate::web_api::install_websocket_constructor(cx, global);
    install_crypto_global(cx, global);
    crate::node_events::install(cx);
    crate::node_path::install(cx);
    crate::node_fs::install(cx);
    crate::node_crypto::install(cx);
    crate::node_http::install(cx);
    crate::node_https::install(cx);
    crate::node_os::install(cx);
    crate::node_url::install(cx, global);
    crate::node_util::install_util(cx);
    crate::node_util::install_assert(cx);
    crate::node_child_process::install(cx);
    crate::node_stream::install(cx);
    crate::node_zlib::install(cx);
    crate::node_net::install(cx);
    crate::node_dns::install(cx);
    crate::node_buffer::install(cx);
    crate::node_string_decoder::install(cx);
    crate::node_querystring::install(cx);
    crate::web_api::install_web_encodings(cx, global);
    crate::web_api::install_atob_btoa(cx, global);
    crate::web_api::install_queue_microtask(cx, global);
}

pub fn install_buffer_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let buf_obj = JS_NewPlainObject(cx));
        if buf_obj.get().is_null() {
            return;
        }

        JS_DefineFunction(
            cx, buf_obj.handle(), c"from".as_ptr(),
            ::std::option::Option::Some(buffer_from), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"alloc".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"isBuffer".as_ptr(),
            ::std::option::Option::Some(buffer_is_buffer), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"concat".as_ptr(),
            ::std::option::Option::Some(buffer_concat), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_obj.handle(), c"allocUnsafe".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(cx, global, c"Buffer".as_ptr(), buf_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_from(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }

    let input = *args.get(0).ptr;
    if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        let bytes = s.as_bytes();
        create_buffer_from_bytes(cx, &args, bytes)
    } else if input.is_object() {
        let obj = input.to_object();
        let obj_handle = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &obj,
        };
        let mut length_val = UndefinedValue();
        let length_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut length_val,
        };
        JS_GetProperty(cx, obj_handle, c"length".as_ptr(), length_handle);
        let len = if length_val.is_int32() { length_val.to_int32() as usize } else { 0 };

        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            let mut elem = UndefinedValue();
            let elem_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            };
            JS_GetElement(cx, obj_handle, i as u32, elem_handle);
            bytes.push(if elem.is_int32() { elem.to_int32() as u8 } else { 0 });
        }
        create_buffer_from_bytes(cx, &args, &bytes)
    } else {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_buffer_from_bytes(
    cx: *mut JSContext,
    args: &CallArgs,
    bytes: &[u8],
) -> bool {
    let buf_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if buf_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_obj };

    let length_val = Int32Value(bytes.len() as i32);
    let length_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &length_val };
    JS_DefineProperty(cx, obj_handle, c"length".as_ptr(), length_handle, JSPROP_ENUMERATE as u32);

    let marker_val = Int32Value(1);
    let marker_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &marker_val };
    JS_DefineProperty(cx, obj_handle, c"_isBuffer".as_ptr(), marker_handle, 0);

    for (i, &byte) in bytes.iter().enumerate() {
        let val = Int32Value(byte as i32);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineElement(cx, obj_handle, i as u32, val_handle, JSPROP_ENUMERATE as u32);
    }

    let to_string_fn = JS_NewFunction(cx, Some(buffer_to_string), 0, 0, c"toString".as_ptr());
    if !to_string_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(to_string_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"toString".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    let slice_fn = JS_NewFunction(cx, Some(buffer_slice), 2, 0, c"slice".as_ptr());
    if !slice_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(slice_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"slice".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    let copy_fn = JS_NewFunction(cx, Some(buffer_copy), 1, 0, c"copy".as_ptr());
    if !copy_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(copy_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"copy".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_to_string(
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
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut length_val = UndefinedValue();
    let length_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut length_val };
    JS_GetProperty(cx, obj_handle, c"length".as_ptr(), length_handle);

    let len = if length_val.is_int32() { length_val.to_int32() as usize } else { 0 };
    let mut bytes = Vec::with_capacity(len);
    for i in 0..len {
        let mut elem = UndefinedValue();
        let elem_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem };
        JS_GetElement(cx, obj_handle, i as u32, elem_handle);
        bytes.push(if elem.is_int32() { elem.to_int32() as u8 } else { 0 });
    }

    let s = String::from_utf8_lossy(&bytes).into_owned();
    let Ok(c_s) = ::std::ffi::CString::new(s) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let js_str = JS_NewStringCopyZ(cx, c_s.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_alloc(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let size = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32().max(0) as usize } else { 0 }
    } else { 0 };

    create_buffer_from_bytes(cx, &args, &vec![0u8; size])
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_buffer(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let v = *args.get(0).ptr;
    if !v.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = v.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut marker = UndefinedValue();
    let marker_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut marker };
    JS_GetProperty(_cx, obj_handle, c"_isBuffer".as_ptr(), marker_handle);
    args.rval().set(mozjs::jsval::BooleanValue(marker.is_int32() && marker.to_int32() == 1));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_concat(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        create_buffer_from_bytes(cx, &args, &[])
    } else {
        let list_val = *args.get(0).ptr;
        if !list_val.is_object() {
            create_buffer_from_bytes(cx, &args, &[])
        } else {
            let list_obj = list_val.to_object();
            let list_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &list_obj };
            let mut len_val = UndefinedValue();
            JS_GetProperty(cx, list_h, c"length".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
            });
            let list_len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

            let mut all_bytes = Vec::new();
            for i in 0..list_len {
                let mut elem = UndefinedValue();
                JS_GetElement(cx, list_h, i as u32, MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut elem,
                });
                if elem.is_object() {
                    let buf_obj = elem.to_object();
                    let buf_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_obj };
                    let mut blen = UndefinedValue();
                    JS_GetProperty(cx, buf_h, c"length".as_ptr(), MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData, ptr: &mut blen,
                    });
                    let b_len = if blen.is_int32() { blen.to_int32() as usize } else { 0 };
                    for j in 0..b_len {
                        let mut byte_val = UndefinedValue();
                        JS_GetElement(cx, buf_h, j as u32, MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
                        });
                        all_bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
                    }
                }
            }
            create_buffer_from_bytes(cx, &args, &all_bytes)
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_slice(
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
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

    let start = if argc > 0 && (*args.get(0).ptr).is_int32() {
        let s = (*args.get(0).ptr).to_int32();
        if s < 0 { (len as i32 + s).max(0) as usize } else { s.min(len as i32) as usize }
    } else { 0 };

    let end = if argc > 1 && (*args.get(1).ptr).is_int32() {
        let e = (*args.get(1).ptr).to_int32();
        if e < 0 { (len as i32 + e).max(0) as usize } else { e.min(len as i32) as usize }
    } else { len };

    let mut bytes = Vec::new();
    for i in start..end.min(len) {
        let mut byte_val = UndefinedValue();
        JS_GetElement(cx, obj_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
        });
        bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
    }
    create_buffer_from_bytes(cx, &args, &bytes)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_copy(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() || argc == 0 {
        args.rval().set(Int32Value(0));
        return true;
    }

    let src_obj = this.to_object();
    let src_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &src_obj };
    let mut src_len_val = UndefinedValue();
    JS_GetProperty(cx, src_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut src_len_val,
    });
    let src_len = if src_len_val.is_int32() { src_len_val.to_int32() as usize } else { 0 };

    let target_val = *args.get(0).ptr;
    if !target_val.is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let tgt_obj = target_val.to_object();
    let tgt_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &tgt_obj };

    let tgt_start = if argc > 1 && (*args.get(1).ptr).is_int32() {
        (*args.get(1).ptr).to_int32().max(0) as usize
    } else { 0 };

    let mut copied = 0usize;
    for i in tgt_start..src_len {
        let mut byte_val = UndefinedValue();
        JS_GetElement(cx, src_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
        });
        let b = if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 };
        let b_val = Int32Value(b as i32);
        let b_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &b_val };
        JS_SetElement(cx, tgt_h, i as u32, b_handle);
        copied += 1;
    }
    args.rval().set(Int32Value(copied as i32));
    true
}

pub fn install_crypto_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let crypto_obj = JS_NewPlainObject(cx));
        if crypto_obj.get().is_null() {
            return;
        }

        JS_DefineFunction(cx, crypto_obj.handle(), c"randomUUID".as_ptr(), Some(crypto_random_uuid), 0, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, crypto_obj.handle(), c"getRandomValues".as_ptr(), Some(crypto_get_random_values), 1, JSPROP_ENUMERATE as u32);

        {
            rooted!(&in(cx) let subtle_obj = JS_NewPlainObject(cx));
            if !subtle_obj.get().is_null() {
                JS_DefineFunction(cx, subtle_obj.handle(), c"digest".as_ptr(), Some(crypto_subtle_digest), 2, JSPROP_ENUMERATE as u32);
                JS_DefineProperty3(cx, crypto_obj.handle(), c"subtle".as_ptr(), subtle_obj.handle(), JSPROP_ENUMERATE as u32);
            }
        }

        JS_DefineProperty3(cx, global, c"crypto".as_ptr(), crypto_obj.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_uuid(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let uuid = format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        rand::random::<u32>(),
        rand::random::<u16>(),
        (rand::random::<u16>() & 0x0fff) | 0x4000,
        (rand::random::<u16>() & 0x3fff) | 0x8000,
        rand::random::<u64>() & 0xffffffffffff);
    let Ok(c_uuid) = ::std::ffi::CString::new(uuid) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let js_str = JS_NewStringCopyZ(_cx, c_uuid.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_random_values(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_val = *args.get(0).ptr;
    if !arr_val.is_object() {
        args.rval().set(arr_val);
        return true;
    }
    let arr = arr_val.to_object();
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr };
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let len = if len_val.is_int32() { len_val.to_int32().max(0) as usize } else { 0 };

    let mut buf = vec![0u8; len];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut buf);
    for (i, &byte) in buf.iter().enumerate() {
        let v = Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_SetElement(cx, arr_h, i as u32, v_h);
    }
    args.rval().set(arr_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_subtle_digest(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, b"crypto.subtle.digest requires algorithm and data\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let algo_val = *args.get(0).ptr;
    let algo = if algo_val.is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked(algo_val.to_string())).to_lowercase()
    } else {
        "sha-256".to_string()
    };

    let data_val = *args.get(1).ptr;
    let bytes = if data_val.is_object() {
        let obj = data_val.to_object();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let mut len_val = UndefinedValue();
        JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
        });
        let len = if len_val.is_int32() { len_val.to_int32().max(0) as usize } else { 0 };
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(cx, obj_h, i as u32, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut elem,
            });
            v.push(if elem.is_int32() { elem.to_int32() as u8 } else { 0 });
        }
        v
    } else if data_val.is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked(data_val.to_string())).into_bytes()
    } else {
        Vec::new()
    };

    let hash = match algo.as_str() {
        "sha-1" | "sha1" => sha1::Sha1::digest(&bytes).to_vec(),
        "sha-256" | "sha256" => sha2::Sha256::digest(&bytes).to_vec(),
        "sha-384" | "sha384" => sha2::Sha384::digest(&bytes).to_vec(),
        "sha-512" | "sha512" => sha2::Sha512::digest(&bytes).to_vec(),
        _ => {
            let msg = format!("Unsupported algorithm: {}", algo);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    };

    let arr_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr_obj };
    let lv = Int32Value(hash.len() as i32);
    let lv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &lv };
    JS_DefineProperty(cx, arr_h, c"length".as_ptr(), lv_h, JSPROP_ENUMERATE as u32);
    for (i, &byte) in hash.iter().enumerate() {
        let v = Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_DefineElement(cx, arr_h, i as u32, v_h, JSPROP_ENUMERATE as u32);
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr_obj));
    true
}
