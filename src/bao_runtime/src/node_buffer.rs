use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const BUFFER_JS: &str = r#"
(function() {
  function SlowBuffer(size) {
    return Buffer.alloc(size);
  }

  return {
    Buffer: Buffer,
    SlowBuffer: SlowBuffer,
    kMaxLength: 2147483647,
    constants: {
      MAX_LENGTH: 2147483647,
      MAX_STRING_LENGTH: 536870888,
    },
  };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ::std::ffi::CString::new("node:buffer").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(BUFFER_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        let exports_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &exports_obj,
        };

        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr };

        for name in &["Buffer", "SlowBuffer", "kMaxLength", "constants"] {
            let cname = ::std::ffi::CString::new(*name).unwrap_or_default();
            let mut val = UndefinedValue();
            JS_GetProperty(cx_raw, exports_h, cname.as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            });
            if !val.is_undefined() {
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                JS_DefineProperty(cx_raw, mod_h, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
            }
        }

        cache_builtin("buffer", mod_obj.get());
    }
}
