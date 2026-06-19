// @trace REQ-ENG-007
use bun_core::ZBox;
use ::std::io::Read;
use ::std::io::Write;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ---------------------------------------------------------------------------
// Buffer → bytes extraction helper
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    if val.is_string() {
        let s = jsstr_to_string(cx, NonNull::new_unchecked(val.to_string()));
        return s.into_bytes();
    }
    if val.is_object() {
        let obj = val.to_object();
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj_root = obj);

        let mut len_val = UndefinedValue();
        JS_GetProperty(cx, obj_root.handle().into(), c"length".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
        });
        let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { return Vec::new() };

        let mut bytes = Vec::with_capacity(len as usize);
        for i in 0..len {
            let mut byte_val = UndefinedValue();
            JS_GetElement(cx, obj_root.handle().into(), i, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut byte_val,
            });
            bytes.push(if byte_val.is_int32() { byte_val.to_int32() as u8 } else { 0 });
        }
        return bytes;
    }
    Vec::new()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_bytes(cx: *mut JSContext, args: &CallArgs, data: Vec<u8>) -> bool {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global_root = global);

    let mut buffer_ctor = UndefinedValue();
    JS_GetProperty(cx, global_root.handle().into(), c"Buffer".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut buffer_ctor,
    });
    if !buffer_ctor.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let buffer_ctor_obj = buffer_ctor.to_object();
    rooted!(&in(wrapped_cx) let buffer_ctor_root = buffer_ctor_obj);
    let mut from_fn = UndefinedValue();
    JS_GetProperty(cx, buffer_ctor_root.handle().into(), c"from".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut from_fn,
    });
    if !from_fn.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let arr_obj = JS_NewPlainObject(cx);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(wrapped_cx) let arr_root = arr_obj);
    for (i, &byte) in data.iter().enumerate() {
        let v = Int32Value(byte as i32);
        rooted!(&in(wrapped_cx) let v_root = v);
        JS_SetElement(cx, arr_root.handle().into(), i as u32, v_root.handle().into());
    }
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_root.handle().into(), c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    if len_val.is_undefined() {
        let len_v = Int32Value(data.len() as i32);
        rooted!(&in(wrapped_cx) let len_root = len_v);
        JS_DefineProperty(cx, arr_root.handle().into(), c"length".as_ptr(), len_root.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let arr_val = mozjs::jsval::ObjectValue(arr_obj);
    rooted!(&in(wrapped_cx) let arr_val_root = arr_val);
    let arr_val_copy = *arr_val_root;
    let call_args = HandleValueArray { length_: 1, elements_: &arr_val_copy as *const JSVal };
    rooted!(&in(wrapped_cx) let ctor_root = from_fn);
    let mut rval = UndefinedValue();
    JS_CallFunctionValue(cx, global_root.handle().into(), ctor_root.handle().into(), &call_args, MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
    });
    args.rval().set(rval);
    true
}

