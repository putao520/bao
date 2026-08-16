// @trace REQ-ENG-007
use ::std::io::Write;
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ---------------------------------------------------------------------------
// Buffer → bytes extraction helper
// Reuses the fast typed-array path from bun_api (ArrayBuffer/Uint8Array
// direct memory access) and falls back to per-element iteration only for
// plain JS Arrays.
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    // Fast path: string, ArrayBuffer, Uint8Array/TypedArray via bun_api helper.
    if let Some(bytes) = crate::bun_api::extract_bytes_from_jsval(cx, val) {
        return bytes;
    }
    // Fallback: plain JS Array (element-by-element).
    if val.is_object() {
        let obj = val.to_object();
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj_root = obj);

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
            len_val.to_int32() as u32
        } else {
            return Vec::new();
        };

        let mut bytes = Vec::with_capacity(len as usize);
        for i in 0..len {
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
        return bytes;
    }
    Vec::new()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_bytes(cx: *mut JSContext, args: &CallArgs, data: Vec<u8>) -> bool {
    // Fast path: create a Uint8Array via bun_api helper, then wrap in Buffer.
    let u8_val = crate::bun_api::bytes_to_js_uint8array(cx, &data);
    if u8_val.is_object() {
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let global_root = global);

            let mut buffer_ctor = UndefinedValue();
            JS_GetProperty(
                cx,
                global_root.handle().into(),
                c"Buffer".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut buffer_ctor,
                },
            );
            if buffer_ctor.is_object() {
                let buffer_ctor_obj = buffer_ctor.to_object();
                rooted!(&in(wrapped_cx) let buffer_ctor_root = buffer_ctor_obj);
                let mut from_fn = UndefinedValue();
                JS_GetProperty(
                    cx,
                    buffer_ctor_root.handle().into(),
                    c"from".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut from_fn,
                    },
                );
                if from_fn.is_object() {
                    rooted!(&in(wrapped_cx) let u8_val_root = u8_val);
                    let u8_val_copy = *u8_val_root;
                    let call_args = HandleValueArray {
                        length_: 1,
                        elements_: &u8_val_copy as *const JSVal,
                    };
                    rooted!(&in(wrapped_cx) let ctor_root = from_fn);
                    let mut rval = UndefinedValue();
                    JS_CallFunctionValue(
                        cx,
                        global_root.handle().into(),
                        ctor_root.handle().into(),
                        &call_args,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rval,
                        },
                    );
                    args.rval().set(rval);
                    return true;
                }
            }
        }
    }
    // Fallback: set the Uint8Array directly if Buffer.from is unavailable.
    args.rval().set(u8_val);
    true
}

// ---------------------------------------------------------------------------
// Options extraction helper — reads level/strategy/memLevel from options obj
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_compression_level(cx: *mut JSContext, opts_val: JSVal) -> flate2::Compression {
    let mut level = flate2::Compression::default();
    if !opts_val.is_object() {
        return level;
    }
    let obj = opts_val.to_object();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);

    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"level".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_int32() {
        let l = val.to_int32();
        if l >= 0 && l <= 9 {
            level = flate2::Compression::new(l as u32);
        }
    }
    level
}

// ---------------------------------------------------------------------------
// ZlibError construction — node throws `ZlibError` with zlib's own message
// ("incorrect header check", "unexpected end of file", …) plus the
// code/errno pair (Z_DATA_ERROR/-3 for data errors, Z_STREAM_ERROR/-2 for
// the encoder path). Same harvest-a-pending-exception pattern as
// bun_api::make_coded_error_value.
// ---------------------------------------------------------------------------

