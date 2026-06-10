// @trace REQ-ENG-007
// 铁律 0: use bun_zlib (C libz) instead of flate2 (miniz_oxide pure Rust duplicate)
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ---------------------------------------------------------------------------
// Buffer → bytes extraction helper (delegates to node_crypto for object,
// adds string support on top)
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_bytes(cx: *mut JSContext, val: JSVal) -> Vec<u8> {
    if val.is_string() {
        let s = jsstr_to_string(cx, NonNull::new_unchecked(val.to_string()));
        return s.into_bytes();
    }
    crate::node_crypto::extract_buffer_bytes(cx, val)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_bytes(cx: *mut JSContext, args: &CallArgs, data: Vec<u8>) -> bool {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let mut buffer_ctor = UndefinedValue();
    JS_GetProperty(cx, global_h, c"Buffer".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut buffer_ctor,
    });
    if !buffer_ctor.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut from_fn = UndefinedValue();
    JS_GetProperty(cx, Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData, ptr: &buffer_ctor.to_object(),
    }, c"from".as_ptr(), MutableHandle::<Value> {
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
    let arr_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &arr_obj };
    for (i, &byte) in data.iter().enumerate() {
        let v = Int32Value(byte as i32);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_SetElement(cx, arr_h, i as u32, v_h);
    }
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, arr_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    if len_val.is_undefined() {
        let len_v = Int32Value(data.len() as i32);
        let len_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &len_v };
        JS_DefineProperty(cx, arr_h, c"length".as_ptr(), len_h, JSPROP_ENUMERATE as u32);
    }

    let arr_val = mozjs::jsval::ObjectValue(arr_obj);
    let arr_val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &arr_val };
    let call_args = HandleValueArray { length_: 1, elements_: arr_val_h.ptr };
    let ctor_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &from_fn };
    let mut rval = UndefinedValue();
    JS_CallFunctionValue(cx, global_h, ctor_h, &call_args, MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
    });
    args.rval().set(rval);
    true
}

// ---------------------------------------------------------------------------
// Native sync functions — accept buffer-like, return Uint8Array
// 铁律 0: use bun_zlib (C libz) instead of flate2 (miniz_oxide pure Rust duplicate)
// ---------------------------------------------------------------------------

/// Sync compress using bun_zlib's deflate with specified window_bits.
/// window_bits: 15=zlib, -15=raw deflate, 31=gzip
fn zlib_compress_sync(data: &[u8], window_bits: core::ffi::c_int) -> Option<Vec<u8>> {
    use bun_zlib::{zStream_struct, ReturnCode, FlushValue, uInt, uLong};
    use core::mem::size_of;

    let mut strm: zStream_struct = unsafe { core::mem::zeroed() };
    strm.next_in = data.as_ptr();
    strm.avail_in = data.len() as uInt;

    let ret = unsafe {
        bun_zlib::deflateInit2_(
            &raw mut strm, 6, 8, window_bits, 8, 0,
            bun_zlib::zlibVersion().cast::<u8>(),
            size_of::<zStream_struct>() as core::ffi::c_int,
        )
    };
    if ret != ReturnCode::Ok { return None; }

    // Pre-allocate using deflateBound (accurate after init)
    let bound = unsafe { bun_zlib::deflateBound(&raw mut strm, data.len() as uLong) } as usize;
    let mut output: Vec<u8> = Vec::with_capacity(bound);
    strm.next_out = output.as_mut_ptr();
    strm.avail_out = output.capacity() as uInt;

    let ret = unsafe { bun_zlib::deflate(&raw mut strm, FlushValue::Finish) };
    unsafe { bun_zlib::deflateEnd(&raw mut strm); }

    if ret != ReturnCode::StreamEnd { return None; }

    unsafe { output.set_len(strm.total_out as usize); }
    Some(output)
}

/// Sync decompress using bun_zlib's ZlibReaderArrayList (handles buffer growth).
/// window_bits: 15=zlib, -15=raw deflate, 31=gzip, 47=auto-detect gzip/zlib
fn zlib_decompress_sync(data: &[u8], window_bits: core::ffi::c_int) -> Option<Vec<u8>> {
    use bun_zlib::{Options, ZlibReaderArrayList};

    let mut output = Vec::new();
    let options = Options { window_bits, ..Default::default() };
    {
        let mut reader = ZlibReaderArrayList::init_with_options(data, &mut output, options).ok()?;
        reader.read_all(true).ok()?;
    }
    Some(output)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_deflate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    match zlib_compress_sync(&data, 15) {
        Some(compressed) => return_bytes(cx, &args, compressed),
        None => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_inflate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    match zlib_decompress_sync(&data, 15) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_deflate_raw_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    match zlib_compress_sync(&data, -15) {
        Some(compressed) => return_bytes(cx, &args, compressed),
        None => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_inflate_raw_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    match zlib_decompress_sync(&data, -15) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_gzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    match zlib_compress_sync(&data, 31) { // 15 + 16 = gzip
        Some(compressed) => return_bytes(cx, &args, compressed),
        None => { args.rval().set(UndefinedValue()); true }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn zlib_gunzip_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let data = if argc > 0 { extract_bytes(cx, *args.get(0).ptr) } else { Vec::new() };
    // 15 + 32 = auto-detect zlib/gzip header
    match zlib_decompress_sync(&data, 47) {
        Some(decompressed) => return_bytes(cx, &args, decompressed),
        None => { args.rval().set(UndefinedValue()); true }
    }
}

// ---------------------------------------------------------------------------
// JS polyfill — classes + constants only
// ---------------------------------------------------------------------------

const ZLIB_JS: &str = r#"
(function() {
  var EE = null;
  try { EE = require("events").EventEmitter; } catch(e) {
    EE = function EE() { this._events = {}; };
    EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); } return this; };
  }

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

        let mod_ptr = mod_obj.get();
        let _mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr };

        w2::JS_DefineFunction(cx, mod_obj.handle(), c"deflateSync".as_ptr(), Some(zlib_deflate_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"inflateSync".as_ptr(), Some(zlib_inflate_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"deflateRawSync".as_ptr(), Some(zlib_deflate_raw_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"inflateRawSync".as_ptr(), Some(zlib_inflate_raw_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"gzipSync".as_ptr(), Some(zlib_gzip_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, mod_obj.handle(), c"gunzipSync".as_ptr(), Some(zlib_gunzip_sync), 1, JSPROP_ENUMERATE as u32);

        let c_filename = c"node:zlib".as_ptr();
        let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx_raw, c_filename, 1) as *mut _) else {
            return;
        };
        let opts = _opts_guard.as_ptr() as *const JS::ReadOnlyCompileOptions;

        let mut src = mozjs::rust::transform_str_to_source_text(ZLIB_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);

        if !ok || !rval.is_object() {
            cache_builtin(cx, "zlib", mod_obj.get());
            return;
        }

        let exports_obj = rval.to_object();
        let exports_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &exports_obj,
        };

        let mod_ptr2 = mod_obj.get();
        let mod_h2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr2 };

        for name in &["Deflate", "Inflate", "Gzip", "Gunzip", "DeflateRaw", "InflateRaw",
                       "createDeflate", "createInflate", "createGzip", "createGunzip",
                       "createDeflateRaw", "createInflateRaw", "constants"] {
            let cname = bun_core::ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(cx_raw, exports_h, cname.as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut val,
            });
            if !val.is_undefined() {
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                JS_DefineProperty(cx_raw, mod_h2, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
            }
        }

        cache_builtin(cx, "zlib", mod_obj.get());
    }
}
