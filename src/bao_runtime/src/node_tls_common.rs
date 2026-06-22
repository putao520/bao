// @trace REQ-ENG-007 [api:node _tls_common module]
//
// Internal TLS common module. In Bun this is a 35-line module that exports
// `translatePeerCertificate` — a function that recursively translates a peer
// certificate's `infoAccess` field from a C-style string format into a JS
// object, and recursively processes `issuerCertificate`.

use bun_core::ZBox;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const TLS_COMMON_JS: &str = r#"
(function() {
  function translatePeerCertificate(c) {
    if (typeof c === 'string' || c == null) return c;
    if (Array.isArray(c)) return c.map(translatePeerCertificate);

    var ret = {};
    if (c.issuerCertificate != null) {
      ret.issuerCertificate = translatePeerCertificate(c.issuerCertificate);
    }

    // Translate infoAccess from C-style format
    if (c.infoAccess) {
      var info = {};
      if (typeof c.infoAccess === 'object') {
        for (var key in c.infoAccess) {
          info[key] = c.infoAccess[key];
        }
      }
      ret.infoAccess = info;
    }

    // Copy all other properties
    for (var k in c) {
      if (k !== 'issuerCertificate' && k !== 'infoAccess') {
        ret[k] = c[k];
      }
    }

    return ret;
  }

  return { translatePeerCertificate: translatePeerCertificate };
})()
"#;

/// Install _tls_common module.
pub fn install(cx: &mut mozjs::context::JSContext) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = "builtin:_tls_common";
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("<_tls_common>".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(TLS_COMMON_JS);
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
        cache_builtin(cx, "_tls_common", exports_obj);
    }
}