/// Build a ZlibError-shaped Error VALUE (message + code + errno stamped),
/// leaving no pending exception behind.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn zlib_error_value(cx: *mut JSContext, message: &str, code: &str, errno: i32) -> JSVal {
    let c_msg = ZBox::from_bytes(message.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    let mut exn = UndefinedValue();
    let exn_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut exn,
    };
    if !JS_GetPendingException(cx, exn_h) || !exn.is_object() {
        JS_ClearPendingException(cx);
        return UndefinedValue();
    }
    JS_ClearPendingException(cx);
    let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let exn_obj = exn.to_object());
    let c_code = ZBox::from_bytes(code.as_bytes());
    let code_js = JS_NewStringCopyZ(cx, c_code.as_ptr());
    if !code_js.is_null() {
        rooted!(&in(cx_ref) let code_val = StringValue(unsafe { &*code_js }));
        JS_DefineProperty(
            cx,
            exn_obj.handle().into(),
            c"code".as_ptr(),
            code_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    rooted!(&in(cx_ref) let errno_val = mozjs::jsval::Int32Value(errno));
    JS_DefineProperty(
        cx,
        exn_obj.handle().into(),
        c"errno".as_ptr(),
        errno_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    ObjectValue(exn_obj.get())
}

/// Throw a ZlibError with a reason; `false` propagates out of the native so
/// the exception reaches JS (node contract: corrupt input throws loudly —
/// returning undefined was a silent swallow).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_zlib_error(cx: *mut JSContext, message: &str, code: &str, errno: i32) -> bool {
    let err_val = zlib_error_value(cx, message, code, errno);
    if err_val.is_object() {
        let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let err_ev = err_val);
        JS_SetPendingException(cx, err_ev.handle().into(), ExceptionStackBehavior::DoNotCapture);
    } else {
        // Error-object construction failed: still fail-closed with a plain
        // pending error instead of returning a success value.
        let c_msg = ZBox::from_bytes(message.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    }
    false
}

/// One-shot decompress through the bun_zlib streaming state machine
/// (multi-member gzip, per-member CRC/ISIZE) with node-classified failures.
/// `window_bits`: 15 zlib, 16 gzip, -15 raw, 47 auto. Throws ZlibError on
/// corrupt/truncated input; `None` means "exception pending, return false".
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn decompress_checked_or_throw(
    cx: *mut JSContext,
    data: &[u8],
    window_bits: core::ffi::c_int,
) -> Option<Vec<u8>> {
    match bun_zlib::inflate_decompress_checked(data, window_bits) {
        Ok(decompressed) => Some(decompressed),
        Err(failure) => {
            throw_zlib_error(cx, failure.message(), "Z_DATA_ERROR", -3);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Native sync functions — accept buffer-like, return Buffer
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn zlib_deflate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let level = if argc > 1 {
        extract_compression_level(cx, *args.get(1).ptr)
    } else {
        flate2::Compression::default()
    };
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), level);
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => return_bytes(cx, &args, compressed),
        Err(_) => throw_zlib_error(cx, "deflate error", "Z_STREAM_ERROR", -2),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn zlib_inflate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    match decompress_checked_or_throw(cx, &data, 15) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => false,
    }
}

/// `zlib.crc32(data[, value])` — Node 18+ zlib CRC-32 checksum with
/// continuation. `value` is a previously returned CRC (default 0); the result
/// equals the CRC of `previous_data + data` (zlib `crc32(crc, buf, len)`
/// semantics). `crc32fast::Hasher` stores the running FINALIZED-crc state
/// (`new()` ⇒ 0 == crc of empty input), so continuing from a prior result
/// seeds with that result directly.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_crc32(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"crc32() requires a data argument (string, Buffer, TypedArray, or DataView)"
                .as_ptr(),
        );
        return false;
    }
    let prior: u32 = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32() as u32
        } else if v.is_double() {
            let d = v.to_double();
            if d < 0.0 || d > u32::MAX as f64 || !d.is_finite() {
                JS_ReportErrorUTF8(cx, c"crc32() value must be a valid unsigned 32-bit integer".as_ptr());
                return false;
            }
            d as u32
        } else if v.is_undefined() {
            0
        } else {
            JS_ReportErrorUTF8(cx, c"crc32() value must be a number".as_ptr());
            return false;
        }
    } else {
        0
    };

    let data = extract_bytes(cx, *args.get(0).ptr);
    let mut hasher = crc32fast::Hasher::new_with_initial(prior);
    hasher.update(&data);
    let crc = hasher.finalize();
    // Node contract: zlib.crc32 returns an UNSIGNED 32-bit integer (0..2^32-1)
    // as a JS Number. Int32Value would sign-flip results ≥ 2^31 (e.g.
    // crc32(0xFF×4) = 4294967295 would surface as -1); f64 represents the
    // full u32 range exactly.
    args.rval().set(mozjs::jsval::DoubleValue(crc as f64));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_deflate_raw_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let level = if argc > 1 {
        extract_compression_level(cx, *args.get(1).ptr)
    } else {
        flate2::Compression::default()
    };
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), level);
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => return_bytes(cx, &args, compressed),
        Err(_) => throw_zlib_error(cx, "deflateRaw error", "Z_STREAM_ERROR", -2),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_inflate_raw_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    match decompress_checked_or_throw(cx, &data, -15) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => false,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn zlib_gzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let level = if argc > 1 {
        extract_compression_level(cx, *args.get(1).ptr)
    } else {
        flate2::Compression::default()
    };
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), level);
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => return_bytes(cx, &args, compressed),
        Err(_) => throw_zlib_error(cx, "gzip error", "Z_STREAM_ERROR", -2),
    }
}

