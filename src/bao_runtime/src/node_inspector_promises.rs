// @trace REQ-ENG-007 [api:node inspector/promises module]
//
// Promise-based inspector API. In Bun this is a 28-line file that re-exports
// `open`, `close`, `url`, `waitForDebugger`, `console` from the base inspector
// module and adds a `Session` class whose `post()` returns a Promise.

use bun_core::ZBox;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;

use crate::require::cache_builtin;

const INSPECTOR_PROMISES_JS: &str = r#"
(function() {
  // Re-export from the base inspector module
  var inspector = (typeof require !== 'undefined') ? require('inspector') : null;
  var open = inspector ? inspector.open : function() {};
  var close = inspector ? inspector.close : function() {};
  var url = inspector ? inspector.url : function() { return undefined; };
  var waitForDebugger = inspector ? inspector.waitForDebugger : function() { return Promise.resolve(); };
  var console = inspector ? inspector.console : undefined;

  // Session class with promise-based post()
  function Session() {
    if (!(this instanceof Session)) return new Session();
    // Inherit from base Session if available
    if (inspector && inspector.Session) {
      inspector.Session.call(this);
    }
    this._connected = false;
  }
  if (inspector && inspector.Session) {
    Session.prototype = Object.create(inspector.Session.prototype);
    Session.prototype.constructor = Session;
  }
  Session.prototype.connect = function() { this._connected = true; };
  Session.prototype.disconnect = function() { this._connected = false; };
  Session.prototype.post = function(method, params) {
    var self = this;
    return new Promise(function(resolve, reject) {
      // If the base Session has a callback-based post, wrap it
      if (inspector && inspector.Session && inspector.Session.prototype.post) {
        try {
          inspector.Session.prototype.post.call(self, method, params, function(err, result) {
            if (err) reject(err);
            else resolve(result);
          });
        } catch(e) {
          reject(e);
        }
      } else {
        // No real CDP backend — resolve with empty result
        resolve({});
      }
    });
  };

  return {
    console: console,
    open: open,
    close: close,
    url: url,
    waitForDebugger: waitForDebugger,
    Session: Session,
  };
})()
"#;

/// Install inspector/promises module.
pub fn install(cx: &mut mozjs::context::JSContext) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = "builtin:inspector/promises";
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("<inspector/promises>".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(INSPECTOR_PROMISES_JS);
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
        cache_builtin(cx, "inspector/promises", exports_obj);
    }
}
