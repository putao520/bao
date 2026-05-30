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

thread_local! {
    static FILE_GLOBALS: RefCell<(Option<String>, Option<String>)> = RefCell::new((None, None));
    static BUFFER_PROTOTYPE: RefCell<Option<*mut JSObject>> = RefCell::new(None);
}

use ::std::cell::RefCell;

pub fn set_file_globals(filename: Option<String>, dirname: Option<String>) {
    FILE_GLOBALS.with(|f| *f.borrow_mut() = (filename, dirname));
}

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
    crate::fetch_api::install_request_constructor(cx, global);
    crate::require::install_require(cx, global);
    install_module_global(cx, global);
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
    install_structured_clone(cx, global);
    crate::node_perf_hooks::install(cx);
    crate::node_timers_module::install(cx);
    crate::node_readline::install(cx);
    crate::node_tls::install_tls(cx);
    install_assert_strict(cx);
    install_file_globals_from_cache(cx, global);
}

pub fn install_module_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let raw = cx.raw_cx();
        rooted!(&in(cx) let mod_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw));
        if mod_obj.get().is_null() {
            return;
        }
        let exports_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw);
        if !exports_obj.is_null() {
            let ev = mozjs::jsval::ObjectValue(exports_obj);
            rooted!(&in(cx) let ev_r = ev);
            let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_obj.get() };
            let ev_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ev_r.get() };
            JS_DefineProperty(raw, mod_h, c"exports".as_ptr(), ev_h, JSPROP_ENUMERATE as u32);
        }
        let dot_str = JS_NewStringCopyZ(raw, c".".as_ptr());
        if !dot_str.is_null() {
            let id_val = mozjs::jsval::StringValue(&*dot_str);
            rooted!(&in(cx) let id_r = id_val);
            let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_obj.get() };
            let id_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &id_r.get() };
            JS_DefineProperty(raw, mod_h, c"id".as_ptr(), id_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
        }
        JS_DefineProperty3(cx, global, c"module".as_ptr(), mod_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

pub fn install_file_globals(
    _cx: &mut bao_engine::context::JsContext,
    filename: &str,
    dirname: &str,
) {
    set_file_globals(Some(filename.to_string()), Some(dirname.to_string()));
}

fn install_file_globals_from_cache(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    let (filename, dirname) = FILE_GLOBALS.with(|f| f.borrow().clone());
    unsafe {
        let raw = cx.raw_cx();
        if let Some(fn_str) = filename {
            if let Ok(c_fn) = ::std::ffi::CString::new(fn_str) {
                let js_str = JS_NewStringCopyZ(raw, c_fn.as_ptr());
                if !js_str.is_null() {
                    let v = StringValue(&*js_str);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(raw, global.into(), c"__filename".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
                }
            }
        }
        if let Some(dir_str) = dirname {
            if let Ok(c_dir) = ::std::ffi::CString::new(dir_str) {
                let js_str = JS_NewStringCopyZ(raw, c_dir.as_ptr());
                if !js_str.is_null() {
                    let v = StringValue(&*js_str);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(raw, global.into(), c"__dirname".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
                }
            }
        }
    }
}

pub fn install_buffer_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let buf_fn = JS_NewFunction(cx.raw_cx(), Some(buffer_constructor), 1, 0, c"Buffer".as_ptr());
        if buf_fn.is_null() {
            return;
        }
        let buf_obj = JS_GetFunctionObject(buf_fn);
        if buf_obj.is_null() {
            return;
        }
        rooted!(&in(cx) let buf_root = buf_obj);

        JS_DefineFunction(
            cx, buf_root.handle(), c"from".as_ptr(),
            ::std::option::Option::Some(buffer_from), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"alloc".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"isBuffer".as_ptr(),
            ::std::option::Option::Some(buffer_is_buffer), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"concat".as_ptr(),
            ::std::option::Option::Some(buffer_concat), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"allocUnsafe".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"allocUnsafeSlow".as_ptr(),
            ::std::option::Option::Some(buffer_alloc), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"byteLength".as_ptr(),
            ::std::option::Option::Some(buffer_byte_length), 1, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"compare".as_ptr(),
            ::std::option::Option::Some(buffer_compare), 2, JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx, buf_root.handle(), c"isEncoding".as_ptr(),
            ::std::option::Option::Some(buffer_is_encoding), 1, JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(cx, global, c"Buffer".as_ptr(), buf_root.handle(), JSPROP_ENUMERATE as u32);

        // Create dedicated Buffer.prototype object (not polluting Object.prototype)
        rooted!(&in(cx) let buf_proto = JS_NewPlainObject(cx));
        if !buf_proto.get().is_null() {
            let proto_val = ObjectValue(buf_proto.get());
            let proto_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &proto_val };
            JS_DefineProperty(cx.raw_cx(), buf_root.handle().into(), c"prototype".as_ptr(), proto_h, 0u32);
            BUFFER_PROTOTYPE.with(|bp| { *bp.borrow_mut() = Some(buf_proto.get()); });
        }
    }

    // Inject Buffer prototype methods via JS eval
    let proto_src = r#"
(function() {
  if (!Buffer.of) {
    Buffer.of = function() {
      var len = arguments.length;
      var buf = Buffer.alloc(len);
      for (var i = 0; i < len; i++) { buf[i] = arguments[i] & 0xFF; }
      return buf;
    };
  }
  var _bp = Buffer.prototype;
  if (!_bp) return;

  _bp.write = function(str, offset, encoding) {
    offset = offset || 0;
    var bytes = (encoding === 'hex') ? str.match(/.{2}/g).map(function(h) { return parseInt(h, 16); }) : [];
    if (encoding !== 'hex') { for (var i = 0; i < str.length && (offset + i) < this.length; i++) { this[offset + i] = str.charCodeAt(i); } }
    else { for (var i = 0; i < bytes.length && (offset + i) < this.length; i++) { this[offset + i] = bytes[i]; } }
    return encoding === 'hex' ? bytes.length : Math.min(str.length, this.length - offset);
  };

  _bp.readUInt8 = function(offset) { return this[offset || 0]; };
  _bp.writeUInt8 = function(val, offset) { this[offset || 0] = val & 0xFF; return offset || 0; };

  _bp.fill = function(val, start, end) {
    start = start || 0; end = end || this.length;
    var b = (typeof val === 'number') ? val & 0xFF : (typeof val === 'string') ? val.charCodeAt(0) : 0;
    for (var i = start; i < end; i++) { this[i] = b; }
    return this;
  };

  _bp.includes = function(val, byteOffset) {
    return this.indexOf(val, byteOffset) !== -1;
  };

  _bp.lastIndexOf = function(val, byteOffset) {
    byteOffset = byteOffset !== undefined ? byteOffset : this.length - 1;
    if (typeof val === 'number') {
      for (var i = byteOffset; i >= 0; i--) { if (this[i] === val) return i; }
    } else if (typeof val === 'string') {
      for (var i = byteOffset; i >= 0; i--) {
        var match = true;
        for (var j = 0; j < val.length && (i + j) < this.length; j++) {
          if (this[i + j] !== val.charCodeAt(j)) { match = false; break; }
        }
        if (match) return i;
      }
    }
    return -1;
  };

  _bp.toJSON = function() {
    return { type: 'Buffer', data: Array.prototype.slice.call(this, 0, this.length) };
  };

  _bp.subarray = function(start, end) {
    start = start || 0; end = end || this.length;
    var result = Buffer.alloc(end - start);
    for (var i = start; i < end; i++) { result[i - start] = this[i]; }
    return result;
  };

  _bp.reverse = function() {
    for (var i = 0, j = this.length - 1; i < j; i++, j--) {
      var tmp = this[i]; this[i] = this[j]; this[j] = tmp;
    }
    return this;
  };

  _bp.entries = function() {
    var buf = this; var idx = 0;
    return { next: function() { return idx < buf.length ? { value: [idx, buf[idx++]], done: false } : { done: true }; }, [Symbol.iterator]: function() { return this; } };
  };

  _bp.keys = function() {
    var buf = this; var idx = 0;
    return { next: function() { return idx < buf.length ? { value: idx++, done: false } : { done: true }; }, [Symbol.iterator]: function() { return this; } };
  };

  _bp.values = function() {
    var buf = this; var idx = 0;
    return { next: function() { return idx < buf.length ? { value: buf[idx++], done: false } : { done: true }; }, [Symbol.iterator]: function() { return this; } };
  };

  _bp.readInt8 = function(offset) { var v = this[offset || 0]; return v > 127 ? v - 256 : v; };
  _bp.readUInt16LE = function(offset) { offset = offset || 0; return this[offset] | (this[offset + 1] << 8); };
  _bp.writeUInt16LE = function(val, offset) { offset = offset || 0; this[offset] = val & 0xFF; this[offset + 1] = (val >> 8) & 0xFF; return offset; };
  _bp.readUInt32LE = function(offset) { offset = offset || 0; return ((this[offset]) | (this[offset+1] << 8) | (this[offset+2] << 16) | (this[offset+3] << 24)) >>> 0; };
  _bp.writeUInt32LE = function(val, offset) { offset = offset || 0; this[offset] = val & 0xFF; this[offset+1] = (val >> 8) & 0xFF; this[offset+2] = (val >> 16) & 0xFF; this[offset+3] = (val >> 24) & 0xFF; return offset; };
  _bp.readInt16LE = function(offset) { var v = _bp.readUInt16LE.call(this, offset); return v > 32767 ? v - 65536 : v; };
  _bp.writeInt16LE = function(val, offset) { return _bp.writeUInt16LE.call(this, val & 0xFFFF, offset); };
  _bp.readInt32LE = function(offset) { return this[offset || 0] | (this[(offset||0)+1] << 8) | (this[(offset||0)+2] << 16) | (this[(offset||0)+3] << 24); };
  _bp.writeInt32LE = function(val, offset) { return _bp.writeUInt32LE.call(this, val >>> 0, offset); };
  _bp.readFloatLE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    u8[0]=this[offset]; u8[1]=this[offset+1]; u8[2]=this[offset+2]; u8[3]=this[offset+3];
    return f32[0];
  };
  _bp.writeFloatLE = function(val, offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    f32[0] = val; this[offset]=u8[0]; this[offset+1]=u8[1]; this[offset+2]=u8[2]; this[offset+3]=u8[3];
    return offset;
  };
  _bp.readDoubleLE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    for (var i = 0; i < 8; i++) u8[i] = this[offset + i];
    return f64[0];
  };
  _bp.writeDoubleLE = function(val, offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    f64[0] = val; for (var i = 0; i < 8; i++) this[offset + i] = u8[i];
    return offset;
  };

  _bp.swap16 = function() {
    for (var i = 0; i < this.length - 1; i += 2) { var t = this[i]; this[i] = this[i+1]; this[i+1] = t; }
    return this;
  };
  _bp.swap32 = function() {
    for (var i = 0; i < this.length - 3; i += 4) {
      var a=this[i], b=this[i+1], c=this[i+2], d=this[i+3];
      this[i]=d; this[i+1]=c; this[i+2]=b; this[i+3]=a;
    }
    return this;
  };
  _bp.swap64 = function() {
    for (var i = 0; i < this.length - 7; i += 8) {
      var t;
      t=this[i]; this[i]=this[i+7]; this[i+7]=t;
      t=this[i+1]; this[i+1]=this[i+6]; this[i+6]=t;
      t=this[i+2]; this[i+2]=this[i+5]; this[i+5]=t;
      t=this[i+3]; this[i+3]=this[i+4]; this[i+4]=t;
    }
    return this;
  };

  _bp.compare = function(other) {
    var len = Math.min(this.length, other.length);
    for (var i = 0; i < len; i++) {
      if (this[i] < other[i]) return -1;
      if (this[i] > other[i]) return 1;
    }
    if (this.length < other.length) return -1;
    if (this.length > other.length) return 1;
    return 0;
  };

  _bp.readUInt16BE = function(offset) { offset = offset || 0; return (this[offset] << 8) | this[offset + 1]; };
  _bp.writeUInt16BE = function(val, offset) { offset = offset || 0; this[offset] = (val >> 8) & 0xFF; this[offset + 1] = val & 0xFF; return offset; };
  _bp.readUInt32BE = function(offset) { offset = offset || 0; return ((this[offset] << 24) | (this[offset+1] << 16) | (this[offset+2] << 8) | this[offset+3]) >>> 0; };
  _bp.writeUInt32BE = function(val, offset) { offset = offset || 0; this[offset] = (val >> 24) & 0xFF; this[offset+1] = (val >> 16) & 0xFF; this[offset+2] = (val >> 8) & 0xFF; this[offset+3] = val & 0xFF; return offset; };
  _bp.readInt16BE = function(offset) { var v = _bp.readUInt16BE.call(this, offset); return v > 32767 ? v - 65536 : v; };
  _bp.readInt32BE = function(offset) { return (this[offset||0] << 24) | (this[(offset||0)+1] << 16) | (this[(offset||0)+2] << 8) | this[(offset||0)+3]; };
  _bp.readFloatBE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    u8[3]=this[offset]; u8[2]=this[offset+1]; u8[1]=this[offset+2]; u8[0]=this[offset+3];
    return f32[0];
  };
  _bp.readDoubleBE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    u8[7]=this[offset]; u8[6]=this[offset+1]; u8[5]=this[offset+2]; u8[4]=this[offset+3];
    u8[3]=this[offset+4]; u8[2]=this[offset+5]; u8[1]=this[offset+6]; u8[0]=this[offset+7];
    return f64[0];
  };
})();
"#;
    unsafe {
        let raw = cx.raw_cx();
        let c_filename = ::std::ffi::CString::new("<buffer-proto>").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(raw, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(proto_src);
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
            mozjs_sys::jsapi::JS::Evaluate2(raw, opts, &mut src, rval_h);
            libc::free(opts as *mut _);
        }
    }
}

