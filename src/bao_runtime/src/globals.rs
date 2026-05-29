use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1,
};

pub fn install_bun_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let bun_obj = JS_NewPlainObject(cx));
        if bun_obj.get().is_null() {
            return;
        }

        let version_str = JS_NewStringCopyZ(
            cx.raw_cx(),
            b"0.1.0\0".as_ptr() as *const ::std::os::raw::c_char,
        );
        if !version_str.is_null() {
            rooted!(&in(cx) let ver_val = StringValue(&*version_str));
            JS_DefineProperty(
                cx.raw_cx(),
                bun_obj.handle().into(),
                c"version".as_ptr(),
                ver_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"env".as_ptr(),
            ::std::option::Option::Some(bun_env),
            0,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"file".as_ptr(),
            ::std::option::Option::Some(bun_file),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"write".as_ptr(),
            ::std::option::Option::Some(bun_write),
            2,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            bun_obj.handle(),
            c"readFile".as_ptr(),
            ::std::option::Option::Some(bun_read_file),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(
            cx,
            global,
            c"Bun".as_ptr(),
            bun_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(
            cx,
            global,
            c"Bao".as_ptr(),
            bun_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

pub fn install_process_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let proc_obj = JS_NewPlainObject(cx));
        if proc_obj.get().is_null() {
            return;
        }

        let arch_cstr = ::std::ffi::CString::new(::std::env::consts::ARCH).unwrap_or_default();
        let arch_str = JS_NewStringCopyZ(
            cx.raw_cx(),
            arch_cstr.as_ptr(),
        );
        if !arch_str.is_null() {
            rooted!(&in(cx) let arch_val = StringValue(&*arch_str));
            JS_DefineProperty(
                cx.raw_cx(),
                proc_obj.handle().into(),
                c"arch".as_ptr(),
                arch_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        let plat_cstr = ::std::ffi::CString::new(::std::env::consts::OS).unwrap_or_default();
        let platform_str = JS_NewStringCopyZ(
            cx.raw_cx(),
            plat_cstr.as_ptr(),
        );
        if !platform_str.is_null() {
            rooted!(&in(cx) let plat_val = StringValue(&*platform_str));
            JS_DefineProperty(
                cx.raw_cx(),
                proc_obj.handle().into(),
                c"platform".as_ptr(),
                plat_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        JS_DefineFunction(
            cx,
            proc_obj.handle(),
            c"cwd".as_ptr(),
            ::std::option::Option::Some(process_cwd),
            0,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineFunction(
            cx,
            proc_obj.handle(),
            c"exit".as_ptr(),
            ::std::option::Option::Some(process_exit),
            1,
            JSPROP_ENUMERATE as u32,
        );

        rooted!(&in(cx) let argv_arr = NewArrayObject1(cx, 0));
        if !argv_arr.get().is_null() {
            JS_DefineProperty3(
                cx,
                proc_obj.handle(),
                c"argv".as_ptr(),
                argv_arr.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }

        JS_DefineProperty3(
            cx,
            global,
            c"process".as_ptr(),
            proc_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_env(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let env_obj = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx) };
    if env_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (key, value) in ::std::env::vars() {
        let Ok(c_key) = ::std::ffi::CString::new(key) else { continue };
        let Ok(c_val) = ::std::ffi::CString::new(value) else { continue };
        let val_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
        if !val_str.is_null() {
            let val = StringValue(&*val_str);
            let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &env_obj };
            JS_DefineProperty(cx, obj_handle, c_key.as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(env_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_file(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || args.get(0).ptr.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let path_val = *args.get(0).ptr;
    if !path_val.is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let path_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    let _ = path_str;
    let s = mozjs::conversions::jsstr_to_string(
        cx,
        ::std::ptr::NonNull::new(path_val.to_string()).unwrap(),
    );
    let file_obj = unsafe { mozjs_sys::jsapi::JS_NewPlainObject(cx) };
    if file_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let c_path = match ::std::ffi::CString::new(s.as_str()) {
        Ok(p) => p,
        Err(_) => { args.rval().set(UndefinedValue()); return true; }
    };
    let path_js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
    if !path_js_str.is_null() {
        let val = StringValue(&*path_js_str);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle, c"path".as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
    }
    if let Ok(meta) = ::std::fs::metadata(&s) {
        let size_val = Int32Value(meta.len() as i32);
        let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &size_val };
        let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle, c"size".as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
        let exists_val = mozjs::jsval::BooleanValue(true);
        let val_handle2 = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &exists_val };
        let obj_handle2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &file_obj };
        JS_DefineProperty(cx, obj_handle2, c"exists".as_ptr(), val_handle2, JSPROP_ENUMERATE as u32);
    }
    args.rval().set(mozjs::jsval::ObjectValue(file_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, b"Bun.write requires 2 arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path_val = *args.get(0).ptr;
    let content_val = *args.get(1).ptr;
    if !path_val.is_string() || !content_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Bun.write requires string arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(path_val.to_string()).unwrap());
    let content = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(content_val.to_string()).unwrap());
    match ::std::fs::write(path.as_str(), content.as_bytes()) {
        Ok(()) => {
            let written = Int32Value(content.len() as i32);
            args.rval().set(written);
            true
        }
        Err(e) => {
            let msg = format!("Bun.write failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_read_file(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"Bun.readFile requires a path argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path_val = *args.get(0).ptr;
    if !path_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Bun.readFile requires a string path\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let path = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(path_val.to_string()).unwrap());
    match ::std::fs::read_to_string(path.as_str()) {
        Ok(content) => {
            let Ok(c_content) = ::std::ffi::CString::new(content) else {
                args.rval().set(UndefinedValue());
                return true;
            };
            let js_str = JS_NewStringCopyZ(cx, c_content.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
        Err(e) => {
            let msg = format!("Bun.readFile failed: {}", e);
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_cwd(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    match ::std::env::current_dir() {
        Ok(dir) => {
            let s = dir.to_string_lossy().into_owned();
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
        }
        Err(_) => { args.rval().set(UndefinedValue()); }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn process_exit(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let code = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() } else { 0 }
    } else {
        0
    };
    ::std::process::exit(code);
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
        let js_str = ::std::ptr::NonNull::new(input.to_string()).unwrap();
        let s = mozjs::conversions::jsstr_to_string(cx, js_str);
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

pub unsafe fn install_all(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    install_bun_global(cx, global);
    install_process_global(cx, global);
    install_buffer_global(cx, global);
    install_fetch_global(cx, global);
    install_response_constructor(cx, global);
    install_headers_constructor(cx, global);
    crate::require::install_require(cx, global);
    crate::timers::install_timer_globals(cx, global);
}

pub fn install_fetch_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"fetch".as_ptr(),
            ::std::option::Option::Some(fetch_fn), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fetch_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, b"fetch requires a URL argument\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, b"fetch requires a string URL\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let url = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(url_val.to_string()).unwrap());

    let method = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut m_val = UndefinedValue();
            let m_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut m_val };
            JS_GetProperty(cx, obj_handle, c"method".as_ptr(), m_handle);
            if m_val.is_string() {
                mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(m_val.to_string()).unwrap()).to_uppercase()
            } else {
                "GET".to_string()
            }
        } else {
            "GET".to_string()
        }
    } else {
        "GET".to_string()
    };

    let body = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut b_val = UndefinedValue();
            let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut b_val };
            JS_GetProperty(cx, obj_handle, c"body".as_ptr(), b_handle);
            if b_val.is_string() {
                Some(mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(b_val.to_string()).unwrap()))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let response = match do_fetch(&url, &method, body.as_deref()) {
        Ok(resp) => resp,
        Err(e) => {
            let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
            if !promise.is_null() {
                let msg = format!("fetch failed: {}", e);
                let Ok(c_msg) = ::std::ffi::CString::new(msg) else {
                    args.rval().set(mozjs::jsval::ObjectValue(promise));
                    return true;
                };
                let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                if !err_obj.is_null() {
                    let err_msg = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                    if !err_msg.is_null() {
                        let msg_val = StringValue(&*err_msg);
                        let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
                        let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
                        JS_SetProperty(cx, err_h, c"message".as_ptr(), msg_h);
                    }
                }
                let err_val = mozjs::jsval::ObjectValue(err_obj);
                let err_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
                let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
                mozjs_sys::jsapi::JS::RejectPromise(cx, promise_h, err_handle);
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise));
            return true;
        }
    };

    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(response.status_code as i32);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(response.status_code >= 200 && response.status_code < 300);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    if let Ok(c_url) = ::std::ffi::CString::new(response.url.as_str()) {
        let url_js = JS_NewStringCopyZ(cx, c_url.as_ptr());
        if !url_js.is_null() {
            let url_val = StringValue(&*url_js);
            let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
            JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
        }
    }

    if let Ok(c_st) = ::std::ffi::CString::new(response.status_text.as_str()) {
        let st_js = JS_NewStringCopyZ(cx, c_st.as_ptr());
        if !st_js.is_null() {
            let st_val = StringValue(&*st_js);
            let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
            JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
        }
    }

    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !headers_obj.is_null() {
        let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };
        for (key, value) in &response.headers {
            let Ok(c_key) = ::std::ffi::CString::new(key.as_str()) else { continue };
            let Ok(c_val) = ::std::ffi::CString::new(value.as_str()) else { continue };
            let val_js = JS_NewStringCopyZ(cx, c_val.as_ptr());
            if !val_js.is_null() {
                let hv = StringValue(&*val_js);
                let hv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hv };
                JS_DefineProperty(cx, h_handle, c_key.as_ptr(), hv_handle, JSPROP_ENUMERATE as u32);
            }
        }
        let hdrs_val = mozjs::jsval::ObjectValue(headers_obj);
        let hdrs_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), hdrs_handle, JSPROP_ENUMERATE as u32);
    }

    let Ok(c_body) = ::std::ffi::CString::new(response.body.clone()) else {
        args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
        return true;
    };
    let body_str = JS_NewStringCopyZ(cx, c_body.as_ptr());
    if !body_str.is_null() {
        let body_val = StringValue(&*body_str);
        let bt_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &body_val };
        JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bt_handle, 0);
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    let resp_val = mozjs::jsval::ObjectValue(resp_obj);
    let resp_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_val };
    let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_h, resp_handle);

    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

struct FetchResponse {
    status_code: u16,
    body: String,
    headers: Vec<(String, String)>,
    url: String,
    status_text: String,
}

fn do_fetch(url: &str, method: &str, body: Option<&str>) -> ::std::result::Result<FetchResponse, String> {
    let req = match method {
        "POST" => minreq::post(url),
        "PUT" => minreq::put(url),
        "DELETE" => minreq::delete(url),
        "PATCH" => minreq::patch(url),
        "HEAD" => minreq::head(url),
        _ => minreq::get(url),
    };

    let req = if let Some(b) = body {
        req.with_body(b)
    } else {
        req
    };

    let resp = req.send().map_err(|e| format!("{}", e))?;
    let headers: Vec<(String, String)> = resp.headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    ::std::result::Result::Ok(FetchResponse {
        status_code: resp.status_code as u16,
        body: resp.as_str().unwrap_or("").to_string(),
        headers,
        url: url.to_string(),
        status_text: String::new(),
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);
    args.rval().set(body_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_json(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, b"response.json(): invalid this\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);

    if !body_val.is_string() {
        JS_ReportErrorUTF8(cx, b"response.json(): body is not a string\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let js_str = body_val.to_string();
    let str_handle = Handle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &js_str };
    let mut rval = UndefinedValue();
    let mut rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_handle, rval_handle);

    if !ok {
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, b"response.json(): invalid JSON\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    args.rval().set(rval);
    true
}

pub fn install_response_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"Response".as_ptr(),
            ::std::option::Option::Some(response_constructor), 2, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(200);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(true);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    let url_js_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    if !url_js_str.is_null() {
        let url_val = StringValue(&*url_js_str);
        let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
        JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
    }

    let st_js_str = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
    if !st_js_str.is_null() {
        let st_val = StringValue(&*st_js_str);
        let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
        JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
    }

    let empty_headers = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !empty_headers.is_null() {
        let h_val = mozjs::jsval::ObjectValue(empty_headers);
        let h_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &h_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), h_handle, JSPROP_ENUMERATE as u32);
    }

    if argc > 0 {
        let body_val = *args.get(0).ptr;
        if body_val.is_string() {
            let body_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(body_val.to_string()).unwrap());
            if let Ok(c_body) = ::std::ffi::CString::new(body_str.as_str()) {
                let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
                if !body_js.is_null() {
                    let bv = StringValue(&*body_js);
                    let bv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bv };
                    JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bv_handle, 0);
                }
            }
        }
    }

    if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let opts_obj = opts.to_object();
            let opts_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };
            let mut st_val = UndefinedValue();
            let st_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut st_val };
            JS_GetProperty(cx, opts_handle, c"status".as_ptr(), st_mh);
            if st_val.is_int32() {
                let st_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
                JS_SetProperty(cx, obj_handle, c"status".as_ptr(), st_h);
                let ok = mozjs::jsval::BooleanValue(st_val.to_int32() >= 200 && st_val.to_int32() < 300);
                let ok_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok };
                JS_SetProperty(cx, obj_handle, c"ok".as_ptr(), ok_h);
            }
        }
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
    true
}

