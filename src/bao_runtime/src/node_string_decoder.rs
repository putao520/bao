use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const STRING_DECODER_JS: &str = r#"
(function() {
  function StringDecoder(encoding) {
    this.encoding = (encoding || 'utf-8').toLowerCase().replace(/[-_]/g, '');
    this._buffer = '';
    this._partial = '';
  }

  StringDecoder.prototype.write = function(buf) {
    var str = typeof buf === 'string' ? buf : (buf && buf.toString ? buf.toString() : '');
    if (this.encoding === 'utf8' || this.encoding === 'utf-8') {
      var combined = this._partial + str;
      this._partial = '';
      var lastChar = combined.charCodeAt(combined.length - 1);
      if (lastChar >= 0xD800 && lastChar <= 0xDBFF) {
        this._partial = combined.slice(combined.length - 1);
        combined = combined.slice(0, combined.length - 1);
      }
      return combined;
    }
    return str;
  };

  StringDecoder.prototype.end = function(buf) {
    var str = '';
    if (buf) str = this.write(buf);
    str += this._partial;
    this._partial = '';
    return str;
  };

  StringDecoder.prototype.text = function(buf, offset) {
    if (!offset) offset = 0;
    var str = typeof buf === 'string' ? buf : (buf && buf.toString ? buf.toString() : '');
    if (offset > 0 && offset < str.length) {
      str = str.slice(offset);
    }
    return this._partial + str;
  };

  StringDecoder.prototype.fill = function(buf) {
    return this.write(buf);
  };

  return {
    StringDecoder: StringDecoder,
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

        let c_filename = ::std::ffi::CString::new("node:string_decoder").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(STRING_DECODER_JS);
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

        for name in &["StringDecoder"] {
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

        cache_builtin("string_decoder", mod_obj.get());
    }
}