/// Set Buffer.prototype as the prototype of a newly created buffer object.
unsafe fn set_buffer_proto(cx: *mut JSContext, obj: *mut JSObject) {
    BUFFER_PROTOTYPE.with(|bp| {
        if let Some(proto) = *bp.borrow() {
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let proto_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &proto };
            let _ = JS_SetPrototype(cx, obj_h, proto_h);
        }
    });
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if !obj.is_null() { set_buffer_proto(cx, obj); args.rval().set(mozjs::jsval::ObjectValue(obj)); }
        return true;
    }
    let first = *args.get(0).ptr;
    if first.is_string() {
        let s = first.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            let bytes = rust_str.as_bytes();
            let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
            if obj.is_null() { args.rval().set(UndefinedValue()); return true; }
            set_buffer_proto(cx, obj);
            for (i, &byte) in bytes.iter().enumerate() {
                let val = mozjs::jsval::Int32Value(byte as i32);
                rooted!(&in(mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))) let v = val);
                JS_DefineElement(cx,
                    Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
                    i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            rooted!(&in(mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))) let len = mozjs::jsval::Int32Value(bytes.len() as i32));
            JS_DefineProperty(cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
                c"length".as_ptr() as *const ::std::os::raw::c_char,
                len.handle().into(), JSPROP_ENUMERATE as u32);
            let buf_val = mozjs::jsval::BooleanValue(true);
            rooted!(&in(mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))) let bv = buf_val);
            JS_DefineProperty(cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
                c"_isBuffer".as_ptr() as *const ::std::os::raw::c_char,
                bv.handle().into(), 0u32);
            args.rval().set(mozjs::jsval::ObjectValue(obj));
            return true;
        }
    }
    if first.is_int32() {
        let size = first.to_int32().max(0) as usize;
        let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
        if obj.is_null() { args.rval().set(UndefinedValue()); return true; }
        set_buffer_proto(cx, obj);
        for i in 0..size {
            rooted!(&in(mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))) let v = mozjs::jsval::Int32Value(0));
            JS_DefineElement(cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
                i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
        }
        rooted!(&in(mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))) let len = mozjs::jsval::Int32Value(size as i32));
        JS_DefineProperty(cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
            c"length".as_ptr() as *const ::std::os::raw::c_char,
            len.handle().into(), JSPROP_ENUMERATE as u32);
        let buf_val = mozjs::jsval::BooleanValue(true);
        rooted!(&in(mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))) let bv = buf_val);
        JS_DefineProperty(cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
            c"_isBuffer".as_ptr() as *const ::std::os::raw::c_char,
            bv.handle().into(), 0u32);
        args.rval().set(mozjs::jsval::ObjectValue(obj));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
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
        let encoding = if argc >= 2 {
            let enc_val = *args.get(1).ptr;
            if enc_val.is_string() {
                jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(enc_val.to_string()))
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        let bytes = if encoding == "hex" {
            (0..s.len()).step_by(2).filter_map(|i| {
                u8::from_str_radix(&s[i..i+2], 16).ok()
            }).collect::<Vec<u8>>()
        } else if encoding == "base64" {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(&s).unwrap_or_default()
        } else if encoding == "base64url" {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&s).unwrap_or_default()
        } else {
            s.as_bytes().to_vec()
        };
        create_buffer_from_bytes(cx, &args, &bytes)
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
    set_buffer_proto(cx, buf_obj);

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

    let equals_fn = JS_NewFunction(cx, Some(buffer_equals), 1, 0, c"equals".as_ptr());
    if !equals_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(equals_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"equals".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
    }

    let indexof_fn = JS_NewFunction(cx, Some(buffer_index_of), 1, 0, c"indexOf".as_ptr());
    if !indexof_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(indexof_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fn_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, obj_handle, c"indexOf".as_ptr(), fn_handle, JSPROP_ENUMERATE as u32);
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

    let fill_byte = if argc >= 2 {
        let fill_val = *args.get(1).ptr;
        if fill_val.is_int32() { fill_val.to_int32() as u8 }
        else if fill_val.is_string() { jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(fill_val.to_string())).chars().next().unwrap_or('\0') as u8 }
        else { 0 }
    } else { 0 };

    create_buffer_from_bytes(cx, &args, &vec![fill_byte; size])
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_equals(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() || argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    let src_obj = this.to_object();
    let src_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &src_obj };
    let mut src_len_val = UndefinedValue();
    JS_GetProperty(cx, src_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut src_len_val,
    });
    let src_len = if src_len_val.is_int32() { src_len_val.to_int32() as usize } else { 0 };

    let other_val = *args.get(0).ptr;
    if !other_val.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let tgt_obj = other_val.to_object();
    let tgt_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &tgt_obj };
    let mut tgt_len_val = UndefinedValue();
    JS_GetProperty(cx, tgt_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut tgt_len_val,
    });
    let tgt_len = if tgt_len_val.is_int32() { tgt_len_val.to_int32() as usize } else { 0 };

    if src_len != tgt_len {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    for i in 0..src_len {
        let mut a_val = UndefinedValue();
        JS_GetElement(cx, src_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut a_val,
        });
        let mut b_val = UndefinedValue();
        JS_GetElement(cx, tgt_h, i as u32, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut b_val,
        });
        let a = if a_val.is_int32() { a_val.to_int32() as u8 } else { 0 };
        let b = if b_val.is_int32() { b_val.to_int32() as u8 } else { 0 };
        if a != b {
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    }
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_index_of(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() || argc == 0 {
        args.rval().set(Int32Value(-1));
        return true;
    }

    let obj = this.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut len_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
    });
    let buf_len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };

    let byte_offset = if argc >= 2 {
        let off_val = *args.get(1).ptr;
        if off_val.is_int32() { off_val.to_int32().max(0) as usize } else { 0 }
    } else {
        0
    };

    let search_val = *args.get(0).ptr;
    if search_val.is_int32() {
        let needle = search_val.to_int32() as u8;
        for i in byte_offset..buf_len {
            let mut elem = UndefinedValue();
            JS_GetElement(cx, obj_h, i as u32, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut elem,
            });
            if elem.is_int32() && elem.to_int32() as u8 == needle {
                args.rval().set(Int32Value(i as i32));
                return true;
            }
        }
    } else if search_val.is_string() {
        let js_str = search_val.to_string();
        let needle_str = jsstr_to_string(cx, NonNull::new_unchecked(js_str));
        let needle: Vec<u8> = needle_str.bytes().collect();
        if needle.is_empty() || needle.len() > buf_len {
            args.rval().set(Int32Value(-1));
            return true;
        }
        'outer: for i in byte_offset..=(buf_len - needle.len()) {
            for j in 0..needle.len() {
                let mut elem = UndefinedValue();
                JS_GetElement(cx, obj_h, (i + j) as u32, MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData, ptr: &mut elem,
                });
                let b = if elem.is_int32() { elem.to_int32() as u8 } else { 0 };
                if b != needle[j] { continue 'outer; }
            }
            args.rval().set(Int32Value(i as i32));
            return true;
        }
    }
    args.rval().set(Int32Value(-1));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_encoding(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let valid = ["utf8", "utf-8", "ascii", "latin1", "binary", "base64", "base64url", "hex", "ucs2", "ucs-2", "utf16le", "utf-16le"];
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let enc_val = *args.get(0).ptr;
    if !enc_val.is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let enc_str = jsstr_to_string(_cx, ::std::ptr::NonNull::new_unchecked(enc_val.to_string()));
    let is_valid = valid.iter().any(|&v| v == enc_str.to_lowercase());
    args.rval().set(mozjs::jsval::BooleanValue(is_valid));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_byte_length(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(Int32Value(0));
        return true;
    }
    let input = *args.get(0).ptr;
    if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        args.rval().set(Int32Value(s.as_bytes().len() as i32));
    } else {
        args.rval().set(Int32Value(0));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_compare(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(Int32Value(0));
        return true;
    }

    let read_bytes = |obj: *mut JSObject| -> (::std::vec::Vec<u8>, usize) {
        let h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let mut len_val = UndefinedValue();
        JS_GetProperty(cx, h, c"length".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val,
        });
        let len = if len_val.is_int32() { len_val.to_int32() as usize } else { 0 };
        let mut bytes = ::std::vec::Vec::with_capacity(len);
        for i in 0..len {
            let mut v = UndefinedValue();
            JS_GetElement(cx, h, i as u32, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut v,
            });
            bytes.push(if v.is_int32() { v.to_int32() as u8 } else { 0 });
        }
        (bytes, len)
    };

    let a_val = *args.get(0).ptr;
    let b_val = *args.get(1).ptr;
    if !a_val.is_object() || !b_val.is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let (a_bytes, _) = read_bytes(a_val.to_object());
    let (b_bytes, _) = read_bytes(b_val.to_object());
    args.rval().set(Int32Value(a_bytes.cmp(&b_bytes) as i32));
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

