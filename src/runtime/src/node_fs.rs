// @trace REQ-ENG-007
use bun_sys::{self, Fd, O, File, Stat};
use bun_paths::PathBuffer;
use bun_core::{FileKind, ZStr};
// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Convert a String path to a ZStr via a PathBuffer. Returns None if path is too long.
#[inline]
fn path_to_zstr<'a>(path: &str, buf: &'a mut PathBuffer) -> Option<&'a ZStr> {
    let bytes = path.as_bytes();
    if bytes.len() >= buf.0.len() { return None; }
    buf.0[..bytes.len()].copy_from_slice(bytes);
    buf.0[bytes.len()] = 0;
    Some(ZStr::from_buf(&buf.0, bytes.len()))
}

/// Convert a String path to a ZStr, with a fallback error throw on overflow.
#[inline]
unsafe fn path_to_zstr_or_throw<'a>(cx: *mut JSContext, path: &str, buf: &'a mut PathBuffer) -> Option<&'a ZStr> {
    match path_to_zstr(path, buf) {
        Some(z) => Some(z),
        None => {
            JS_ReportErrorUTF8(cx, c"Path too long".as_ptr());
            None
        }
    }
}

const FS_STREAM_JS: &str = r#"
(function() {
  var fs = globalThis.__fs_stream_ref;

  var EE = null;
  try { EE = require("events").EventEmitter; } catch(e) {
    EE = function EE() { this._events = {}; };
    EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); } return this; };
  }

  function createReadStream(path, opts) {
    var s = new EE();
    s.path = path;
    s.readable = true;
    s.writable = false;
    s.bytesRead = 0;
    var encoding = (opts && opts.encoding) || null;
    try {
      var data = fs.readFileSync(path, encoding);
      s.bytesRead = (typeof data === 'string') ? data.length : 0;
      setTimeout(function() {
        s.emit('open', 0);
        if (data) s.emit('data', data);
        s.emit('end');
        s.emit('close');
      }, 0);
    } catch(e) {
      setTimeout(function() { s.emit('error', e); }, 0);
    }
    s.pipe = function(dest) {
      this.on('data', function(c) { dest.write(c); });
      this.on('end', function() { dest.end(); });
      return dest;
    };
    s.destroy = function() { this.readable = false; this.emit('close'); return this; };
    return s;
  }

  function createWriteStream(path, opts) {
    var s = new EE();
    s.path = path;
    s.readable = false;
    s.writable = true;
    s.bytesWritten = 0;
    s._buffer = [];
    s._ended = false;
    setTimeout(function() { s.emit('open', 0); }, 0);
    s.write = function(chunk) {
      if (this._ended) return false;
      this._buffer.push(typeof chunk === 'string' ? chunk : String(chunk));
      this.bytesWritten += (typeof chunk === 'string') ? chunk.length : 0;
      return true;
    };
    s.end = function(chunk) {
      if (chunk) this._buffer.push(typeof chunk === 'string' ? chunk : String(chunk));
      this._ended = true;
      this.writable = false;
      try {
        fs.writeFileSync(this.path, this._buffer.join(''));
        this.emit('finish');
      } catch(e) {
        this.emit('error', e);
      }
      this.emit('close');
      return this;
    };
    s.destroy = function() { this.writable = false; this.emit('close'); return this; };
    return s;
  }

  return { createReadStream: createReadStream, createWriteStream: createWriteStream };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let fs_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if fs_obj.get().is_null() {
        return;
    }

    unsafe {
        // Sync methods
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readFileSync".as_ptr(), Some(fs_read_file_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"writeFileSync".as_ptr(), Some(fs_write_file_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"appendFileSync".as_ptr(), Some(fs_append_file_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"existsSync".as_ptr(), Some(fs_exists_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"mkdirSync".as_ptr(), Some(fs_mkdir_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readdirSync".as_ptr(), Some(fs_readdir_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"statSync".as_ptr(), Some(fs_stat_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"lstatSync".as_ptr(), Some(fs_lstat_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"unlinkSync".as_ptr(), Some(fs_unlink_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"rmdirSync".as_ptr(), Some(fs_rmdir_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"rmSync".as_ptr(), Some(fs_rm_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"renameSync".as_ptr(), Some(fs_rename_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"copyFileSync".as_ptr(), Some(fs_copy_file_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"chmodSync".as_ptr(), Some(fs_chmod_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"realpathSync".as_ptr(), Some(fs_realpath_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readlinkSync".as_ptr(), Some(fs_readlink_sync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"symlinkSync".as_ptr(), Some(fs_symlink_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"linkSync".as_ptr(), Some(fs_link_sync), 2, JSPROP_ENUMERATE as u32);

        // Async methods
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readFile".as_ptr(), Some(fs_read_file), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"writeFile".as_ptr(), Some(fs_write_file), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"mkdir".as_ptr(), Some(fs_mkdir), 2, JSPROP_ENUMERATE as u32);

        // Constants
        let constants: &[(&str, i32)] = &[
            ("F_OK", 0), ("R_OK", 4), ("W_OK", 2), ("X_OK", 1),
        ];
        for (name, value) in constants {
            let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
            rooted!(&in(cx) let val = mozjs::jsval::Int32Value(*value));
            JS_DefineProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c_name.as_ptr(),
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // fs.promises namespace
        rooted!(&in(cx) let promises_obj = w2::JS_NewPlainObject(cx));
        if !promises_obj.get().is_null() {
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"readFile".as_ptr(), Some(fs_promises_read_file), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"writeFile".as_ptr(), Some(fs_promises_write_file), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"stat".as_ptr(), Some(fs_promises_stat), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"readdir".as_ptr(), Some(fs_promises_readdir), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"mkdir".as_ptr(), Some(fs_promises_mkdir), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"unlink".as_ptr(), Some(fs_promises_unlink), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"rename".as_ptr(), Some(fs_promises_rename), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"copyFile".as_ptr(), Some(fs_promises_copy_file), 2, JSPROP_ENUMERATE as u32);

            rooted!(&in(cx) let prom_val = mozjs::jsval::ObjectValue(promises_obj.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c"promises".as_ptr(),
                prom_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Evaluate createReadStream/createWriteStream polyfill
    unsafe {
        let global = JS::CurrentGlobalOrNull(cx.raw_cx());
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let fs_val = mozjs::jsval::ObjectValue(fs_obj.get());
            let fs_val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fs_val };
            JS_DefineProperty(cx.raw_cx(), global_h, c"__fs_stream_ref".as_ptr(), fs_val_h, JSPROP_ENUMERATE as u32);

            let c_filename = c"node:fs:streams".as_ptr();
            let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(NewCompileOptions(cx.raw_cx(), c_filename, 1) as *mut _) else { return; };
            let opts = _opts_guard.as_ptr();
                let mut src = mozjs::rust::transform_str_to_source_text(FS_STREAM_JS);
                let mut rval = UndefinedValue();
                let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
                let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts as *const _, &mut src, rval_handle);

                if ok && rval.is_object() {
                    let exports = rval.to_object();
                    let exports_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &exports };
                    let fs_ptr = fs_obj.get();
                    let fs_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &fs_ptr };

                    for name in &["createReadStream", "createWriteStream"] {
                        let cname = bun_core::ZBox::from_bytes(name.as_bytes());
                        let mut val = UndefinedValue();
                        JS_GetProperty(cx.raw_cx(), exports_h, cname.as_ptr(),
                            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
                        if !val.is_undefined() {
                            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                            JS_DefineProperty(cx.raw_cx(), fs_h, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
                        }
                    }
                }

            JS_DeleteProperty1(cx.raw_cx(), global_h, c"__fs_stream_ref".as_ptr());
        }
    }

    cache_builtin(cx, "fs", fs_obj.get());
}

// --- Argument helpers ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_path_arg(cx: *mut JSContext, args: &CallArgs, index: u32) -> ::std::result::Result<::std::string::String, bool> {
    if args.argc_ <= index {
        JS_ReportErrorUTF8(cx, c"Missing path argument".as_ptr());
        return ::std::result::Result::Err(false);
    }
    let val = *args.get(index).ptr;
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return ::std::result::Result::Ok(crate::jsstr_to_rust_string(cx, s));
        }
    }
    JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
    ::std::result::Result::Err(false)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_encoding_opt(cx: *mut JSContext, args: &CallArgs, index: u32) -> ::std::option::Option<::std::string::String> {
    if args.argc_ <= index {
        return ::std::option::Option::None;
    }
    let val = *args.get(index).ptr;
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return ::std::option::Option::Some(crate::jsstr_to_rust_string(cx, s));
        }
    }
    if val.is_object() {
        let obj = val.to_object();
        let mut enc_val = UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let enc_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut enc_val };
        JS_GetProperty(cx, obj_h, c"encoding".as_ptr(), enc_h);
        if enc_val.is_string() {
            let s = enc_val.to_string();
            if !s.is_null() {
                return ::std::option::Option::Some(crate::jsstr_to_rust_string(cx, s));
            }
        }
    }
    ::std::option::Option::None
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_string_content(cx: *mut JSContext, args: &CallArgs, data: &[u8], encoding: ::std::option::Option<&str>) -> bool {
    match encoding {
        Some("utf-8" | "utf8" | "text") | None => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = bun_core::ZBox::from_bytes(s.as_ref().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("hex") => {
            let hex = bun_core::fmt::bytes_to_hex_lower_string(data);
            let c_str = bun_core::ZBox::from_bytes(hex.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = bun_core::ZBox::from_bytes(encoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("latin1" | "binary") => {
            let s: ::std::string::String = data.iter().map(|&b| b as char).collect();
            let c_str = bun_core::ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some(_) => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = bun_core::ZBox::from_bytes(s.as_ref().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
    }
    true
}

unsafe fn throw_bun_fs_error(cx: *mut JSContext, op: &str, path: &str, err: &bun_sys::Error) -> bool {
    let errno = err.errno as i32;
    let code = bun_core::ErrnoNames::SYS.name(errno).unwrap_or("ERR");
    let msg = format!("{} '{}': {} (errno {})", op, path, err, errno);
    let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
    let code_str = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(code.as_bytes()).as_ptr());
    if !code_str.is_null() {
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        if JS_IsExceptionPending(cx) {
            rooted!(in(cx) let mut exn = UndefinedValue());
            JS_GetPendingException(cx, exn.handle_mut().into());
            let exn_val = exn.get();
            if !exn_val.is_undefined() && exn_val.is_object() {
                let exn_obj = exn_val.to_object();
                let code_val = StringValue(&*code_str);
                let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &exn_obj };
                let code_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &code_val };
                JS_DefineProperty(cx, obj_h, c"code".as_ptr(), code_h, JSPROP_ENUMERATE as u32);
                let path_val = bun_core::ZBox::from_bytes(path.as_bytes());
                let path_str = JS_NewStringCopyZ(cx, path_val.as_ptr());
                if !path_str.is_null() {
                    let path_v = StringValue(&*path_str);
                    let path_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &path_v };
                    JS_DefineProperty(cx, obj_h, c"path".as_ptr(), path_h, JSPROP_ENUMERATE as u32);
                }
                JS_SetPendingException(cx, exn.handle().into(), ExceptionStackBehavior::DoNotCapture);
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    }
    false
}

// --- Sync file operations ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let encoding = get_encoding_opt(cx, &args, 1);
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match File::read_from(Fd::cwd(), zpath.as_bytes()) {
        ::std::result::Result::Ok(data) => return_string_content(cx, &args, &data, encoding.as_deref()),
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "readFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };

    let result = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            File::write_file(Fd::cwd(), zpath, rust_str.as_bytes())
        } else {
            File::write_file(Fd::cwd(), zpath, &[] as &[u8])
        }
    } else if data_val.is_object() {
        let bytes = crate::node_crypto::extract_buffer_bytes(cx, data_val);
        File::write_file(Fd::cwd(), zpath, &bytes)
    } else {
        File::write_file(Fd::cwd(), zpath, &[] as &[u8])
    };

    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "writeFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_append_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let data = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() {
            crate::jsstr_to_rust_string(cx, s).into_bytes()
        } else { Vec::new() }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else { Vec::new() };

    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match File::open(zpath, O::WRONLY | O::CREAT | O::APPEND | O::CLOEXEC, 0o666) {
        ::std::result::Result::Ok(file) => {
            match file.write_all(&data) {
                ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
                ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "appendFileSync", &path, &e),
            }
        }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "appendFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_exists_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut path_buf = PathBuffer::default();
    let exists = path_to_zstr(&path, &mut path_buf).map_or(false, |z| bun_sys::exists_z(z));
    args.rval().set(mozjs::jsval::BooleanValue(exists));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    let result = if recursive {
        bun_sys::mkdir_recursive_at(Fd::cwd(), path.as_bytes())
    } else {
        bun_sys::mkdir(zpath, 0o755)
    };
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "mkdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let with_file_types = get_bool_option(cx, &args, 1, "withFileTypes");

    let dir_fd = match bun_sys::open_dir_for_iteration(Fd::cwd(), path.as_bytes()) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(e) => return throw_bun_fs_error(cx, "readdirSync", &path, &e),
    };
    let mut iter = bun_sys::dir_iterator::iterate(dir_fd);
    let mut names: Vec<::std::string::String> = Vec::new();
    let mut is_dirs: Vec<bool> = Vec::new();
    while let ::std::result::Result::Ok(Some(entry)) = iter.next() {
        let name_bytes = entry.name.slice_u8();
        names.push(::std::string::String::from_utf8_lossy(name_bytes).into_owned());
        is_dirs.push(entry.kind == FileKind::Directory);
    }
    let _ = bun_sys::close(dir_fd);
    // SAFETY: construct wrapped cx to use rooted! and w2:: functions
    let mut wrapped_cx = unsafe {
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))
    };
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, names.len()) });
    if arr.get().is_null() { args.rval().set(UndefinedValue()); return true; }
    for (i, name) in names.iter().enumerate() {
        if with_file_types {
            let dirent = create_dirent(cx, name, is_dirs[i]);
            if !dirent.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(dirent));
                unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, val.handle().into(), JSPROP_ENUMERATE as u32); }
            }
        } else {
            let c_name = bun_core::ZBox::from_bytes(name.as_str().as_bytes());
            let js_str = unsafe { JS_NewStringCopyZ(cx, c_name.as_ptr()) };
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe { JS_DefineElement(cx, arr.handle().into(), i as u32, val.handle().into(), JSPROP_ENUMERATE as u32); }
            }
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_stat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match bun_sys::stat(zpath) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "statSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lstat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match bun_sys::lstat(zpath) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "lstatSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_unlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match bun_sys::unlink(zpath) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "unlinkSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rmdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match bun_sys::rmdir(zpath) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "rmdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rm_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    let result = if recursive {
        bun_sys::delete_tree_absolute(path.as_bytes()).map_err(|e| bun_sys::Error::from_code_int(bun_core::Error::from(e).as_u16() as i32, bun_sys::Tag::rmdir))
    } else {
        bun_sys::unlink(zpath)
    };
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "rmSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rename_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&from) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let mut from_buf = PathBuffer::default();
    let mut to_buf = PathBuffer::default();
    let Some(zfrom) = path_to_zstr_or_throw(cx, &from, &mut from_buf) else { return false };
    let Some(zto) = path_to_zstr_or_throw(cx, &to, &mut to_buf) else { return false };
    match bun_sys::rename(zfrom, zto) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "renameSync", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_copy_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&from) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let mut from_buf = PathBuffer::default();
    let mut to_buf = PathBuffer::default();
    let Some(zfrom) = path_to_zstr_or_throw(cx, &from, &mut from_buf) else { return false };
    let Some(zto) = path_to_zstr_or_throw(cx, &to, &mut to_buf) else { return false };
    // Open source, copy to dest via bun_sys copy_file
    let in_fd = match bun_sys::open(zfrom, O::RDONLY | O::CLOEXEC, 0) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(e) => return throw_bun_fs_error(cx, "copyFileSync", &from, &e),
    };
    match bun_sys::copy_file_z_slow_with_handle(in_fd, Fd::cwd(), zto) {
        ::std::result::Result::Ok(_) => { let _ = bun_sys::close(in_fd); args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => { let _ = bun_sys::close(in_fd); throw_bun_fs_error(cx, "copyFileSync", &from, &e) },
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chmod_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mode_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else if mode_val.is_double() { mode_val.to_double() as u32 } else { 0o644 };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match bun_sys::chmod(zpath, mode) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "chmodSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    let mut resolved_buf = bun_core::PathBuffer::default();
    match bun_sys::realpath(zpath, &mut resolved_buf) {
        ::std::result::Result::Ok(resolved_bytes) => {
            let s = ::std::string::String::from_utf8_lossy(resolved_bytes);
            let c_str = bun_core::ZBox::from_bytes(s.as_ref().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "realpathSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    let mut link_buf = PathBuffer::default();
    match bun_sys::readlink(zpath, &mut link_buf.0) {
        ::std::result::Result::Ok(len) => {
            let s = ::std::string::String::from_utf8_lossy(&link_buf[..len]);
            let c_str = bun_core::ZBox::from_bytes(s.as_ref().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "readlinkSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_symlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let path = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut target_buf = PathBuffer::default();
    let mut path_buf2 = PathBuffer::default();
    let Some(ztarget) = path_to_zstr_or_throw(cx, &target, &mut target_buf) else { return false };
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf2) else { return false };
    match bun_sys::symlink(ztarget, zpath) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "symlinkSync", &target, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_link_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut from_buf = PathBuffer::default();
    let mut to_buf = PathBuffer::default();
    let Some(zfrom) = path_to_zstr_or_throw(cx, &from, &mut from_buf) else { return false };
    let Some(zto) = path_to_zstr_or_throw(cx, &to, &mut to_buf) else { return false };
    match bun_sys::link(zfrom, zto) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "linkSync", &from, &e),
    }
}

// --- Async (callback-based) ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let encoding = get_encoding_opt(cx, &args, 1);
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    match File::read_from(Fd::cwd(), zpath.as_bytes()) {
        ::std::result::Result::Ok(data) => {
            return_string_content(cx, &args, &data, encoding.as_deref())
        }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "readFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let bytes = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() { crate::jsstr_to_rust_string(cx, s).into_bytes() } else { Vec::new() }
    } else { Vec::new() };
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };

    match File::write_file(Fd::cwd(), zpath, &bytes) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_bun_fs_error(cx, "writeFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let mut path_buf = PathBuffer::default();
    let Some(zpath) = path_to_zstr_or_throw(cx, &path, &mut path_buf) else { return false };
    let result = if recursive {
        bun_sys::mkdir_recursive_at(Fd::cwd(), path.as_bytes())
    } else {
        bun_sys::mkdir(zpath, 0o755)
    };
    match result {
        ::std::result::Result::Ok(()) => {
            if argc > 1 && (*args.get(argc - 1).ptr).is_object() {
                let cb = (*args.get(argc - 1).ptr).to_object();
                let cb_val = mozjs::jsval::ObjectValue(cb);
                let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };
                let null_args = HandleValueArray::empty();
                let global = CurrentGlobalOrNull(cx);
                if !global.is_null() {
                    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                    let mut rval = UndefinedValue();
                    JS_CallFunctionValue(cx, global_h, cb_h, &null_args, MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
                    });
                    JS_ClearPendingException(cx);
                }
            }
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => {
            if argc > 1 && (*args.get(argc - 1).ptr).is_object() {
                let cb = (*args.get(argc - 1).ptr).to_object();
                let cb_val = mozjs::jsval::ObjectValue(cb);
                let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };
                let err_msg = format!("EACCES: mkdir '{}': {}", path, e);
                let c_err = bun_core::ZBox::from_bytes(err_msg.as_bytes());
                let err_obj = JS_NewPlainObject(cx);
                if !err_obj.is_null() {
                    let msg_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
                    if !msg_str.is_null() {
                        let msg_val = mozjs::jsval::StringValue(&*msg_str);
                        let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
                        JS_DefineProperty(cx,
                            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj },
                            c"message".as_ptr(),
                            msg_h, JSPROP_ENUMERATE as u32);
                    }
                    let code_str = JS_NewStringCopyZ(cx, c"EACCES".as_ptr());
                    if !code_str.is_null() {
                        let code_val = mozjs::jsval::StringValue(&*code_str);
                        let code_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &code_val };
                        JS_DefineProperty(cx,
                            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj },
                            c"code".as_ptr(),
                            code_h, JSPROP_ENUMERATE as u32);
                    }
                    let err_val = mozjs::jsval::ObjectValue(err_obj);
                    let err_args = HandleValueArray { length_: 1, elements_: &err_val as *const JSVal };
                    let global = CurrentGlobalOrNull(cx);
                    if !global.is_null() {
                        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                        let mut rval = UndefinedValue();
                        JS_CallFunctionValue(cx, global_h, cb_h, &err_args, MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
                        });
                        JS_ClearPendingException(cx);
                    }
                }
                args.rval().set(UndefinedValue());
                true
            } else {
                throw_bun_fs_error(cx, "mkdir", &path, &e)
            }
        }
    }
}

