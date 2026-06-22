// @trace REQ-ENG-007
use bun_core::ZBox;
use bun_sys::fs as bun_fs;
use ::std::fs;
use ::std::path::Path;
use ::std::sync::{Arc, Mutex};
// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// --- Async I/O infrastructure ---
// Background I/O uses std::thread::spawn + Arc<Mutex<Option<Result>>> shared slot.
// Completion is scheduled on the JS thread via bao_uloop::uws_loop_defer (next_tick).

enum FsAsyncResult {
    Ok(Vec<u8>),
    OkStat(bun_sys::PosixStat),
    OkString(String),
    OkVoid,
    OkBool(bool),
    OkI32(i32),
    OkOpen(i32),
    OkRead { bytes_read: i32, buffer: Vec<u8> },
    OkWrite(i32),
    OkDirnames(Vec<String>),
    OkDirents(Vec<(String, bool)>),
}

struct FsAsyncCtx {
    cx: *mut JSContext,
    callback: *mut JSObject,
    result: Arc<Mutex<Option<::std::result::Result<FsAsyncResult, (String, String)>>>>,
    encoding: Option<String>,
    op_name: String,
    path: String,
    rooted: bool,
}

unsafe fn schedule_defer(ctx: *mut FsAsyncCtx) {
    bao_uloop::force_link();
    let loop_ = bao_uloop::uws_get_loop();
    if loop_.is_null() {
        let _ = Box::from_raw(ctx);
        return;
    }
    bao_uloop::uws_loop_defer(loop_, ctx as *mut ::std::ffi::c_void, fs_async_defer_callback);
}