// ---------------------------------------------------------------------------
// Native sync functions — accept buffer-like, return Uint8Array
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_deflate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => return_bytes(cx, &args, compressed),
        Err(_) => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_inflate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    let mut decoder = flate2::read::ZlibDecoder::new(&data[..]);
    let mut decompressed = Vec::new();
    match decoder.read_to_end(&mut decompressed) {
        Ok(_) => return_bytes(cx, &args, decompressed),
        Err(_) => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_deflate_raw_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => return_bytes(cx, &args, compressed),
        Err(_) => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_inflate_raw_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    let mut decoder = flate2::read::DeflateDecoder::new(&data[..]);
    let mut decompressed = Vec::new();
    match decoder.read_to_end(&mut decompressed) {
        Ok(_) => return_bytes(cx, &args, decompressed),
        Err(_) => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_gzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(&data);
    match encoder.finish() {
        Ok(compressed) => return_bytes(cx, &args, compressed),
        Err(_) => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_gunzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    let mut decoder = flate2::read::GzDecoder::new(&data[..]);
    let mut decompressed = Vec::new();
    match decoder.read_to_end(&mut decompressed) {
        Ok(_) => return_bytes(cx, &args, decompressed),
        Err(_) => { args.rval().set(UndefinedValue()); true }
    }
}

// ---------------------------------------------------------------------------
// JS polyfill — classes + constants only
// ---------------------------------------------------------------------------

const ZLIB_JS: &str = r#"
(function() {
  function EE() { this._events = {}; }
  EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
  EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return !!ls; };

  function ZlibBase() { EE.call(this); this._chunks = []; this._ended = false; }
  ZlibBase.prototype = Object.create(EE.prototype);
  ZlibBase.prototype.constructor = ZlibBase;
  ZlibBase.prototype.write = function(chunk) { this._chunks.push(chunk); return true; };
  ZlibBase.prototype.end = function(chunk) { if (chunk) this._chunks.push(chunk); this._ended = true; this.emit("end"); return this; };
  ZlibBase.prototype.pipe = function(dest) { this.on("data", function(c) { dest.write(c); }); this.on("end", function() { dest.end(); }); return dest; };

  function Deflate(opts) { ZlibBase.call(this); }
  Deflate.prototype = Object.create(ZlibBase.prototype);

  function Inflate(opts) { ZlibBase.call(this); }
  Inflate.prototype = Object.create(ZlibBase.prototype);

  function Gzip(opts) { ZlibBase.call(this); }
  Gzip.prototype = Object.create(ZlibBase.prototype);

  function Gunzip(opts) { ZlibBase.call(this); }
  Gunzip.prototype = Object.create(ZlibBase.prototype);

  function DeflateRaw(opts) { ZlibBase.call(this); }
  DeflateRaw.prototype = Object.create(ZlibBase.prototype);

  function InflateRaw(opts) { ZlibBase.call(this); }
  InflateRaw.prototype = Object.create(ZlibBase.prototype);

  return {
    Deflate: Deflate, Inflate: Inflate, Gzip: Gzip, Gunzip: Gunzip,
    DeflateRaw: DeflateRaw, InflateRaw: InflateRaw,
    createDeflate: function(o) { return new Deflate(o); },
    createInflate: function(o) { return new Inflate(o); },
    createGzip: function(o) { return new Gzip(o); },
    createGunzip: function(o) { return new Gunzip(o); },
    createDeflateRaw: function(o) { return new DeflateRaw(o); },
    createInflateRaw: function(o) { return new InflateRaw(o); },
    constants: {
      Z_NO_FLUSH: 0, Z_FINISH: 4, Z_OK: 0, Z_STREAM_END: 1,
      Z_DATA_ERROR: -3, Z_BUF_ERROR: -5,
    },
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

        w2::JS_DefineFunction(cx, mod_obj.handle(), c"deflateSync".as_ptr(), Some(zlib_deflate_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"inflateSync".as_ptr(), Some(zlib_inflate_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"deflateRawSync".as_ptr(), Some(zlib_deflate_raw_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"inflateRawSync".as_ptr(), Some(zlib_inflate_raw_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"gzipSync".as_ptr(), Some(zlib_gzip_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"gunzipSync".as_ptr(), Some(zlib_gunzip_sync), 1, JSPROP_ENUMERATE as u32);

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

        for name in &["Deflate", "Inflate", "Gzip", "Gunzip", "DeflateRaw", "InflateRaw",
                       "createDeflate", "createInflate", "createGzip", "createGunzip",
                       "createDeflateRaw", "createInflateRaw", "constants"] {
            let cname = ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(cx_raw, exports_rooted.handle().into(), cname.as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut val,
            });
            if !val.is_undefined() {
                rooted!(&in(cx) let val_root = val);
                JS_DefineProperty(cx_raw, mod_obj.handle().into(), cname.as_ptr(), val_root.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        cache_builtin(cx, "zlib", mod_obj.get());
    }
}
