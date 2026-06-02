// @trace REQ-ENG-007
use ::std::ffi::CString;
use ::std::fs;
use ::std::path::Path;
// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const FS_STREAM_JS: &str = r#"
(function() {
  var fs = globalThis.__fs_stream_ref;

  function EE() { this._events = {}; }
  EE.prototype.on = function(e, fn) {
    (this._events[e] || (this._events[e] = [])).push(fn);
    return this;
  };
  EE.prototype.emit = function(e) {
    var a = Array.prototype.slice.call(arguments, 1);
    var ls = this._events[e];
    if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a);
    return !!ls;
  };

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
            let c_name = CString::new(*name).unwrap_or_default();
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

            let c_filename = CString::new("node:fs:streams").unwrap_or_default();
            let opts = NewCompileOptions(cx.raw_cx(), c_filename.as_ptr(), 1);
            if !opts.is_null() {
                let mut src = mozjs::rust::transform_str_to_source_text(FS_STREAM_JS);
                let mut rval = UndefinedValue();
                let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
                let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut src, rval_handle);
                libc::free(opts as *mut _);

                if ok && rval.is_object() {
                    let exports = rval.to_object();
                    let exports_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &exports };
                    let fs_ptr = fs_obj.get();
                    let fs_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &fs_ptr };

                    for name in &["createReadStream", "createWriteStream"] {
                        let cname = CString::new(*name).unwrap_or_default();
                        let mut val = UndefinedValue();
                        JS_GetProperty(cx.raw_cx(), exports_h, cname.as_ptr(),
                            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
                        if !val.is_undefined() {
                            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                            JS_DefineProperty(cx.raw_cx(), fs_h, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
                        }
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
            let c_str = CString::new(s.as_ref()).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("hex") => {
            let hex: ::std::string::String = data.iter().map(|b| format!("{:02x}", b)).collect();
            let c_str = CString::new(hex).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = CString::new(encoded).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("latin1" | "binary") => {
            let s: ::std::string::String = data.iter().map(|&b| b as char).collect();
            let c_str = CString::new(s).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some(_) => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = CString::new(s.as_ref()).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_fs_error(cx: *mut JSContext, op: &str, path: &str, err: &::std::io::Error) -> bool {
    let code = match err.kind() {
        ::std::io::ErrorKind::NotFound => "ENOENT",
        ::std::io::ErrorKind::PermissionDenied => "EACCES",
        ::std::io::ErrorKind::AlreadyExists => "EEXIST",
        ::std::io::ErrorKind::IsADirectory => "EISDIR",
        ::std::io::ErrorKind::NotADirectory => "ENOTDIR",
        _ => "ERR",
    };
    let msg = format!("{} '{}': {}", op, path, err);
    let c_msg = CString::new(msg).unwrap_or_default();
    let code_str = JS_NewStringCopyZ(cx, CString::new(code).unwrap_or_default().as_ptr());
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
                let path_val = CString::new(path.as_bytes()).unwrap_or_default();
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
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let encoding = get_encoding_opt(cx, &args, 1);
    match fs::read(&path) {
        ::std::result::Result::Ok(data) => return_string_content(cx, &args, &data, encoding.as_deref()),
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };

    let result = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            fs::write(&path, rust_str.as_bytes())
        } else {
            fs::write(&path, &[] as &[u8])
        }
    } else {
        fs::write(&path, &[] as &[u8])
    };

    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "writeFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_append_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let data = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() {
            crate::jsstr_to_rust_string(cx, s).into_bytes()
        } else { Vec::new() }
    } else { Vec::new() };

    use ::std::io::Write;
    match ::std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        ::std::result::Result::Ok(mut file) => {
            match file.write_all(&data) {
                ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
                ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFileSync", &path, &e),
            }
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_exists_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    args.rval().set(mozjs::jsval::BooleanValue(Path::new(&path).exists()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive { fs::create_dir_all(&path) } else { fs::create_dir(&path) };
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "mkdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let with_file_types = get_bool_option(cx, &args, 1, "withFileTypes");

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let mut names: Vec<::std::string::String> = Vec::new();
            let mut is_dirs: Vec<bool> = Vec::new();
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
                is_dirs.push(entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false));
            }
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
                    let c_name = CString::new(name.as_str()).unwrap_or_default();
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
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_stat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    match fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "statSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lstat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "lstatSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_unlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::remove_file(&path) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "unlinkSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rmdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::remove_dir(&path) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rmdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rm_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rmSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rename_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&from) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::rename(&from, &to) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "renameSync", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_copy_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&from) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "copyFileSync", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chmod_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mode_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else if mode_val.is_double() { mode_val.to_double() as u32 } else { 0o644 };
    #[cfg(unix)]
    let result = {
        use ::std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let result = fs::set_permissions(&path, fs::Permissions::new());
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "chmodSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    match fs::canonicalize(&path) {
        ::std::result::Result::Ok(resolved) => {
            let s = resolved.to_string_lossy();
            let c_str = CString::new(s.as_ref()).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "realpathSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    match fs::read_link(&path) {
        ::std::result::Result::Ok(target) => {
            let s = target.to_string_lossy();
            let c_str = CString::new(s.as_ref()).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readlinkSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_symlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let path = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    #[cfg(unix)]
    let result = ::std::os::unix::fs::symlink(&target, &path);
    #[cfg(not(unix))]
    let result = fs::hard_link(&target, &path);
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "symlinkSync", &target, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_link_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    match fs::hard_link(&from, &to) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "linkSync", &from, &e),
    }
}

// --- Async (callback-based) ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let encoding = get_encoding_opt(cx, &args, 1);

    match fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            return_string_content(cx, &args, &data, encoding.as_deref())
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readFile", &path, &e),
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

    match fs::write(&path, &bytes) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "writeFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = CString::new(e).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive { fs::create_dir_all(&path) } else { fs::create_dir(&path) };
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
                let c_err = CString::new(err_msg).unwrap_or_default();
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
                throw_fs_error(cx, "mkdir", &path, &e)
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

promise_simple_op!(fs_promises_mkdir, |p: &str| fs::create_dir_all(p), "mkdir");
promise_simple_op!(fs_promises_unlink, |p: &str| fs::remove_file(p), "unlink");

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let null_obj = ::std::ptr::null_mut::<JSObject>();
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &null_obj });
    if promise.is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::rename(&from, &to) {
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
    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => resolve_undefined(cx, promise),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("copyFile '{}': {}", from, e)),
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

    match fs::read(&path) {
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
    match fs::write(&path, &bytes) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("writeFile '{}': {}", path, e)),
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
    match fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            let val = mozjs::jsval::ObjectValue(stats);
            let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
            mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, val_h);
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise, &format!("stat '{}': {}", path, e)),
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

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let names: Vec<::std::string::String> = entries.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, names.len()) });
            if arr.get().is_null() { args.rval().set(mozjs::jsval::ObjectValue(promise.get())); return true; }
            for (idx, name) in names.iter().enumerate() {
                let c_name = CString::new(name.as_str()).unwrap_or_default();
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
    let c_key = CString::new(key).unwrap_or_default();
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
            let c_str = CString::new(s.as_ref()).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        Some("hex") => {
            let hex: ::std::string::String = data.iter().map(|b| format!("{:02x}", b)).collect();
            let c_str = CString::new(hex).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = CString::new(encoded).unwrap_or_default();
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        _ => UndefinedValue(),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_stats_object(cx: *mut JSContext, meta: &fs::Metadata) -> *mut JSObject {
    let stats = JS_NewPlainObject(cx);
    if stats.is_null() { return stats; }
    let stats_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &stats };

    let is_file = meta.is_file();
    let is_dir = meta.is_dir();
    let is_symlink = meta.file_type().is_symlink();

    define_num_prop(cx, stats_h, "size", meta.len() as f64);

    #[cfg(unix)]
    {
        use ::std::os::unix::fs::MetadataExt;
        define_num_prop(cx, stats_h, "dev", meta.dev() as f64);
        define_num_prop(cx, stats_h, "ino", meta.ino() as f64);
        define_num_prop(cx, stats_h, "mode", meta.mode() as f64);
        define_num_prop(cx, stats_h, "nlink", meta.nlink() as f64);
        define_num_prop(cx, stats_h, "uid", meta.uid() as f64);
        define_num_prop(cx, stats_h, "gid", meta.gid() as f64);
        define_num_prop(cx, stats_h, "rdev", meta.rdev() as f64);
        define_num_prop(cx, stats_h, "blksize", meta.blksize() as f64);
        define_num_prop(cx, stats_h, "blocks", meta.blocks() as f64);
        define_num_prop(cx, stats_h, "atimeMs", meta.atime() as f64 * 1000.0);
        define_num_prop(cx, stats_h, "mtimeMs", meta.mtime() as f64 * 1000.0);
        define_num_prop(cx, stats_h, "ctimeMs", meta.ctime() as f64 * 1000.0);
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
    let c_name = CString::new(name).unwrap_or_default();
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
        let c_msg = CString::new(msg).unwrap_or_default();
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
    let c_name = CString::new(name).unwrap_or_default();
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
    let c_name = CString::new(name).unwrap_or_default();
    let js_val = mozjs::jsval::BooleanValue(val);
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &js_val };
    JS_DefineProperty(cx, obj, c_name.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_hidden_bool(cx: *mut JSContext, obj: *mut JSObject, prop: &str) -> bool {
    let c_name = CString::new(prop).unwrap_or_default();
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
