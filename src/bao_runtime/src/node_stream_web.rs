// @trace REQ-ENG-007 [api:node stream/web module]
//
// Re-exports the Web Streams API constructors from the global scope.
// In Bun this is a 20-line file that re-exports ReadableStream, WritableStream,
// etc. Bao's node_stream already installs these on the global; this module
// collects them into a require()-able namespace.

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Web stream constructor names to re-export.
const WEB_STREAM_NAMES: &[&str] = &[
    "ReadableStream",
    "ReadableStreamDefaultReader",
    "ReadableStreamBYOBReader",
    "ReadableStreamBYOBRequest",
    "ReadableByteStreamController",
    "ReadableStreamDefaultController",
    "TransformStream",
    "TransformStreamDefaultController",
    "WritableStream",
    "WritableStreamDefaultWriter",
    "WritableStreamDefaultController",
    "ByteLengthQueuingStrategy",
    "CountQueuingStrategy",
    "TextEncoderStream",
    "TextDecoderStream",
    "CompressionStream",
    "DecompressionStream",
];

/// Install stream/web module — re-exports Web Streams constructors.
pub fn install(cx: &mut mozjs::context::JSContext) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = "builtin:stream/web";
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    unsafe {
        let raw_cx = cx.raw_cx();
        let global = CurrentGlobalOrNull(raw_cx);
        if global.is_null() {
            return;
        }

        rooted!(&in(cx) let global_root = global);
        rooted!(&in(cx) let mod_obj = w2::JS_NewPlainObject(cx));
        if mod_obj.get().is_null() {
            return;
        }

        for name in WEB_STREAM_NAMES {
            let cname = bun_core::ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(
                raw_cx,
                global_root.handle().into(),
                cname.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut val,
                },
            );
            if val.is_object() {
                rooted!(&in(cx) let val_root = val);
                JS_DefineProperty(
                    raw_cx,
                    mod_obj.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "stream/web", mod_obj.get());
    }
}