// gunzipSync — forced gzip (node windowBits 15|16) through the streaming
// state machine: multi-member streams decode ALL members (RFC 1952 §2.2
// concatenation, same engine as the HTTP pipeline), each member's CRC32 +
// ISIZE verified; corrupt input throws ZlibError instead of returning
// undefined (was: single member + silent swallow).
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn zlib_gunzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    match decompress_checked_or_throw(cx, &data, 16) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => false,
    }
}

// unzipSync — auto-detect (node windowBits 15+32) through the same state
// machine: sniffs gzip/zlib headers, falls back to raw deflate, and THROWS
// on input none of them can parse. The old three-decoder cascade treated a
// successful decode of EMPTY output as failure, so unzipSync(gzipSync(''))
// returned undefined; empty members are legitimate and now decode.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_unzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    match decompress_checked_or_throw(cx, &data, 47) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Brotli sync functions — using bun_brotli crate
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_brotli_options(cx: *mut JSContext, opts_val: JSVal) -> (u32, u32) {
    // Returns (quality, lgwin)
    let mut quality = 11u32; // brotli default
    let mut lgwin = 22u32; // brotli default
    if !opts_val.is_object() {
        return (quality, lgwin);
    }
    let obj = opts_val.to_object();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);

    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"quality".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_int32() {
        let q = val.to_int32();
        if q >= 0 && q <= 11 {
            quality = q as u32;
        }
    }

    let mut val2 = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"lgwin".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val2,
        },
    );
    if val2.is_int32() {
        let w = val2.to_int32();
        if w >= 10 && w <= 24 {
            lgwin = w as u32;
        }
    }

    (quality, lgwin)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_brotli_compress_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let (quality, lgwin) = if argc > 1 {
        extract_brotli_options(cx, *args.get(1).ptr)
    } else {
        (11, 22)
    };
    let compressed = bun_brotli::compress(&data, quality, lgwin);
    return_bytes(cx, &args, compressed)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_brotli_decompress_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    match bun_brotli::decompress(&data) {
        Ok(decompressed) => return_bytes(cx, &args, decompressed),
        Err(e) => throw_zlib_error(cx, &format!("brotli decompress failed: {e}"), "Z_DATA_ERROR", -3),
    }
}

// ---------------------------------------------------------------------------
// Async callback-style functions — deflate/inflate/gzip/gunzip/unzip/brotli
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn call_callback(cx: *mut JSContext, callback: JSVal, err: JSVal, result: JSVal) {
    if !callback.is_object() {
        return;
    }
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);
    let cb_obj = callback.to_object();
    let cb_val = mozjs::jsval::ObjectValue(cb_obj);
    rooted!(&in(wrapped_cx) let cb_root = cb_val);

    let vals = [err, result];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: vals.as_ptr(),
    };
    let mut rval = UndefinedValue();
    JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        cb_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
}

