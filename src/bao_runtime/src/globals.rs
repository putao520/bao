use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject,
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
        let c_key = match ::std::ffi::CString::new(key) {
            ::std::result::Result::Ok(k) => k,
            ::std::result::Result::Err(_) => continue,
        };
        let c_val = match ::std::ffi::CString::new(value) {
            ::std::result::Result::Ok(v) => v,
            ::std::result::Result::Err(_) => continue,
        };
        let val_str = JS_NewStringCopyZ(cx, c_val.as_ptr());
        if !val_str.is_null() {
            let val = StringValue(&*val_str);
            let mut val_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &val,
            };
            let mut obj_handle = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &env_obj,
            };
            JS_DefineProperty(cx, obj_handle, c_key.as_ptr(), val_handle, JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(mozjs::jsval::ObjectValue(env_obj));
    true
}
