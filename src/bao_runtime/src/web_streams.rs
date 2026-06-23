// @trace REQ-ENG-005 [api:Web Streams] — ReadableStream / WritableStream / TransformStream
// and all associated classes, queuing strategies, and encoding streams.
//
// Ported from Bun's JSC-based TypeScript implementation, adapted for SpiderMonkey:
// - $putByIdDirectPrivate / $getByIdDirectPrivate → WeakMap-based private slot storage
// - $is*() type checks → Symbol-brand + instanceof checks
// - Bun.* native calls → pure JS equivalents
// - TypeScript types stripped

use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_NewPlainObject;

/// Install Web Streams API constructors on the global object.
pub fn install_web_streams(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    let src = include_str!("web_streams.js");

    unsafe {
        let raw = cx.raw_cx();
        let mut rval = UndefinedValue();
        let opts = mozjs::glue::NewCompileOptions(
            raw,
            c"web_streams".as_ptr(),
            1,
        );
        if !opts.is_null() {
            let mut src_text = mozjs::rust::transform_str_to_source_text(src);
            mozjs_sys::jsapi::JS::Evaluate2(
                raw,
                opts,
                &mut src_text,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            libc::free(opts as *mut _);
        }
    }
}

/// Install Web Streams API constructors on a target object instead of global.
/// Used for scoped installations (e.g. node:stream/web module).
pub fn install_web_streams_on_target(
    cx: &mut mozjs::context::JSContext,
    target: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let raw = cx.raw_cx();
        let global = CurrentGlobalOrNull(raw);
        if global.is_null() {
            return;
        }
        rooted!(&in(cx) let global_root = global);
        install_web_streams(cx, global_root.handle());

        // Copy constructor references from global to target
        let names = [
            "ReadableStream", "ReadableStreamDefaultReader", "ReadableStreamBYOBReader",
            "ReadableStreamDefaultController", "ReadableByteStreamController",
            "ReadableStreamBYOBRequest",
            "WritableStream", "WritableStreamDefaultWriter", "WritableStreamDefaultController",
            "TransformStream", "TransformStreamDefaultController",
            "ByteLengthQueuingStrategy", "CountQueuingStrategy",
            "TextEncoderStream", "TextDecoderStream",
            "CompressionStream", "DecompressionStream",
        ];

        rooted!(&in(cx) let target_root = target.get());
        for name in &names {
            let mut val = UndefinedValue();
            let cname = bun_core::ZBox::from_bytes(name.as_bytes());
            JS_GetProperty(
                raw,
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
                    raw,
                    target_root.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn web_streams_js_source_is_not_empty() {
        let src = include_str!("web_streams.js");
        assert!(!src.is_empty(), "web_streams.js must not be empty");
        assert!(src.contains("ReadableStream"), "must define ReadableStream");
        assert!(src.contains("WritableStream"), "must define WritableStream");
        assert!(src.contains("TransformStream"), "must define TransformStream");
    }

    #[test]
    fn web_streams_js_defines_all_constructors() {
        let src = include_str!("web_streams.js");
        let required = [
            "ReadableStream",
            "ReadableStreamDefaultReader",
            "ReadableStreamBYOBReader",
            "ReadableStreamDefaultController",
            "ReadableByteStreamController",
            "ReadableStreamBYOBRequest",
            "WritableStream",
            "WritableStreamDefaultWriter",
            "WritableStreamDefaultController",
            "TransformStream",
            "TransformStreamDefaultController",
            "ByteLengthQueuingStrategy",
            "CountQueuingStrategy",
            "TextEncoderStream",
            "TextDecoderStream",
            "CompressionStream",
            "DecompressionStream",
        ];
        for name in &required {
            assert!(
                src.contains(name),
                "web_streams.js must define constructor: {}",
                name
            );
        }
    }
}
