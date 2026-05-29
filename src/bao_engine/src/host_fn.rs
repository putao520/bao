use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_DefineFunction;
use mozjs::rust::wrappers2::JS_DefineProperty3;
use mozjs::rust::wrappers2::JS_NewPlainObject;

pub fn install_console(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    rooted!(&in(cx) let console_obj = unsafe { JS_NewPlainObject(cx) });
    if console_obj.get().is_null() {
        return;
    }

    unsafe {
        JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"log".as_ptr(),
            ::std::option::Option::Some(console_log),
            0,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(
            cx,
            global,
            c"console".as_ptr(),
            console_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

unsafe extern "C" fn console_log(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    for i in 0..argc {
        if i > 0 {
            print!(" ");
        }
        let handle = args.get(i);
        let val = *handle.ptr;
        print_value(cx, val);
    }
    println!();
    args.rval().set(UndefinedValue());
    true
}

unsafe fn print_value(cx: *mut JSContext, val: JSVal) {
    if val.is_undefined() {
        print!("undefined");
    } else if val.is_null() {
        print!("null");
    } else if val.is_boolean() {
        print!("{}", val.to_boolean());
    } else if val.is_int32() {
        print!("{}", val.to_int32());
    } else if val.is_double() {
        let d = val.to_double();
        if d.is_nan() {
            print!("NaN");
        } else if d.is_infinite() {
            print!("{}", if d > 0.0 { "Infinity" } else { "-Infinity" });
        } else {
            print!("{}", d);
        }
    } else if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            let rust_str = mozjs::conversions::jsstr_to_string(
                cx,
                ::std::ptr::NonNull::new(s).unwrap(),
            );
            print!("{}", rust_str);
        }
    } else if val.is_object() {
        print!("[object Object]");
    }
}