pub fn install_structured_clone(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"structuredClone".as_ptr(),
            ::std::option::Option::Some(structured_clone_fn), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn structured_clone_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;

    if val.is_undefined() || val.is_null() || val.is_boolean() || val.is_int32() || val.is_double() || val.is_string() {
        args.rval().set(val);
        return true;
    }

    if val.is_object() {
        let obj = val.to_object();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };

        let mut ctor_name = UndefinedValue();
        JS_GetProperty(cx, obj_h, c"constructor".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor_name });
        if ctor_name.is_object() {
            let ctor = ctor_name.to_object();
            let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor };
            let mut name_val = UndefinedValue();
            JS_GetProperty(cx, ctor_h, c"name".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut name_val });
            if name_val.is_string() {
                let name = crate::js_to_rust_string(cx, name_val);
                match name.as_str() {
                    "Date" => {
                        let mut time_val = UndefinedValue();
                        JS_GetProperty(cx, obj_h, c"getTime".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut time_val });
                        if time_val.is_object() {
                            let get_time_fn = time_val.to_object();
                            let gt_val = ObjectValue(get_time_fn);
                            let gt_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &gt_val };
                            let global = CurrentGlobalOrNull(cx);
                            if !global.is_null() {
                                let _global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                                let mut ms_rval = UndefinedValue();
                                JS_CallFunctionValue(cx, obj_h, gt_h, &HandleValueArray::empty(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ms_rval });
                                let ms = if ms_rval.is_double() { ms_rval.to_double() } else if ms_rval.is_int32() { ms_rval.to_int32() as f64 } else { 0.0 };
                                let src = format!("new Date({})", ms);
                                let mut eval_rval = UndefinedValue();
                                let eval_opts = mozjs::glue::NewCompileOptions(cx, b"clone\0".as_ptr() as *const ::std::os::raw::c_char, 1);
                                if !eval_opts.is_null() {
                                    let mut src_text = mozjs::rust::transform_str_to_source_text(&src);
                                    JS::Evaluate2(cx, eval_opts, &mut src_text, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut eval_rval });
                                    libc::free(eval_opts as *mut _);
                                }
                                args.rval().set(eval_rval);
                                return true;
                            }
                        }
                    }
                    "RegExp" => {
                        let mut source_val = UndefinedValue();
                        JS_GetProperty(cx, obj_h, c"source".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut source_val });
                        let mut flags_val = UndefinedValue();
                        JS_GetProperty(cx, obj_h, c"flags".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut flags_val });
                        let source = if source_val.is_string() { crate::js_to_rust_string(cx, source_val) } else { "".to_string() };
                        let flags = if flags_val.is_string() { crate::js_to_rust_string(cx, flags_val) } else { "".to_string() };
                        let src = format!("new RegExp(\"{}\", \"{}\")", source.replace('\\', "\\\\").replace('"', "\\\""), flags);
                        let mut eval_rval = UndefinedValue();
                        let eval_opts = mozjs::glue::NewCompileOptions(cx, b"clone\0".as_ptr() as *const ::std::os::raw::c_char, 1);
                        if !eval_opts.is_null() {
                            let mut src_text = mozjs::rust::transform_str_to_source_text(&src);
                            JS::Evaluate2(cx, eval_opts, &mut src_text, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut eval_rval });
                            libc::free(eval_opts as *mut _);
                        }
                        args.rval().set(eval_rval);
                        return true;
                    }
                    _ => {}
                }
            }
        }

        let mut json_rval = UndefinedValue();
        let json_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut json_rval };
        let json_src = mozjs::rust::transform_str_to_source_text("(function(o){try{return JSON.parse(JSON.stringify(o))}catch(e){return o}})");
        let json_opts = mozjs::glue::NewCompileOptions(cx, b"json_clone\0".as_ptr() as *const ::std::os::raw::c_char, 1);
        if !json_opts.is_null() {
            let mut json_fn_val = UndefinedValue();
            JS::Evaluate2(cx, json_opts, &mut ::std::mem::MaybeUninit::new(json_src).assume_init(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut json_fn_val });
            libc::free(json_opts as *mut _);
            if json_fn_val.is_object() {
                let global = CurrentGlobalOrNull(cx);
                if !global.is_null() {
                    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                    let fn_val = ObjectValue(json_fn_val.to_object());
                    let fn_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
                    let obj_val = ObjectValue(obj);
                    let obj_arg = HandleValueArray { length_: 1, elements_: &obj_val };
                    JS_CallFunctionValue(cx, global_h, fn_h, &obj_arg, json_rval_h);
                    args.rval().set(json_rval);
                    return true;
                }
            }
        }
    }

    args.rval().set(val);
    true
}

pub fn install_assert_strict(cx: &mut mozjs::context::JSContext) {
    crate::require::cache_assert_strict(cx);
}
