use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const QS_JS: &str = r#"
(function() {
  function encode(s) {
    return encodeURIComponent(s).replace(/%20/g, '+');
  }

  function decode(s) {
    return decodeURIComponent(s.replace(/\+/g, ' '));
  }

  function parse(str, sep, eq) {
    sep = sep || '&';
    eq = eq || '=';
    var obj = {};
    if (!str || str.length === 0) return obj;
    str = str.replace(/^\?/, '');
    var pairs = str.split(sep);
    for (var i = 0; i < pairs.length; i++) {
      var pair = pairs[i];
      var idx = pair.indexOf(eq);
      var key, val;
      if (idx >= 0) {
        key = decode(pair.slice(0, idx));
        val = decode(pair.slice(idx + 1));
      } else {
        key = decode(pair);
        val = '';
      }
      if (obj.hasOwnProperty(key)) {
        if (!Array.isArray(obj[key])) {
          obj[key] = [obj[key]];
        }
        obj[key].push(val);
      } else {
        obj[key] = val;
      }
    }
    return obj;
  }

  function stringify(obj, sep, eq) {
    sep = sep || '&';
    eq = eq || '=';
    var pairs = [];
    for (var key in obj) {
      if (!obj.hasOwnProperty(key)) continue;
      var val = obj[key];
      if (Array.isArray(val)) {
        for (var i = 0; i < val.length; i++) {
          pairs.push(encode(key) + eq + encode(String(val[i])));
        }
      } else {
        pairs.push(encode(key) + eq + encode(String(val)));
      }
    }
    return pairs.join(sep);
  }

  function escape(str) {
    return encode(str);
  }

  function unescape(str) {
    return decode(str);
  }

  return {
    parse: parse,
    stringify: stringify,
    escape: escape,
    unescape: unescape,
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
        let c_filename = ::std::ffi::CString::new("node:querystring").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(QS_JS);
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

        for name in &["parse", "stringify", "escape", "unescape"] {
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

        cache_builtin(cx, "querystring", mod_obj.get());
    }
}
