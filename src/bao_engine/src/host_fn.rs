use ::std::ptr::NonNull;

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_DefineFunction;
use mozjs::rust::wrappers2::JS_DefineProperty3;
use mozjs::rust::wrappers2::JS_NewPlainObject;

use crate::error::JsError;
use crate::value::JsValue;

const HOST_OBJECT_SLOT: u32 = 0;

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

pub trait HostObject: Sized {
    unsafe fn from_private(_cx: *mut JSContext, thisv: JSVal) -> *mut Self {
        if !thisv.is_object() {
            return ::std::ptr::null_mut();
        }
        let obj = thisv.to_object();
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, HOST_OBJECT_SLOT, &mut slot);
        let ptr = slot.to_private() as *mut Self;
        if ptr.is_null() {
            ::std::ptr::null_mut()
        } else {
            ptr
        }
    }

    unsafe fn to_private(&self, obj: *mut JSObject) {
        let val = mozjs::jsval::PrivateValue(self as *const Self as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, HOST_OBJECT_SLOT, &val);
    }
}

pub unsafe fn extract_setter_value(cx: *mut JSContext, args: &CallArgs) -> JsValue {
    if args.argc_ > 0 {
        crate::value::jsval_to_jsvalue(cx, *args.get(0).ptr)
    } else {
        JsValue::Undefined
    }
}

impl JsError {
    pub fn throw_on(&self, cx: *mut JSContext) {
        let msg = ::std::ffi::CString::new(self.message.as_str())
            .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
        unsafe {
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, msg.as_ptr());
        }
    }
}

impl JsValue {
    pub fn set_as_rval(self, args: &mut CallArgs) {
        args.rval().set(self.to_jsval());
    }

    pub fn to_jsval(&self) -> JSVal {
        match self {
            JsValue::Undefined => UndefinedValue(),
            JsValue::Null => mozjs::jsval::NullValue(),
            JsValue::Bool(b) => mozjs::jsval::BooleanValue(*b),
            JsValue::Number(n) => {
                if *n == (*n as i32) as f64 && n.abs() < i32::MAX as f64 {
                    mozjs::jsval::Int32Value(*n as i32)
                } else {
                    mozjs::jsval::DoubleValue(*n)
                }
            }
            JsValue::String(_) | JsValue::Object(_) => UndefinedValue(),
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
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

#[allow(unsafe_op_in_unsafe_fn)]
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
                NonNull::new(s).unwrap(),
            );
            print!("{}", rust_str);
        }
    } else if val.is_object() {
        print!("[object Object]");
    }
}
