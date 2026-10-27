// @trace REQ-ENG-007
use ::std::ptr::NonNull;
use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, UndefinedValue};
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
            JS_GetProperty(
                cx_raw,
                global.handle().into(),
                c"Buffer".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut buf_val,
                },
            );
            if !buf_val.is_undefined() {
                rooted!(&in(cx) let buf_root = buf_val);
                JS_DefineProperty(
                    cx_raw,
                    mod_obj.handle().into(),
                    c"Buffer".as_ptr(),
                    buf_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // @trace REQ-ENG-007 [entity:Buffer] — Node.js ≥22 kMaxLength =
        // 2^53-1 (MAX_SAFE_INTEGER); the legacy 2^32 value made
        // `new Uint8Array(kMaxLength + 1)` silently allocate instead of
        // throwing (node/test/parallel/test-buffer-alloc.js). Keep in sync
        // with globals.rs MAX_BUFFER_SIZE backstop and the JS-side
        // K_MAX_LENGTH constants.
        rooted!(&in(cx) let kmax = mozjs::jsval::DoubleValue(9007199254740991.0_f64));
        JS_DefineProperty(
            cx_raw,
            mod_obj.handle().into(),
            c"kMaxLength".as_ptr(),
            kmax.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // @trace REQ-ENG-007 [entity:Buffer] — Node.js kStringMaxLength =
        // 536870888 ((1<<29)-24). Required by
        // node/test/parallel/test-buffer-constants.js (kStringMaxLength ===
        // constants.MAX_STRING_LENGTH) and test-buffer-tostring-rangeerror.
        rooted!(&in(cx) let kstrmax = mozjs::jsval::DoubleValue(536870888.0_f64));
        JS_DefineProperty(
            cx_raw,
            mod_obj.handle().into(),
            c"kStringMaxLength".as_ptr(),
            kstrmax.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // @trace REQ-ENG-007 [api:buffer.poolSize] — Node.js Buffer pool size
        // constant (default 8192). Exposed on BOTH the `buffer` module object
        // (`require('buffer').poolSize`) and the `Buffer` constructor
        // (`Buffer.poolSize`) — Node.js keeps the canonical binding on
        // Buffer.poolSize and re-exports it on the module. Writable data
        // property (user code may reassign to grow/shrink the alloc pool).
        rooted!(&in(cx) let pool_size = Int32Value(8192));
        JS_DefineProperty(
            cx_raw,
            mod_obj.handle().into(),
            c"poolSize".as_ptr(),
            pool_size.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // @trace REQ-ENG-007 [entity:Buffer] — Node.js module-level constants.
        // `INSPECT_MAX_BYTES` is the visible-buffer cap used by util.inspect
        // when stringifying a Buffer (default 512 in Node.js). Node.js ≥16
        // defines it as an ACCESSOR with range validation:
        //   non-number → TypeError ERR_INVALID_ARG_TYPE
        //   NaN / negative → RangeError ERR_OUT_OF_RANGE ("It must be >= 0")
        // node/test/parallel/test-buffer-set-inspect-max-bytes.js drives both
        // throw shapes on the module face. One closure holds the slot; the
        // same accessor pair is installed on the module object AND the Buffer
        // constructor below so both faces share one binding (Node.js
        // canonical-binding parity).
        let inspect_src = r#"(function(target1, target2) {
          var slot = 512;
          function receivedDesc(v) {
            if (v === null || v === undefined) return ' Received ' + String(v);
            var t = typeof v;
            if (t === 'function') return ' Received function ' + (v.name || '(anonymous)');
            if (t === 'object') {
              var name = v.constructor && v.constructor.name;
              if (typeof name === 'string' && name !== '') return ' Received an instance of ' + name;
              return ' Received ' + String(v);
            }
            // node quotes string values in the inspected form ('abc' —
            // node's util.inspect renders strings quoted).
            var inspected = (t === 'string') ? ("'" + v + "'") : String(v);
            if (inspected.length > 28) inspected = inspected.slice(0, 25) + '...';
            return ' Received type ' + t + ' (' + inspected + ')';
          }
          var desc = {
            get: function() { return slot; },
            set: function(v) {
              if (typeof v !== 'number') {
                var te = new TypeError(
                  'The "INSPECT_MAX_BYTES" argument must be of type number.' + receivedDesc(v));
                te.code = 'ERR_INVALID_ARG_TYPE';
                throw te;
              }
              if (Number.isNaN(v) || v < 0) {
                var re = new RangeError(
                  'The value of "INSPECT_MAX_BYTES" is out of range. It must be >= 0. Received ' + v);
                re.code = 'ERR_OUT_OF_RANGE';
                throw re;
              }
              slot = v;
            },
            enumerable: true,
            configurable: true
          };
          // Install on whichever targets are real objects (module face and
          // Buffer constructor face).
          var targets = [target1, target2];
          for (var i = 0; i < targets.length; i++) {
            if (targets[i] && typeof targets[i] === 'object') {
              Object.defineProperty(targets[i], 'INSPECT_MAX_BYTES', desc);
            }
          }
          return true;
        })"#;
        let inspect_filename = ZBox::from_bytes("node:buffer-inspect-max-bytes".as_bytes());
        let inspect_opts = mozjs::glue::NewCompileOptions(cx_raw, inspect_filename.as_ptr(), 1);
        if !inspect_opts.is_null() {
            let mut inspect_text = mozjs::rust::transform_str_to_source_text(inspect_src);
            let mut inspect_fn = UndefinedValue();
            let inspect_fn_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut inspect_fn,
            };
            mozjs_sys::jsapi::JS::Evaluate2(cx_raw, inspect_opts, &mut inspect_text, inspect_fn_h);
            mozjs::glue::DeleteCompileOptions(inspect_opts);
            // Locate the Buffer constructor (may not exist yet in web-only
            // realms — the accessor is then only on the module face).
            let mut buf_val = UndefinedValue();
            JS_GetProperty(
                cx_raw,
                global.handle().into(),
                c"Buffer".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut buf_val,
                },
            );
            if inspect_fn.is_object() {
                rooted!(&in(cx) let fn_root = inspect_fn.to_object());
                let mod_val = unsafe { ObjectValue(mod_obj.get()) };
                // unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
                rooted!(&in(cx) let args_arr_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                    mod_val,
                    buf_val,
                ]));
                let args_arr = JS::HandleValueArray::from(&args_arr_rooted);
                rooted!(&in(cx) let null_obj: *mut JSObject = ::std::ptr::null_mut());
                rooted!(&in(cx) let fn_val = ObjectValue(fn_root.get()));
                let mut call_rval = UndefinedValue();
                let _ = JS_CallFunctionValue(
                    cx_raw,
                    null_obj.handle().into(),
                    fn_val.handle().into(),
                    &args_arr,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut call_rval,
                    },
                );
            }
        }

        rooted!(&in(cx) let constants_obj = w2::JS_NewPlainObject(cx));
        if !constants_obj.get().is_null() {
            // @trace REQ-ENG-007 [entity:Buffer.constants] — Node.js ≥22
            // surface: constants.MAX_LENGTH = kMaxLength = 2^53-1
            // (MAX_SAFE_INTEGER); constants.MAX_STRING_LENGTH = 536870888
            // ((1<<29)-24). node/test/parallel/test-buffer-constants.js pins
            // kMaxLength === MAX_LENGTH and kStringMaxLength ===
            // MAX_STRING_LENGTH. Both exceed i32 range, so emit as doubles.
            rooted!(&in(cx) let cmax = mozjs::jsval::DoubleValue(9007199254740991.0_f64));
            JS_DefineProperty(
                cx_raw,
                constants_obj.handle().into(),
                c"MAX_LENGTH".as_ptr(),
                cmax.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            rooted!(&in(cx) let smax = mozjs::jsval::DoubleValue(536870888.0_f64));
            JS_DefineProperty(
                cx_raw,
                constants_obj.handle().into(),
                c"MAX_STRING_LENGTH".as_ptr(),
                smax.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            rooted!(&in(cx) let constants_val = ObjectValue(constants_obj.get()));
            JS_DefineProperty(
                cx_raw,
                mod_obj.handle().into(),
                c"constants".as_ptr(),
                constants_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // @trace REQ-ENG-007 [api:buffer.SlowBuffer] — node v24 SlowBuffer
        // (deprecated but present): STRICT `typeof size === 'number'`
        // validation — SlowBuffer() / SlowBuffer('6') / SlowBuffer({}) /
        // SlowBuffer(true) all throw TypeError ERR_INVALID_ARG_TYPE
        // 'The "size" argument must be of type number.' + received descriptor
        // (test-buffer-slow drives the matrix; the previous Buffer.alloc
        // alias accepted everything and returned a 0-length buffer for
        // missing args). Valid sizes delegate to the allocUnsafeSlow native
        // (same uninitialised non-pooled allocation node gives).
        let slow_buf_src = r#"(function() {
          function receivedDesc(v) {
            if (v === null || v === undefined) return ' Received ' + String(v);
            var t = typeof v;
            if (t === 'function') return ' Received function ' + (v.name || '(anonymous)');
            if (t === 'object') {
              var name = v.constructor && v.constructor.name;
              if (typeof name === 'string' && name !== '') return ' Received an instance of ' + name;
              return ' Received ' + String(v);
            }
            // node quotes string values in the inspected form ('abc' —
            // node's util.inspect renders strings quoted).
            var inspected = (t === 'string') ? ("'" + v + "'") : String(v);
            if (inspected.length > 28) inspected = inspected.slice(0, 25) + '...';
            return ' Received type ' + t + ' (' + inspected + ')';
          }
          function SlowBuffer(size) {
            if (typeof size !== 'number') {
              var e = new TypeError(
                'The "size" argument must be of type number.' + receivedDesc(size));
              e.code = 'ERR_INVALID_ARG_TYPE';
              throw e;
            }
            return Buffer.allocUnsafeSlow(size);
          }
          return SlowBuffer;
        })()"#;
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
            mozjs::glue::DeleteCompileOptions(opts);
            if !rval.is_undefined() {
                rooted!(&in(cx) let sb_root = rval);
                JS_DefineProperty(
                    cx_raw,
                    mod_obj.handle().into(),
                    c"SlowBuffer".as_ptr(),
                    sb_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // @trace REQ-ENG-007 [api:buffer] — isAscii / isUtf8 / resolveObjectURL.
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
            mozjs::glue::DeleteCompileOptions(lazy_opts);
        }

        // @trace REQ-ENG-007 [api:buffer] — re-export Blob/File on the
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
            mozjs::glue::DeleteCompileOptions(blob_opts);
            if blob_rval.is_object() {
                rooted!(&in(cx) let blob_obj = blob_rval.to_object());
                for prop in &["Blob", "File"] {
                    let cprop = ::std::ffi::CString::new(*prop).unwrap();
                    let mut val = UndefinedValue();
                    JS_GetProperty(
                        cx_raw,
                        blob_obj.handle().into(),
                        cprop.as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut val,
                        },
                    );
                    if !val.is_undefined() {
                        rooted!(&in(cx) let val_root = val);
                        JS_DefineProperty(
                            cx_raw,
                            mod_obj.handle().into(),
                            cprop.as_ptr(),
                            val_root.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
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
            // @trace REQ-ENG-007 [api:buffer.resolveObjectURL]
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
            mozjs_sys::jsapi::JS::Evaluate2(
                cx_raw,
                extras_opts,
                &mut extras_src_text,
                extras_rval_h,
            );
            mozjs::glue::DeleteCompileOptions(extras_opts);
            if extras_rval.is_object() {
                rooted!(&in(cx) let extras_obj = extras_rval.to_object());
                // Copy isAscii / isUtf8 / resolveObjectURL onto the module
                // object so they appear as named exports.
                for prop in &["isAscii", "isUtf8", "resolveObjectURL"] {
                    let cprop = ::std::ffi::CString::new(*prop).unwrap();
                    let mut val = UndefinedValue();
                    JS_GetProperty(
                        cx_raw,
                        extras_obj.handle().into(),
                        cprop.as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut val,
                        },
                    );
                    rooted!(&in(cx) let val_root = val);
                    JS_DefineProperty(
                        cx_raw,
                        mod_obj.handle().into(),
                        cprop.as_ptr(),
                        val_root.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
        }

        // @trace REQ-ENG-007 [api:buffer.isAscii/isUtf8] — Native SM functions
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

        // @trace REQ-ENG-007 [api:buffer.transcode] — Bun.transcode is a
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
        rooted!(&in(cx) let transcode_obj = {
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

    install_internal_errors(cx);
}

// @trace REQ-ENG-001 [api:internal/errors] — Node.js `internal/errors` module
// face (lib/internal/errors.js): a table of error-code classes exposed as
// `codes`, each carrying its Node.js `.code` and formatting its message from
// constructor arguments exactly like the Node.js messages table. Installed
// through the same cache_builtin channel as `internal/test/binding`
// (node_internal_test_binding.rs) so node test fixtures
// (`require('internal/errors')`, e.g. node/test/parallel/test-buffer-fill.js)
// resolve the real module instead of failing at load.
//
// Template fidelity: each template below mirrors the corresponding Node.js
// error message verbatim (verified against node v24):
//   ERR_OUT_OF_RANGE(str, x, y)  → RangeError
//       `The value of "${str}" is out of range. It must be ${x}. Received ${y}`
//   ERR_INVALID_ARG_TYPE(name, expected, actual) → TypeError
//       `The "${name}" argument must be ${expected}. Received ${actual}`
//   ERR_UNKNOWN_ENCODING(enc)    → TypeError `Unknown encoding: ${enc}`
//   ERR_BUFFER_OUT_OF_BOUNDS(name) → RangeError
//       `${name} is outside of buffer bounds` (or the generic message)
//   ERR_STRING_TOO_LONG(str)     → Error `${str}`
fn install_internal_errors(cx: &mut mozjs::context::JSContext) {
    let source = r#"
(function() {
  'use strict';
  var codes = {};

  function makeError(name, Base, template) {
    var K = function() {
      var args = Array.prototype.slice.call(arguments);
      var msg = template.apply(null, args);
      // Node.js constructs the base error with the formatted message.
      Base.call(this, msg);
      this.message = msg;
      this.code = name;
      Error.captureStackTrace ? Error.captureStackTrace(this, K) : void 0;
      return this;
    };
    Object.defineProperty(K, 'name', { value: name, writable: false, enumerable: false, configurable: true });
    K.prototype = Object.create(Base.prototype, {
      constructor: { value: K, writable: true, enumerable: false, configurable: true }
    });
    codes[name] = K;
    return K;
  }

  makeError('ERR_OUT_OF_RANGE', RangeError, function(str, x, y) {
    return 'The value of "' + str + '" is out of range. It must be ' + x + '. Received ' + y;
  });
  makeError('ERR_INVALID_ARG_TYPE', TypeError, function(name, expected, actual) {
    return 'The "' + name + '" argument must be ' + expected + '. Received ' + actual;
  });
  makeError('ERR_UNKNOWN_ENCODING', TypeError, function(enc) {
    return 'Unknown encoding: ' + enc;
  });
  makeError('ERR_BUFFER_OUT_OF_BOUNDS', RangeError, function(name) {
    return name
      ? (name + ' is outside of buffer bounds')
      : 'Attempt to access memory outside buffer bounds';
  });
  makeError('ERR_STRING_TOO_LONG', Error, function(str) {
    return str;
  });

  return { codes: codes };
})()
"#;
    unsafe {
        let raw_cx = cx.raw_cx();
        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let opts = mozjs::glue::NewCompileOptions(raw_cx, c"<internal/errors>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            mozjs::glue::DeleteCompileOptions(opts);
            if !ok || !rval.is_object() {
                return;
            }
            let obj = rval.to_object();
            rooted!(&in(cx) let obj_rooted = obj);
            cache_builtin(cx, "internal/errors", obj_rooted.get());
        }
    }
}

// @trace REQ-ENG-007 [api:buffer.transcode] — Bun's transcode is a
// masqueradesAsUndefined native: present on the buffer module but reports
// `typeof === "undefined"` while still being callable. We install it via the
// mozjs fork's `JS_NewEmulatesUndefinedFunction` (which creates a callable
// NativeObject carrying JSCLASS_EMULATES_UNDEFINED), so typeof yields
// "undefined" and invoking the property still dispatches here. Each call
// throws "Not implemented" (the surface Bun documents for this stub when
// iconv support is absent).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_transcode(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let msg = ::std::ffi::CString::new("Not implemented").unwrap();
    {
        let mut cx_s = unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx)) };
        mozjs::error::throw_type_error(&mut cx_s, msg.as_ref());
    }
    args.rval().set(UndefinedValue());
    false
}

// Single truth site for the Buffer.isAscii / Buffer.isUtf8 byte predicates —
// BOTH faces delegate here:
//   * require('buffer') module face — the natives below
//   * GLOBAL Buffer face — globals.rs `buffer_is_ascii` / `buffer_is_utf8`
// The predicate itself comes from the workspace validator owner
// `bun_simdutf_sys` (repo law 复用优先: workspace crate over a std hand-path);
// per-face JS plumbing (CallArgs → byte view via the house extractor
// `collect_byte_view`) stays at each registration site.
// @trace REQ-ENG-007 [api:buffer.isAscii] [code:bun_simdutf_sys]
pub(crate) fn is_ascii_bytes(bytes: &[u8]) -> bool {
    bun_simdutf_sys::validate_ascii(bytes)
}

// @trace REQ-ENG-007 [api:buffer.isUtf8] [code:bun_simdutf_sys]
pub(crate) fn is_utf8_bytes(bytes: &[u8]) -> bool {
    bun_simdutf_sys::validate_utf8(bytes)
}

// @trace REQ-ENG-007 [api:buffer.isAscii] [code:bun_simdutf_sys] — Native
// SMFunction returning a boolean primitive. Accepts Buffer/Uint8Array/
// TypedArray/DataView/ArrayBuffer. Returns true iff every byte is < 0x80.
// Validation delegates to the single owner `is_ascii_bytes`.
// Used both as a function AND as a constructor (new isAscii(buf)) — SM
// preserves primitive returns from C++ natives invoked as constructors,
// matching Bun's behaviour.
// @trace REQ-ENG-007 [api:buffer.isAscii] — input predicate for the
// isAscii/isUtf8 faces: only ArrayBuffer / Buffer / TypedArray / DataView
// qualify; every other value (strings, numbers, booleans, bigints, symbols,
// plain objects/arrays, null, undefined) makes the predicate return FALSE —
// predicate semantics, NOT a throw (globals_coverage_c3 pin:
// Buffer.isAscii(42) === false). node v24 exposes no isAscii/isUtf8 at all
// (live-verified v24.19.0: typeof Buffer.isAscii === 'undefined'), so this
// face is bao-registered with no upstream oracle to diverge from. Detached
// buffers report zero bytes and validate as empty (true).
unsafe fn input_is_buffer_source(input: JSVal) -> bool {
    if !input.is_object() {
        return false;
    }
    let obj = input.to_object();
    let is_ab = unsafe { mozjs_sys::jsapi::JS::IsArrayBufferObject(obj) };
    let is_view = unsafe { mozjs_sys::jsapi::JS_IsArrayBufferViewObject(obj) };
    is_ab || is_view
}

// pub(crate): the GLOBAL Buffer.isAscii face (globals.rs) delegates to this
// same native so both faces share one validation + truth site.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn buffer_is_ascii(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    if !input_is_buffer_source(input) {
        // Predicate semantics: non-BufferSource input → false (no throw).
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let bytes = match collect_byte_view(cx, input) {
        Some(b) => b,
        None => {
            // Detached / unreachable backing store → zero bytes → empty is ASCII.
            args.rval().set(mozjs::jsval::BooleanValue(true));
            return true;
        }
    };
    let is_ascii = is_ascii_bytes(bytes.as_slice());
    args.rval().set(mozjs::jsval::BooleanValue(is_ascii));
    true
}

// @trace REQ-ENG-007 [api:buffer.isUtf8] [code:bun_simdutf_sys] — Native
// SMFunction returning a boolean primitive. Validates UTF-8 byte sequences
// per RFC 3629 via the single owner `is_utf8_bytes` (rejects overlong
// encodings, surrogates, and malformed continuation bytes).
// pub(crate): the GLOBAL Buffer.isUtf8 face (globals.rs) delegates here.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe extern "C" fn buffer_is_utf8(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    if !input_is_buffer_source(input) {
        // Predicate semantics: non-BufferSource input → false (no throw).
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let bytes = match collect_byte_view(cx, input) {
        Some(b) => b,
        None => {
            // Detached backing store → zero bytes → empty is valid UTF-8.
            args.rval().set(mozjs::jsval::BooleanValue(true));
            return true;
        }
    };
    let is_utf8 = is_utf8_bytes(bytes.as_slice());
    args.rval().set(mozjs::jsval::BooleanValue(is_utf8));
    true
}

// @trace REQ-ENG-007 — Extracts a byte slice from a Buffer/Uint8Array/
// TypedArray/DataView/ArrayBuffer input. Returns None on null/undefined or
// unrecognized input. The returned Vec is a copy that survives GC.
// pub(crate): also reused by fetch_api for WHATWG fetch body byte forms.
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn collect_byte_view(cx: *mut JSContext, v: JSVal) -> Option<Vec<u8>> {
    use ::std::ptr;
    if v.is_null_or_undefined() {
        return None;
    }
    if !v.is_object() {
        return None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = v.to_object());
    // Try TypedArray / DataView / Buffer (JS_GetObjectAsUint8Array handles
    // all TypedArray kinds; Buffer is a Uint8Array subclass).
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data_ptr: *mut u8 = ptr::null_mut();
    let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
        obj_root.get(),
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
        obj_root.get(),
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
    let ab_unwrapped =
        mozjs_sys::jsapi::JS::GetObjectAsArrayBuffer(obj_root.get(), &mut ab_length, &mut ab_data);
    if !ab_unwrapped.is_null() && !ab_data.is_null() {
        let slice = ::std::slice::from_raw_parts(ab_data, ab_length);
        return Some(slice.to_vec());
    }
    if !ab_unwrapped.is_null() {
        return Some(Vec::new());
    }
    None
}