pub fn install_headers_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"Headers".as_ptr(),
            ::std::option::Option::Some(headers_constructor), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if headers_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };

    let get_fn = JS_NewFunction(cx, Some(headers_get), 1, 0, c"get".as_ptr());
    if !get_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(get_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"get".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let set_fn = JS_NewFunction(cx, Some(headers_set), 2, 0, c"set".as_ptr());
    if !set_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(set_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"set".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let has_fn = JS_NewFunction(cx, Some(headers_has), 1, 0, c"has".as_ptr());
    if !has_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(has_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"has".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(headers_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_get(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(name_js).unwrap());
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    };
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    if val.is_undefined() || val.is_null() {
        args.rval().set(mozjs::jsval::NullValue());
    } else {
        args.rval().set(val);
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_set(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, b"Headers.set requires name and value\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let name_val = *args.get(0).ptr;
    let value_val = *args.get(1).ptr;
    if !name_val.is_string() || !value_val.is_string() {
        JS_ReportErrorUTF8(cx, b"Headers.set requires string arguments\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    let name_js = name_val.to_string();
    let name_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(name_js).unwrap());
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &value_val };
    JS_SetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_has(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = mozjs::conversions::jsstr_to_string(cx, ::std::ptr::NonNull::new(name_js).unwrap());
    let Ok(c_name) = ::std::ffi::CString::new(name_str.as_str()) else {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    };
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(mozjs::jsval::BooleanValue(!val.is_undefined() && !val.is_null()));
    true
}