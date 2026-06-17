// @trace REQ-ENG-007
use bun_core::ZBox;
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
        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
        if !global.is_null() {
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

        // @trace REQ-ENG-005 [entity:Buffer] — Node.js module-level constants.
        // `INSPECT_MAX_BYTES` is the visible-buffer cap used by util.inspect
        // when stringifying a Buffer (default 512 in Node.js). Upstream tests
        // (buffer-inspectmaxbytes.test.ts) read, set, and re-read it, so the
        // property must be a plain writable data property — no getter/setter.
        let inspect_max = Int32Value(512);
        let inspect_max_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &inspect_max };
        JS_DefineProperty(
            cx_raw,
            mod_h,
            c"INSPECT_MAX_BYTES".as_ptr(),
            inspect_max_h,
            JSPROP_ENUMERATE as u32,
        );
        // Also expose on the Buffer constructor itself (Node.js keeps the
        // canonical binding on Buffer.INSPECT_MAX_BYTES; node:buffer.INSPECT_MAX_BYTES
        // is the same value re-exported). Use the same writable data descriptor
        // shape so test assignments propagate.
        let mut buf_val = UndefinedValue();
        JS_GetProperty(cx_raw, global_h, c"Buffer".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut buf_val,
        });
        if buf_val.is_object() {
            let buf_obj = buf_val.to_object();
            let buf_obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_obj };
            JS_DefineProperty(
                cx_raw,
                buf_obj_h,
                c"INSPECT_MAX_BYTES".as_ptr(),
                inspect_max_h,
                JSPROP_ENUMERATE as u32,
            );
        }

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
        let c_filename = ZBox::from_bytes("node:buffer".as_bytes());
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

        // @trace REQ-ENG-005 [api:buffer] — isAscii / isUtf8 / resolveObjectURL.
        // Node.js 19+ surface: `Buffer.isAscii(input)` and `Buffer.isUtf8(input)`
        // return true if the input is a pure ASCII / valid UTF-8 byte sequence.
        // `resolveObjectURL(url)` reverses `URL.createObjectURL(blob)` —
        // returns the Blob referenced by the blob: URL.
        let extras_src = r#"(function() {
            var isAscii = function(buf) {
              if (buf == null) return false;
              var arr = (buf instanceof Uint8Array) ? buf : new Uint8Array(buf.buffer || buf, buf.byteOffset || 0, buf.byteLength || 0);
              for (var i = 0; i < arr.length; i++) {
                if (arr[i] > 127) return false;
              }
              return true;
            };
            var isUtf8 = function(buf) {
              if (buf == null) return false;
              var arr = (buf instanceof Uint8Array) ? buf : new Uint8Array(buf.buffer || buf, buf.byteOffset || 0, buf.byteLength || 0);
              var i = 0;
              while (i < arr.length) {
                var b = arr[i];
                if (b < 0x80) { i++; continue; }
                var need;
                var min;
                if ((b & 0xE0) === 0xC0) { need = 1; min = 0x80; }
                else if ((b & 0xF0) === 0xE0) { need = 2; min = 0x800; }
                else if ((b & 0xF8) === 0xF0) { need = 3; min = 0x10000; }
                else return false;
                if (i + need >= arr.length) return false;
                var cp = b & ((1 << (6 - need + 2)) - 1);
                for (var j = 0; j < need; j++) {
                  var c = arr[i + 1 + j];
                  if ((c & 0xC0) !== 0x80) return false;
                  cp = (cp << 6) | (c & 0x3F);
                }
                if (cp < min) return false;
                if (cp >= 0xD800 && cp <= 0xDFFF) return false;
                i += 1 + need;
              }
              return true;
            };
            // resolveObjectURL: Node.js's Blob URL store. Bao does not yet
            // maintain a blob: registry; mirror Bun's stub and return undefined
            // rather than throwing so structural probes (buffer-resolveObjectURL)
            // can verify the export exists.
            var resolveObjectURL = function(_url) { return undefined; };
            return { isAscii: isAscii, isUtf8: isUtf8, resolveObjectURL: resolveObjectURL };
        })()"#;
        let extras_filename = ZBox::from_bytes("node:buffer-extras".as_bytes());
        let extras_opts = mozjs::glue::NewCompileOptions(cx_raw, extras_filename.as_ptr(), 1);
        if !extras_opts.is_null() {
            let mut extras_src_text = mozjs::rust::transform_str_to_source_text(extras_src);
            let mut extras_rval = UndefinedValue();
            let extras_rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut extras_rval,
            };
            mozjs_sys::jsapi::JS::Evaluate2(cx_raw, extras_opts, &mut extras_src_text, extras_rval_h);
            libc::free(extras_opts as *mut _);
            if extras_rval.is_object() {
                let extras_obj = extras_rval.to_object();
                // Copy isAscii / isUtf8 / resolveObjectURL onto the module
                // object so they appear as named exports.
                for prop in &["isAscii", "isUtf8", "resolveObjectURL"] {
                    let cprop = ::std::ffi::CString::new(*prop).unwrap();
                    let mut val = UndefinedValue();
                    let extras_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &extras_obj };
                    JS_GetProperty(cx_raw, extras_h, cprop.as_ptr(), MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut val,
                    });
                    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                    JS_DefineProperty(cx_raw, mod_h, cprop.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
                }
            }
        }

        cache_builtin(cx, "buffer", mod_obj.get());
    }
}
