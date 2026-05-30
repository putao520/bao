use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, Int32Value, ObjectValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let mut buf_val = UndefinedValue();
            JS_GetProperty(cx_raw, global_h, c"Buffer".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut buf_val,
            });
            if !buf_val.is_undefined() {
                let mod_ptr = mod_obj.get();
                let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr };
                let buf_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_val };
                JS_DefineProperty(cx_raw, mod_h, c"Buffer".as_ptr(), buf_h, JSPROP_ENUMERATE as u32);
            }
        }

        let kmax = Int32Value(2147483647);
        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr };
        let kmax_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &kmax };
        JS_DefineProperty(cx_raw, mod_h, c"kMaxLength".as_ptr(), kmax_h, JSPROP_ENUMERATE as u32);

        rooted!(&in(cx) let constants_obj = w2::JS_NewPlainObject(cx));
        if !constants_obj.get().is_null() {
            let cmax = Int32Value(2147483647);
            let cmax_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cmax };
            JS_DefineProperty(cx_raw, constants_obj.handle().into(), c"MAX_LENGTH".as_ptr(), cmax_h, JSPROP_ENUMERATE as u32);
            let smax = Int32Value(536870888);
            let smax_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &smax };
            JS_DefineProperty(cx_raw, constants_obj.handle().into(), c"MAX_STRING_LENGTH".as_ptr(), smax_h, JSPROP_ENUMERATE as u32);
            let constants_val = ObjectValue(constants_obj.get());
            let constants_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &constants_val };
            JS_DefineProperty(cx_raw, mod_h, c"constants".as_ptr(), constants_h, JSPROP_ENUMERATE as u32);
        }

        // SlowBuffer = Buffer.alloc alias via JS
        let slow_buf_src = "Buffer.alloc";
        let c_filename = ::std::ffi::CString::new("node:buffer").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(slow_buf_src);
            let mut rval = UndefinedValue();
            let rval_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
            libc::free(opts as *mut _);
            if !rval.is_undefined() {
                let sb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &rval };
                JS_DefineProperty(cx_raw, mod_h, c"SlowBuffer".as_ptr(), sb_h, JSPROP_ENUMERATE as u32);
            }
        }

        cache_builtin(cx, "buffer", mod_obj.get());
    }
}
