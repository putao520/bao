// @trace REQ-ENG-001 [api:internal/test/binding]
//
// `internal/test/binding` — Node's internal test-only module that exposes
// the raw C++ `internalBinding()` helper used by Node's own test fixtures
// (test/common/index.js: `const { internalBinding } = require('internal/test/binding')`).
//
// Honest boundary: bao has no C++ `node::binding` layer, so `internalBinding`
// cannot return real native bindings. The minimal real surface is the
// function itself, which throws `ReferenceError` for any requested binding
// name — fail-closed: a test that actually consumes a native binding
// (e.g. `internalBinding('async_wrap')`) fails loudly at the consumption
// point instead of silently receiving a stub object. Modules that only
// destructure `{ internalBinding }` (like common/index.js) load fine.

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const INTERNAL_TEST_BINDING_JS: &str = r#"
(function() {
  'use strict';

  // Node's internalBinding(name) maps to the C++ NODE_MODULE registers.
  // bao has no C++ binding layer — any real binding request is a hard
  // failure (fail-closed), never a silent stub.
  function internalBinding(name) {
    throw new ReferenceError(
      "No such module: " + name + " (bao has no C++ internalBinding layer)"
    );
  }

  return { internalBinding: internalBinding };
})()
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let source = INTERNAL_TEST_BINDING_JS;
    unsafe {
        let raw_cx = cx.raw_cx();
        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let opts = NewCompileOptions(raw_cx, c"<internal/test/binding>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            mozjs::glue::DeleteCompileOptions(opts);
            if !ok {
                JS_ReportErrorUTF8(
                    raw_cx,
                    c"failed to evaluate internal/test/binding source".as_ptr(),
                );
                return;
            }
            if !rval.is_object() {
                return;
            }
            let obj = rval.to_object();
            rooted!(&in(cx) let obj_rooted = obj);
            // Mirror Node's behavior: require('internal/test/binding')
            // throws if the module cannot be loaded or is missing — here
            // we only cache it when evaluation produced a real object.
            cache_builtin(cx, "internal/test/binding", obj_rooted.get());
        }
    }
}
