// @trace REQ-ENG-007
use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{UndefinedValue, Int32Value, ObjectValue, JSVal};
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
        rooted!(&in(cx) let global = CurrentGlobalOrNull(cx_raw));
        if !global.get().is_null() {
            let mut buf_val = UndefinedValue();
            JS_GetProperty(cx_raw, global.handle().into(), c"Buffer".as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut buf_val,
            });
            if !buf_val.is_undefined() {
                rooted!(&in(cx) let buf_root = buf_val);
                JS_DefineProperty(cx_raw, mod_obj.handle().into(), c"Buffer".as_ptr(), buf_root.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        rooted!(&in(cx) let kmax = mozjs::jsval::DoubleValue(4294967296.0_f64));
        JS_DefineProperty(cx_raw, mod_obj.handle().into(), c"kMaxLength".as_ptr(), kmax.handle().into(), JSPROP_ENUMERATE as u32);

        // @trace REQ-ENG-005 [entity:Buffer] — Node.js module-level constants.
        // `INSPECT_MAX_BYTES` is the visible-buffer cap used by util.inspect
        // when stringifying a Buffer (default 512 in Node.js). Upstream tests
        // (buffer-inspectmaxbytes.test.ts) read, set, and re-read it, so the
        // property must be a plain writable data property — no getter/setter.
        rooted!(&in(cx) let inspect_max = Int32Value(512));
        JS_DefineProperty(
            cx_raw,
            mod_obj.handle().into(),
            c"INSPECT_MAX_BYTES".as_ptr(),
            inspect_max.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        // Also expose on the Buffer constructor itself (Node.js keeps the
        // canonical binding on Buffer.INSPECT_MAX_BYTES; node:buffer.INSPECT_MAX_BYTES
        // is the same value re-exported). Use the same writable data descriptor
        // shape so test assignments propagate.
        let mut buf_val = UndefinedValue();
        JS_GetProperty(cx_raw, global.handle().into(), c"Buffer".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut buf_val,
        });
        if buf_val.is_object() {
            rooted!(&in(cx) let buf_obj = buf_val.to_object());
            JS_DefineProperty(
                cx_raw,
                buf_obj.handle().into(),
                c"INSPECT_MAX_BYTES".as_ptr(),
                inspect_max.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        rooted!(&in(cx) let constants_obj = w2::JS_NewPlainObject(cx));
        if !constants_obj.get().is_null() {
            // @trace REQ-ENG-005 [entity:Buffer.constants] — Node.js surface:
            // constants.MAX_LENGTH = 2^32 (4294967296) on 64-bit platforms.
            // Exceeds i32 range, so emit as a JS double. buffer.test.js
            // "constants" asserts the exact value.
            rooted!(&in(cx) let cmax = mozjs::jsval::DoubleValue(4294967296.0_f64));
            JS_DefineProperty(cx_raw, constants_obj.handle().into(), c"MAX_LENGTH".as_ptr(), cmax.handle().into(), JSPROP_ENUMERATE as u32);
            rooted!(&in(cx) let smax = Int32Value(2147483647));
            JS_DefineProperty(cx_raw, constants_obj.handle().into(), c"MAX_STRING_LENGTH".as_ptr(), smax.handle().into(), JSPROP_ENUMERATE as u32);
            rooted!(&in(cx) let constants_val = ObjectValue(constants_obj.get()));
            JS_DefineProperty(cx_raw, mod_obj.handle().into(), c"constants".as_ptr(), constants_val.handle().into(), JSPROP_ENUMERATE as u32);
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
                rooted!(&in(cx) let sb_root = rval);
                JS_DefineProperty(cx_raw, mod_obj.handle().into(), c"SlowBuffer".as_ptr(), sb_root.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // @trace REQ-ENG-005 [api:buffer] — isAscii / isUtf8 / resolveObjectURL.
        // Node.js 19+ surface: `Buffer.isAscii(input)` and `Buffer.isUtf8(input)`
        // return true if the input is a pure ASCII / valid UTF-8 byte sequence.
        // `resolveObjectURL(url)` reverses `URL.createObjectURL(blob)` —
        // returns the Blob referenced by the blob: URL.
        // Re-run the Blob-URL static installer now that node_url::install has
        // registered the URL constructor (web_api_constructors runs earlier).
        let lazy_src = "if (typeof globalThis._bao_run_blob_url_statics === 'function') globalThis._bao_run_blob_url_statics();";
        let lazy_filename = ZBox::from_bytes("node:buffer-blob-url-lazy".as_bytes());
        let lazy_opts = mozjs::glue::NewCompileOptions(cx_raw, lazy_filename.as_ptr(), 1);
        if !lazy_opts.is_null() {
            let mut lazy_src_text = mozjs::rust::transform_str_to_source_text(lazy_src);
            let mut lazy_rval = UndefinedValue();
            let lazy_rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut lazy_rval,
            };
            mozjs_sys::jsapi::JS::Evaluate2(cx_raw, lazy_opts, &mut lazy_src_text, lazy_rval_h);
            libc::free(lazy_opts as *mut _);
        }

        // @trace REQ-ENG-005 [api:buffer] — re-export Blob/File on the
        // `buffer` module object so `import { Blob } from "buffer"` resolves
        // (Node.js 19+ surface, mirror of node-fallbacks/buffer.js). Blob
        // itself is installed by install_web_api_constructors on globalThis.
        let blob_src = r#"(function() {
            var out = {};
            if (typeof globalThis.Blob === 'function') out.Blob = globalThis.Blob;
            if (typeof globalThis.File === 'function') out.File = globalThis.File;
            return out;
        })()"#;
        let blob_filename = ZBox::from_bytes("node:buffer-blob".as_bytes());
        let blob_opts = mozjs::glue::NewCompileOptions(cx_raw, blob_filename.as_ptr(), 1);
        if !blob_opts.is_null() {
            let mut blob_src_text = mozjs::rust::transform_str_to_source_text(blob_src);
            let mut blob_rval = UndefinedValue();
            let blob_rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut blob_rval,
            };
            mozjs_sys::jsapi::JS::Evaluate2(cx_raw, blob_opts, &mut blob_src_text, blob_rval_h);
            libc::free(blob_opts as *mut _);
            if blob_rval.is_object() {
                rooted!(&in(cx) let blob_obj = blob_rval.to_object());
                for prop in &["Blob", "File"] {
                    let cprop = ::std::ffi::CString::new(*prop).unwrap();
                    let mut val = UndefinedValue();
                    JS_GetProperty(cx_raw, blob_obj.handle().into(), cprop.as_ptr(), MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut val,
                    });
                    if !val.is_undefined() {
                        rooted!(&in(cx) let val_root = val);
                        JS_DefineProperty(cx_raw, mod_obj.handle().into(), cprop.as_ptr(), val_root.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                }
            }
        }

        let extras_src = r#"(function() {
            var isAscii = function(buf) {
              if (buf == null) return false;
              var arr;
              if (buf instanceof Uint8Array) {
                arr = buf;
              } else if (buf instanceof ArrayBuffer) {
                arr = new Uint8Array(buf);
              } else if (buf && typeof buf === 'object' && buf.buffer instanceof ArrayBuffer) {
                arr = new Uint8Array(buf.buffer, buf.byteOffset || 0, buf.byteLength || 0);
              } else if (buf && typeof buf === 'object' && typeof buf.length === 'number') {
                arr = buf;
              } else {
                return false;
              }
              for (var i = 0; i < arr.length; i++) {
                if (arr[i] > 127) return false;
              }
              return true;
            };
            var isUtf8 = function(buf) {
              if (buf == null) return false;
              var arr;
              if (buf instanceof Uint8Array) {
                arr = buf;
              } else if (buf instanceof ArrayBuffer) {
                arr = new Uint8Array(buf);
              } else if (buf && typeof buf === 'object' && buf.buffer instanceof ArrayBuffer) {
                arr = new Uint8Array(buf.buffer, buf.byteOffset || 0, buf.byteLength || 0);
              } else if (buf && typeof buf === 'object' && typeof buf.length === 'number') {
                arr = buf;
              } else {
                return false;
              }
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
            // resolveObjectURL: Node.js's Blob URL store.
            // @trace REQ-ENG-005 [api:buffer.resolveObjectURL]
            // Backed by the global _bao_blob_registry maintained alongside
            // URL.createObjectURL/revokeObjectURL (see web_api_constructors
            // in globals.rs). Returns undefined for non-strings, missing, or
            // already-revoked URLs — matches Node.js's surface.
            var resolveObjectURL = function(url) {
              if (typeof url !== 'string' || url == null) return undefined;
              var reg = globalThis._bao_blob_registry;
              if (!reg || typeof reg.get !== 'function') return undefined;
              if (!reg.has(url)) return undefined;
              return reg.get(url);
            };
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
                rooted!(&in(cx) let extras_obj = extras_rval.to_object());
                // Copy isAscii / isUtf8 / resolveObjectURL onto the module
                // object so they appear as named exports.
                for prop in &["isAscii", "isUtf8", "resolveObjectURL"] {
                    let cprop = ::std::ffi::CString::new(*prop).unwrap();
                    let mut val = UndefinedValue();
                    JS_GetProperty(cx_raw, extras_obj.handle().into(), cprop.as_ptr(), MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut val,
                    });
                    rooted!(&in(cx) let val_root = val);
                    JS_DefineProperty(cx_raw, mod_obj.handle().into(), cprop.as_ptr(), val_root.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
        }

        // @trace REQ-ENG-005 [api:buffer.isAscii/isUtf8] — Native SM functions
        // so `new isAscii(...)` returns the primitive boolean (SM honours
        // primitive return values from C++ natives invoked as constructors;
        // JS functions discard them). buffer.test.js "isAscii" drives
        //   new isAscii(new Buffer("...")) → toBeFalse
        // which only works when isAscii is a native callable. We replace the
        // JS-side installer's binding with a native SMFunction. The native
        // accepts a Buffer/Uint8Array/TypedArray/DataView/ArrayBuffer, reads
        // its byte view, and scans for ASCII (>127) or UTF-8 validity.
        //
        // JSFUN_CONSTRUCTOR (0x400) marks the function as constructible so
        // `new isAscii(buf)` is accepted. SM's [[Construct]] path for a native
        // C++ function honours the primitive return value set via CallArgs —
        // matching Bun's `masqueradesAsUndefined`-style behaviour where
        // `new` does NOT override the primitive with a fresh `this`.
        // Reference: js/src/jsapi.h `JSFUN_CONSTRUCTOR`; js/src/vm/Interpreter.cpp
        // `InvokeConstructor` (NativeImpl) copies `args.rval()` straight through.
        const JSFUN_CONSTRUCTOR: u32 = 0x400;
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"isAscii".as_ptr(),
            Some(buffer_is_ascii),
            1,
            (JSPROP_ENUMERATE as u32) | JSFUN_CONSTRUCTOR,
        );
        w2::JS_DefineFunction(
            cx,
            mod_obj.handle(),
            c"isUtf8".as_ptr(),
            Some(buffer_is_utf8),
            1,
            (JSPROP_ENUMERATE as u32) | JSFUN_CONSTRUCTOR,
        );

        // @trace REQ-ENG-005 [api:buffer.transcode] — Bun.transcode is a
        // masqueradesAsUndefined function: `typeof BufferModule.transcode`
        // returns "undefined" but invoking it throws "Not implemented".
        //
        // SpiderMonkey has no JSFunction-level flag for this, but its
        // |TypeOfObject| checks |JSCLASS_EMULATES_UNDEFINED| before
        // |isCallable()|, so a callable NativeObject whose JSClass carries
        // that flag satisfies both assertions:
        //   * typeof → "undefined"   (EmulatesUndefined short-circuits)
        //   * callable              (callHook() != nullptr ⇒ isCallable())
        //
        // We expose this via the mozjs fork's `JS_NewEmulatesUndefinedFunction`
        // (js/src/jsapi.cpp), then install it as the `transcode` property so
        // buffer.test.js's two assertions both pass.
        rooted!(&in(cx) let transcode_obj = unsafe {
            w2::JS_NewEmulatesUndefinedFunction(
                cx,
                Some(buffer_transcode),
                0,
                c"transcode".as_ptr(),
            )
        });
        if !transcode_obj.is_null() {
            // JS_DefineProperty's (Handle<Value>) overload is the only one the
            // mozjs_sys bindings expose, so wrap the callable object in a Value.
            rooted!(&in(cx) let tc_val = mozjs::jsval::ObjectValue(transcode_obj.get()));
            JS_DefineProperty(
                cx_raw,
                mod_obj.handle().into(),
                c"transcode".as_ptr(),
                tc_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        cache_builtin(cx, "buffer", mod_obj.get());
    }
}

// @trace REQ-ENG-005 [api:buffer.transcode] — Bun's transcode is a
// masqueradesAsUndefined native: present on the buffer module but reports
// `typeof === "undefined"` while still being callable. We install it via the
// mozjs fork's `JS_NewEmulatesUndefinedFunction` (which creates a callable
// NativeObject carrying JSCLASS_EMULATES_UNDEFINED), so typeof yields
// "undefined" and invoking the property still dispatches here. Each call
// throws "Not implemented" (the surface Bun documents for this stub when
// iconv support is absent).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_transcode(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let msg = ::std::ffi::CString::new("Not implemented").unwrap();
    mozjs::error::throw_type_error(cx, msg.as_ref());
    args.rval().set(UndefinedValue());
    false
}

// @trace REQ-ENG-005 [api:buffer.isAscii] [code:bun_simdutf_sys] — Native
// SMFunction returning a boolean primitive. Accepts Buffer/Uint8Array/
// TypedArray/DataView/ArrayBuffer. Returns true iff every byte is <= 127.
// Validation runs through `bun_simdutf_sys::validate_ascii`, which FFI-calls
// into bun-simdutf.cpp (AVX2/NEON SIMD, ~3-10× faster than a byte loop).
// Used both as a function AND as a constructor (new isAscii(buf)) — SM
// preserves primitive returns from C++ natives invoked as constructors,
// matching Bun's behaviour.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_ascii(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = *args.get(0).ptr;
    let bytes = match collect_byte_view(cx, input) {
        Some(b) => b,
        None => {
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    let is_ascii = bun_simdutf_sys::validate_ascii(bytes.as_slice());
    args.rval().set(mozjs::jsval::BooleanValue(is_ascii));
    true
}

// @trace REQ-ENG-005 [api:buffer.isUtf8] [code:bun_simdutf_sys] — Native
// SMFunction returning a boolean primitive. Validates UTF-8 byte sequences
// per RFC 3629 via `bun_simdutf_sys::validate_utf8` (SIMD-accelerated; rejects
// overlong encodings, surrogates, and malformed continuation bytes with the
// same semantics as the hand-written DFA it replaces).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_utf8(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = *args.get(0).ptr;
    let bytes = match collect_byte_view(cx, input) {
        Some(b) => b,
        None => {
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    };
    let is_utf8 = bun_simdutf_sys::validate_utf8(bytes.as_slice());
    args.rval().set(mozjs::jsval::BooleanValue(is_utf8));
    true
}

// @trace REQ-ENG-005 — Extracts a byte slice from a Buffer/Uint8Array/
// TypedArray/DataView/ArrayBuffer input. Returns None on null/undefined or
// unrecognized input. The returned Vec is a copy that survives GC.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn collect_byte_view(_cx: *mut JSContext, v: JSVal) -> Option<Vec<u8>> {
    use ::std::ptr;
    if v.is_null_or_undefined() {
        return None;
    }
    if !v.is_object() {
        return None;
    }
    let obj = v.to_object();
    // Try TypedArray / DataView / Buffer (JS_GetObjectAsUint8Array handles
    // all TypedArray kinds; Buffer is a Uint8Array subclass).
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ptr::null_mut();
    let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        obj,
        &mut length,
        &mut is_shared,
        &mut data_ptr,
    );
    if !unwrapped.is_null() && !data_ptr.is_null() {
        let slice = ::std::slice::from_raw_parts(data_ptr, length);
        return Some(slice.to_vec());
    }
    if !unwrapped.is_null() {
        return Some(Vec::new());
    }
    // Try ArrayBufferView (DataView, etc.): returns the underlying array.
    let mut view_length: usize = 0;
    let mut view_shared = false;
    let mut view_data: *mut u8 = ptr::null_mut();
    let view_unwrapped = mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
        obj,
        &mut view_length,
        &mut view_shared,
        &mut view_data,
    );
    if !view_unwrapped.is_null() && !view_data.is_null() {
        let slice = ::std::slice::from_raw_parts(view_data, view_length);
        return Some(slice.to_vec());
    }
    if !view_unwrapped.is_null() {
        return Some(Vec::new());
    }
    // Try plain ArrayBuffer via JS::GetObjectAsArrayBuffer.
    let mut ab_length: usize = 0;
    let mut ab_data: *mut u8 = ptr::null_mut();
    let ab_unwrapped = mozjs_sys::jsapi::JS::GetObjectAsArrayBuffer(obj, &mut ab_length, &mut ab_data);
    if !ab_unwrapped.is_null() && !ab_data.is_null() {
        let slice = ::std::slice::from_raw_parts(ab_data, ab_length);
        return Some(slice.to_vec());
    }
    if !ab_unwrapped.is_null() {
        return Some(Vec::new());
    }
    None
}
