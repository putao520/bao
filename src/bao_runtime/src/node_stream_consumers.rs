// @trace REQ-ENG-007 [api:node stream/consumers module]
//
// Stream consumer helpers — async functions that consume a readable stream
// and return the respective type. In Bun this is a 54-line TypeScript file.
// Bao implements it as an embedded JS IIFE since these are pure JS functions
// that use async/await and the stream's async iterator.

use bun_core::ZBox;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const STREAM_CONSUMERS_JS: &str = r#"
(function() {
  async function blob(stream) {
    var chunks = [];
    for await (var chunk of stream) chunks.push(chunk);
    return new Blob(chunks);
  }

  async function arrayBuffer(stream) {
    var ret = await blob(stream);
    return ret.arrayBuffer();
  }

  async function bytes(stream) {
    var ret = await blob(stream);
    return new Uint8Array(ret.arrayBuffer ? await ret.arrayBuffer() : ret);
  }

  async function buffer(stream) {
    var ab = await arrayBuffer(stream);
    // Buffer.from is available from the global Buffer
    if (typeof Buffer !== 'undefined' && Buffer.from) {
      return Buffer.from(ab);
    }
    return new Uint8Array(ab);
  }

  async function text(stream) {
    var dec = new TextDecoder();
    var str = '';
    for await (var chunk of stream) {
      if (typeof chunk === 'string') str += chunk;
      else str += dec.decode(chunk, { stream: true });
    }
    str += dec.decode(undefined, { stream: false });
    return str;
  }

  async function json(stream) {
    var str = await text(stream);
    return JSON.parse(str);
  }

  return {
    arrayBuffer: arrayBuffer,
    bytes: bytes,
    text: text,
    json: json,
    buffer: buffer,
    blob: blob,
  };
})()
"#;

/// Install stream/consumers module.
pub fn install(cx: &mut mozjs::context::JSContext) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = "builtin:stream/consumers";
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("<stream/consumers>".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(STREAM_CONSUMERS_JS);
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
        cache_builtin(cx, "stream/consumers", exports_obj);
    }
}