/// Create a JS Buffer from a byte slice using the fast Uint8Array path.
/// Returns UndefinedValue on failure.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn make_buffer_val(cx: *mut JSContext, data: &[u8]) -> JSVal {
    let u8_val = crate::bun_api::bytes_to_js_uint8array(cx, data);
    if !u8_val.is_object() {
        return UndefinedValue();
    }
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return u8_val;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);

    let mut buffer_ctor = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"Buffer".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut buffer_ctor,
        },
    );
    if !buffer_ctor.is_object() {
        return u8_val;
    }
    let buffer_ctor_obj = buffer_ctor.to_object();
    rooted!(&in(wrapped_cx) let buffer_ctor_root = buffer_ctor_obj);
    let mut from_fn = UndefinedValue();
    JS_GetProperty(
        cx,
        buffer_ctor_root.handle().into(),
        c"from".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut from_fn,
        },
    );
    if !from_fn.is_object() {
        return u8_val;
    }
    rooted!(&in(wrapped_cx) let u8_val_root = u8_val);
    let u8_val_copy = *u8_val_root;
    let call_args = HandleValueArray {
        length_: 1,
        elements_: &u8_val_copy as *const JSVal,
    };
    rooted!(&in(wrapped_cx) let ctor_root = from_fn);
    let mut buf_rval = UndefinedValue();
    JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        ctor_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut buf_rval,
        },
    );
    buf_rval
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_deflate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let level = if argc > 1 {
        extract_compression_level(cx, *args.get(1).ptr)
    } else {
        flate2::Compression::default()
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), level);
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => {
            if callback.is_object() {
                let buf_val = make_buffer_val(cx, &compressed);
                call_callback(cx, callback, UndefinedValue(), buf_val);
            }
        }
        Err(_) => {
            if callback.is_object() {
                let err_val = zlib_error_value(cx, "deflate error", "Z_STREAM_ERROR", -2);
                call_callback(cx, callback, err_val, UndefinedValue());
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_inflate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    match bun_zlib::inflate_decompress_checked(&data, 15) {
        Ok(decompressed) => {
            if callback.is_object() {
                let buf_val = make_buffer_val(cx, &decompressed);
                call_callback(cx, callback, UndefinedValue(), buf_val);
            }
        }
        Err(failure) => {
            if callback.is_object() {
                let err_val = zlib_error_value(cx, failure.message(), "Z_DATA_ERROR", -3);
                call_callback(cx, callback, err_val, UndefinedValue());
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_gzip(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let level = if argc > 1 && !(*args.get(1).ptr).is_object() {
        extract_compression_level(cx, *args.get(1).ptr)
    } else {
        flate2::Compression::default()
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), level);
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => {
            if callback.is_object() {
                let buf_val = make_buffer_val(cx, &compressed);
                call_callback(cx, callback, UndefinedValue(), buf_val);
            }
        }
        Err(_) => {
            if callback.is_object() {
                let err_val = zlib_error_value(cx, "gzip error", "Z_STREAM_ERROR", -2);
                call_callback(cx, callback, err_val, UndefinedValue());
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_gunzip(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    // Same engine as gunzipSync: multi-member + per-member CRC/ISIZE; the
    // callback receives a ZlibError-shaped Error (was: bare string, and the
    // old GzDecoder silently dropped every member after the first).
    match bun_zlib::inflate_decompress_checked(&data, 16) {
        Ok(decompressed) => {
            if callback.is_object() {
                let buf_val = make_buffer_val(cx, &decompressed);
                call_callback(cx, callback, UndefinedValue(), buf_val);
            }
        }
        Err(failure) => {
            if callback.is_object() {
                let err_val = zlib_error_value(cx, failure.message(), "Z_DATA_ERROR", -3);
                call_callback(cx, callback, err_val, UndefinedValue());
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_unzip(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    // Auto-detect through the same state machine as unzipSync (empty
    // members are legitimate output, failures carry a ZlibError value).
    match bun_zlib::inflate_decompress_checked(&data, 47) {
        Ok(decompressed) => {
            if callback.is_object() {
                let buf_val = make_buffer_val(cx, &decompressed);
                call_callback(cx, callback, UndefinedValue(), buf_val);
            }
        }
        Err(failure) => {
            if callback.is_object() {
                let err_val = zlib_error_value(cx, failure.message(), "Z_DATA_ERROR", -3);
                call_callback(cx, callback, err_val, UndefinedValue());
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_brotli_compress(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let (quality, lgwin) = if argc > 1 && !(*args.get(1).ptr).is_object() {
        extract_brotli_options(cx, *args.get(1).ptr)
    } else {
        (11, 22)
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let compressed = bun_brotli::compress(&data, quality, lgwin);
    if callback.is_object() {
        let buf_val = make_buffer_val(cx, &compressed);
        call_callback(cx, callback, UndefinedValue(), buf_val);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_brotli_decompress(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 {
        extract_bytes(cx, *args.get(0).ptr)
    } else {
        Vec::new()
    };
    let callback = if argc > 2 {
        *args.get(2).ptr
    } else if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    match bun_brotli::decompress(&data) {
        Ok(decompressed) => {
            if callback.is_object() {
                let buf_val = make_buffer_val(cx, &decompressed);
                call_callback(cx, callback, UndefinedValue(), buf_val);
            }
        }
        Err(e) => {
            if callback.is_object() {
                let err_val = zlib_error_value(
                    cx,
                    &format!("brotli decompress failed: {e}"),
                    "Z_DATA_ERROR",
                    -3,
                );
                call_callback(cx, callback, err_val, UndefinedValue());
            }
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ---------------------------------------------------------------------------
// JS polyfill — Transform stream classes + constants + convenience wrappers
// ---------------------------------------------------------------------------

const ZLIB_JS: &str = r#"
(function() {
  // ── Minimal EventEmitter (shared with node:stream, duplicated for isolation) ──
  function EE() { this._events = {}; }
  EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
  EE.prototype.once = function(e, fn) { var self = this; function w() { self.removeListener(e, w); fn.apply(this, arguments); }; fn._onceWrapper = w; return this.on(e, w); };
  EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) { ls = ls.slice(); for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); } return !!ls; };
  EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var idx = ls.indexOf(fn); if (idx === -1 && fn._onceWrapper) idx = ls.indexOf(fn._onceWrapper); if (idx >= 0) ls.splice(idx, 1); } return this; };
  EE.prototype.removeAllListeners = function(e) { if (e) delete this._events[e]; else this._events = {}; return this; };

  // ── Minimal Transform base (Duplex: Readable + Writable) ──
  function RS(opts) { this.buffer = []; this.length = 0; this.ended = false; this.endEmitted = false; this.flowing = false; this.paused = false; this.hwm = (opts && opts.highWaterMark) || 16384; }
  function WS(opts) { this.buffer = []; this.writing = false; this.ended = false; this.finished = false; this.hwm = (opts && opts.highWaterMark) || 16384; this.corked = 0; this.corkBuffer = []; }

  function ZlibTransform(opts) {
    if (!(this instanceof ZlibTransform)) return new ZlibTransform(opts);
    EE.call(this);
    this._readableState = new RS(opts);
    this._writableState = new WS(opts);
    this._chunks = [];
    this._processing = false;
    this.readable = true;
    this.writable = true;
    this.destroyed = false;
    this.bytesWritten = 0;
    this.bytesRead = 0;
  }
  ZlibTransform.prototype = Object.create(EE.prototype);
  ZlibTransform.prototype.constructor = ZlibTransform;

  ZlibTransform.prototype.push = function(chunk) {
    var s = this._readableState;
    if (chunk === null) { s.ended = true; if (s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); } return false; }
    s.buffer.push(chunk);
    s.length += (chunk && chunk.length) || 1;
    if (s.flowing && !s.paused) { var d = s.buffer.shift(); s.length -= (d && d.length) || 1; this.emit("data", d); if (s.ended && s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); } }
    return s.length < s.hwm;
  };

  ZlibTransform.prototype.write = function(chunk) {
    if (this._writableState.ended) return false;
    this._chunks.push(chunk);
    this.bytesWritten += (chunk && chunk.length) || 0;
    return true;
  };

  ZlibTransform.prototype.end = function(chunk) {
    if (chunk) this.write(chunk);
    this._writableState.ended = true;
    this.writable = false;
    return this;
  };

  ZlibTransform.prototype.read = function() {
    var s = this._readableState;
    if (s.buffer.length > 0) { var d = s.buffer.shift(); s.length -= (d && d.length) || 1; if (s.ended && s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); } return d; }
    return null;
  };

  ZlibTransform.prototype.pipe = function(dest) { this.on("data", function(c) { dest.write(c); }); this.on("end", function() { dest.end(); }); return dest; };

  ZlibTransform.prototype.on = function(e, fn) {
    EE.prototype.on.call(this, e, fn);
    if (e === "data") { this._readableState.flowing = true; this._readableState.paused = false; }
    return this;
  };

  ZlibTransform.prototype.flush = function(kind) { /* no-op in base; subclasses override */ };
  ZlibTransform.prototype.reset = function() { this._chunks = []; this.bytesWritten = 0; this.bytesRead = 0; };
  ZlibTransform.prototype.destroy = function(err) { if (this.destroyed) return this; this.destroyed = true; this.readable = false; this.writable = false; if (err) this.emit("error", err); this.emit("close"); return this; };

  // ── Deflate/Inflate/Gzip/Gunzip/DeflateRaw/InflateRaw Transform classes ──
  // These accumulate chunks and process on end/flush via native sync functions.

  function Deflate(opts) { ZlibTransform.call(this, opts); this._level = (opts && opts.level) || -1; }
  Deflate.prototype = Object.create(ZlibTransform.prototype);
  Deflate.prototype.constructor = Deflate;
  Deflate.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_deflateSync(all, { level: this._level });
  };

  function Inflate(opts) { ZlibTransform.call(this, opts); }
  Inflate.prototype = Object.create(ZlibTransform.prototype);
  Inflate.prototype.constructor = Inflate;
  Inflate.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_inflateSync(all);
  };

  function Gzip(opts) { ZlibTransform.call(this, opts); this._level = (opts && opts.level) || -1; }
  Gzip.prototype = Object.create(ZlibTransform.prototype);
  Gzip.prototype.constructor = Gzip;
  Gzip.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_gzipSync(all, { level: this._level });
  };

  function Gunzip(opts) { ZlibTransform.call(this, opts); }
  Gunzip.prototype = Object.create(ZlibTransform.prototype);
  Gunzip.prototype.constructor = Gunzip;
  Gunzip.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_gunzipSync(all);
  };

  function DeflateRaw(opts) { ZlibTransform.call(this, opts); this._level = (opts && opts.level) || -1; }
  DeflateRaw.prototype = Object.create(ZlibTransform.prototype);
  DeflateRaw.prototype.constructor = DeflateRaw;
  DeflateRaw.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_deflateRawSync(all, { level: this._level });
  };

  function InflateRaw(opts) { ZlibTransform.call(this, opts); }
  InflateRaw.prototype = Object.create(ZlibTransform.prototype);
  InflateRaw.prototype.constructor = InflateRaw;
  InflateRaw.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_inflateRawSync(all);
  };

  // ── Brotli Transform classes ──
  function BrotliCompress(opts) { ZlibTransform.call(this, opts); this._quality = (opts && opts.quality) || 11; this._lgwin = (opts && opts.lgwin) || 22; }
  BrotliCompress.prototype = Object.create(ZlibTransform.prototype);
  BrotliCompress.prototype.constructor = BrotliCompress;
  BrotliCompress.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_brotliCompressSync(all, { quality: this._quality, lgwin: this._lgwin });
  };

  function BrotliDecompress(opts) { ZlibTransform.call(this, opts); }
  BrotliDecompress.prototype = Object.create(ZlibTransform.prototype);
  BrotliDecompress.prototype.constructor = BrotliDecompress;
  BrotliDecompress.prototype._process = function() {
    var all = Buffer.concat(this._chunks);
    this._chunks = [];
    return __zlib_brotliDecompressSync(all);
  };

  // ── Patch end() on all transform classes to process + emit data/finish ──
  // The native sync bridges throw ZlibError on corrupt input; end() turns
  // that into the node 'error' event (and does NOT emit end/finish after a
  // failure — a bad stream must surface, not silently END with len=0).
  var classes = [Deflate, Inflate, Gzip, Gunzip, DeflateRaw, InflateRaw, BrotliCompress, BrotliDecompress];
  classes.forEach(function(Cls) {
    var origEnd = Cls.prototype.end;
    Cls.prototype.end = function(chunk) {
      if (chunk) this.write(chunk);
      this._writableState.ended = true;
      this.writable = false;
      var result;
      try {
        result = this._process();
      } catch (e) {
        this.destroy(e);
        return this;
      }
      if (result !== undefined && result !== null) {
        this.bytesRead += result.length;
        this.push(result);
      }
      this.push(null);
      this.emit("finish");
      return this;
    };
    Cls.prototype.flush = function(kind) {
      try {
        var result = this._process();
        if (result !== undefined && result !== null) this.push(result);
      } catch(e) {
        this.emit("error", e);
      }
    };
  });

  // ── Constants ──
  var constants = {
    Z_NO_FLUSH: 0, Z_PARTIAL_FLUSH: 1, Z_SYNC_FLUSH: 2, Z_FULL_FLUSH: 3,
    Z_FINISH: 4, Z_BLOCK: 5, Z_TREES: 6,
    Z_OK: 0, Z_STREAM_END: 1, Z_NEED_DICT: 2,
    Z_ERRNO: -1, Z_STREAM_ERROR: -2, Z_DATA_ERROR: -3, Z_MEM_ERROR: -4,
    Z_BUF_ERROR: -5, Z_VERSION_ERROR: -6,
    Z_NO_COMPRESSION: 0, Z_BEST_SPEED: 1, Z_BEST_COMPRESSION: 9, Z_DEFAULT_COMPRESSION: -1,
    Z_FILTERED: 1, Z_HUFFMAN_ONLY: 2, Z_RLE: 3, Z_FIXED: 4, Z_DEFAULT_STRATEGY: 0,
    ZLIB_VERSION: "1.2.13", DEFLATE: 1, INFLATE: 2, GZIP: 3, GUNZIP: 4, DEFLATERAW: 5, INFLATERAW: 6, UNZIP: 7,
    BROTLI_DECODE: 8, BROTLI_ENCODE: 9,
    // Brotli constants
    BROTLI_OK: 0, BROTLI_ERROR: -1,
    BROTLI_MODE_GENERIC: 0, BROTLI_MODE_TEXT: 1, BROTLI_MODE_FONT: 2,
    BROTLI_DEFAULT_QUALITY: 11, BROTLI_MIN_QUALITY: 0, BROTLI_MAX_QUALITY: 11,
    BROTLI_DEFAULT_WINDOW: 22, BROTLI_MIN_WINDOW_BITS: 10, BROTLI_MAX_WINDOW_BITS: 24,
    BROTLI_LARGE_MAX_WINDOW_BITS: 24,
    BROTLI_MIN_INPUT_BLOCK_BITS: 16, BROTLI_MAX_INPUT_BLOCK_BITS: 24,
    BROTLI_MAX_INPUT_BLOCK_BITS: 24,
    BROTLI_OPERATION_PROCESS: 0, BROTLI_OPERATION_FLUSH: 1,
    BROTLI_OPERATION_FINISH: 2, BROTLI_OPERATION_EMIT_METADATA: 3,
  };

  // ── Export ──
  return {
    // Transform stream classes
    Deflate: Deflate, Inflate: Inflate, Gzip: Gzip, Gunzip: Gunzip,
    DeflateRaw: DeflateRaw, InflateRaw: InflateRaw,
    BrotliCompress: BrotliCompress, BrotliDecompress: BrotliDecompress,
    // Factory functions
    createDeflate: function(o) { return new Deflate(o); },
    createInflate: function(o) { return new Inflate(o); },
    createGzip: function(o) { return new Gzip(o); },
    createGunzip: function(o) { return new Gunzip(o); },
    createDeflateRaw: function(o) { return new DeflateRaw(o); },
    createInflateRaw: function(o) { return new InflateRaw(o); },
    createBrotliCompress: function(o) { return new BrotliCompress(o); },
    createBrotliDecompress: function(o) { return new BrotliDecompress(o); },
    // Constants
    constants: constants,
  };
})();
"#;

// ---------------------------------------------------------------------------
// Module install
// ---------------------------------------------------------------------------

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();

        // Sync functions
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"deflateSync".as_ptr(),
            Some(zlib_deflate_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"inflateSync".as_ptr(),
            Some(zlib_inflate_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );
        // @trace REQ-ENG-007 [api:zlib.crc32] — Node 18+: crc32(data[, value])
        // computes/continues the zlib CRC-32 (crc32fast, same backend as
        // bun_zlib per REQ-BAO-API-010).
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"crc32".as_ptr(),
            Some(zlib_crc32),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"deflateRawSync".as_ptr(),
            Some(zlib_deflate_raw_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"inflateRawSync".as_ptr(),
            Some(zlib_inflate_raw_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"gzipSync".as_ptr(),
            Some(zlib_gzip_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"gunzipSync".as_ptr(),
            Some(zlib_gunzip_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"unzipSync".as_ptr(),
            Some(zlib_unzip_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Brotli sync functions
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"brotliCompressSync".as_ptr(),
            Some(zlib_brotli_compress_sync),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"brotliDecompressSync".as_ptr(),
            Some(zlib_brotli_decompress_sync),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Async callback-style functions
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"deflate".as_ptr(),
            Some(zlib_deflate),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"inflate".as_ptr(),
            Some(zlib_inflate),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"gzip".as_ptr(),
            Some(zlib_gzip),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"gunzip".as_ptr(),
            Some(zlib_gunzip),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"unzip".as_ptr(),
            Some(zlib_unzip),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"brotliCompress".as_ptr(),
            Some(zlib_brotli_compress),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"brotliDecompress".as_ptr(),
            Some(zlib_brotli_decompress),
            3,
            JSPROP_ENUMERATE as u32,
        );

        // Internal helpers used by JS Transform classes (prefixed with __)
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_deflateSync".as_ptr(),
            Some(zlib_deflate_sync),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_inflateSync".as_ptr(),
            Some(zlib_inflate_sync),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_gzipSync".as_ptr(),
            Some(zlib_gzip_sync),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_gunzipSync".as_ptr(),
            Some(zlib_gunzip_sync),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_deflateRawSync".as_ptr(),
            Some(zlib_deflate_raw_sync),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_inflateRawSync".as_ptr(),
            Some(zlib_inflate_raw_sync),
            1,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_brotliCompressSync".as_ptr(),
            Some(zlib_brotli_compress_sync),
            2,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"__zlib_brotliDecompressSync".as_ptr(),
            Some(zlib_brotli_decompress_sync),
            1,
            0,
        );

        // The ZLIB_JS IIFE calls these host bridges as FREE variables inside
        // the transform classes' _process() methods (`return
        // __zlib_deflateSync(all, {...})`) — free-variable lookup goes to the
        // GLOBAL, never to this module object. Defining them only on mod_obj
        // meant every _process() call threw ReferenceError (caught by the
        // end()/flush() wrappers and re-emitted as 'error'), so the whole
        // Gzip/Deflate/Brotli stream surface was broken. Mirror them onto
        // the global (non-enumerable, configurable) so the IIFE sees them
        // (same class as the http2 fix, commit 854677b0).
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let bridges: &[(&str, JSNative, u32)] = &[
                ("__zlib_deflateSync", Some(zlib_deflate_sync), 2),
                ("__zlib_inflateSync", Some(zlib_inflate_sync), 1),
                ("__zlib_gzipSync", Some(zlib_gzip_sync), 2),
                ("__zlib_gunzipSync", Some(zlib_gunzip_sync), 1),
                ("__zlib_deflateRawSync", Some(zlib_deflate_raw_sync), 2),
                ("__zlib_inflateRawSync", Some(zlib_inflate_raw_sync), 1),
                (
                    "__zlib_brotliCompressSync",
                    Some(zlib_brotli_compress_sync),
                    2,
                ),
                (
                    "__zlib_brotliDecompressSync",
                    Some(zlib_brotli_decompress_sync),
                    1,
                ),
            ];
            for &(name, native, nargs) in bridges {
                let c_name = ZBox::from_bytes(name);
                w2::JS_DefineFunction(
                    cx,
                    global_root.handle(),
                    c_name.as_ptr(),
                    native,
                    nargs,
                    0,
                );
            }
        }

        let c_filename = ZBox::from_bytes("node:zlib".as_bytes());
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(ZLIB_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            cache_builtin(cx, "zlib", mod_obj.get());
            return;
        }

        let exports_obj = rval.to_object();
        rooted!(&in(cx) let exports_rooted = exports_obj);

        for name in &[
            "Deflate",
            "Inflate",
            "Gzip",
            "Gunzip",
            "DeflateRaw",
            "InflateRaw",
            "BrotliCompress",
            "BrotliDecompress",
            "createDeflate",
            "createInflate",
            "createGzip",
            "createGunzip",
            "createDeflateRaw",
            "createInflateRaw",
            "createBrotliCompress",
            "createBrotliDecompress",
            "constants",
        ] {
            let cname = ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(
                cx_raw,
                exports_rooted.handle().into(),
                cname.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut val,
                },
            );
            if !val.is_undefined() {
                rooted!(&in(cx) let val_root = val);
                JS_DefineProperty(
                    cx_raw,
                    mod_obj.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "zlib", mod_obj.get());
    }
}