// --- fs.promises ---

macro_rules! promise_simple_op {
    ($fn_name:ident, $op:expr, $op_name:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $fn_name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
            let null_obj = ::std::ptr::null_mut::<JSObject>();
            let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
            if promise.is_null() { args.rval().set(UndefinedValue()); return false; }
            match $op(&path) {
                ::std::result::Result::Ok(()) => {
                    resolve_undefined(cx, promise);
                }
                ::std::result::Result::Err(e) => {
                    reject_with_error(cx, promise, &format!("{} '{}': {}", $op_name, path, e));
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise));
            true
        }
    };
}

promise_simple_op!(fs_promises_mkdir, |p: &str| { let _c = bun_core::ZBox::from_bytes(p.as_bytes()); bun_sys::mkdir_recursive_at(Fd::cwd(), p.as_bytes()) }, "mkdir");
promise_simple_op!(fs_promises_unlink, |p: &str| { let c = bun_core::ZBox::from_bytes(p.as_bytes()); unsafe { bun_sys::unlink(bun_core::ZStr::from_raw(c.as_ptr().cast::<u8>(), p.len())) } }, "unlink");

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let null_obj = ::std::ptr::null_mut::<JSObject>();
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
    if promise.is_null() { args.rval().set(UndefinedValue()); return false; }
    let c_from = bun_core::ZBox::from_bytes(from.as_bytes());
    let c_to = bun_core::ZBox::from_bytes(to.as_bytes());
    let zfrom = unsafe { bun_core::ZStr::from_raw(c_from.as_ptr().cast::<u8>(), from.len()) };
    let zto = unsafe { bun_core::ZStr::from_raw(c_to.as_ptr().cast::<u8>(), to.len()) };
    match bun_sys::rename(zfrom, zto) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("rename '{}': {}", from, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_copy_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let null_obj = ::std::ptr::null_mut::<JSObject>();
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
    if promise.is_null() { args.rval().set(UndefinedValue()); return false; }
    let c_from = bun_core::ZBox::from_bytes(from.as_bytes());
    let c_to = bun_core::ZBox::from_bytes(to.as_bytes());
    let zfrom = unsafe { bun_core::ZStr::from_raw(c_from.as_ptr().cast::<u8>(), from.len()) };
    let zto = unsafe { bun_core::ZStr::from_raw(c_to.as_ptr().cast::<u8>(), to.len()) };
    let in_fd = match bun_sys::open(zfrom, O::RDONLY | O::CLOEXEC, 0) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(e) => {
            reject_with_error(cx, promise, &format!("copyFile '{}': {}", from, e));
            args.rval().set(mozjs::jsval::ObjectValue(promise));
            return true;
        }
    };
    match bun_sys::copy_file_z_slow_with_handle(in_fd, Fd::cwd(), zto) {
        ::std::result::Result::Ok(_) => { let _ = bun_sys::close(in_fd); resolve_undefined(cx, promise) }
        ::std::result::Result::Err(e) => { let _ = bun_sys::close(in_fd); reject_with_error(cx, promise, &format!("copyFile '{}': {}", from, e)) },
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let encoding = get_encoding_opt(cx, &args, 1);
    let null_obj = ::std::ptr::null_mut::<JSObject>();
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
    if promise.is_null() { args.rval().set(UndefinedValue()); return false; }

    match File::read_from(Fd::cwd(), path.as_bytes()) {
        ::std::result::Result::Ok(data) => {
            let val = string_or_buffer(cx, &data, encoding.as_deref());
            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
            mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, val_h);
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("readFile '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_write_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let bytes = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() { crate::jsstr_to_rust_string(cx, s).into_bytes() } else { Vec::new() }
    } else { Vec::new() };
    let null_obj = ::std::ptr::null_mut::<JSObject>();
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
    if promise.is_null() { args.rval().set(UndefinedValue()); return false; }
    let mut path_buf = PathBuffer::default();
    let zpath = path_to_zstr(&path, &mut path_buf);
    match zpath {
        Some(zp) => match File::write_file(Fd::cwd(), zp, &bytes) {
            ::std::result::Result::Ok(()) => resolve_undefined(cx, promise),
            ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("writeFile '{}': {}", path, e)),
        },
        None => reject_with_error(cx, promise, &format!("writeFile '{}': Path too long", path)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_stat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let null_obj = ::std::ptr::null_mut::<JSObject>();
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
    if promise.is_null() { args.rval().set(UndefinedValue()); return false; }
    let mut path_buf = PathBuffer::default();
    let zpath = path_to_zstr(&path, &mut path_buf);
    match zpath {
        Some(zp) => match bun_sys::stat(zp) {
            ::std::result::Result::Ok(meta) => {
                let stats = create_stats_object(cx, &meta);
                let val = mozjs::jsval::ObjectValue(stats);
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
                mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, val_h);
            }
            ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("stat '{}': {}", path, e)),
        },
        None => reject_with_error(cx, promise, &format!("stat '{}': Path too long", path)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_readdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    let mut wrapped_cx = unsafe {
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))
    };
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }

    match bun_sys::open_dir_for_iteration(Fd::cwd(), path.as_bytes()) {
        ::std::result::Result::Ok(dir_fd) => {
            let mut iter = bun_sys::dir_iterator::iterate(dir_fd);
            let mut names: Vec<::std::string::String> = Vec::new();
            while let ::std::result::Result::Ok(Some(entry)) = iter.next() {
                names.push(::std::string::String::from_utf8_lossy(entry.name.slice_u8()).into_owned());
            }
            let _ = bun_sys::close(dir_fd);
            rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, names.len()) });
            if arr.get().is_null() { args.rval().set(mozjs::jsval::ObjectValue(promise.get())); return true; }
            for (idx, name) in names.iter().enumerate() {
                let c_name = bun_core::ZBox::from_bytes(name.as_str().as_bytes());
                let js_str = unsafe { JS_NewStringCopyZ(cx, c_name.as_ptr()) };
                if !js_str.is_null() {
                    rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                    unsafe { JS_DefineElement(cx, arr.handle().into(), idx as u32, val.handle().into(), JSPROP_ENUMERATE as u32); }
                }
            }
            rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), arr_val.handle().into()); }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("readdir '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

// --- Helper functions ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_bool_option(cx: *mut JSContext, args: &CallArgs, opt_index: u32, key: &str) -> bool {
    if args.argc_ <= opt_index { return false; }
    let opt_val = *args.get(opt_index).ptr;
    if !opt_val.is_object() { return false; }
    let obj = opt_val.to_object();
    let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
    let mut val = UndefinedValue();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_h, c_key.as_ptr(), val_h);
    val.is_boolean() && val.to_boolean()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn string_or_buffer(cx: *mut JSContext, data: &[u8], encoding: ::std::option::Option<&str>) -> JSVal {
    match encoding {
        Some("utf-8" | "utf8" | "text") | None => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = bun_core::ZBox::from_bytes(s.as_ref().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        Some("hex") => {
            let hex = bun_core::fmt::bytes_to_hex_lower_string(data);
            let c_str = bun_core::ZBox::from_bytes(hex.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = bun_core::ZBox::from_bytes(encoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        _ => UndefinedValue(),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_stats_object(cx: *mut JSContext, st: &Stat) -> *mut JSObject {
    let stats = JS_NewPlainObject(cx);
    if stats.is_null() { return stats; }
    let stats_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &stats };

    let kind = bun_core::kind_from_mode(st.st_mode);
    let is_file = kind == FileKind::File;
    let is_dir = kind == FileKind::Directory;
    let is_symlink = kind == FileKind::SymLink;

    define_num_prop(cx, stats_h, "size", st.st_size as f64);

    #[cfg(unix)]
    {
        define_num_prop(cx, stats_h, "dev", st.st_dev as f64);
        define_num_prop(cx, stats_h, "ino", st.st_ino as f64);
        define_num_prop(cx, stats_h, "mode", st.st_mode as f64);
        define_num_prop(cx, stats_h, "nlink", st.st_nlink as f64);
        define_num_prop(cx, stats_h, "uid", st.st_uid as f64);
        define_num_prop(cx, stats_h, "gid", st.st_gid as f64);
        define_num_prop(cx, stats_h, "rdev", st.st_rdev as f64);
        define_num_prop(cx, stats_h, "blksize", st.st_blksize as f64);
        define_num_prop(cx, stats_h, "blocks", st.st_blocks as f64);
        define_num_prop(cx, stats_h, "atimeMs", st.st_atime as f64 * 1000.0);
        define_num_prop(cx, stats_h, "mtimeMs", st.st_mtime as f64 * 1000.0);
        define_num_prop(cx, stats_h, "ctimeMs", st.st_ctime as f64 * 1000.0);
    }

    // Store boolean values as hidden properties for method callbacks
    define_bool_prop(cx, stats_h, "_isFile", is_file);
    define_bool_prop(cx, stats_h, "_isDirectory", is_dir);
    define_bool_prop(cx, stats_h, "_isSymbolicLink", is_symlink);

    // Node.js Stats methods
    JS_DefineFunction(cx, stats_h, c"isFile".as_ptr(), Some(stats_is_file), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, stats_h, c"isDirectory".as_ptr(), Some(stats_is_directory), 0, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(cx, stats_h, c"isSymbolicLink".as_ptr(), Some(stats_is_symlink), 0, JSPROP_ENUMERATE as u32);

    stats
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_dirent(cx: *mut JSContext, name: &str, is_dir: bool) -> *mut JSObject {
    let dirent = JS_NewPlainObject(cx);
    if dirent.is_null() { return dirent; }
    let dirent_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &dirent };
    let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
    if !js_str.is_null() {
        let name_val = mozjs::jsval::StringValue(&*js_str);
        let name_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &name_val };
        JS_DefineProperty(cx, dirent_h, c"name".as_ptr(), name_h, JSPROP_ENUMERATE as u32);
    }
    define_bool_prop(cx, dirent_h, "isFile", !is_dir);
    define_bool_prop(cx, dirent_h, "isDirectory", is_dir);
    dirent
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_undefined(cx: *mut JSContext, promise: *mut JSObject) {
    let val = UndefinedValue();
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
    let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, val_h);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_with_error(cx: *mut JSContext, promise: *mut JSObject, msg: &str) {
    let err_obj = JS_NewPlainObject(cx);
    if !err_obj.is_null() {
        let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            let msg_val = mozjs::jsval::StringValue(&*js_str);
            let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
            let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
            JS_DefineProperty(cx, err_h, c"message".as_ptr(), msg_h, JSPROP_ENUMERATE as u32);
        }
    }
    let err_val = mozjs::jsval::ObjectValue(err_obj);
    let err_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
    let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
    mozjs_sys::jsapi::JS::RejectPromise(cx, promise_h, err_h);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_num_prop(cx: *mut JSContext, obj: Handle<*mut JSObject>, name: &str, val: f64) {
    let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
    let js_val = if val == (val as i32) as f64 && val.abs() < i32::MAX as f64 {
        mozjs::jsval::Int32Value(val as i32)
    } else {
        mozjs::jsval::DoubleValue(val)
    };
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &js_val };
    JS_DefineProperty(cx, obj, c_name.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_bool_prop(cx: *mut JSContext, obj: Handle<*mut JSObject>, name: &str, val: bool) {
    let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
    let js_val = mozjs::jsval::BooleanValue(val);
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &js_val };
    JS_DefineProperty(cx, obj, c_name.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_hidden_bool(cx: *mut JSContext, obj: *mut JSObject, prop: &str) -> bool {
    let c_name = bun_core::ZBox::from_bytes(prop.as_bytes());
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_h, c_name.as_ptr(), val_h);
    val.to_boolean()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(cx, this, "_isFile")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_directory(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(cx, this, "_isDirectory")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(cx, this, "_isSymbolicLink")));
    true
}