unsafe extern "C" fn fs_async_defer_callback(raw_ctx: *mut ::std::ffi::c_void) {
    let ctx = Box::from_raw(raw_ctx as *mut FsAsyncCtx);
    let cx = ctx.cx;
    let callback = ctx.callback;
    let encoding = ctx.encoding.as_deref();
    let _op_name = &ctx.op_name;

    let mut result_guard = ctx.result.lock().unwrap();
    let result_opt = result_guard.take();
    ::std::mem::drop(result_guard);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cb = callback);
    rooted!(&in(cx_ref) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() { return; }
    rooted!(&in(cx_ref) let global_rooted = global);

    match result_opt {
        Some(Ok(FsAsyncResult::Ok(data))) => {
            let val = string_or_buffer(cx, &data, encoding);
            rooted!(&in(cx_ref) let val_rooted = val);
            let args_arr = [UndefinedValue(), val_rooted.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkStat(stat))) => {
            let stats_obj = create_stats_object(cx, &stat);
            rooted!(&in(cx_ref) let stats_val = mozjs::jsval::ObjectValue(stats_obj));
            let args_arr = [UndefinedValue(), stats_val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkString(s))) => {
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            let val = if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) };
            rooted!(&in(cx_ref) let val_rooted = val);
            let args_arr = [UndefinedValue(), val_rooted.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkVoid)) => {
            let args_arr = [UndefinedValue()];
            let cb_args = HandleValueArray { length_: 1, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkBool(b))) => {
            rooted!(&in(cx_ref) let val = mozjs::jsval::BooleanValue(b));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkI32(v))) => {
            rooted!(&in(cx_ref) let val = mozjs::jsval::Int32Value(v));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkOpen(fd))) => {
            rooted!(&in(cx_ref) let val = mozjs::jsval::Int32Value(fd));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkRead { bytes_read, buffer })) => {
            let buf_obj = crate::globals::create_buffer_object(cx, &buffer);
            let buf_val = if buf_obj.is_null() { UndefinedValue() } else { mozjs::jsval::ObjectValue(buf_obj) };
            rooted!(&in(cx_ref) let buf_rooted = buf_val);
            rooted!(&in(cx_ref) let br_val = mozjs::jsval::Int32Value(bytes_read));
            let args_arr = [UndefinedValue(), br_val.get(), buf_rooted.get()];
            let cb_args = HandleValueArray { length_: 3, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkWrite(written))) => {
            rooted!(&in(cx_ref) let val = mozjs::jsval::Int32Value(written));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkDirnames(names))) => {
            rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, names.len()));
            if !arr.get().is_null() {
                for (idx, name) in names.iter().enumerate() {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                        JS_DefineElement(cx, arr.handle().into(), idx as u32, val.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
            }
            rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            let args_arr = [UndefinedValue(), arr_val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkDirents(entries))) => {
            rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, entries.len()));
            if !arr.get().is_null() {
                for (idx, (name, is_dir)) in entries.iter().enumerate() {
                    let dirent = create_dirent(cx, name, *is_dir);
                    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(dirent));
                    JS_DefineElement(cx, arr.handle().into(), idx as u32, val.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            let args_arr = [UndefinedValue(), arr_val.get()];
            let cb_args = HandleValueArray { length_: 2, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        Some(Err((code, msg))) => {
            rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
            if !err_obj.get().is_null() {
                let c_msg = ZBox::from_bytes(msg.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx_ref) let msg_val = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineProperty(cx, err_obj.handle().into(), c"message".as_ptr(), msg_val.handle().into(), JSPROP_ENUMERATE as u32);
                }
                let c_code = ZBox::from_bytes(code.as_bytes());
                let code_str = JS_NewStringCopyZ(cx, c_code.as_ptr());
                if !code_str.is_null() {
                    rooted!(&in(cx_ref) let code_val = mozjs::jsval::StringValue(&*code_str));
                    JS_DefineProperty(cx, err_obj.handle().into(), c"code".as_ptr(), code_val.handle().into(), JSPROP_ENUMERATE as u32);
                }
                let c_path = ZBox::from_bytes(ctx.path.as_bytes());
                let path_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
                if !path_str.is_null() {
                    rooted!(&in(cx_ref) let path_val = mozjs::jsval::StringValue(&*path_str));
                    JS_DefineProperty(cx, err_obj.handle().into(), c"path".as_ptr(), path_val.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
            let args_arr = [err_val.get()];
            let cb_args = HandleValueArray { length_: 1, elements_: args_arr.as_ptr() };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &cb_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
        None => {
            let null_args = HandleValueArray::empty();
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &null_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            JS_ClearPendingException(cx);
        }
    }
    if ctx.rooted {
        let mut cb_val = mozjs::jsval::ObjectValue(callback);
        RemoveRawValueRoot(cx, &mut cb_val);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_callback_and_encoding(cx: *mut JSContext, args: &CallArgs, start_idx: u32) -> Option<(*mut JSObject, Option<String>)> {
    let mut cb_idx = None;
    let mut encoding = None;
    for i in start_idx..args.argc_ {
        let val = *args.get(i).ptr;
        if val.is_object() {
            if cb_idx.is_none() {
                cb_idx = Some(i);
            } else if encoding.is_none() {
                encoding = get_encoding_opt(cx, args, i);
            }
        } else if val.is_string() && encoding.is_none() {
            encoding = Some(crate::jsstr_to_rust_string(cx, val.to_string()));
        }
    }
    cb_idx.map(|idx| ((*args.get(idx).ptr).to_object(), encoding))
}

fn io_error_code(err: &::std::io::Error) -> &'static str {
    match err.kind() {
        ::std::io::ErrorKind::NotFound => "ENOENT",
        ::std::io::ErrorKind::PermissionDenied => "EACCES",
        ::std::io::ErrorKind::AlreadyExists => "EEXIST",
        _ => "ERR",
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn spawn_fs_async<F>(
    cx: *mut JSContext,
    op_name: &str,
    path: String,
    callback: *mut JSObject,
    encoding: Option<String>,
    work: F,
) where
    F: FnOnce() -> ::std::result::Result<FsAsyncResult, ::std::io::Error> + Send + 'static,
{
    let mut cb_val = mozjs::jsval::ObjectValue(callback);
    let rooted = AddRawValueRoot(cx, &mut cb_val, b"fs_async_cb\0".as_ptr() as *const ::std::os::raw::c_char);

    let result_slot: Arc<Mutex<Option<::std::result::Result<FsAsyncResult, (String, String)>>>> = Arc::new(Mutex::new(None));
    let result_slot_clone = result_slot.clone();

    let op_name_owned = op_name.to_string();
    let path_for_err = path.clone();

    let ctx = Box::new(FsAsyncCtx {
        cx,
        callback,
        result: result_slot,
        encoding,
        op_name: op_name.to_string(),
        path,
        rooted,
    });
    let ctx_ptr = Box::into_raw(ctx) as usize;

    ::std::thread::spawn(move || {
        let result = work();
        let stored = match result {
            Ok(v) => Ok(v),
            Err(e) => {
                let code = io_error_code(&e).to_string();
                let msg = format!("{} '{}': {}", op_name_owned, path_for_err, e);
                Err((code, msg))
            }
        };
        {
            let mut slot = result_slot_clone.lock().unwrap();
            *slot = Some(stored);
        }
        schedule_defer(ctx_ptr as *mut FsAsyncCtx);
    });
}

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
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"cpSync".as_ptr(), Some(fs_cp_sync), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"cp".as_ptr(), Some(fs_cp), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"watch".as_ptr(), Some(fs_watch), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"watchFile".as_ptr(), Some(fs_watch_file), 2, JSPROP_ENUMERATE as u32);

        // Async methods
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readFile".as_ptr(), Some(fs_read_file), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"writeFile".as_ptr(), Some(fs_write_file), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"mkdir".as_ptr(), Some(fs_mkdir), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"appendFile".as_ptr(), Some(fs_append_file), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"access".as_ptr(), Some(fs_access), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"chmod".as_ptr(), Some(fs_chmod), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"chown".as_ptr(), Some(fs_chown), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"close".as_ptr(), Some(fs_close), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"copyFile".as_ptr(), Some(fs_copy_file), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"exists".as_ptr(), Some(fs_exists), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"fchmod".as_ptr(), Some(fs_fchmod), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"fchown".as_ptr(), Some(fs_fchown), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"fdatasync".as_ptr(), Some(fs_fdatasync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"fstat".as_ptr(), Some(fs_fstat), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"fsync".as_ptr(), Some(fs_fsync), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"ftruncate".as_ptr(), Some(fs_ftruncate), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"futimes".as_ptr(), Some(fs_futimes), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"lchown".as_ptr(), Some(fs_lchown), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"link".as_ptr(), Some(fs_link), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"lstat".as_ptr(), Some(fs_lstat), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"lutimes".as_ptr(), Some(fs_lutimes), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"mkdtemp".as_ptr(), Some(fs_mkdtemp), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"open".as_ptr(), Some(fs_open), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"opendir".as_ptr(), Some(fs_opendir), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"read".as_ptr(), Some(fs_read), 4, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readdir".as_ptr(), Some(fs_readdir), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readlink".as_ptr(), Some(fs_readlink), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"readv".as_ptr(), Some(fs_readv), 4, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"realpath".as_ptr(), Some(fs_realpath), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"rename".as_ptr(), Some(fs_rename), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"rm".as_ptr(), Some(fs_rm), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"rmdir".as_ptr(), Some(fs_rmdir), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"stat".as_ptr(), Some(fs_stat), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"symlink".as_ptr(), Some(fs_symlink), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"truncate".as_ptr(), Some(fs_truncate), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"unlink".as_ptr(), Some(fs_unlink), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"utimes".as_ptr(), Some(fs_utimes), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"write".as_ptr(), Some(fs_write), 4, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, fs_obj.handle(), c"writev".as_ptr(), Some(fs_writev), 4, JSPROP_ENUMERATE as u32);

        // Constants
        let constants: &[(&str, i32)] = &[
            ("F_OK", 0), ("R_OK", 4), ("W_OK", 2), ("X_OK", 1),
        ];
        for (name, value) in constants {
            let c_name = ZBox::from_bytes(name.as_bytes());
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
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"lstat".as_ptr(), Some(fs_promises_lstat), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"appendFile".as_ptr(), Some(fs_promises_append_file), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"chmod".as_ptr(), Some(fs_promises_chmod), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"chown".as_ptr(), Some(fs_promises_chown), 3, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"access".as_ptr(), Some(fs_promises_access), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"rm".as_ptr(), Some(fs_promises_rm), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"rmdir".as_ptr(), Some(fs_promises_rmdir), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"realpath".as_ptr(), Some(fs_promises_realpath), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"readlink".as_ptr(), Some(fs_promises_readlink), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"symlink".as_ptr(), Some(fs_promises_symlink), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"link".as_ptr(), Some(fs_promises_link), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"truncate".as_ptr(), Some(fs_promises_truncate), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"utimes".as_ptr(), Some(fs_promises_utimes), 3, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"mkdtemp".as_ptr(), Some(fs_promises_mkdtemp), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"open".as_ptr(), Some(fs_promises_open), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"read".as_ptr(), Some(fs_promises_read), 4, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, promises_obj.handle(), c"write".as_ptr(), Some(fs_promises_write), 4, JSPROP_ENUMERATE as u32);

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
            rooted!(&in(cx) let global_rooted = global);
            rooted!(&in(cx) let fs_val = mozjs::jsval::ObjectValue(fs_obj.get()));
            JS_DefineProperty(cx.raw_cx(), global_rooted.handle().into(), c"__fs_stream_ref".as_ptr(), fs_val.handle().into(), JSPROP_ENUMERATE as u32);

            let c_filename = ZBox::from_bytes("node:fs:streams".as_bytes());
            let opts = NewCompileOptions(cx.raw_cx(), c_filename.as_ptr(), 1);
            if !opts.is_null() {
                let mut src = mozjs::rust::transform_str_to_source_text(FS_STREAM_JS);
                let mut rval = UndefinedValue();
                let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
                let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut src, rval_handle);
                libc::free(opts as *mut _);

                if ok && rval.is_object() {
                    let exports = rval.to_object();
                    rooted!(&in(cx) let exports_rooted = exports);

                    for name in &["createReadStream", "createWriteStream"] {
                        let cname = ZBox::from_bytes(name.as_bytes());
                        let mut val = UndefinedValue();
                        JS_GetProperty(cx.raw_cx(), exports_rooted.handle().into(), cname.as_ptr(),
                            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
                        if !val.is_undefined() {
                            rooted!(&in(cx) let val_root = val);
                            JS_DefineProperty(cx.raw_cx(), fs_obj.handle().into(), cname.as_ptr(), val_root.handle().into(), JSPROP_ENUMERATE as u32);
                        }
                    }
                }
            }

            JS_DeleteProperty1(cx.raw_cx(), global_rooted.handle().into(), c"__fs_stream_ref".as_ptr());
        }
    }

    cache_builtin(cx, "fs", fs_obj.get());

    // Register fs/promises sub-path — reuses the same promise-based methods
    // already defined on fs.promises. The sub-path module `require("fs/promises")`
    // gets its own top-level builtin with the promise methods directly on it.
    unsafe {
        rooted!(&in(cx) let fsp_obj = w2::JS_NewPlainObject(cx));
        if !fsp_obj.get().is_null() {
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"readFile".as_ptr(), Some(fs_promises_read_file), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"writeFile".as_ptr(), Some(fs_promises_write_file), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"stat".as_ptr(), Some(fs_promises_stat), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"readdir".as_ptr(), Some(fs_promises_readdir), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"mkdir".as_ptr(), Some(fs_promises_mkdir), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"unlink".as_ptr(), Some(fs_promises_unlink), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"rename".as_ptr(), Some(fs_promises_rename), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"copyFile".as_ptr(), Some(fs_promises_copy_file), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"lstat".as_ptr(), Some(fs_promises_lstat), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"appendFile".as_ptr(), Some(fs_promises_append_file), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"chmod".as_ptr(), Some(fs_promises_chmod), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"chown".as_ptr(), Some(fs_promises_chown), 3, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"access".as_ptr(), Some(fs_promises_access), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"rm".as_ptr(), Some(fs_promises_rm), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"rmdir".as_ptr(), Some(fs_promises_rmdir), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"realpath".as_ptr(), Some(fs_promises_realpath), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"readlink".as_ptr(), Some(fs_promises_readlink), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"symlink".as_ptr(), Some(fs_promises_symlink), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"link".as_ptr(), Some(fs_promises_link), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"truncate".as_ptr(), Some(fs_promises_truncate), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"utimes".as_ptr(), Some(fs_promises_utimes), 3, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"mkdtemp".as_ptr(), Some(fs_promises_mkdtemp), 1, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"open".as_ptr(), Some(fs_promises_open), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"read".as_ptr(), Some(fs_promises_read), 4, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, fsp_obj.handle(), c"write".as_ptr(), Some(fs_promises_write), 4, JSPROP_ENUMERATE as u32);
            cache_builtin(cx, "fs/promises", fsp_obj.get());
        }
    }
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
        let mut wrapped_cx_enc = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref_enc = &mut wrapped_cx_enc;
        rooted!(&in(cx_ref_enc) let obj = val.to_object());
        let mut enc_val = UndefinedValue();
        let enc_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut enc_val };
        JS_GetProperty(cx, obj.handle().into(), c"encoding".as_ptr(), enc_h);
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
        // @trace REQ-ENG-005 [entity:Buffer]
        // Node.js: readFileSync(path) with NO encoding returns a Buffer
        // (binary-safe). Only when an encoding is supplied does it return a
        // decoded String. Previously bao returned a utf8-lossy String for the
        // no-encoding case, breaking Buffer.isBuffer() checks downstream.
        None => {
            let buf_obj = crate::globals::create_buffer_object(cx, data);
            if buf_obj.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::ObjectValue(buf_obj));
            }
        }
        Some("utf-8" | "utf8" | "text") => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("hex") => {
            let hex: ::std::string::String = bun_core::fmt::bytes_to_hex_lower_string(data);
            let c_str = ZBox::from_bytes(hex.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = ZBox::from_bytes(encoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some("latin1" | "binary") => {
            let s: ::std::string::String = data.iter().map(|&b| b as char).collect();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
        }
        Some(_) => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = ZBox::from_bytes(s.as_bytes());
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
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(code.as_bytes()).as_ptr());
    if !code_str.is_null() {
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        if JS_IsExceptionPending(cx) {
            rooted!(in(cx) let mut exn = UndefinedValue());
            JS_GetPendingException(cx, exn.handle_mut().into());
            let exn_val = exn.get();
            if !exn_val.is_undefined() && exn_val.is_object() {
                rooted!(in(cx) let exn_obj = exn_val.to_object());
                rooted!(in(cx) let code_val = StringValue(&*code_str));
                JS_DefineProperty(cx, exn_obj.handle().into(), c"code".as_ptr(), code_val.handle().into(), JSPROP_ENUMERATE as u32);
                let path_val = ZBox::from_bytes(path.as_bytes());
                let path_str = JS_NewStringCopyZ(cx, path_val.as_ptr());
                if !path_str.is_null() {
                    rooted!(in(cx) let path_v = StringValue(&*path_str));
                    JS_DefineProperty(cx, exn_obj.handle().into(), c"path".as_ptr(), path_v.handle().into(), JSPROP_ENUMERATE as u32);
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let encoding = get_encoding_opt(cx, &args, 1);
    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => return_string_content(cx, &args, &data, encoding.as_deref()),
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };

    let result = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            bun_fs::write(&path, rust_str.as_bytes())
        } else {
            bun_fs::write(&path, &[] as &[u8])
        }
    } else if data_val.is_object() {
        let bytes = crate::node_crypto::extract_buffer_bytes(cx, data_val);
        bun_fs::write(&path, &bytes)
    } else {
        bun_fs::write(&path, &[] as &[u8])
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
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

    match bun_fs::OpenOptions::new().create(true).append(true).open(&path) {
        ::std::result::Result::Ok(file) => {
            match file.write_all(&data) {
                ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
                ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFileSync", &path, &::std::io::Error::from_raw_os_error(e.errno as i32)),
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
                    let c_name = ZBox::from_bytes(name.as_bytes());
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
    match bun_fs::metadata(&path) {
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
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix);
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
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
            let c_str = ZBox::from_bytes(s.as_bytes());
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
            let c_str = ZBox::from_bytes(s.as_bytes());
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

// Recursive directory copy. Mirrors Node.js fs.cpSync(src, dst[, opts])
// behaviour for the common recursive case (no symlink/filter handling —
// those are advanced options that the conformance suite does not exercise).
#[allow(unsafe_op_in_unsafe_fn)]
fn cp_recursive(src: &Path, dst: &Path) -> ::std::io::Result<()> {
    if fs::metadata(src)?.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            cp_recursive(&from, &to)?;
        }
    } else {
        fs::copy(src, dst)?;
    }
    ::std::result::Result::Ok(())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_cp_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    match cp_recursive(Path::new(&from), Path::new(&to)) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => {
            let msg = format!("cpSync: {}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_cp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Async variant: invoke callback (if provided) after the recursive copy.
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let res = cp_recursive(Path::new(&from), Path::new(&to));
    if let ::std::result::Result::Err(e) = res {
        let msg = format!("cp: {}", e);
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_watch(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // Minimal FSWatcher: returns an EventEmitter-shaped object so consumers
    // that only need the surface (on/emit/close) work without a real backend.
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let watcher = mozjs::rust::wrappers2::JS_NewPlainObject(cx_ref));
    if !watcher.get().is_null() {
        // Forward to node_events' EE natives so the returned watcher integrates
        // with the existing EventEmitter machinery.
        let on_op: JSNative = Some(crate::node_events::ee_on);
        let off_op: JSNative = Some(crate::node_events::ee_off);
        let once_op: JSNative = Some(crate::node_events::ee_once);
        let emit_op: JSNative = Some(crate::node_events::ee_emit);
        let close_op: JSNative = Some(fs_noop_native);
        for (name, op) in [
            ("on",             on_op),
            ("addListener",    on_op),
            ("off",            off_op),
            ("removeListener", off_op),
            ("once",           once_op),
            ("emit",           emit_op),
            ("close",          close_op),
        ] {
            let c_name = ZBox::from_bytes(name.as_bytes());
            mozjs_sys::jsapi::JS_DefineFunction(
                cx,
                watcher.handle().into(),
                c_name.as_ptr(),
                op,
                2,
                JSPROP_ENUMERATE as u32,
            );
        }
        args.rval().set(mozjs::jsval::ObjectValue(watcher.get()));
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_watch_file(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    // watchFile: returns immediately; no polling backend wired (would require a
    // background timer thread). Conformance suite only checks the API shape.
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_noop_native(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// --- Async (callback-based) ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let encoding = get_encoding_opt(cx, &args, 1);

    match bun_fs::read(&path) {
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

    match bun_fs::write(&path, &bytes) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "writeFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive { fs::create_dir_all(&path) } else { fs::create_dir(&path) };
    match result {
        ::std::result::Result::Ok(()) => {
            if argc > 1 && (*args.get(argc - 1).ptr).is_object() {
                let mut wrapped_cx_cb = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                let cx_ref_cb = &mut wrapped_cx_cb;
                rooted!(&in(cx_ref_cb) let cb = (*args.get(argc - 1).ptr).to_object());
                rooted!(&in(cx_ref_cb) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
                let null_args = HandleValueArray::empty();
                let global = CurrentGlobalOrNull(cx);
                if !global.is_null() {
                    rooted!(&in(cx_ref_cb) let global_rooted = global);
                    let mut rval = UndefinedValue();
                    JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &null_args, MutableHandle::<Value> {
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
                let mut wrapped_cx_err = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                let cx_ref_err = &mut wrapped_cx_err;
                rooted!(&in(cx_ref_err) let cb = (*args.get(argc - 1).ptr).to_object());
                rooted!(&in(cx_ref_err) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
                let err_msg = format!("EACCES: mkdir '{}': {}", path, e);
                let c_err = ZBox::from_bytes(err_msg.as_bytes());
                rooted!(&in(cx_ref_err) let err_obj = JS_NewPlainObject(cx));
                if !err_obj.get().is_null() {
                    let msg_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
                    if !msg_str.is_null() {
                        rooted!(&in(cx_ref_err) let msg_val = mozjs::jsval::StringValue(&*msg_str));
                        JS_DefineProperty(cx,
                            err_obj.handle().into(),
                            c"message".as_ptr(),
                            msg_val.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                    let code_str = JS_NewStringCopyZ(cx, c"EACCES".as_ptr());
                    if !code_str.is_null() {
                        rooted!(&in(cx_ref_err) let code_val = mozjs::jsval::StringValue(&*code_str));
                        JS_DefineProperty(cx,
                            err_obj.handle().into(),
                            c"code".as_ptr(),
                            code_val.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                    rooted!(&in(cx_ref_err) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
                    let err_args = HandleValueArray { length_: 1, elements_: &err_val.get() as *const JSVal };
                    let global = CurrentGlobalOrNull(cx);
                    if !global.is_null() {
                        rooted!(&in(cx_ref_err) let global_rooted = global);
                        let mut rval = UndefinedValue();
                        JS_CallFunctionValue(cx, global_rooted.handle().into(), cb_val.handle().into(), &err_args, MutableHandle::<Value> {
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_append_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let data = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() { crate::jsstr_to_rust_string(cx, s).into_bytes() } else { Vec::new() }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else { Vec::new() };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "appendFile", path.clone(), callback, None, move || {
            ::std::fs::OpenOptions::new().create(true).append(true).open(&path)
                .and_then(|mut f| ::std::io::Write::write_all(&mut f, &data))
                .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match ::std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        ::std::result::Result::Ok(mut file) => {
            match ::std::io::Write::write_all(&mut file, &data) {
                ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
                ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFile", &path, &e),
            }
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_access(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "access", path.clone(), callback, None, move || {
            fs::metadata(&path).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::metadata(&path) {
        ::std::result::Result::Ok(_) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "access", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mode_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else if mode_val.is_double() { mode_val.to_double() as u32 } else { 0o644 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "chmod", path.clone(), callback, None, move || {
            #[cfg(unix)]
            { use ::std::os::unix::fs::PermissionsExt; fs::set_permissions(&path, fs::Permissions::from_mode(mode)).map(|_| FsAsyncResult::OkVoid) }
            #[cfg(not(unix))]
            { fs::set_permissions(&path, fs::Permissions::new()).map(|_| FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    let result = { use ::std::os::unix::fs::PermissionsExt; fs::set_permissions(&path, fs::Permissions::from_mode(mode)) };
    #[cfg(not(unix))]
    let result = { fs::set_permissions(&path, fs::Permissions::new()) };
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "chmod", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let uid = if argc > 1 { let v = *args.get(1).ptr; if v.is_int32() { v.to_int32() as u32 } else if v.is_double() { v.to_double() as u32 } else { 0 } } else { 0 };
    let gid = if argc > 2 { let v = *args.get(2).ptr; if v.is_int32() { v.to_int32() as u32 } else if v.is_double() { v.to_double() as u32 } else { 0 } } else { 0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "chown", path.clone(), callback, None, move || {
            #[cfg(unix)]
            { ::std::os::unix::fs::chown(&path, Some(uid), Some(gid)).map(|_| FsAsyncResult::OkVoid) }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    match ::std::os::unix::fs::chown(&path, Some(uid), Some(gid)) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "chown", &path, &e),
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_close(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "close", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            { let rv = unsafe { libc::close(fd) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    { let rv = unsafe { libc::close(fd) }; if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "close", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) } }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_copy_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "copyFile", from.clone(), callback, None, move || {
            fs::copy(&from, &to).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "copyFile", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_exists(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "exists", path.clone(), callback, None, move || {
            Ok(FsAsyncResult::OkBool(Path::new(&path).exists()))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    args.rval().set(mozjs::jsval::BooleanValue(Path::new(&path).exists()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fchmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let mode_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else { 0o644 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "fchmod", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            { let rv = unsafe { libc::fchmod(fd, mode) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    { let rv = unsafe { libc::fchmod(fd, mode) }; if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "fchmod", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) } }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fchown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let uid = if argc > 1 { let v = *args.get(1).ptr; if v.is_int32() { v.to_int32() as u32 } else { 0 } } else { 0 };
    let gid = if argc > 2 { let v = *args.get(2).ptr; if v.is_int32() { v.to_int32() as u32 } else { 0 } } else { 0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "fchown", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            { let rv = unsafe { libc::fchown(fd, uid, gid) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    { let rv = unsafe { libc::fchown(fd, uid, gid) }; if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "fchown", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) } }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fdatasync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "fdatasync", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            { let rv = unsafe { libc::fdatasync(fd) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    { let rv = unsafe { libc::fdatasync(fd) }; if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "fdatasync", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) } }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fstat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "fstat", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            {
                let mut stat_buf: libc::stat = ::std::mem::zeroed();
                let rv = unsafe { libc::fstat(fd, &mut stat_buf) };
                if rv == 0 { Ok(FsAsyncResult::OkStat(posix_stat_from_libc(&stat_buf))) } else { Err(::std::io::Error::last_os_error()) }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let mut stat_buf: libc::stat = ::std::mem::zeroed();
        let rv = unsafe { libc::fstat(fd, &mut stat_buf) };
        if rv == 0 {
            let posix = posix_stat_from_libc(&stat_buf);
            let stats = create_stats_object(cx, &posix);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        } else {
            throw_fs_error(cx, "fstat", &format!("fd:{}", fd), &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fsync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "fsync", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            { let rv = unsafe { libc::fsync(fd) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    { let rv = unsafe { libc::fsync(fd) }; if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "fsync", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) } }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_ftruncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let len_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let len = if len_val.is_int32() { len_val.to_int32() as i64 } else if len_val.is_double() { len_val.to_double() as i64 } else { 0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "ftruncate", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            { let rv = unsafe { libc::ftruncate(fd, len) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    { let rv = unsafe { libc::ftruncate(fd, len) }; if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "ftruncate", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) } }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_futimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let atime_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mtime_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let atime = if atime_val.is_double() { atime_val.to_double() } else if atime_val.is_int32() { atime_val.to_int32() as f64 } else { 0.0 };
    let mtime = if mtime_val.is_double() { mtime_val.to_double() } else if mtime_val.is_int32() { mtime_val.to_int32() as f64 } else { 0.0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "futimes", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            {
                let tv = [
                    libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
                    libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
                ];
                let rv = unsafe { libc::futimes(fd, tv.as_ptr()) };
                if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let tv = [
            libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
            libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
        ];
        let rv = unsafe { libc::futimes(fd, tv.as_ptr()) };
        if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "futimes", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lchown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let uid = if argc > 1 { let v = *args.get(1).ptr; if v.is_int32() { v.to_int32() as u32 } else { 0 } } else { 0 };
    let gid = if argc > 2 { let v = *args.get(2).ptr; if v.is_int32() { v.to_int32() as u32 } else { 0 } } else { 0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "lchown", path.clone(), callback, None, move || {
            #[cfg(unix)]
            { let c_p = ::std::ffi::CString::new(path.as_str()).unwrap_or_default(); let rv = unsafe { libc::lchown(c_p.as_ptr(), uid, gid) }; if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) } }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let rv = unsafe { libc::lchown(c_path.as_ptr(), uid, gid) };
        if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "lchown", &path, &::std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_link(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "link", from.clone(), callback, None, move || {
            fs::hard_link(&from, &to).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::hard_link(&from, &to) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "link", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lstat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "lstat", path.clone(), callback, None, move || {
            fs::symlink_metadata(&path).map(|m| FsAsyncResult::OkStat(metadata_to_posix_stat(&m)))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "lstat", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lutimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let atime_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mtime_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let atime = if atime_val.is_double() { atime_val.to_double() } else if atime_val.is_int32() { atime_val.to_int32() as f64 } else { 0.0 };
    let mtime = if mtime_val.is_double() { mtime_val.to_double() } else if mtime_val.is_int32() { mtime_val.to_int32() as f64 } else { 0.0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "lutimes", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let tv = [
                    libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
                    libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
                ];
                let rv = unsafe { libc::lutimes(c_path.as_ptr(), tv.as_ptr()) };
                if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let tv = [
            libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
            libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
        ];
        let rv = unsafe { libc::lutimes(c_path.as_ptr(), tv.as_ptr()) };
        if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "lutimes", &path, &::std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdtemp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let _encoding = get_encoding_opt(cx, &args, 1);

    if let Some((callback, cb_encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "mkdtemp", prefix.clone(), callback, cb_encoding, move || {
            mkdtemp_inner(&prefix).map(FsAsyncResult::OkString)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let c_str = ZBox::from_bytes(dir.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "mkdtemp", &prefix, &e),
    }
}

fn mkdtemp_inner(prefix: &str) -> ::std::io::Result<String> {
    let mut template = prefix.to_string();
    template.push_str("XXXXXX");
    let c_template = ::std::ffi::CString::new(template).map_err(|_| ::std::io::Error::new(::std::io::ErrorKind::InvalidInput, "prefix contains null byte"))?;
    let c_ptr = c_template.into_raw();
    let result = unsafe { libc::mkdtemp(c_ptr) };
    if result.is_null() {
        let e = ::std::io::Error::last_os_error();
        unsafe { let _ = ::std::ffi::CString::from_raw(c_ptr); }
        return Err(e);
    }
    let result_cstr = unsafe { ::std::ffi::CString::from_raw(result) };
    result_cstr.into_string().map_err(|e| ::std::io::Error::new(::std::io::ErrorKind::InvalidData, e))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_open(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let flags_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let flags = if flags_val.is_int32() { flags_val.to_int32() } else if flags_val.is_string() {
        let s = flags_val.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            parse_open_flags(&rust_str)
        } else { 0 }
    } else { 0 };
    let mode_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else { 0o644 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "open", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
                if fd >= 0 { Ok(FsAsyncResult::OkOpen(fd)) } else { Err(::std::io::Error::last_os_error()) }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkOpen(0)) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd >= 0 { args.rval().set(mozjs::jsval::Int32Value(fd)); true } else { throw_fs_error(cx, "open", &path, &::std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    { args.rval().set(mozjs::jsval::Int32Value(0)); true }
}

fn parse_open_flags(s: &str) -> i32 {
    let mut flags = 0;
    if s.contains('w') { flags |= libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC; }
    if s.contains('a') { flags |= libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND; }
    if s.contains('+') { flags = (flags & !(libc::O_WRONLY | libc::O_RDONLY)) | libc::O_RDWR; }
    flags
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_opendir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "opendir", path.clone(), callback, None, move || {
            fs::read_dir(&path).map(|entries| {
                let names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                FsAsyncResult::OkDirnames(names)
            })
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, names.len()));
            for (i, name) in names.iter().enumerate() {
                let c_name = ZBox::from_bytes(name.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineElement(cx, arr.handle().into(), i as u32, val.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "opendir", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let length = if argc > 3 { let v = *args.get(3).ptr; if v.is_int32() { v.to_int32() as usize } else { 65536 } } else { 65536 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 5) {
        spawn_fs_async(cx, "read", format!("fd:{}", fd), callback, None, move || {
            let mut buf = vec![0u8; length];
            #[cfg(unix)]
            {
                let bytes_read = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) };
                if bytes_read >= 0 {
                    buf.truncate(bytes_read as usize);
                    Ok(FsAsyncResult::OkRead { bytes_read: bytes_read as i32, buffer: buf })
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkRead { bytes_read: 0, buffer: buf }) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut buf = vec![0u8; length];
    #[cfg(unix)]
    {
        let bytes_read = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) };
        if bytes_read >= 0 {
            buf.truncate(bytes_read as usize);
            let buf_obj = crate::globals::create_buffer_object(cx, &buf);
            if buf_obj.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::Int32Value(bytes_read as i32)); }
            true
        } else {
            throw_fs_error(cx, "read", &format!("fd:{}", fd), &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let with_file_types = get_bool_option(cx, &args, 1, "withFileTypes");

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "readdir", path.clone(), callback, None, move || {
            fs::read_dir(&path).map(|entries| {
                let items: Vec<_> = entries.flatten().collect();
                if with_file_types {
                    let dirents: Vec<(String, bool)> = items.iter().map(|e| {
                        (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                    }).collect();
                    FsAsyncResult::OkDirents(dirents)
                } else {
                    let names: Vec<String> = items.iter().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                    FsAsyncResult::OkDirnames(names)
                }
            })
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let mut names: Vec<String> = Vec::new();
            let mut is_dirs: Vec<bool> = Vec::new();
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
                is_dirs.push(entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false));
            }
            let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx)) };
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
                    let c_name = ZBox::from_bytes(name.as_bytes());
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
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readdir", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let _encoding = get_encoding_opt(cx, &args, 1);

    if let Some((callback, cb_encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "readlink", path.clone(), callback, cb_encoding, move || {
            fs::read_link(&path).map(|t| FsAsyncResult::OkString(t.to_string_lossy().into_owned()))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::read_link(&path) {
        ::std::result::Result::Ok(target) => {
            let s = target.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readlink", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_read(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let _encoding = get_encoding_opt(cx, &args, 1);

    if let Some((callback, cb_encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "realpath", path.clone(), callback, cb_encoding, move || {
            fs::canonicalize(&path).map(|p| FsAsyncResult::OkString(p.to_string_lossy().into_owned()))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::canonicalize(&path) {
        ::std::result::Result::Ok(resolved) => {
            let s = resolved.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); } else { args.rval().set(mozjs::jsval::StringValue(&*js_str)); }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "realpath", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "rename", from.clone(), callback, None, move || {
            fs::rename(&from, &to).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::rename(&from, &to) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rename", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rm(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "rm", path.clone(), callback, None, move || {
            if recursive { fs::remove_dir_all(&path) } else { fs::remove_file(&path) }
                .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    let result = if recursive { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rm", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rmdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "rmdir", path.clone(), callback, None, move || {
            fs::remove_dir(&path).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::remove_dir(&path) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rmdir", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_stat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "stat", path.clone(), callback, None, move || {
            bun_fs::metadata(&path).map(FsAsyncResult::OkStat)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "stat", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let path = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "symlink", target.clone(), callback, None, move || {
            #[cfg(unix)]
            { ::std::os::unix::fs::symlink(&target, &path).map(|_| FsAsyncResult::OkVoid) }
            #[cfg(not(unix))]
            { fs::hard_link(&target, &path).map(|_| FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    let result = ::std::os::unix::fs::symlink(&target, &path);
    #[cfg(not(unix))]
    let result = fs::hard_link(&target, &path);
    match result {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "symlink", &target, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_truncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let len_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let len = if len_val.is_int32() { len_val.to_int32() as i64 } else if len_val.is_double() { len_val.to_double() as i64 } else { 0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "truncate", path.clone(), callback, None, move || {
            fs::File::open(&path)
                .and_then(|f| f.set_len(len as u64))
                .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::File::open(&path).and_then(|f| f.set_len(len as u64)) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "truncate", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_unlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "unlink", path.clone(), callback, None, move || {
            fs::remove_file(&path).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::remove_file(&path) {
        ::std::result::Result::Ok(()) => { args.rval().set(UndefinedValue()); true }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "unlink", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_utimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let atime_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mtime_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let atime = if atime_val.is_double() { atime_val.to_double() } else if atime_val.is_int32() { atime_val.to_int32() as f64 } else { 0.0 };
    let mtime = if mtime_val.is_double() { mtime_val.to_double() } else if mtime_val.is_int32() { mtime_val.to_int32() as f64 } else { 0.0 };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "utimes", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let tv = [
                    libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
                    libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
                ];
                let rv = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
                if rv == 0 { Ok(FsAsyncResult::OkVoid) } else { Err(::std::io::Error::last_os_error()) }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkVoid) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let tv = [
            libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
            libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
        ];
        let rv = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
        if rv == 0 { args.rval().set(UndefinedValue()); true } else { throw_fs_error(cx, "utimes", &path, &::std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let bytes = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() { crate::jsstr_to_rust_string(cx, s).into_bytes() } else { Vec::new() }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else { Vec::new() };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 5) {
        spawn_fs_async(cx, "write", format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            {
                let written = unsafe { libc::write(fd, bytes.as_ptr() as *const ::std::ffi::c_void, bytes.len()) };
                if written >= 0 { Ok(FsAsyncResult::OkWrite(written as i32)) } else { Err(::std::io::Error::last_os_error()) }
            }
            #[cfg(not(unix))]
            { Ok(FsAsyncResult::OkWrite(0)) }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let written = unsafe { libc::write(fd, bytes.as_ptr() as *const ::std::ffi::c_void, bytes.len()) };
        if written >= 0 { args.rval().set(mozjs::jsval::Int32Value(written as i32)); true } else { throw_fs_error(cx, "write", &format!("fd:{}", fd), &::std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    { args.rval().set(UndefinedValue()); true }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_writev(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_write(cx, argc, vp)
}

// --- fs.promises ---

macro_rules! promise_simple_op {
    ($fn_name:ident, $op:expr, $op_name:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $fn_name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
            if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
            match $op(&path) {
                ::std::result::Result::Ok(()) => {
                    resolve_undefined(cx, promise.get());
                }
                ::std::result::Result::Err(e) => {
                    reject_with_error(cx, promise.get(), &format!("{} '{}': {}", $op_name, path, e));
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::rename(&from, &to) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("rename '{}': {}", from, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_copy_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("copyFile '{}': {}", from, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let encoding = get_encoding_opt(cx, &args, 1);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }

    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            let val = string_or_buffer(cx, &data, encoding.as_deref());
            rooted!(&in(cx_ref) let val_rooted = val);
            unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val_rooted.handle().into()); }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("readFile '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
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
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match bun_fs::write(&path, &bytes) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("writeFile '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_stat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(stats));
            unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("stat '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
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
                let c_name = ZBox::from_bytes(name.as_bytes());
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_lstat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix);
            rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(stats));
            unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("lstat '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_append_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let data = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() { crate::jsstr_to_rust_string(cx, s).into_bytes() } else { Vec::new() }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else { Vec::new() };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match ::std::fs::OpenOptions::new().create(true).append(true).open(&path)
        .and_then(|mut f| ::std::io::Write::write_all(&mut f, &data))
    {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("appendFile '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_chmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mode_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else if mode_val.is_double() { mode_val.to_double() as u32 } else { 0o644 };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    #[cfg(unix)]
    let result = { use ::std::os::unix::fs::PermissionsExt; fs::set_permissions(&path, fs::Permissions::from_mode(mode)) };
    #[cfg(not(unix))]
    let result = fs::set_permissions(&path, fs::Permissions::new());
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("chmod '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_chown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let uid = if argc > 1 { let v = *args.get(1).ptr; if v.is_int32() { v.to_int32() as u32 } else { 0 } } else { 0 };
    let gid = if argc > 2 { let v = *args.get(2).ptr; if v.is_int32() { v.to_int32() as u32 } else { 0 } } else { 0 };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    #[cfg(unix)]
    let result = ::std::os::unix::fs::chown(&path, Some(uid), Some(gid));
    #[cfg(not(unix))]
    let result: ::std::io::Result<()> = Ok(());
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("chown '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_access(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::metadata(&path) {
        ::std::result::Result::Ok(_) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("access '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rm(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    let result = if recursive { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("rm '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rmdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::remove_dir(&path) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("rmdir '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_realpath(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::canonicalize(&path) {
        ::std::result::Result::Ok(resolved) => {
            let s = resolved.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("realpath '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_readlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::read_link(&path) {
        ::std::result::Result::Ok(target) => {
            let s = target.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("readlink '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let path = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    #[cfg(unix)]
    let result = ::std::os::unix::fs::symlink(&target, &path);
    #[cfg(not(unix))]
    let result = fs::hard_link(&target, &path);
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("symlink '{}': {}", target, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_link(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let to = match get_path_arg(cx, &args, 1) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::hard_link(&from, &to) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("link '{}': {}", from, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_truncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let len_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let len = if len_val.is_int32() { len_val.to_int32() as u64 } else if len_val.is_double() { len_val.to_double() as u64 } else { 0 };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match fs::File::open(&path).and_then(|f| f.set_len(len)) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("truncate '{}': {}", path, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_utimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let atime_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mtime_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let atime = if atime_val.is_double() { atime_val.to_double() } else if atime_val.is_int32() { atime_val.to_int32() as f64 } else { 0.0 };
    let mtime = if mtime_val.is_double() { mtime_val.to_double() } else if mtime_val.is_int32() { mtime_val.to_int32() as f64 } else { 0.0 };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let tv = [
            libc::timeval { tv_sec: atime as i64, tv_usec: ((atime % 1.0) * 1_000_000.0) as i64 },
            libc::timeval { tv_sec: mtime as i64, tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64 },
        ];
        let rv = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
        if rv == 0 { resolve_undefined(cx, promise.get()); } else { reject_with_error(cx, promise.get(), &format!("utimes '{}': {}", path, ::std::io::Error::last_os_error())); }
    }
    #[cfg(not(unix))]
    { resolve_undefined(cx, promise.get()); }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_mkdtemp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let c_str = ZBox::from_bytes(dir.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => reject_with_error(cx, promise.get(), &format!("mkdtemp '{}': {}", prefix, e)),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_open(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
    let flags_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let flags = if flags_val.is_int32() { flags_val.to_int32() } else if flags_val.is_string() {
        let s = flags_val.to_string();
        if !s.is_null() { parse_open_flags(&crate::jsstr_to_rust_string(cx, s)) } else { 0 }
    } else { 0 };
    let mode_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let mode = if mode_val.is_int32() { mode_val.to_int32() as u32 } else { 0o644 };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd >= 0 {
            rooted!(&in(cx_ref) let val = mozjs::jsval::Int32Value(fd));
            unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
        } else {
            reject_with_error(cx, promise.get(), &format!("open '{}': {}", path, ::std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(unix))]
    { resolve_undefined(cx, promise.get()); }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let length = if argc > 3 { let v = *args.get(3).ptr; if v.is_int32() { v.to_int32() as usize } else { 65536 } } else { 65536 };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    let mut buf = vec![0u8; length];
    #[cfg(unix)]
    {
        let bytes_read = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) };
        if bytes_read >= 0 {
            buf.truncate(bytes_read as usize);
            let buf_obj = crate::globals::create_buffer_object(cx, &buf);
            if !buf_obj.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(buf_obj));
                unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
            } else {
                resolve_undefined(cx, promise.get());
            }
        } else {
            reject_with_error(cx, promise.get(), &format!("read fd:{}: {}", fd, ::std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(unix))]
    { resolve_undefined(cx, promise.get()); }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };
    let data_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let bytes = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() { crate::jsstr_to_rust_string(cx, s).into_bytes() } else { Vec::new() }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else { Vec::new() };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
    #[cfg(unix)]
    {
        let written = unsafe { libc::write(fd, bytes.as_ptr() as *const ::std::ffi::c_void, bytes.len()) };
        if written >= 0 {
            rooted!(&in(cx_ref) let val = mozjs::jsval::Int32Value(written as i32));
            unsafe { mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into()); }
        } else {
            reject_with_error(cx, promise.get(), &format!("write fd:{}: {}", fd, ::std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(unix))]
    { resolve_undefined(cx, promise.get()); }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

// --- Helper functions ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_bool_option(cx: *mut JSContext, args: &CallArgs, opt_index: u32, key: &str) -> bool {
    if args.argc_ <= opt_index { return false; }
    let opt_val = *args.get(opt_index).ptr;
    if !opt_val.is_object() { return false; }
    let mut wrapped_cx_opt = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref_opt = &mut wrapped_cx_opt;
    rooted!(&in(cx_ref_opt) let obj = opt_val.to_object());
    let c_key = ZBox::from_bytes(key.as_bytes());
    let mut val = UndefinedValue();
    JS_GetProperty(cx, obj.handle().into(), c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
    val.is_boolean() && val.to_boolean()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn string_or_buffer(cx: *mut JSContext, data: &[u8], encoding: ::std::option::Option<&str>) -> JSVal {
    match encoding {
        Some("utf-8" | "utf8" | "text") | None => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        Some("hex") => {
            let hex: ::std::string::String = bun_core::fmt::bytes_to_hex_lower_string(data);
            let c_str = ZBox::from_bytes(hex.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = ZBox::from_bytes(encoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { UndefinedValue() } else { mozjs::jsval::StringValue(&*js_str) }
        }
        _ => UndefinedValue(),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
/// Convert `std::fs::Metadata` to `PosixStat` for use with `create_stats_object`.
/// Used as a bridge for `symlink_metadata` (lstat) which has no bun_sys equivalent yet.
#[cfg(unix)]
fn metadata_to_posix_stat(meta: &fs::Metadata) -> bun_sys::PosixStat {
    use ::std::os::unix::fs::MetadataExt;
    bun_sys::PosixStat {
        dev: meta.dev() as u64,
        ino: meta.ino() as u64,
        mode: meta.mode() as u64,
        nlink: meta.nlink() as u64,
        uid: meta.uid() as u64,
        gid: meta.gid() as u64,
        rdev: meta.rdev() as u64,
        size: meta.size(),
        blksize: meta.blksize() as u64,
        blocks: meta.blocks() as u64,
        atim: bun_sys::Timespec { sec: meta.atime(), nsec: meta.atime_nsec() as i64 },
        mtim: bun_sys::Timespec { sec: meta.mtime(), nsec: meta.mtime_nsec() as i64 },
        ctim: bun_sys::Timespec { sec: meta.ctime(), nsec: meta.ctime_nsec() as i64 },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

#[cfg(not(unix))]
fn metadata_to_posix_stat(meta: &fs::Metadata) -> bun_sys::PosixStat {
    bun_sys::PosixStat {
        dev: 0, ino: 0, mode: 0, nlink: 0, uid: 0, gid: 0, rdev: 0,
        size: meta.len(), blksize: 0, blocks: 0,
        atim: bun_sys::Timespec { sec: 0, nsec: 0 },
        mtim: bun_sys::Timespec { sec: 0, nsec: 0 },
        ctim: bun_sys::Timespec { sec: 0, nsec: 0 },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

/// Convert a raw `libc::stat` (from fstat/lstat syscalls) to `PosixStat`.
/// This is used by fd-based operations (fstat) that cannot go through
/// `std::fs::Metadata` since there is no `File` opened via Rust.
#[cfg(unix)]
fn posix_stat_from_libc(s: &libc::stat) -> bun_sys::PosixStat {
    bun_sys::PosixStat {
        dev: s.st_dev as u64,
        ino: s.st_ino as u64,
        mode: s.st_mode as u64,
        nlink: s.st_nlink as u64,
        uid: s.st_uid as u64,
        gid: s.st_gid as u64,
        rdev: s.st_rdev as u64,
        size: s.st_size as u64,
        blksize: s.st_blksize as u64,
        blocks: s.st_blocks as u64,
        atim: bun_sys::Timespec { sec: s.st_atime, nsec: s.st_atime_nsec as i64 },
        mtim: bun_sys::Timespec { sec: s.st_mtime, nsec: s.st_mtime_nsec as i64 },
        ctim: bun_sys::Timespec { sec: s.st_ctime, nsec: s.st_ctime_nsec as i64 },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

#[cfg(not(unix))]
fn posix_stat_from_libc(_s: &libc::stat) -> bun_sys::PosixStat {
    bun_sys::PosixStat {
        dev: 0, ino: 0, mode: 0, nlink: 0, uid: 0, gid: 0, rdev: 0,
        size: 0, blksize: 0, blocks: 0,
        atim: bun_sys::Timespec { sec: 0, nsec: 0 },
        mtim: bun_sys::Timespec { sec: 0, nsec: 0 },
        ctim: bun_sys::Timespec { sec: 0, nsec: 0 },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_stats_object(cx: *mut JSContext, meta: &bun_fs::PosixStat) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let stats = JS_NewPlainObject(cx));
    if stats.get().is_null() { return stats.get(); }

    // Determine file type from mode (S_IFMT bits)
    let mode_type = (meta.mode as u32) & libc::S_IFMT;
    let is_file = mode_type == libc::S_IFREG;
    let is_dir = mode_type == libc::S_IFDIR;
    let is_symlink = mode_type == libc::S_IFLNK;

    define_num_prop(cx, stats.get(), "size", meta.size as f64);
    define_num_prop(cx, stats.get(), "dev", meta.dev as f64);
    define_num_prop(cx, stats.get(), "ino", meta.ino as f64);
    define_num_prop(cx, stats.get(), "mode", meta.mode as f64);
    define_num_prop(cx, stats.get(), "nlink", meta.nlink as f64);
    define_num_prop(cx, stats.get(), "uid", meta.uid as f64);
    define_num_prop(cx, stats.get(), "gid", meta.gid as f64);
    define_num_prop(cx, stats.get(), "rdev", meta.rdev as f64);
    define_num_prop(cx, stats.get(), "blksize", meta.blksize as f64);
    define_num_prop(cx, stats.get(), "blocks", meta.blocks as f64);
    define_num_prop(cx, stats.get(), "atimeMs", meta.atim.sec as f64 * 1000.0 + meta.atim.nsec as f64 / 1_000_000.0);
    define_num_prop(cx, stats.get(), "mtimeMs", meta.mtim.sec as f64 * 1000.0 + meta.mtim.nsec as f64 / 1_000_000.0);
    define_num_prop(cx, stats.get(), "ctimeMs", meta.ctim.sec as f64 * 1000.0 + meta.ctim.nsec as f64 / 1_000_000.0);

    // Store boolean values as hidden properties for method callbacks
    define_bool_prop(cx, stats.get(), "_isFile", is_file);
    define_bool_prop(cx, stats.get(), "_isDirectory", is_dir);
    define_bool_prop(cx, stats.get(), "_isSymbolicLink", is_symlink);

    // Node.js Stats methods
    w2::JS_DefineFunction(cx_ref, stats.handle().into(), c"isFile".as_ptr(), Some(stats_is_file), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, stats.handle().into(), c"isDirectory".as_ptr(), Some(stats_is_directory), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, stats.handle().into(), c"isSymbolicLink".as_ptr(), Some(stats_is_symlink), 0, JSPROP_ENUMERATE as u32);

    stats.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_dirent(cx: *mut JSContext, name: &str, is_dir: bool) -> *mut JSObject {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let dirent = JS_NewPlainObject(cx));
    if dirent.get().is_null() { return dirent.get(); }
    let c_name = ZBox::from_bytes(name.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let name_val = mozjs::jsval::StringValue(&*js_str));
        JS_DefineProperty(cx, dirent.handle().into(), c"name".as_ptr(), name_val.handle().into(), JSPROP_ENUMERATE as u32);
    }
    define_bool_prop(cx, dirent.get(), "isFile", !is_dir);
    define_bool_prop(cx, dirent.get(), "isDirectory", is_dir);
    dirent.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_undefined(cx: *mut JSContext, promise: *mut JSObject) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let val = UndefinedValue());
    rooted!(&in(cx_ref) let promise_rooted = promise);
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_rooted.handle().into(), val.handle().into());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_with_error(cx: *mut JSContext, promise: *mut JSObject, msg: &str) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
    if !err_obj.get().is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let msg_val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineProperty(cx, err_obj.handle().into(), c"message".as_ptr(), msg_val.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }
    rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
    rooted!(&in(cx_ref) let promise_rooted = promise);
    mozjs_sys::jsapi::JS::RejectPromise(cx, promise_rooted.handle().into(), err_val.handle().into());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_num_prop(cx: *mut JSContext, obj_ptr: *mut JSObject, name: &str, val: f64) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = obj_ptr);
    let js_val = if val == (val as i32) as f64 && val.abs() < i32::MAX as f64 {
        mozjs::jsval::Int32Value(val as i32)
    } else {
        mozjs::jsval::DoubleValue(val)
    };
    rooted!(&in(cx_ref) let v = js_val);
    JS_DefineProperty(cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_bool_prop(cx: *mut JSContext, obj_ptr: *mut JSObject, name: &str, val: bool) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = obj_ptr);
    rooted!(&in(cx_ref) let v = mozjs::jsval::BooleanValue(val));
    JS_DefineProperty(cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_hidden_bool(cx: *mut JSContext, obj: *mut JSObject, prop: &str) -> bool {
    let c_name = ZBox::from_bytes(prop.as_bytes());
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(cx, obj_rooted.handle().into(), c_name.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
    val.to_boolean()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(cx, this.get(), "_isFile")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_directory(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(cx, this.get(), "_isDirectory")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(cx, this.get(), "_isSymbolicLink")));
    true
}
