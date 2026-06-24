// @trace REQ-ENG-001
//! Initialize — JS runtime initialization utilities.

use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

pub unsafe fn eval_and_print(cx: *mut JSContext, source: &str, filename: &str) {
    let c_filename = CString::new(filename).unwrap_or_default();
    let opts = unsafe { mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr(), 1) };
    if opts.is_null() {
        return;
    }

    let mut src = mozjs::rust::transform_str_to_source_text(source);
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_handle) };
    unsafe { libc::free(opts as *mut _) };

    if !ok {
        return;
    }

    if !rval.is_undefined() {
        if rval.is_string() {
            let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
            rooted!(&in(wrapped_cx) let rval_root = rval);
            let js_str = unsafe { mozjs::rust::ToString(cx, rval_root.handle().into()) };
            if !js_str.is_null() {
                let rust_str = unsafe { mozjs::conversions::jsstr_to_string(cx, NonNull::new_unchecked(js_str)) };
                println!("{}", rust_str);
            }
        } else if rval.is_number() {
            println!("{}", rval.to_number());
        } else if rval.is_boolean() {
            let b = rval.to_boolean();
            println!("{}", if b { "true" } else { "false" });
        }
    }
}

pub fn initialize() {
    // JS runtime initialization is handled by bao_engine::context.
}
