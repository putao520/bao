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

pub unsafe fn install_all(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    install_bun_global(cx, global);
    install_process_global(cx, global);
    crate::timers::install_timer_globals(cx, global);
}
