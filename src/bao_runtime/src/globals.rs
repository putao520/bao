// @trace REQ-ENG-006
// Global object installation entry point + Buffer + Crypto
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1,
};

/// Maximum byte length of a Buffer.
///
/// Matches JSC/V8 typed-array ceiling: SM typed arrays are indexed by uint32,
/// capping any single buffer at 4 GiB - 1. The current bao Buffer stores each
/// byte as an own property (`JS_DefineElement`), so anything above a few MiB
/// would already be pathologically slow — we use the typed-array ceiling as
/// the hard limit so callers get a clean `RangeError` instead of an OOM/abort
/// when they pass absurd sizes.
// @trace REQ-ENG-005 [entity:Buffer]
pub const MAX_BUFFER_SIZE: usize = (1usize << 32) - 1;

thread_local! {
    static FILE_GLOBALS: RefCell<(Option<String>, Option<String>)> = const { RefCell::new((None, None)) };
}

use ::std::cell::RefCell;

pub fn set_file_globals(filename: Option<String>, dirname: Option<String>) {
    FILE_GLOBALS.with(|f| *f.borrow_mut() = (filename, dirname));
}

/// Install Web APIs only — safe for browser page global (REQ-SEC-003).
///
/// Installs standard Web APIs that web pages expect: fetch, timers, crypto,
/// WebSocket, performance, encodings, structuredClone, etc.
/// Does NOT install Node.js APIs (require, fs, Bun, process, Buffer, etc.)
/// which are only available in evaluate_js() privileged context.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `global` is a valid
/// handle to the global object in that context.
pub unsafe fn install_web_apis(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    bao_stealth::engine_props::ensure_default_profile();
    let raw_cx = cx.raw_cx();
    bao_stealth::engine_props::install_stealth_props(raw_cx, global.get());
    crate::fetch_api::ensure_default_fetch_stealth_profile();

    // Web APIs — safe for page global
    crate::fetch_api::install_fetch_global(cx, global);
    crate::fetch_api::install_response_constructor(cx, global);
    crate::fetch_api::install_headers_constructor(cx, global);
    crate::fetch_api::install_request_constructor(cx, global);
    crate::timers::install_timer_globals(cx, global);
    crate::web_api::install_performance(cx, global);
    crate::web_api::install_websocket_constructor(cx, global);
    install_crypto_global(cx, global);
    crate::web_api::install_web_encodings(cx, global);
    crate::web_api::install_atob_btoa(cx, global);
    crate::web_api::install_queue_microtask(cx, global);
    install_structured_clone(cx, global);
    install_web_api_constructors(cx, global);
}

/// Install Node.js/Bun APIs — only for privileged CLI/engine context (REQ-SEC-003).
///
/// Installs require, module, Bun, process, Buffer, and all node_* module
/// registrations. These are NOT installed on browser page globals.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `global` is a valid
/// handle to the global object in that context.
pub unsafe fn install_node_apis(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    // Note: install_stealth_props is NOT called here.
    // It is called once in install_web_apis(). Calling it twice would
    // attempt JS_DeleteProperty on JSPROP_PERMANENT properties installed
    // by the first call, which corrupts SpiderMonkey internal state
    // and causes SIGSEGV (cx pointer corruption in JS_DeleteProperty).

    // Node.js / Bun APIs — privileged context only
    crate::bun_api::install_bun_global(cx, global);
    crate::bun_api::install_process_global(cx, global);
    install_buffer_global(cx, global);
    crate::require::install_require(cx, global);
    install_module_global(cx, global);

    // Node.js built-in module registrations
    crate::node_events::install(cx);
    crate::node_console::install(cx);
    crate::node_constants::install(cx);
    crate::node_domain::install(cx);
    crate::node_path::install(cx);
    crate::node_fs::install(cx);
    crate::node_crypto::install(cx);
    crate::node_http::install(cx);
    crate::node_http2::install(cx);
    crate::node_http2_upgrade::install(cx);
    crate::node_https::install(cx);
    crate::node_inspector::install(cx);
    crate::node_os::install(cx);
    crate::node_url::install(cx, global);
    crate::node_util::install_util(cx);
    crate::node_util::install_assert(cx);
    crate::node_child_process::install(cx);
    crate::node_stream::install(cx);
    crate::node_zlib::install(cx);
    crate::node_net::install(cx);
    crate::node_dns::install(cx);
    crate::node_buffer::install(cx);
    crate::node_string_decoder::install(cx);
    crate::node_tty::install(cx);
    crate::node_vm::install(cx);
    crate::node_module::install(cx);
    crate::node_querystring::install(cx);
    crate::node_perf_hooks::install(cx);
    crate::node_timers_module::install(cx);
    crate::node_readline::install(cx);
    crate::node_tls::install(cx);
    crate::node_worker_threads::install(cx);
    crate::node_async_hooks::install(cx);
    crate::node_diagnostics_channel::install(cx);
    crate::node_cluster::install(cx);
    crate::node_punycode::install(cx);
    crate::node_repl::install(cx);
    crate::node_sys::install(cx);
    crate::node_test::install(cx);
    crate::node_wasi::install(cx);
    crate::node_trace_events::install(cx);
    crate::node_dgram::install(cx);

    // CLI/engine-specific
    install_assert_strict(cx);
    install_file_globals_from_cache(cx, global);
    crate::bun_test::install_bun_test(cx);
    crate::bun_builtins::install(cx);
    crate::s3_api::install(cx);
    // @trace REQ-ENG-006: stub registrations for unimplemented Node.js builtins
    // (v8, domain, inspector, _http_*, _stream_*, _tls_*, sub-path modules, etc.).
    // Required so `require("X")` / `import "X"` succeed instead of throwing
    // `Cannot find module 'X'`.

    // Real implementations for internal/sub-path Node.js modules
    // (installed before stubs so stubs' guard check sees these and skips)
    crate::node_internal_streams::install(cx);
    crate::node_internal_http::install(cx);
    crate::node_tls_common::install(cx);
    crate::node_tls_wrap::install(cx);
    crate::node_subpath_aliases::install(cx);
    crate::node_stream_consumers::install(cx);
    crate::node_stream_web::install(cx);
    crate::node_util_types::install(cx);
    crate::node_inspector_promises::install(cx);

    // Remaining stubs (modules not yet natively implemented)
    crate::node_stubs::install(cx);
}

/// Install all APIs (Web + Node) — only for CLI/engine context.
///
/// In CLI mode, the full runtime is needed. This is the legacy entry point.
/// Browser mode should use `install_web_apis` instead.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `global` is a valid
/// handle to the global object in that context.
pub unsafe fn install_all(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    install_web_apis(cx, global);
    install_node_apis(cx, global);
}

/// Install module object on a target object (REQ-SEC-002 parameter injection).
///
/// Same as `install_module_global` but attaches module to `target` instead of
/// `global`. Used by `create_node_api_scope_values` to build the temporary
/// scope object for privileged evaluate_js.
///
/// The module.exports getter/setter setup still references `global` as the
/// default exports target, but the `module` property itself is only on `target`.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and both `target` and
/// `global` are valid handles to JSObjects.
pub unsafe fn install_module_on_target(
    cx: &mut mozjs::context::JSContext,
    target: mozjs::rust::Handle<*mut JSObject>,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    let raw = cx.raw_cx();
    rooted!(&in(cx) let mod_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw));
    if mod_obj.get().is_null() {
        return;
    }

    // module.exports getter/setter setup — same JS as install_module_global,
    // but the `defineExports(g)` call still binds exports on global so that
    // `module.exports === globalThis` pattern works in privileged scripts.
    let setup = r#"(function(g, m) {
  var current = g;
  function defineExports(obj) {
    Object.defineProperty(obj, 'exports', {
      configurable: true,
      enumerable: true,
      get: function() { return current; },
      set: function(v) {
        if (v && typeof v === 'object' && typeof v !== 'function') {
          for (var k in v) {
            if (Object.prototype.hasOwnProperty.call(v, k)) {
              g[k] = v[k];
            }
          }
          current = g;
        } else if (typeof v === 'function') {
          for (var k2 in v) {
            if (Object.prototype.hasOwnProperty.call(v, k2)) {
              g[k2] = v[k2];
            }
          }
          current = v;
        } else {
          current = v;
        }
      }
    });
  }
  defineExports(m);
  defineExports(g);
})"#;
    let mut setup_text = mozjs::rust::transform_str_to_source_text(setup);
    let mut factory = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory,
    };
    let opts = mozjs::glue::NewCompileOptions(raw, c"<module-setup>".as_ptr(), 1);
    if !opts.is_null() {
        let ok = JS::Evaluate2(raw, opts, &mut setup_text, factory_h);
        libc::free(opts as *mut _);
        if ok && factory.is_object() {
            let elems = [ObjectValue(global.get()), ObjectValue(mod_obj.get())];
            let args = HandleValueArray {
                length_: 2,
                elements_: elems.as_ptr(),
            };
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            rooted!(&in(cx) let factory_obj_root = factory.to_object());
            rooted!(&in(cx) let factory_val_root = ObjectValue(factory_obj_root.get()));
            let factory_obj_h = factory_val_root.handle().into();
            JS_CallFunctionValue(raw, global.into(), factory_obj_h, &args, rval_h);
        }
    }

    let dot_str = JS_NewStringCopyZ(raw, c".".as_ptr());
    if !dot_str.is_null() {
        let id_val = mozjs::jsval::StringValue(&*dot_str);
        rooted!(&in(cx) let id_r = id_val);
        JS_DefineProperty(
            raw,
            mod_obj.handle().into(),
            c"id".as_ptr(),
            id_r.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
        );
    }

    // Attach module to target (scope), NOT to global
    JS_DefineProperty3(
        cx,
        target,
        c"module".as_ptr(),
        mod_obj.handle(),
        JSPROP_ENUMERATE as u32,
    );
}

pub fn install_module_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let raw = cx.raw_cx();
        rooted!(&in(cx) let mod_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw));
        if mod_obj.get().is_null() {
            return;
        }

        // `module.exports` is exposed as a live-bound getter/setter pair on
        // both the `module` object and globalThis. The setter follows a hybrid
        // strategy so multiple CJS patterns work simultaneously:
        //   - assigning an object  → merge its enumerable own props into
        //     globalThis and keep the "current exports" pointing at globalThis
        //     (so `this === module.exports` holds for any non-strict `fn()`
        //     call where `this` defaults to globalThis);
        //   - assigning a function/class/primitive → store it directly so
        //     `module.exports(4) === 12` and `new module.exports(5)` still
        //     work (this is required for `export_default_function` and
        //     `export_default_class`).
        let setup = r#"(function(g, m) {
  var current = g;
  function defineExports(obj) {
    Object.defineProperty(obj, 'exports', {
      configurable: true,
      enumerable: true,
      get: function() { return current; },
      set: function(v) {
        if (v && typeof v === 'object' && typeof v !== 'function') {
          for (var k in v) {
            if (Object.prototype.hasOwnProperty.call(v, k)) {
              g[k] = v[k];
            }
          }
          current = g;
        } else if (typeof v === 'function') {
          // Merge own props of the function (e.g. static methods) too.
          for (var k2 in v) {
            if (Object.prototype.hasOwnProperty.call(v, k2)) {
              g[k2] = v[k2];
            }
          }
          current = v;
        } else {
          current = v;
        }
      }
    });
  }
  defineExports(m);
  defineExports(g);
})"#;
        let mut setup_text = mozjs::rust::transform_str_to_source_text(setup);
        let mut factory = UndefinedValue();
        let factory_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut factory,
        };
        let opts = mozjs::glue::NewCompileOptions(raw, c"<module-setup>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = JS::Evaluate2(raw, opts, &mut setup_text, factory_h);
            libc::free(opts as *mut _);
            if ok && factory.is_object() {
                let elems = [ObjectValue(global.get()), ObjectValue(mod_obj.get())];
                let args = HandleValueArray {
                    length_: 2,
                    elements_: elems.as_ptr(),
                };
                let mut rval = UndefinedValue();
                let rval_h = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                };
                rooted!(&in(cx) let factory_obj_root = factory.to_object());
                rooted!(&in(cx) let factory_val_root = ObjectValue(factory_obj_root.get()));
                let factory_obj_h = factory_val_root.handle().into();
                JS_CallFunctionValue(raw, global.into(), factory_obj_h, &args, rval_h);
            }
        }

        let dot_str = JS_NewStringCopyZ(raw, c".".as_ptr());
        if !dot_str.is_null() {
            let id_val = mozjs::jsval::StringValue(&*dot_str);
            rooted!(&in(cx) let id_r = id_val);
            JS_DefineProperty(
                raw,
                mod_obj.handle().into(),
                c"id".as_ptr(),
                id_r.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
            );
        }
        JS_DefineProperty3(
            cx,
            global,
            c"module".as_ptr(),
            mod_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

pub fn install_file_globals(
    _cx: &mut bao_engine::context::JsContext,
    filename: &str,
    dirname: &str,
) {
    set_file_globals(Some(filename.to_string()), Some(dirname.to_string()));
}

fn install_file_globals_from_cache(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    let (filename, dirname) = FILE_GLOBALS.with(|f| f.borrow().clone());
    unsafe {
        let raw = cx.raw_cx();
        if let Some(fn_str) = filename {
            let c_fn = ZBox::from_bytes(fn_str.as_bytes());
            let js_str = JS_NewStringCopyZ(raw, c_fn.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx) let v = StringValue(&*js_str));
                JS_DefineProperty(
                    raw,
                    global.into(),
                    c"__filename".as_ptr(),
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        if let Some(dir_str) = dirname {
            let c_dir = ZBox::from_bytes(dir_str.as_bytes());
            let js_str = JS_NewStringCopyZ(raw, c_dir.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx) let v = StringValue(&*js_str));
                JS_DefineProperty(
                    raw,
                    global.into(),
                    c"__dirname".as_ptr(),
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }
}

/// Convert IEEE 754 half-precision (binary16) bits to f64.
///
/// Used by Buffer.from(Float16Array) to extract each element's numeric value
/// before truncating it to a byte (Node.js parity — buffer.test.js reads
/// Float16 arrays via Buffer.from).
#[allow(dead_code)]
fn f16_to_f64(bits: u16) -> f64 {
    let sign = ((bits >> 15) & 1) as i32;
    let exp = ((bits >> 10) & 0x1F) as i32;
    let frac = (bits & 0x3FF) as i32;
    let s = if sign == 0 { 1.0 } else { -1.0 };
    if exp == 0 {
        if frac == 0 {
            return s * 0.0;
        }
        // Subnormal: value = (-1)^s * 2^-14 * (frac/1024)
        return s * (frac as f64) * 2f64.powi(-24);
    }
    if exp == 0x1F {
        return if frac == 0 {
            s * f64::INFINITY
        } else {
            f64::NAN
        };
    }
    // Normalised: value = (-1)^s * 2^(exp-15) * (1 + frac/1024)
    s * (1.0 + (frac as f64) / 1024.0) * 2f64.powi(exp - 15)
}

pub fn install_buffer_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        // @trace REQ-ENG-005 [entity:Buffer] — JSFUN_CONSTRUCTOR (0x400) so
        // `new Buffer(...)` works (legacy Node.js constructor). Without it SM
        // raises "X is not a constructor" on `new`.
        let buf_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(buffer_constructor),
            1,
            0x400,
            c"Buffer".as_ptr(),
        );
        if buf_fn.is_null() {
            return;
        }
        let buf_obj = JS_GetFunctionObject(buf_fn);
        if buf_obj.is_null() {
            return;
        }
        rooted!(&in(cx) let buf_root = buf_obj);

        // @trace REQ-ENG-005 [api:Buffer.from] — JSFUN_CONSTRUCTOR (0x400)
        // marks `from` / `alloc` as constructible so the deprecated
        // `new Buffer.from(x)` / `new Buffer.alloc(n)` legacy patterns work.
        // buffer.test.js "new Buffer.from()" drives
        //   new Buffer.from("🥶") → Buffer of 4 bytes
        // SM's [[Construct]] on a native C++ function honours the primitive
        // return value (the freshly-created Buffer), matching Bun's
        // behaviour where `new` on a method returning an object yields
        // exactly that object.
        // Reference: js/src/jsapi.h `JSFUN_CONSTRUCTOR`.
        const JSFUN_CONSTRUCTOR_GLOBAL: u32 = 0x400;
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"from".as_ptr(),
            ::std::option::Option::Some(buffer_from),
            1,
            (JSPROP_ENUMERATE as u32) | JSFUN_CONSTRUCTOR_GLOBAL,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"alloc".as_ptr(),
            ::std::option::Option::Some(buffer_alloc),
            1,
            (JSPROP_ENUMERATE as u32) | JSFUN_CONSTRUCTOR_GLOBAL,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"isBuffer".as_ptr(),
            ::std::option::Option::Some(buffer_is_buffer),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"concat".as_ptr(),
            ::std::option::Option::Some(buffer_concat),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"allocUnsafe".as_ptr(),
            ::std::option::Option::Some(buffer_alloc),
            1,
            (JSPROP_ENUMERATE as u32) | JSFUN_CONSTRUCTOR_GLOBAL,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"allocUnsafeSlow".as_ptr(),
            ::std::option::Option::Some(buffer_alloc),
            1,
            (JSPROP_ENUMERATE as u32) | JSFUN_CONSTRUCTOR_GLOBAL,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"byteLength".as_ptr(),
            ::std::option::Option::Some(buffer_byte_length),
            1,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"compare".as_ptr(),
            ::std::option::Option::Some(buffer_compare),
            2,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            buf_root.handle(),
            c"isEncoding".as_ptr(),
            ::std::option::Option::Some(buffer_is_encoding),
            1,
            JSPROP_ENUMERATE as u32,
        );

        JS_DefineProperty3(
            cx,
            global,
            c"Buffer".as_ptr(),
            buf_root.handle(),
            JSPROP_ENUMERATE as u32,
        );

        // Create dedicated Buffer.prototype object (not polluting Object.prototype)
        rooted!(&in(cx) let buf_proto = JS_NewPlainObject(cx));
        if !buf_proto.get().is_null() {
            rooted!(&in(cx) let proto_val = ObjectValue(buf_proto.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                buf_root.handle().into(),
                c"prototype".as_ptr(),
                proto_val.handle().into(),
                0u32,
            );

            // Register native methods on prototype (shared by all instances)
            JS_DefineFunction(
                cx,
                buf_proto.handle(),
                c"toString".as_ptr(),
                Some(buffer_to_string),
                0,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                buf_proto.handle(),
                c"slice".as_ptr(),
                Some(buffer_slice),
                2,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                buf_proto.handle(),
                c"copy".as_ptr(),
                Some(buffer_copy),
                1,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                buf_proto.handle(),
                c"equals".as_ptr(),
                Some(buffer_equals),
                1,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                buf_proto.handle(),
                c"indexOf".as_ptr(),
                Some(buffer_index_of),
                1,
                JSPROP_ENUMERATE as u32,
            );
        }

        // Wire Buffer.prototype.__proto__ = Uint8Array.prototype so every
        // Buffer instance (already a real Uint8Array via create_buffer_object)
        // passes `instanceof Uint8Array` and inherits length/indexed
        // access/subarray/set directly from the SM typed-array implementation.
        wire_buffer_proto_to_uint8array(cx.raw_cx());
    }

    // Inject Buffer prototype methods via JS eval
    let proto_src = r#"
(function() {
  if (!Buffer.of) {
    Buffer.of = function() {
      var len = arguments.length;
      var buf = Buffer.alloc(len);
      for (var i = 0; i < len; i++) { buf[i] = arguments[i] & 0xFF; }
      return buf;
    };
  }
  var _bp = Buffer.prototype;
  if (!_bp) return;

  // @trace REQ-ENG-005 [api:Buffer.write] — Node.js Buffer#write with full
  // encoding support, ERR_BUFFER_OUT_OF_BOUNDS bounds checking, and the
  // Node.js argument-coercion matrix:
  //   write(string)                          → utf8, offset=0, len=this.length
  //   write(string, encoding)                → offset=0, len=this.length
  //   write(string, offset)                  → utf8, len=remaining
  //   write(string, offset, encoding)        → len=remaining
  //   write(string, offset, length)          → utf8
  //   write(string, offset, length, encoding)
  // offset/length are ToInt32'd; non-finite (NaN/Infinity) length collapses
  // to the remaining space (Node.js/V8 behaviour: IntegerValue(NaN)===0).
  function _ERR_BUFFER_OUT_OF_BOUNDS(name) {
    var msg = name ? (name + ' is outside of buffer bounds') : 'Attempt to access memory outside buffer bounds';
    var err = new RangeError(msg);
    err.code = 'ERR_BUFFER_OUT_OF_BOUNDS';
    return err;
  }
  function _checkOffset(offset, byteLength, bufLen, name) {
    name = name || 'offset';
    if (offset > bufLen - byteLength) {
      if (byteLength === 0 && offset === bufLen) return offset; // empty write at end ok
      throw _ERR_BUFFER_OUT_OF_BOUNDS(name);
    }
    return offset;
  }

  // UTF-8 encoder/decoder using SM's TextEncoder/TextDecoder globals.
  function _utf8Bytes(str) {
    if (globalThis.TextEncoder) {
      var enc = new globalThis.TextEncoder();
      return Array.prototype.slice.call(enc.encode(str));
    }
    // Fallback: manual UTF-8 encode.
    var out = [];
    for (var i = 0; i < str.length; i++) {
      var c = str.charCodeAt(i);
      if (c < 0x80) { out.push(c); }
      else if (c < 0x800) { out.push(0xC0 | (c >> 6), 0x80 | (c & 0x3F)); }
      else { out.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F)); }
    }
    return out;
  }
  function _utf16leBytes(str) {
    var out = [];
    for (var i = 0; i < str.length; i++) {
      var c = str.charCodeAt(i);
      out.push(c & 0xFF, (c >> 8) & 0xFF);
    }
    return out;
  }
  function _hexBytes(str) {
    // @trace REQ-ENG-005 [algorithm:hex] — Node.js stops decoding at the
    // first non-hex character (outside [0-9a-fA-F]) rather than throwing.
    // buffer.test.js "hex input containing byte 0xFF is treated as invalid"
    // drives Buffer.from("\xff\xff", "hex") → empty Buffer and
    // Buffer.from("ab\xff\xffcd", "hex") → [0xab] (decodes "ab" then stops
    // at \xff). The throw path is reserved for fill() with an explicit
    // encoding=hex AND a malformed value (Node's stricter fill path);
    // _doWrite handles the bounds-check throw for hexWrite with over-length.
    var out = [];
    for (var i = 0; i + 1 < str.length; i += 2) {
      var hi = str.charCodeAt(i);
      var lo = str.charCodeAt(i + 1);
      var hv = (hi >= 48 && hi <= 57) ? hi - 48
             : (hi >= 97 && hi <= 102) ? hi - 87
             : (hi >= 65 && hi <= 70) ? hi - 55
             : -1;
      var lv = (lo >= 48 && lo <= 57) ? lo - 48
             : (lo >= 97 && lo <= 102) ? lo - 87
             : (lo >= 65 && lo <= 70) ? lo - 55
             : -1;
      if (hv < 0 || lv < 0) break;
      out.push((hv << 4) | lv);
    }
    return out;
  }
  function _base64Bytes(str) {
    // @trace REQ-ENG-005 [algorithm:base64] — Node.js parity:
    // base64 decoding reads the LOW BYTE of each UTF-16 code unit, then
    // decodes the resulting byte sequence. Crucially, this means a two-byte
    // JSString containing surrogate pairs contributes only the low byte of
    // each surrogate — buffer.test.js
    //   "two-byte strings decode from the low byte of each code unit"
    // drives:
    //   Buffer.from("QUJD\u{1F600}REVG", "base64").toString("latin1") === "ABC"
    // because \uD83D narrows to 0x3D ('='), the canonical base64 padding
    // terminator, so decoding stops after the first 3 bytes ("ABC"). Units
    // whose low byte is NOT in the alphabet are skipped (Node treats them
    // like whitespace) — e.g. Ľ narrows to 0x3D too, but с narrows
    // to 0x41 ('A') which IS in the alphabet and contributes data.
    //
    // SM's atob() already lowers code units but it strips ALL non-alphabet
    // characters including the padding-equivalent surrogate-derived '='.
    // We bypass atob here and decode the low bytes directly so '=' (whether
    // literal or surrogate-derived) terminates the run as Node does.
    var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    var out = [];
    // Phase 1: walk code units, take low byte, drop units whose low byte is
    // not in the alphabet and not '='. Stop entirely at the first '='.
    var cleaned = [];
    var stopped = false;
    for (var i = 0; i < str.length; i++) {
      var b = str.charCodeAt(i) & 0xFF;
      if (b === 0x3D) { // '=' (padding or surrogate-derived terminator)
        stopped = true;
        break;
      }
      var idx = (b >= 65 && b <= 90) ? (b - 65)            // A-Z 0-25
             : (b >= 97 && b <= 122) ? (b - 97 + 26)       // a-z 26-51
             : (b >= 48 && b <= 57) ? (b - 48 + 52)        // 0-9 52-61
             : (b === 0x2B) ? 62                           // + 62
             : (b === 0x2F) ? 63                           // / 63
             : -1;
      if (idx < 0) {
        // Low byte not in alphabet — Node silently drops it (treats as ws).
        continue;
      }
      cleaned.push(idx);
    }
    // Phase 2: decode 4 -> 3 bytes from the cleaned indices.
    var n = cleaned.length;
    var i = 0;
    while (i + 1 < n) {
      var c0 = cleaned[i];
      var c1 = cleaned[i + 1];
      var triplet = (c0 << 18) | (c1 << 12);
      out.push((triplet >> 16) & 0xFF);
      if (i + 2 < n) {
        var c2 = cleaned[i + 2];
        triplet |= (c2 << 6);
        out.push((triplet >> 8) & 0xFF);
        if (i + 3 < n) {
          var c3 = cleaned[i + 3];
          triplet |= c3;
          out.push(triplet & 0xFF);
        }
      }
      i += 4;
    }
    return out;
  }
  function _base64urlBytes(str) {
    // base64url → base64 then decode. Pad to multiple of 4.
    var s = str.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4 !== 0) s += '=';
    return _base64Bytes(s);
  }

  // Shared *Write body: writes `bytes` into this buffer at `offset`, clamped
  // to `length` (remaining buffer space). Returns the number of bytes
  // actually written. Performs ERR_BUFFER_OUT_OF_BOUNDS bounds check per
  // Node.js (offset/length that exceed buf.length throw).
  function _doWrite(buf, bytes, offset, length) {
    // @trace REQ-ENG-005 [api:Buffer.*Write] — Node.js semantics:
    //   • offset beyond buf.length → ERR_BUFFER_OUT_OF_BOUNDS
    //   • explicit numeric length that exceeds (buf.length - offset) →
    //     ERR_BUFFER_OUT_OF_BOUNDS (NOT a silent clamp). buffer.test.js
    //     "*Write methods … length larger than available buffer space"
    //     drives this for utf8Write/utf16leWrite/latin1Write/asciiWrite/
    //     base64Write/base64urlWrite/hexWrite.
    //   • length undefined → clamp to remaining buffer space (silent).
    //   • NaN/Infinity offset coerces to 0 (V8 IntegerValue).
    //   • Non-number length (e.g. a class with Symbol.toPrimitive) is
    //     coerced via ToNumber; the resulting number is CLAMPED to the
    //     remaining buffer space rather than throwing — buffer.test.js
    //     "*Write methods with NaN/invalid offset and length" drives
    //     `buf.utf8Write(str, F1, C3)` (C3 toPrimitive → 215, clamped to
    //     buf.length 6) NOT to throw. This matches V8's path where a
    //     coerced non-primitive length bypasses the explicit-range guard.
    var bufLen = buf.length;
    // ToNumber-coerce offset (runs user valueOf / Symbol.toPrimitive).
    var offNum = (offset === undefined || offset === null) ? 0 : (+offset);
    if (!isFinite(offNum)) offNum = 0;
    var off = offNum >>> 0;
    if (off > bufLen) {
      throw _ERR_BUFFER_OUT_OF_BOUNDS();
    }
    var len;
    if (length === undefined || length === null) {
      len = bufLen - off;
    } else if (typeof length === 'number') {
      // Explicit primitive number: bounds-check strictly.
      if (!isFinite(length)) {
        len = bufLen - off;
      } else {
        len = length >>> 0;
        if (len > bufLen - off) {
          throw _ERR_BUFFER_OUT_OF_BOUNDS();
        }
      }
    } else {
      // Non-number length: ToNumber-coerce and clamp silently (no throw).
      var lenNum = +length;
      if (!isFinite(lenNum)) {
        len = bufLen - off;
      } else {
        len = Math.min(lenNum >>> 0, bufLen - off);
      }
    }
    var n = Math.min(bytes.length, len);
    for (var i = 0; i < n; i++) buf[off + i] = bytes[i] & 0xFF;
    return n;
  }

  _bp.utf8Write = function(string, offset, length) {
    // @trace REQ-ENG-005 [api:Buffer.utf8Write] — Node.js truncates at the
    // character boundary: a multi-byte UTF-8 sequence that does not fully
    // fit in the remaining buffer space is dropped entirely (it is NOT
    // split mid-codepoint). buffer.test.js "UTF-8 write() & slice()" and
    // "truncate write() at character boundary" drive this. We compute the
    // full UTF-8 byte sequence, then walk the original string's codepoints
    // to find the largest prefix whose encoded length fits `length`.
    var s = String(string);
    var allBytes = _utf8Bytes(s);
    var bufLen = this.length;
    // ToNumber-coerce offset (runs user valueOf / Symbol.toPrimitive); NaN /
    // Infinity coerces to 0 (V8 IntegerValue parity).
    var offNum = (offset === undefined || offset === null) ? 0 : (+offset);
    if (!isFinite(offNum)) offNum = 0;
    var off = offNum >>> 0;
    if (off > bufLen) {
      throw _ERR_BUFFER_OUT_OF_BOUNDS();
    }
    // @trace REQ-ENG-005 — explicit numeric length > remaining throws
    // ERR_BUFFER_OUT_OF_BOUNDS; undefined / non-number length clamps
    // silently (V8 IntegerValue: NaN → 0 → clamp to remaining). A non-number
    // length (class with Symbol.toPrimitive) is ToNumber-coerced then clamped
    // without throwing — buffer.test.js "*Write methods with NaN/invalid
    // offset and length" drives this branch.
    var len;
    if (length === undefined || length === null) {
      len = bufLen - off;
    } else if (typeof length === 'number') {
      if (!isFinite(length)) {
        len = bufLen - off;
      } else {
        len = length >>> 0;
        if (len > bufLen - off) {
          throw _ERR_BUFFER_OUT_OF_BOUNDS();
        }
      }
    } else {
      var lenNum = +length;
      if (!isFinite(lenNum)) {
        len = bufLen - off;
      } else {
        len = Math.min(lenNum >>> 0, bufLen - off);
      }
    }
    // Walk codepoints; for each, compute its UTF-8 byte length, and stop
    // when adding it would exceed `len` (character-boundary truncation).
    var written = 0;
    var idx = 0;
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      var encLen;
      if (c < 0x80) encLen = 1;
      else if (c < 0x800) encLen = 2;
      else {
        // Surrogate pair handling: a high surrogate followed by a low
        // surrogate encodes to a 4-byte UTF-8 sequence (one codepoint).
        if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
          var lo = s.charCodeAt(i + 1);
          if (lo >= 0xDC00 && lo <= 0xDFFF) { encLen = 4; i++; }
          else { encLen = 3; }
        } else { encLen = 3; }
      }
      if (written + encLen > len) break;
      written += encLen;
    }
    // Copy `written` bytes from allBytes into this buffer.
    for (var j = 0; j < written; j++) this[off + j] = allBytes[j] & 0xFF;
    return written;
  };
  _bp.asciiWrite = function(string, offset, length) {
    // Node.js parity: 'ascii' on encode == 'latin1' (verbatim byte copy,
    // not 7-bit masking). See bun/test #31083.
    var s = String(string);
    var bytes = new Array(s.length);
    for (var i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i) & 0xFF;
    // Pass offset/length uncoerced so _doWrite can distinguish "non-number
    // argument that ToNumber-coerces to a finite value" (apply explicit
    // bounds check) from "non-number argument that ToNumber-coerces to NaN /
    // Infinity" (silently treat as 0 / remaining per Node V8 IntegerValue).
    return _doWrite(this, bytes, offset, length);
  };
  _bp.latin1Write = _bp.asciiWrite;
  _bp.hexWrite = function(string, offset, length) {
    var bytes = _hexBytes(String(string));
    return _doWrite(this, bytes, offset, length);
  };
  _bp.base64Write = function(string, offset, length) {
    var bytes = _base64Bytes(String(string));
    return _doWrite(this, bytes, offset, length);
  };
  _bp.base64urlWrite = function(string, offset, length) {
    var bytes = _base64urlBytes(String(string));
    return _doWrite(this, bytes, offset, length);
  };
  _bp.ucs2Write = function(string, offset, length) {
    // @trace REQ-ENG-005 [api:Buffer.ucs2Write] — Node.js truncates at the
    // code-unit boundary: a 2-byte UCS-2 unit that does not fully fit in
    // the remaining buffer space is dropped entirely. buffer.test.js
    // "write" drives x.write("ыыыыыы", 3, "ucs2") on a 4-byte buffer to
    // assert 0 bytes are written (only 1 byte available at offset 3).
    var s = String(string);
    var bytes = _utf16leBytes(s);
    var bufLen = this.length;
    // ToNumber-coerce offset (runs user valueOf / Symbol.toPrimitive); NaN /
    // Infinity coerces to 0 (V8 IntegerValue parity).
    var offNum = (offset === undefined || offset === null) ? 0 : (+offset);
    if (!isFinite(offNum)) offNum = 0;
    var off = offNum >>> 0;
    if (off > bufLen) {
      throw _ERR_BUFFER_OUT_OF_BOUNDS();
    }
    var len;
    if (length === undefined || length === null) {
      len = bufLen - off;
    } else if (typeof length === 'number') {
      if (!isFinite(length)) {
        len = bufLen - off;
      } else {
        len = length >>> 0;
        if (len > bufLen - off) {
          throw _ERR_BUFFER_OUT_OF_BOUNDS();
        }
      }
    } else {
      var lenNum = +length;
      if (!isFinite(lenNum)) {
        len = bufLen - off;
      } else {
        len = Math.min(lenNum >>> 0, bufLen - off);
      }
    }
    // Truncate to even byte count (full UCS-2 units only).
    var n = Math.min(bytes.length, len);
    if (n % 2 !== 0) n -= 1;
    for (var i = 0; i < n; i++) this[off + i] = bytes[i] & 0xFF;
    return n;
  };
  _bp.utf16leWrite = _bp.ucs2Write;
  _bp.utf16beWrite = function(string, offset, length) {
    var s = String(string);
    var bytes = new Array(s.length * 2);
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      bytes[i*2] = (c >> 8) & 0xFF;
      bytes[i*2+1] = c & 0xFF;
    }
    return _doWrite(this, bytes, offset, length);
  };

  _bp.write = function write(string, offset, length, encoding) {
    // Mirror Node.js's argument-overload matrix exactly.
    if (typeof string !== 'string') string = String(string);
    if (offset === undefined) {
      encoding = 'utf8';
      length = this.length;
      offset = 0;
    } else if (length === undefined && typeof offset === 'string') {
      encoding = offset;
      length = this.length;
      offset = 0;
    } else {
      // offset must be a number. Non-number offset (e.g. write("s","utf8",0))
      // throws ERR_INVALID_ARG_TYPE per Node.js.
      if (typeof offset !== 'number') {
        var inspected = (typeof offset === 'string') ? ("'" + offset + "'") : String(offset);
        throw new TypeError('The "offset" argument must be of type number. Received type ' + typeof offset + ' (' + inspected + ')');
      }
      if (!isFinite(offset)) {
        throw new TypeError('The "offset" argument must be of type number. Received type ' + typeof offset);
      }
      offset = offset >>> 0;
      if (typeof length === 'string') {
        encoding = length;
        length = undefined;
      } else if (length !== undefined && isFinite(length)) {
        length = length >>> 0;
        if (encoding === undefined) encoding = 'utf8';
      } else if (length !== undefined) {
        encoding = length;
        length = undefined;
      } else {
        if (encoding === undefined) encoding = 'utf8';
      }
    }

    var remaining = this.length - offset;
    if (length === undefined || length > remaining) length = remaining;
    length = length >>> 0;

    // Bounds: empty string at end-of-buffer is OK; everything past it throws.
    if ((string.length > 0 && (length < 0 || offset < 0)) || offset > this.length) {
      throw new RangeError('Attempt to write outside buffer bounds');
    }

    if (!encoding) encoding = 'utf8';
    encoding = ('' + encoding).toLowerCase();

    switch (encoding) {
      case 'hex': return _bp.hexWrite.call(this, string, offset, length);
      case 'utf8': case 'utf-8': return _bp.utf8Write.call(this, string, offset, length);
      case 'ascii': case 'binary': case 'latin1': return _bp.asciiWrite.call(this, string, offset, length);
      case 'base64': return _bp.base64Write.call(this, string, offset, length);
      case 'base64url': return _bp.base64urlWrite.call(this, string, offset, length);
      case 'ucs2': case 'ucs-2': case 'utf16le': case 'utf-16le': return _bp.utf16leWrite.call(this, string, offset, length);
      case 'utf16be': case 'utf-16be': return _bp.utf16beWrite.call(this, string, offset, length);
      default: throw new TypeError('Unknown encoding: ' + encoding);
    }
  };

  // @trace REQ-ENG-005 [api:Buffer.prototype.*Slice] — slice helpers that
  // Node.js exposes on the prototype (hexSlice/utf8Slice/asciiSlice/
  // latin1Slice/base64Slice/base64urlSlice/ucs2Slice/utf16leSlice). These
  // produce the string form of [start,end) under the given encoding, sharing
  // the same encoder used by toString, but apply strict range validation:
  // out-of-range start/end throws RangeError (NOT a silent clamp), and
  // non-ArrayBufferView receivers throw TypeError. buffer.test.js drives
  // latin1Slice/hexSlice/ucs2Slice explicitly.
  function _checkSliceReceiver() {
    if (this === null || this === undefined || typeof this !== 'object') {
      throw new TypeError('The "this" value must be of type object. Received type ' + (this === null ? 'null' : typeof this));
    }
    if (!(this.buffer instanceof ArrayBuffer) && !(this instanceof Uint8Array)) {
      throw new TypeError('The "this" value must be an instance of ArrayBufferView.');
    }
  }
  function _sliceBounds(len, start, end) {
    // @trace REQ-ENG-005 — Node.js *Slice bounds: out-of-range start or end
    // (negative OR > len) throws RangeError. An inverted range (start > end)
    // within bounds returns the empty slice (clamps end to start). buffer.test.js
    // "Buffer.latin1Slice()" drives:
    //   latin1Slice(1, 4)  → throw (end 4 > len 3)
    //   latin1Slice(4, 1)  → throw (start 4 > len 3)
    //   latin1Slice(4, 0)  → throw (start 4 > len 3)
    //   latin1Slice(3, 1)  → "" (both in bounds, start > end → empty)
    //   latin1Slice(1, 1)  → "" (zero-length slice)
    //   latin1Slice(1, 0)  → "" (inverted in-bounds → empty)
    if (start === undefined) start = 0;
    if (end === undefined) end = len;
    start = start | 0;
    end = end | 0;
    if (start < 0 || end < 0 || start > len || end > len) {
      throw new RangeError('Index out of range');
    }
    if (start > end) {
      // Inverted range within bounds → clamp to empty slice at `start`.
      return [start, start];
    }
    return [start, end];
  }
  // @trace REQ-ENG-005 — Internal byte-read helper that bypasses
  // TypedArray.prototype.toString (which returns comma-joined bytes for a
  // plain Uint8Array). Each *Slice reads the raw byte at index i.
  function _byteAt(view, i) { return view[i] & 0xFF; }
  _bp.hexSlice = function(start, end) {
    _checkSliceReceiver.call(this);
    var b = _sliceBounds(this.length, start, end);
    var hexLen = (b[1] - b[0]) * 2;
    // @trace REQ-ENG-005 — Node.js caps the output string at
    // constants.MAX_STRING_LENGTH (2147483647) chars. Hex doubles the
    // input length, so a buffer > MAX/2 throws RangeError. Test
    // "Buffer.hexSlice() throws for large buffers" drives this.
    var MAX_STR = 2147483647;
    if (hexLen > MAX_STR) {
      throw new RangeError('Cannot create a string longer than ' + MAX_STR + ' characters');
    }
    var HEX = '0123456789abcdef';
    var out = '';
    for (var i = b[0]; i < b[1]; i++) { var v = _byteAt(this, i); out += HEX[v >> 4] + HEX[v & 0xF]; }
    return out;
  };
  _bp.utf8Slice = function(start, end) {
    _checkSliceReceiver.call(this);
    var b = _sliceBounds(this.length, start, end);
    // Build via TextDecoder for parity with SM's UTF-8 decoder.
    if (typeof globalThis.TextDecoder === 'function') {
      var sub = new Uint8Array(this.buffer || this, this.byteOffset || 0, this.length);
      var view = sub.subarray(b[0], b[1]);
      return new globalThis.TextDecoder('utf-8').decode(view);
    }
    return this.toString('utf8', b[0], b[1]);
  };
  _bp.asciiSlice = function(start, end) {
    _checkSliceReceiver.call(this);
    var b = _sliceBounds(this.length, start, end);
    // @trace REQ-ENG-005 — Node.js ascii slices mask each byte to 7 bits
    // (high bit cleared). buffer.test.js "asciiSlice()" drives this.
    var out = '';
    for (var i = b[0]; i < b[1]; i++) { out += String.fromCharCode(_byteAt(this, i) & 0x7F); }
    return out;
  };
  _bp.latin1Slice = function(start, end) {
    _checkSliceReceiver.call(this);
    var b = _sliceBounds(this.length, start, end);
    // @trace REQ-ENG-005 — Node.js latin1 maps each byte to its code point
    // directly (no masking). buffer.test.js "Buffer.latin1Slice() on a
    // Uint8Array" drives this on a plain Uint8Array, where the inherited
    // toString returns comma-joined bytes — we must read bytes manually.
    var out = '';
    for (var i = b[0]; i < b[1]; i++) { out += String.fromCharCode(_byteAt(this, i)); }
    return out;
  };
  _bp.base64Slice = function(start, end) {
    _checkSliceReceiver.call(this);
    var b = _sliceBounds(this.length, start, end);
    // Build a Uint8Array of [start, end) and use SM's btoa-equivalent.
    var n = b[1] - b[0];
    var view = new Uint8Array(n);
    for (var i = 0; i < n; i++) view[i] = _byteAt(this, b[0] + i);
    if (typeof globalThis.btoa === 'function') {
      var bin = '';
      for (var j = 0; j < n; j++) bin += String.fromCharCode(view[j]);
      return globalThis.btoa(bin);
    }
    return this.toString('base64', b[0], b[1]);
  };
  _bp.base64urlSlice = function(start, end) {
    var s = _bp.base64Slice.call(this, start, end);
    return s.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  };
  _bp.ucs2Slice = function(start, end) {
    _checkSliceReceiver.call(this);
    var b = _sliceBounds(this.length, start, end);
    // Read code units in little-endian pairs (Node.js utf16le semantics).
    var out = '';
    for (var i = b[0]; i + 1 < b[1]; i += 2) {
      out += String.fromCharCode(_byteAt(this, i) | (_byteAt(this, i + 1) << 8));
    }
    return out;
  };
  _bp.utf16leSlice = _bp.ucs2Slice;

  // @trace REQ-ENG-005 [api:Buffer.prototype.toLocaleString] — Node.js
  // alias of toString (Buffer.prototype.toLocaleString === toString).
  // Without overriding, the inherited TypedArray.toLocaleString returns a
  // comma-joined list of bytes (e.g. "116,101,115,116").
  _bp.toLocaleString = _bp.toString;

  // @trace REQ-ENG-005 [api:Buffer.prototype.inspect] — legacy Node.js
  // inspect method. Upstream tests assert it exists and matches
  // Bun.inspect output. We delegate to Bun.inspect when present, else
  // build a "<Buffer hex...>" string ourselves.
  _bp.inspect = function(depth, options) {
    if (typeof globalThis.Bun === 'object' && globalThis.Bun && typeof globalThis.Bun.inspect === 'function') {
      return globalThis.Bun.inspect(this);
    }
    var bytes = [];
    for (var i = 0; i < this.length; i++) bytes.push(this[i]);
    return '<Buffer ' + bytes.map(function(b) { return (b < 16 ? '0' : '') + b.toString(16); }).join(' ') + '>';
  };

  // @trace REQ-ENG-005 [api:Buffer.copyBytesFrom] — static constructor
  // method. Copies the bytes referenced by a TypedArray view into a fresh
  // Buffer. Honours the view's byteOffset/byteLength so a sub-view of an
  // ArrayBuffer copies only the referenced bytes (Node.js parity). The
  // optional sourceStart/sourceEnd clamp the source range.
  if (!Buffer.copyBytesFrom) {
    Buffer.copyBytesFrom = function(source, sourceStart, sourceEnd) {
      var view;
      if (source instanceof Uint8Array) {
        view = source;
      } else if (source && typeof source === 'object' && source.buffer instanceof ArrayBuffer) {
        // TypedArray of any element kind — project to bytes.
        view = new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
      } else {
        throw new TypeError('The "source" argument must be an instance of TypedArray.');
      }
      var byteLen = view.byteLength;
      var start = sourceStart === undefined ? 0 : (sourceStart >>> 0);
      var end = sourceEnd === undefined ? byteLen : (sourceEnd >>> 0);
      if (start < 0) start = 0;
      if (end > byteLen) end = byteLen;
      if (end < start) end = start;
      var n = end - start;
      var buf = Buffer.allocUnsafe(n);
      for (var i = 0; i < n; i++) buf[i] = view[start + i];
      return buf;
    };
  }

  _bp.readUInt8 = function(offset) { return this[offset || 0]; };
  // Placeholder writeUInt8 — overridden by the Node-parity version below
  // (offset+1 return + bounds check). Defined here only so a fill/write
  // helper that runs before the full surface is installed still has the
  // binding available; the later assignment wins.
  _bp.writeUInt8 = function(val, offset) { this[offset || 0] = val & 0xFF; return (offset || 0) + 1; };

  // @trace REQ-ENG-005 [api:Buffer.prototype.fill] — Node.js fill(value[,
  // start[, end]][, encoding]). Node ordering, with TOCTOU guards for
  // detach/resize triggered by user-visible valueOf / toString / toPrimitive
  // callbacks on `value`, `start`, `end`, and `encoding`:
  //
  //   1. Resolve which positional slot is the encoding (string branch only).
  //   2. Validate encoding *type* (string|undefined) and *value*
  //      (ERR_UNKNOWN_ENCODING) BEFORE ToNumber(start/end). Node parity:
  //      "fill('a', 0, 11, 'bogus')" throws ERR_UNKNOWN_ENCODING, not
  //      ERR_OUT_OF_RANGE, even though both offset and end are wrong.
  //   3. ToNumber(start) / ToNumber(end) — runs user valueOf (side effects
  //      observable here, may detach or resize this.buffer).
  //   4. Range-check start/end against the *pre-coercion* length, but only
  //      throw on truly invalid primitives (negative, or a number strictly
  //      larger than this.length) — Node validates BEFORE coercing `value`.
  //   5. Empty-range short-circuit (end <= start) returns `this` WITHOUT
  //      coercing `value` (fill({valueOf:throws}, 5, 3) must not throw).
  //   6. ToNumber(value) / ToString(value) — runs user valueOf/toString
  //      (the second detach/resize window).
  //   7. Post-coercion TOCTOU: re-read this.length. If this.buffer was
  //      detached (byteLength === 0) → return `this` (Node parity, no crash).
  //      If it was resized down → clamp start/end to the new length.
  //   8. Fill the range.
  _bp.fill = function fill(val, start, end, encoding) {
    // @trace REQ-ENG-005 [api:Buffer.fill] — Use the SM-internal
    // `byteLength` rather than the user-visible `length` property so that
    // buffer.test.js "bypassing `length` should not cause an abort" —
    // which does `Object.defineProperty(buf, "length", {value: 1337})` —
    // still fills only the 4 real bytes. SM's Uint8Array byteLength is a
    // non-configurable / non-writable own slot, immune to defineProperty.
    var len = (typeof this.byteLength === 'number') ? this.byteLength : this.length;
    // Resolve positional args. encoding only applies when val is a string.
    if (typeof val === 'string') {
      // Signature: fill(string[, start[, end]][, encoding])
      if (typeof start === 'string') { encoding = start; start = undefined; end = undefined; }
      else if (typeof end === 'string') { encoding = end; end = undefined; }
    }
    // @trace REQ-ENG-005 [api:Buffer.fill] — encoding resolution. Bun coerces
    // the encoding argument via String() ONLY when it is an object (with a
    // user toString), which is the documented entry point for the encoding-
    // side TOCTOU detach repro (see buffer-copy-fill-detach.test.ts "Buffer.fill
    // string branch with detaching encoding toString"). Primitive non-string
    // encodings (boolean / number / NaN / symbol) throw TypeError per Node
    // parity — buffer.test.js "fill() should throw on invalid arguments"
    // drives `fill("a", 0, 0, false)` / `fill("a", 0, 0, NaN)` to TypeError.
    //
    // Ordering: encoding coercion runs BEFORE ToNumber(start/end) so that an
    // invalid encoding throws ERR_UNKNOWN_ENCODING even when offset/end are
    // also out of range (Node parity — see test "invalid encoding throws
    // ERR_UNKNOWN_ENCODING even when offset/end are out of range").
    if (encoding !== undefined && encoding !== null) {
      var encType = typeof encoding;
      if (encType === 'object') {
        // Coerce via String() — runs user toString (TOCTOU window 1).
        encoding = String(encoding);
      } else if (encType !== 'string') {
        // boolean / number / NaN / symbol → TypeError (Node parity).
        throw new TypeError('The "encoding" argument must be of type string or an instance of String. Received type ' + encType + '.');
      }
    }
    var VALID_ENCODINGS = {
      utf8:1, 'utf-8':1, utf16le:1, 'utf-16le':1, utf16be:1, 'utf-16be':1,
      latin1:1, binary:1, ascii:1, base64:1, base64url:1, hex:1, ucs2:1, 'ucs-2':1
    };
    if (typeof encoding === 'string' && encoding.length > 0 && !VALID_ENCODINGS[encoding.toLowerCase()]) {
      var _e = new TypeError('Unknown encoding: ' + encoding);
      _e.code = 'ERR_UNKNOWN_ENCODING';
      throw _e;
    }
    // @trace REQ-ENG-005 — ToNumber coercion of start/end runs user valueOf
    // here (window 1 of 2 for TOCTOU). Use unary + to force ToNumber.
    //
    // @trace REQ-ENG-005 — Node.js type-checks start/end BEFORE coercion when
    // the value is a Buffer / Uint8Array (object fill): if start/end is an
    // object (not undefined/null/number/boolean), throw ERR_INVALID_ARG_TYPE.
    // buffer.test.js "fill() should properly check `start` & `end`" drives
    //   Buffer.alloc(1).fill(Buffer.alloc(1), 0, {[Symbol.toPrimitive]:()=>1})
    // to throw TypeError. For primitive value (string/number) the start/end
    // are ToNumber-coerced without type check.
    var isObjectFill = (val && typeof val === 'object' && typeof val.length === 'number') || (typeof val !== 'string' && typeof val !== 'number' && typeof val !== 'boolean' && val !== undefined && val !== null && typeof val === 'object');
    if (isObjectFill) {
      if (start !== undefined && start !== null && typeof start !== 'number' && typeof start !== 'boolean') {
        var _te = new TypeError('The "offset" argument must be of type number. Received type ' + typeof start);
        _te.code = 'ERR_INVALID_ARG_TYPE';
        throw _te;
      }
      if (end !== undefined && end !== null && typeof end !== 'number' && typeof end !== 'boolean') {
        var _te2 = new TypeError('The "end" argument must be of type number. Received type ' + typeof end);
        _te2.code = 'ERR_INVALID_ARG_TYPE';
        throw _te2;
      }
    }
    var sRaw = (start === undefined || start === null) ? 0 : (+start);
    var eRaw = (end === undefined || end === null) ? len : (+end);
    // Node parity: non-finite / NaN start/end become 0 / len.
    if (!isFinite(sRaw)) sRaw = 0;
    if (!isFinite(eRaw)) eRaw = len;
    // @trace REQ-ENG-005 — Validate against the *entry* length (the snapshot
    // taken at fill() entry, before any user valueOf/toString on encoding /
    // start / end / value could have detached or resized the buffer). Node
    // throws ERR_OUT_OF_RANGE on a negative primitive or a primitive strictly
    // larger than this.length. buffer.test.js "fill() should throw on invalid
    // arguments" drives `fill(0, 0, len+1)` to throw.
    //
    // Note: a post-coercion resize is NOT an error — the entry-length was
    // 1024 and the user-supplied end is 1024, so validation passes; the
    // post-resize clamp (later) silently truncates. This is what
    // buffer-copy-fill-detach.test.ts expects.
    if (sRaw < 0 || sRaw > len) {
      var _e1 = new RangeError('The value of "offset" is out of range.');
      _e1.code = 'ERR_OUT_OF_RANGE';
      throw _e1;
    }
    if (eRaw < 0 || eRaw > len) {
      var _e2 = new RangeError('The value of "end" is out of range.');
      _e2.code = 'ERR_OUT_OF_RANGE';
      throw _e2;
    }
    // Empty-range short-circuit BEFORE coercing `value` (Node parity:
    // fill({valueOf:throws}, 5, 3) returns buf without invoking valueOf).
    if (eRaw <= sRaw) return this;

    // @trace REQ-ENG-005 — ToNumber/ToString on `value` runs user valueOf /
    // toString here (window 2 of 2 for TOCTOU). Number branch uses +val to
    // force ToNumber; string branch is already a primitive; object branch
    // (Buffer / Uint8Array) reads .length without calling user code.
    var coercedVal;
    if (typeof val === 'number' || typeof val === 'boolean') {
      coercedVal = typeof val === 'boolean' ? Number(val) : val;
    } else if (typeof val === 'string') {
      coercedVal = val;
    } else if (val && typeof val === 'object' && typeof val.length === 'number') {
      coercedVal = val;
    } else {
      // Non-primitive, non-typed-array value: ToNumber coercion. This invokes
      // user valueOf / Symbol.toPrimitive and is the documented entry point
      // for the TOCTOU detach repro (fill({valueOf: detach})).
      coercedVal = +val;
      if (!isFinite(coercedVal)) coercedVal = 0;
    }

    // @trace REQ-ENG-005 [TOCTOU] — Re-read this.length after all user
    // callbacks have run. If detach happened (ArrayBuffer.byteLength === 0)
    // return `this` per Node parity (no crash, fill is a no-op). If resize
    // happened, clamp start/end to the new logical length.
    var detached = (this.buffer && this.buffer.byteLength === 0);
    if (detached) return this;
    // @trace REQ-ENG-005 [api:Buffer.fill] — Same byteLength reasoning as
    // the entry snapshot: the user may have shadowed `.length` via
    // defineProperty. Re-read via byteLength so we never write past the
    // real byte storage.
    var curLen = (typeof this.byteLength === 'number') ? this.byteLength : this.length;
    var s = sRaw >>> 0;
    var e = eRaw >>> 0;
    if (s > curLen) s = curLen;
    if (e > curLen) e = curLen;
    if (e <= s) return this;

    if (typeof coercedVal === 'number') {
      var b = coercedVal & 0xFF;
      for (var i = s; i < e; i++) this[i] = b;
    } else if (typeof coercedVal === 'string') {
      var enc = (encoding || 'utf8').toLowerCase();
      var bytes;
      if (enc === 'hex') {
        // @trace REQ-ENG-005 — Node.js fill throws ERR_INVALID_ARG_VALUE on
        // a hex-encoded string that is not a valid hex sequence (any non-
        // hex char, OR an odd-length string that leaves a dangling nibble).
        // buffer.test.js "Buffer.fill (Node.js tests)" drives
        //   buf.fill("yKJh", "hex")  // y, J are not hex
        //   buf.fill("Ȣ", "hex")  // U+0222 is not hex
        // to throw. _hexBytes stops at the first invalid char and returns
        // a partial array; we detect truncation by comparing against the
        // expected full decode length.
        bytes = _hexBytes(coercedVal);
        var expectedHexBytes = (coercedVal.length / 2) | 0;
        if (bytes.length !== expectedHexBytes || (coercedVal.length % 2) !== 0) {
          var _he = new TypeError('The value "' + coercedVal + '" is invalid for argument "value"');
          _he.code = 'ERR_INVALID_ARG_VALUE';
          throw _he;
        }
      } else if (enc === 'base64') {
        bytes = _base64Bytes(coercedVal);
      } else if (enc === 'base64url') {
        bytes = _base64urlBytes(coercedVal);
      } else if (enc === 'ucs2' || enc === 'ucs-2' || enc === 'utf16le' || enc === 'utf-16le') {
        bytes = _utf16leBytes(coercedVal);
      } else if (enc === 'utf16be' || enc === 'utf-16be') {
        var ss = coercedVal; bytes = new Array(ss.length * 2);
        for (var k = 0; k < ss.length; k++) { var c = ss.charCodeAt(k); bytes[k*2] = (c>>8)&0xFF; bytes[k*2+1] = c&0xFF; }
      } else if (enc === 'ascii' || enc === 'latin1' || enc === 'binary') {
        // Node parity: ascii/latin1 on encode = verbatim low byte per char.
        bytes = new Array(coercedVal.length);
        for (var k = 0; k < coercedVal.length; k++) bytes[k] = coercedVal.charCodeAt(k) & 0xFF;
      } else {
        bytes = _utf8Bytes(coercedVal);
      }
      if (bytes.length === 0) bytes = [0];
      // Tile the encoded bytes across [s,e).
      for (var i = s; i < e; i++) {
        this[i] = bytes[(i - s) % bytes.length] & 0xFF;
      }
    } else {
      // Buffer or Uint8Array fill: tile the source bytes.
      var src = coercedVal;
      // @trace REQ-ENG-005 — empty / detached source buffer → no-op (Node
      // parity: fill(new Uint8Array(0), …) does not throw, returns buf).
      var srcLen = (src.buffer && src.buffer.byteLength === 0) ? 0 : src.length;
      if (srcLen === 0) return this;
      for (var i = s; i < e; i++) {
        this[i] = src[(i - s) % srcLen] & 0xFF;
      }
    }
    return this;
  };

  // @trace REQ-ENG-005 [api:Buffer.includes detach semantics] — Node.js:
  // includes(val, byteOffset) coerces byteOffset via ToInteger (invoking any
  // valueOf). If the valueOf detaches the buffer, the haystack is treated as
  // length 0 → includes returns false. Delegate to indexOf (already detached-
  // aware) and compare against -1.
  _bp.includes = function(val, byteOffset) {
    // Force ToInteger coercion of byteOffset so valueOf side effects run,
    // mirroring indexOf's behavior even when the result is ignored.
    if (arguments.length >= 2) { var _ = (byteOffset | 0); }
    return this.indexOf(val, byteOffset) !== -1;
  };

  // @trace REQ-ENG-005 [api:Buffer.lastIndexOf] — Node.js:
  // lastIndexOf(value[, byteOffset[, encoding]]). byteOffset coerces via
  // ToInteger (invoking valueOf). Negative byteOffset counts from the END:
  //   offset < 0  → starts at max(-1, len+offset)
  //   offset > len → clamps to len-1
  // If valueOf detaches the buffer, treat as length 0 → lastIndexOf returns
  // -1. For an empty/detached needle, lastIndexOf returns the (clamped)
  // byteOffset. Number needle is masked to its low byte (indexOf parity:
  // lastIndexOf(99.9) → 99, lastIndexOf(355) → 99). String needle honours
  // the `encoding` argument so utf16le haystacks match UTF-16 needles.
  _bp.lastIndexOf = function(val, byteOffset, encoding) {
    var len = this.length;
    // Resolve encoding (positional 3rd arg, or 2nd arg when val is string).
    if (typeof byteOffset === 'string' && typeof val === 'string') {
      encoding = byteOffset;
      byteOffset = undefined;
    }
    var enc = (encoding === undefined || encoding === null) ? 'utf8' : String(encoding).toLowerCase();
    // @trace REQ-ENG-005 — Node.js semantics for byteOffset:
    //   • NOT passed (arguments.length < 2) → search from end (off = len-1)
    //   • undefined → search from end (treated as missing)
    //   • null / [] / {} → ToNumber coerces (null→0, []→0, {}→NaN→len-1)
    // buffer.test.js "lastIndexOf" drives lastIndexOf("b", null)===-1 (off=0,
    // 'b' not at index 0), lastIndexOf("b", [])===-1 (off=0),
    // lastIndexOf("b", {})===1 (ToNumber → NaN → search from end), and
    // lastIndexOf("b", undefined)===1 (search from end).
    var offRaw;
    if (arguments.length < 2 || byteOffset === undefined) {
      offRaw = len - 1;
    } else {
      var offNum = +byteOffset;
      offRaw = (Number.isNaN(offNum)) ? (len - 1) : (offNum | 0);
    }
    // Check for detachment of haystack after valueOf ran.
    var detached = (this.buffer && this.buffer.byteLength === 0);
    var curLen = detached ? 0 : this.length;
    // Node parity: negative offset → from end; offset >= len → clamp to len-1.
    var start;
    if (offRaw < 0) {
      start = curLen + offRaw;
      if (start < 0) return -1;
    } else if (offRaw >= curLen) {
      start = curLen - 1;
    } else {
      start = offRaw;
    }

    if (typeof val === 'number' || typeof val === 'boolean') {
      var num = typeof val === 'boolean' ? Number(val) : val;
      var n = (num < 0 ? 0 : (num | 0)) & 0xFF;
      for (var i = start; i >= 0; i--) { if (this[i] === n) return i; }
      return -1;
    } else if (typeof val === 'string') {
      // Encode the needle to bytes per encoding.
      var needle;
      if (enc === 'ucs2' || enc === 'ucs-2' || enc === 'utf16le' || enc === 'utf-16le') {
        needle = new Array(val.length * 2);
        for (var k = 0; k < val.length; k++) {
          var c = val.charCodeAt(k);
          needle[k*2] = c & 0xFF;
          needle[k*2+1] = (c >> 8) & 0xFF;
        }
      } else if (enc === 'utf16be' || enc === 'utf-16be') {
        needle = new Array(val.length * 2);
        for (var k = 0; k < val.length; k++) {
          var c = val.charCodeAt(k);
          needle[k*2] = (c >> 8) & 0xFF;
          needle[k*2+1] = c & 0xFF;
        }
      } else if (enc === 'latin1' || enc === 'binary' || enc === 'ascii') {
        needle = new Array(val.length);
        for (var k = 0; k < val.length; k++) needle[k] = val.charCodeAt(k) & 0xFF;
      } else if (enc === 'hex') {
        needle = _hexBytes(val);
      } else if (enc === 'base64') {
        needle = Array.from(_base64Bytes(val));
      } else if (enc === 'base64url') {
        needle = Array.from(_base64urlBytes(val));
      } else {
        // Default UTF-8.
        needle = _utf8Bytes(val);
      }
      var nl = needle.length;
      if (nl === 0) return start;
      if (curLen === 0 || nl > curLen) return -1;
      for (var i = Math.min(start, curLen - nl); i >= 0; i--) {
        var match = true;
        for (var j = 0; j < nl; j++) {
          if (this[i + j] !== needle[j]) { match = false; break; }
        }
        if (match) return i;
      }
      return -1;
    } else if (val && typeof val === 'object') {
      // Buffer/Uint8Array needle.
      var needleLen = (val.buffer && val.buffer.byteLength === 0) ? 0 : val.length;
      if (needleLen === 0) return start; // empty needle → offset
      if (curLen === 0 || needleLen > curLen) return -1;
      for (var i = Math.min(start, curLen - needleLen); i >= 0; i--) {
        var match = true;
        for (var j = 0; j < needleLen; j++) {
          if (this[i + j] !== val[j]) { match = false; break; }
        }
        if (match) return i;
      }
      return -1;
    }
    return -1;
  };

  _bp.toJSON = function() {
    // @trace REQ-ENG-005 [api:Buffer.toJSON] — Use byteLength instead of
    // the user-visible `length` so a Buffer whose `.length` was shadowed
    // via defineProperty still serializes only the real bytes
    // (buffer.test.js "bypassing `length` should not cause an abort").
    var realLen = (typeof this.byteLength === 'number') ? this.byteLength : this.length;
    return { type: 'Buffer', data: Array.prototype.slice.call(this, 0, realLen) };
  };

  // @trace REQ-ENG-005 [api:Buffer.subarray detach] — Node.js throws
  // TypeError when subarray is called on a detached TypedArray.
  // buffer.test.js "subarray() on detached buffer throws TypeError" drives
  // this. SM: a detached TypedArray has buffer.byteLength === 0.
  // @trace REQ-ENG-005 — Negative start counts from the end (Node.js parity).
  // buffer.test.js "Buffer.subarray" drives sub.slice(-1) on "uf" → "f".
  // The `start = start || 0` coercion earlier broke this (it treated -1 as
  // truthy but then the original code's `start || 0` would have left -1 as
  // is, but the actual bug is that the for-loop range never runs from a
  // negative start; we now normalise negative offsets before copying).
  _bp.subarray = function subarray(start, end) {
    if (this.buffer && this.buffer.byteLength === 0) {
      throw new TypeError('Cannot perform %TypedArray%.prototype.subarray on a detached ArrayBuffer');
    }
    var len = this.length;
    // Normalise start: undefined → 0; negative → from end; clamp [0, len].
    if (start === undefined || start === null) start = 0;
    else {
      var sn = (typeof start === 'number') ? start : (+start | 0);
      if (Number.isNaN(sn)) sn = 0;
      if (sn < 0) { sn = len + sn; if (sn < 0) sn = 0; }
      if (sn > len) sn = len;
      start = sn;
    }
    // Normalise end: undefined → len; negative → from end; clamp [0, len].
    if (end === undefined || end === null) end = len;
    else {
      var en = (typeof end === 'number') ? end : (+end | 0);
      if (Number.isNaN(en)) en = len;
      if (en < 0) { en = len + en; if (en < 0) en = 0; }
      if (en > len) en = len;
      end = en;
    }
    if (end < start) end = start;
    var result = Buffer.alloc(end - start);
    for (var i = start; i < end; i++) { result[i - start] = this[i]; }
    return result;
  };

  _bp.reverse = function() {
    for (var i = 0, j = this.length - 1; i < j; i++, j--) {
      var tmp = this[i]; this[i] = this[j]; this[j] = tmp;
    }
    return this;
  };

  _bp.entries = function() {
    var buf = this; var idx = 0;
    return { next: function() { return idx < buf.length ? { value: [idx, buf[idx++]], done: false } : { done: true }; }, [Symbol.iterator]: function() { return this; } };
  };

  _bp.keys = function() {
    var buf = this; var idx = 0;
    return { next: function() { return idx < buf.length ? { value: idx++, done: false } : { done: true }; }, [Symbol.iterator]: function() { return this; } };
  };

  _bp.values = function() {
    var buf = this; var idx = 0;
    return { next: function() { return idx < buf.length ? { value: buf[idx++], done: false } : { done: true }; }, [Symbol.iterator]: function() { return this; } };
  };

  _bp.readInt8 = function(offset) { var v = this[offset || 0]; return v > 127 ? v - 256 : v; };
  _bp.readUInt16LE = function(offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkReadBounds(this.length, offset, 2); return this[offset] | (this[offset + 1] << 8); };
  _bp.writeUInt16LE = function(val, offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkWriteBounds(this.length, offset, 2); this[offset] = val & 0xFF; this[offset + 1] = (val >> 8) & 0xFF; return offset + 2; };
  _bp.readUInt32LE = function(offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkReadBounds(this.length, offset, 4); return ((this[offset]) | (this[offset+1] << 8) | (this[offset+2] << 16) | (this[offset+3] << 24)) >>> 0; };
  _bp.writeUInt32LE = function(val, offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkWriteBounds(this.length, offset, 4); this[offset] = val & 0xFF; this[offset+1] = (val >> 8) & 0xFF; this[offset+2] = (val >> 16) & 0xFF; this[offset+3] = (val >> 24) & 0xFF; return offset + 4; };
  _bp.readInt16LE = function(offset) { var v = _bp.readUInt16LE.call(this, offset); return v > 32767 ? v - 65536 : v; };
  _bp.writeInt16LE = function(val, offset) { return _bp.writeUInt16LE.call(this, val & 0xFFFF, offset); };
  _bp.readInt32LE = function(offset) { return this[offset || 0] | (this[(offset||0)+1] << 8) | (this[(offset||0)+2] << 16) | (this[(offset||0)+3] << 24); };
  _bp.writeInt32LE = function(val, offset) { return _bp.writeUInt32LE.call(this, val >>> 0, offset); };
  _bp.readFloatLE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    u8[0]=this[offset]; u8[1]=this[offset+1]; u8[2]=this[offset+2]; u8[3]=this[offset+3];
    return f32[0];
  };
  // @trace REQ-ENG-005 [api:Buffer bounds] — write/read helpers that throw
  // RangeError (ERR_BUFFER_OUT_OF_BOUNDS) when [offset, offset+byteLength)
  // exceeds the buffer length. Used by writeFloat/Double{LE,BE},
  // writeUInt16/32{LE,BE}, and read counterparts. buffer.test.js
  // "buffer overflow" / "ERR_BUFFER_OUT_OF_BOUNDS" drive this.
  function _checkWriteBounds(bufLen, offset, byteLength) {
    if (offset > bufLen - byteLength) {
      throw _ERR_BUFFER_OUT_OF_BOUNDS();
    }
  }
  function _checkReadBounds(bufLen, offset, byteLength) {
    if (offset > bufLen - byteLength) {
      throw _ERR_BUFFER_OUT_OF_BOUNDS();
    }
  }

  _bp.writeFloatLE = function(val, offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkWriteBounds(this.length, offset, 4);
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    f32[0] = val; this[offset]=u8[0]; this[offset+1]=u8[1]; this[offset+2]=u8[2]; this[offset+3]=u8[3];
    return offset + 4;
  };
  _bp.readDoubleLE = function(offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkReadBounds(this.length, offset, 8);
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    for (var i = 0; i < 8; i++) u8[i] = this[offset + i];
    return f64[0];
  };
  _bp.writeDoubleLE = function(val, offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkWriteBounds(this.length, offset, 8);
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    f64[0] = val; for (var i = 0; i < 8; i++) this[offset + i] = u8[i];
    return offset + 8;
  };

  _bp.swap16 = function() {
    // @trace REQ-ENG-005 [api:Buffer.swap16] — Node.js throws RangeError
    // "Buffer size must be a multiple of 16-bits" when length is odd.
    if (this.length & 1) {
      throw new RangeError('Buffer size must be a multiple of 16-bits');
    }
    for (var i = 0; i < this.length - 1; i += 2) { var t = this[i]; this[i] = this[i+1]; this[i+1] = t; }
    return this;
  };
  _bp.swap32 = function() {
    if (this.length & 3) {
      throw new RangeError('Buffer size must be a multiple of 32-bits');
    }
    for (var i = 0; i < this.length - 3; i += 4) {
      var a=this[i], b=this[i+1], c=this[i+2], d=this[i+3];
      this[i]=d; this[i+1]=c; this[i+2]=b; this[i+3]=a;
    }
    return this;
  };
  _bp.swap64 = function() {
    if (this.length & 7) {
      throw new RangeError('Buffer size must be a multiple of 64-bits');
    }
    for (var i = 0; i < this.length - 7; i += 8) {
      var t;
      t=this[i]; this[i]=this[i+7]; this[i+7]=t;
      t=this[i+1]; this[i+1]=this[i+6]; this[i+6]=t;
      t=this[i+2]; this[i+2]=this[i+5]; this[i+5]=t;
      t=this[i+3]; this[i+3]=this[i+4]; this[i+4]=t;
    }
    return this;
  };

  // @trace REQ-ENG-006 [api:Buffer.prototype.compare]
  // Node.js Buffer.prototype.compare(target[, targetStart[, targetEnd[,
  // sourceStart[, sourceEnd]]]]) — returns -1/0/1 ordering and performs
  // out-of-range validation on `targetEnd` and `sourceEnd` BEFORE applying
  // the `start >= end` early-return. This ordering matters: per Node.js
  // semantics, an inverted range with end > buffer.length still throws
  // ERR_OUT_OF_RANGE, while start >= end with both in range returns early.
  _bp.compare = function(other, targetStart, targetEnd, sourceStart, sourceEnd) {
    var thisLen = this.length;
    var otherLen = other.length;
    // Defaults per Node.js: targetStart=0, targetEnd=otherLen, sourceStart=0,
    // sourceEnd=thisLen.
    var tStart = (targetStart === undefined) ? 0 : (targetStart | 0);
    var tEnd = (targetEnd === undefined) ? otherLen : (targetEnd | 0);
    var sStart = (sourceStart === undefined) ? 0 : (sourceStart | 0);
    var sEnd = (sourceEnd === undefined) ? thisLen : (sourceEnd | 0);

    // Node.js validates `end` against the respective buffer length BEFORE
    // the start >= end early-return. So an inverted range whose end exceeds
    // buffer length still throws.
    if (tEnd > otherLen) {
      var e1 = new RangeError('\"targetEnd\" is outside of buffer bounds');
      throw e1;
    }
    if (sEnd > thisLen) {
      var e2 = new RangeError('\"sourceEnd\" is outside of buffer bounds');
      throw e2;
    }

    // Now apply the start >= end early-return. Zero-length comparison
    // ordering: equal → 0; target empty / source non-empty → 1 (source is
    // longer than the empty target → source "wins"); source empty / target
    // non-empty → -1.
    if (tStart >= tEnd && sStart >= sEnd) return 0;
    if (tStart >= tEnd) {
      // target side is zero-length: source is longer → 1.
      return sStart < sEnd ? 1 : 0;
    }
    if (sStart >= sEnd) {
      return tStart < tEnd ? -1 : 0;
    }

    // Compare the bounded sub-ranges byte-by-byte.
    var cmpLen = Math.min(tEnd - tStart, sEnd - sStart);
    for (var i = 0; i < cmpLen; i++) {
      var a = this[sStart + i];
      var b = other[tStart + i];
      if (a < b) return -1;
      if (a > b) return 1;
    }
    // Equal prefix: longer range wins.
    if ((tEnd - tStart) < (sEnd - sStart)) return -1;
    if ((tEnd - tStart) > (sEnd - sStart)) return 1;
    return 0;
  };

  _bp.readUInt16BE = function(offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkReadBounds(this.length, offset, 2); return (this[offset] << 8) | this[offset + 1]; };
  _bp.writeUInt16BE = function(val, offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkWriteBounds(this.length, offset, 2); this[offset] = (val >> 8) & 0xFF; this[offset + 1] = val & 0xFF; return offset + 2; };
  _bp.readUInt32BE = function(offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkReadBounds(this.length, offset, 4); return ((this[offset] << 24) | (this[offset+1] << 16) | (this[offset+2] << 8) | this[offset+3]) >>> 0; };
  _bp.writeUInt32BE = function(val, offset) { offset = offset === undefined ? 0 : (offset >>> 0); _checkWriteBounds(this.length, offset, 4); this[offset] = (val >> 24) & 0xFF; this[offset+1] = (val >> 16) & 0xFF; this[offset+2] = (val >> 8) & 0xFF; this[offset+3] = val & 0xFF; return offset + 4; };
  _bp.readInt16BE = function(offset) { var v = _bp.readUInt16BE.call(this, offset); return v > 32767 ? v - 65536 : v; };
  _bp.readInt32BE = function(offset) { var o = offset === undefined ? 0 : (offset >>> 0); _checkReadBounds(this.length, o, 4); return (this[o] << 24) | (this[o+1] << 16) | (this[o+2] << 8) | this[o+3]; };
  _bp.readFloatBE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    u8[3]=this[offset]; u8[2]=this[offset+1]; u8[1]=this[offset+2]; u8[0]=this[offset+3];
    return f32[0];
  };
  _bp.readDoubleBE = function(offset) {
    offset = offset || 0;
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    u8[7]=this[offset]; u8[6]=this[offset+1]; u8[5]=this[offset+2]; u8[4]=this[offset+3];
    u8[3]=this[offset+4]; u8[2]=this[offset+5]; u8[1]=this[offset+6]; u8[0]=this[offset+7];
    return f64[0];
  };
  _bp.writeInt32BE = function(val, offset) { return _bp.writeUInt32BE.call(this, val >>> 0, offset); };
  _bp.writeFloatBE = function(val, offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkWriteBounds(this.length, offset, 4);
    var buf = new ArrayBuffer(4); var u8 = new Uint8Array(buf); var f32 = new Float32Array(buf);
    f32[0] = val; this[offset+3]=u8[0]; this[offset+2]=u8[1]; this[offset+1]=u8[2]; this[offset]=u8[3];
    return offset + 4;
  };
  _bp.writeDoubleBE = function(val, offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkWriteBounds(this.length, offset, 8);
    var buf = new ArrayBuffer(8); var u8 = new Uint8Array(buf); var f64 = new Float64Array(buf);
    f64[0] = val; for (var i = 0; i < 8; i++) this[offset + 7 - i] = u8[i];
    return offset + 8;
  };

  // @trace REQ-ENG-005 [api:Buffer.writeInt8/16BE + writeInt/Uint{LE,BE}] —
  // Node.js variable-length signed/unsigned writers. byteLength range 1..6.
  // buffer.test.js "offset returns are correct" and "common write{U}IntLE/BE()"
  // drive these. Aliases writeUint8/16/32 + writeInt8.
  _bp.writeInt8 = function(val, offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkWriteBounds(this.length, offset, 1);
    this[offset] = val & 0xFF;
    return offset + 1;
  };
  _bp.writeInt16BE = function(val, offset) { return _bp.writeUInt16BE.call(this, val & 0xFFFF, offset); };
  // @trace REQ-ENG-005 [api:Buffer.writeUInt{LE,BE}] — Node.js accepts values
  // up to 6 bytes (48 bits) per call. `val >>> 0` truncates to 32 bits and
  // breaks the 5/6-byte path (buffer.test.js "common write{U}IntLE/BE()"
  // drives writeUIntLE(0x1234567890, 0, 5)). Use Math.floor to preserve the
  // full value across the 4-byte boundary.
  function _writeUIntLE(buf, val, offset, byteLength) {
    offset = offset >>> 0;
    _checkWriteBounds(buf.length, offset, byteLength);
    var v = Math.floor(val);
    if (v < 0) v = 0;
    for (var i = 0; i < byteLength; i++) {
      buf[offset + i] = v & 0xFF;
      v = Math.floor(v / 256);
    }
    return offset + byteLength;
  }
  function _writeUIntBE(buf, val, offset, byteLength) {
    offset = offset >>> 0;
    _checkWriteBounds(buf.length, offset, byteLength);
    var v = Math.floor(val);
    if (v < 0) v = 0;
    for (var i = byteLength - 1; i >= 0; i--) {
      buf[offset + i] = v & 0xFF;
      v = Math.floor(v / 256);
    }
    return offset + byteLength;
  }
  _bp.writeUIntLE = function(val, offset, byteLength) { return _writeUIntLE(this, val, offset === undefined ? 0 : offset, byteLength); };
  _bp.writeUIntBE = function(val, offset, byteLength) { return _writeUIntBE(this, val, offset === undefined ? 0 : offset, byteLength); };
  _bp.writeIntLE = function(val, offset, byteLength) {
    var v = val < 0 ? Math.pow(2, 8 * byteLength) + val : val;
    return _writeUIntLE(this, v, offset === undefined ? 0 : offset, byteLength);
  };
  _bp.writeIntBE = function(val, offset, byteLength) {
    var v = val < 0 ? Math.pow(2, 8 * byteLength) + val : val;
    return _writeUIntBE(this, v, offset === undefined ? 0 : offset, byteLength);
  };
  // @trace REQ-ENG-005 — Node alias surface.
  _bp.writeUint8 = _bp.writeUInt8;
  _bp.writeUint16LE = _bp.writeUInt16LE;
  _bp.writeUint16BE = _bp.writeUInt16BE;
  _bp.writeUint32LE = _bp.writeUInt32LE;
  _bp.writeUint32BE = _bp.writeUInt32BE;
  _bp.writeUintLE = _bp.writeUIntLE;
  _bp.writeUintBE = _bp.writeUIntBE;
  // writeBigUint64 aliases are assigned AFTER _bp.writeBigUInt64LE/BE are
  // defined further down (line ~1719+). Defining them now would alias
  // undefined and silently break Buffer.from(...).writeBigUint64*.
  // @trace REQ-ENG-005 [api:Buffer.prototype.read/writeBigInt*] — Node.js
  // BigInt read/write. Throws ERR_BUFFER_OUT_OF_BOUNDS (RangeError with
  // .code) when offset is out of bounds, and returns offset+8 (bytes written)
  // on success. Tests buffer.test.js:ERR_BUFFER_OUT_OF_BOUNDS drive this path
  // for bufferLength 0..6 with both (val) and (val, 0) call shapes.
  function _checkBigIntBounds(buf, offset) {
    offset = offset >>> 0;
    if (buf.length < 8 || offset > buf.length - 8) {
      throw _ERR_BUFFER_OUT_OF_BOUNDS();
    }
    return offset;
  }
  _bp.readBigInt64LE = function(offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    var lo = _bp.readUInt32LE.call(this, offset);
    var hi = _bp.readInt32LE.call(this, offset + 4);
    return BigInt(hi) << 32n | BigInt(lo >>> 0);
  };
  _bp.readBigUInt64LE = function(offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    var lo = _bp.readUInt32LE.call(this, offset);
    var hi = _bp.readUInt32LE.call(this, offset + 4);
    return (BigInt(hi >>> 0) << 32n) | BigInt(lo >>> 0);
  };
  _bp.readBigInt64BE = function(offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    var hi = _bp.readInt32BE.call(this, offset);
    var lo = _bp.readUInt32BE.call(this, offset + 4);
    return BigInt(hi) << 32n | BigInt(lo >>> 0);
  };
  _bp.readBigUInt64BE = function(offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    var hi = _bp.readUInt32BE.call(this, offset);
    var lo = _bp.readUInt32BE.call(this, offset + 4);
    return (BigInt(hi >>> 0) << 32n) | BigInt(lo >>> 0);
  };

  // @trace REQ-ENG-005 [api:Buffer.readInt/UintBE/LE] — Node.js variable-
  // length (1..6 byte) big-endian / little-endian signed / unsigned integer
  // readers. `noAssert` (3rd positional) is accepted but ignored (Node
  // deprecated it and Bun matches — bounds-check always). buffer.test.js
  // "read" drives readIntLE(0,3)/readIntBE(0,3)/readIntLE(0,5)/readIntBE(0,5)
  // and readUintBE(0,5)/readUintBE(0,6) across the full set of test values.
  function _readUIntLE(buf, offset, byteLength) {
    offset = offset >>> 0;
    _checkReadBounds(buf.length, offset, byteLength);
    // @trace REQ-ENG-005 [algorithm:LE] — Little-endian reads the low byte
    // first: byte at offset is multiplied by 256^0, next by 256^1, etc.
    // buffer.test.js "common write{U}IntLE/BE()" drives readUIntLE(0,3) on
    // buf=[0x56,0x34,0x12] expecting 0x123456.
    var val = 0;
    for (var i = 0; i < byteLength; i++) {
      val += buf[offset + i] * Math.pow(256, i);
    }
    return val;
  }
  function _readUIntBE(buf, offset, byteLength) {
    offset = offset >>> 0;
    _checkReadBounds(buf.length, offset, byteLength);
    var val = 0;
    for (var i = 0; i < byteLength; i++) {
      val = val * 256 + buf[offset + i];
    }
    return val;
  }
  _bp.readUIntLE = function(offset, byteLength) { return _readUIntLE(this, offset === undefined ? 0 : offset, byteLength); };
  _bp.readUIntBE = function(offset, byteLength) { return _readUIntBE(this, offset === undefined ? 0 : offset, byteLength); };
  _bp.readIntLE = function(offset, byteLength) {
    var val = _readUIntLE(this, offset === undefined ? 0 : offset, byteLength);
    var msb = Math.pow(2, 8 * byteLength - 1);
    return val >= msb ? val - msb * 2 : val;
  };
  _bp.readIntBE = function(offset, byteLength) {
    var val = _readUIntBE(this, offset === undefined ? 0 : offset, byteLength);
    var msb = Math.pow(2, 8 * byteLength - 1);
    return val >= msb ? val - msb * 2 : val;
  };

  // @trace REQ-ENG-005 — Node alias surface: readUint8/16/32/LE/BE = readUInt*.
  _bp.readUint8 = _bp.readUInt8;
  _bp.readUint16LE = _bp.readUInt16LE;
  _bp.readUint16BE = _bp.readUInt16BE;
  _bp.readUint32LE = _bp.readUInt32LE;
  _bp.readUint32BE = _bp.readUInt32BE;
  _bp.readUintLE = _bp.readUIntLE;
  _bp.readUintBE = _bp.readUIntBE;
  _bp.readInt8Alias = _bp.readInt8;
  _bp.readBigUint64LE = _bp.readBigUInt64LE;
  _bp.readBigUint64BE = _bp.readBigUInt64BE;

  // @trace REQ-ENG-005 [api:Buffer.readFloat/DoubleLE/BE] — IEEE-754 readers
  // using DataView on a 4/8-byte copy. Buffer.test.js "read" drives
  // readFloatBE(0)=1000 and readFloatLE(0)=1000 on a 4-byte buffer.
  _bp.readFloatLE = function(offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkReadBounds(this.length, offset, 4);
    var ab = new ArrayBuffer(4);
    var view = new DataView(ab);
    for (var i = 0; i < 4; i++) view.setUint8(i, this[offset + i]);
    return view.getFloat32(0, true);
  };
  _bp.readFloatBE = function(offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkReadBounds(this.length, offset, 4);
    var ab = new ArrayBuffer(4);
    var view = new DataView(ab);
    for (var i = 0; i < 4; i++) view.setUint8(i, this[offset + i]);
    return view.getFloat32(0, false);
  };
  _bp.readDoubleLE = function(offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkReadBounds(this.length, offset, 8);
    var ab = new ArrayBuffer(8);
    var view = new DataView(ab);
    for (var i = 0; i < 8; i++) view.setUint8(i, this[offset + i]);
    return view.getFloat64(0, true);
  };
  _bp.readDoubleBE = function(offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkReadBounds(this.length, offset, 8);
    var ab = new ArrayBuffer(8);
    var view = new DataView(ab);
    for (var i = 0; i < 8; i++) view.setUint8(i, this[offset + i]);
    return view.getFloat64(0, false);
  };
  _bp.writeBigInt64LE = function(val, offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    val = BigInt(val);
    // @trace REQ-ENG-005 — range check for signed 64-bit. Node throws
    // RangeError ERR_OUT_OF_RANGE when val is outside [-2^63, 2^63).
    if (val < -0x8000000000000000n || val > 0x7fffffffffffffffn) {
      var e = new RangeError('The value of "value" is out of range. >= -(2n ** 63n) and < 2 ** 63n');
      e.code = 'ERR_OUT_OF_RANGE';
      throw e;
    }
    _bp.writeUInt32LE.call(this, Number(val & 0xFFFFFFFFn), offset);
    _bp.writeInt32LE.call(this, Number(val >> 32n), offset + 4);
    return offset + 8;
  };
  _bp.writeBigUInt64LE = function(val, offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    val = BigInt(val);
    if (val < 0n || val > 0xffffffffffffffffn) {
      var e = new RangeError('The value of "value" is out of range. >= 0n and < 2n ** 64n');
      e.code = 'ERR_OUT_OF_RANGE';
      throw e;
    }
    return _bp.writeBigInt64LE.call(this, val, offset);
  };
  _bp.writeBigInt64BE = function(val, offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    val = BigInt(val);
    if (val < -0x8000000000000000n || val > 0x7fffffffffffffffn) {
      var e = new RangeError('The value of "value" is out of range. >= -(2n ** 63n) and < 2 ** 63n');
      e.code = 'ERR_OUT_OF_RANGE';
      throw e;
    }
    _bp.writeInt32BE.call(this, Number(val >> 32n), offset);
    _bp.writeUInt32BE.call(this, Number(val & 0xFFFFFFFFn), offset + 4);
    return offset + 8;
  };
  _bp.writeBigUInt64BE = function(val, offset) {
    offset = _checkBigIntBounds(this, offset === undefined ? 0 : offset);
    val = BigInt(val);
    if (val < 0n || val > 0xffffffffffffffffn) {
      var e = new RangeError('The value of "value" is out of range. >= 0n and < 2n ** 64n');
      e.code = 'ERR_OUT_OF_RANGE';
      throw e;
    }
    return _bp.writeBigInt64BE.call(this, val, offset);
  };
  // @trace REQ-ENG-005 — Node alias surface (BigInt writers; assigned AFTER
  // the canonical _bp.writeBigUInt64LE/BE are defined). buffer.test.js
  // "write alias" drives writeBigUint64LE/BE === writeBigUInt64LE/BE.
  _bp.writeBigUint64LE = _bp.writeBigUInt64LE;
  _bp.writeBigUint64BE = _bp.writeBigUInt64BE;

  // @trace REQ-ENG-005 [api:Buffer.prototype.offset/parent] — Node.js
  // legacy aliases. `offset` mirrors `byteOffset` (deprecated in Node but
  // still read by upstream tests). `parent` is the owning ArrayBufferView's
  // buffer (Buffer.parent === Buffer.buffer for pooled buffers; we expose
  // the underlying ArrayBuffer for parity).
  // @trace REQ-ENG-005 — Node.js Buffer.prototype.{parent,offset} are
  // prototype-level accessors that must NOT throw when called on the
  // prototype itself (Buffer.prototype.parent === undefined,
  // Buffer.prototype.offset === undefined). buffer.test.js "prototype
  // getters should not throw" drives this — earlier versions accessed
  // this.buffer / this.byteOffset unconditionally, which throws "get buffer
  // method called on incompatible Object" on the bare prototype.
  if (!Object.prototype.hasOwnProperty.call(_bp, 'offset')) {
    Object.defineProperty(_bp, 'offset', {
      configurable: true, enumerable: false,
      get: function() {
        if (typeof this !== 'object' || this === null || !ArrayBuffer.isView(this)) return undefined;
        return this.byteOffset || 0;
      }
    });
  }
  if (!Object.prototype.hasOwnProperty.call(_bp, 'parent')) {
    Object.defineProperty(_bp, 'parent', {
      configurable: true, enumerable: false,
      get: function() {
        if (typeof this !== 'object' || this === null || !ArrayBuffer.isView(this)) return undefined;
        return this.buffer;
      }
    });
  }
  _bp.readUInt8 = function(offset) { return this[offset || 0]; };
  // @trace REQ-ENG-005 — Node.js writeUInt8 returns offset+1 (the next
  // writable byte index). buffer.test.js "offset returns are correct"
  // drives writeUInt8(0, 2) expecting 3.
  _bp.writeUInt8 = function(val, offset) {
    offset = offset === undefined ? 0 : (offset >>> 0);
    _checkWriteBounds(this.length, offset, 1);
    this[offset] = val & 0xFF;
    return offset + 1;
  };

  // @trace REQ-ENG-005 [api:Buffer.alloc] — wrap the native alloc to support
  // multi-byte string fill + encoding and Buffer-fill (tiled). The native
  // alloc handles integer fills and single-char strings directly; for
  // anything richer we delegate to Buffer.prototype.fill after allocating a
  // zeroed buffer. Node parity: Buffer.alloc(2, "ab") == <Buffer 61 62>,
  // Buffer.alloc(4, "\x80", "ascii") == <Buffer 80 80 80 80>,
  // Buffer.alloc(1, otherBuf) tiles otherBuf's bytes.
  if (!Buffer.__bao_alloc_wrapped) {
    var _nativeAlloc = Buffer.alloc;
    Buffer.alloc = function alloc(size, fill, encoding) {
      if (fill === undefined) return _nativeAlloc.call(this, size);
      if (typeof fill === 'number') return _nativeAlloc.call(this, size, fill);
      if (typeof fill === 'string') {
        if (fill.length <= 1 && encoding === undefined) {
          return _nativeAlloc.call(this, size, fill);
        }
        // Multi-char string or explicit encoding: allocate zeroed then fill.
        var buf = _nativeAlloc.call(this, size, 0);
        if (buf && buf.fill) buf.fill(fill, 0, buf.length, encoding);
        return buf;
      }
      if (fill && typeof fill === 'object') {
        // Buffer or Uint8Array fill: tile the source bytes.
        // @trace REQ-ENG-005 — Node.js throws ERR_INVALID_ARG_VALUE when the
        // fill value is a zero-length Buffer / Uint8Array (there is no byte
        // to tile). buffer.test.js "alloc() should throw on invalid data"
        // drives Buffer.alloc(1, Buffer.alloc(0)).
        var fillLen = (fill.buffer && fill.buffer.byteLength === 0) ? 0 : fill.length;
        if (fillLen === 0) {
          var _e = new TypeError('The value "' + fill + '" is invalid for argument "value"');
          _e.code = 'ERR_INVALID_ARG_VALUE';
          throw _e;
        }
        var buf = _nativeAlloc.call(this, size, 0);
        if (buf && buf.fill) buf.fill(fill, 0, buf.length);
        return buf;
      }
      return _nativeAlloc.call(this, size, fill);
    };
    // Carry over static surface so Buffer.alloc.X still works (skip/only/if).
    Buffer.alloc.skip = _nativeAlloc.skip;
    Buffer.alloc.only = _nativeAlloc.only;
    Buffer.__bao_alloc_wrapped = true;
  }
})();
"#;
    unsafe {
        let raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("<buffer-proto>".as_bytes());
        let opts = mozjs::glue::NewCompileOptions(raw, c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(proto_src);
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            mozjs_sys::jsapi::JS::Evaluate2(raw, opts, &mut src, rval_h);
            libc::free(opts as *mut _);
        }
    }
}

/// Set Buffer.prototype as the prototype of a newly created buffer object.
///
/// The bao Buffer instance is now a real SM `Uint8Array` (typed array backed
/// by an inline ArrayBuffer). Its prototype chain is:
///
/// ```text
/// instance -> Buffer.prototype -> Uint8Array.prototype -> TypedArray.prototype -> ...
/// ```
///
/// `set_buffer_proto` only rebinds the immediate prototype of the instance to
/// `Buffer.prototype`; `Buffer.prototype.__proto__` is wired to
/// `Uint8Array.prototype` once in `install_buffer_global` (see
/// `wire_buffer_proto_to_uint8array`).
unsafe fn set_buffer_proto(cx: *mut JSContext, obj: *mut JSObject) {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return;
    }
    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let global_root = global);
    let mut buffer_val = UndefinedValue();
    let buffer_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut buffer_val,
    };
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"Buffer".as_ptr(),
        buffer_h,
    );
    if !buffer_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let buffer_root = buffer_val.to_object());
    let mut proto_val = UndefinedValue();
    let proto_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut proto_val,
    };
    JS_GetProperty(
        cx,
        buffer_root.handle().into(),
        c"prototype".as_ptr(),
        proto_h,
    );
    if !proto_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let proto_root = proto_val.to_object());
    rooted!(&in(cx_ref) let obj_root = obj);
    let _ = JS_SetPrototype(cx, obj_root.handle().into(), proto_root.handle().into());
}

/// Wire `Buffer.prototype.__proto__` to `Uint8Array.prototype`.
///
/// Makes every Buffer instance — already a real `Uint8Array` after
/// `create_buffer_object` — pass `instanceof Uint8Array`, and inherit
/// `length`/indexed access/`subarray`/`slice`/`set`/typed element reads
/// directly from the SM typed-array implementation.
// @trace REQ-ENG-005 [entity:Buffer]
unsafe fn wire_buffer_proto_to_uint8array(cx: *mut JSContext) {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return;
    }
    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let global_root = global);

    // Buffer.prototype
    let mut buffer_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"Buffer".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut buffer_val,
        },
    );
    if !buffer_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let buffer_root = buffer_val.to_object());
    let mut proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        buffer_root.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto_val,
        },
    );
    if !proto_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let buf_proto = proto_val.to_object());

    // Uint8Array.prototype
    let mut u8_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"Uint8Array".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut u8_val,
        },
    );
    if !u8_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let u8_ctor = u8_val.to_object());
    let mut u8_proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        u8_ctor.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut u8_proto_val,
        },
    );
    if !u8_proto_val.is_object() {
        return;
    }
    rooted!(&in(cx_ref) let u8_proto = u8_proto_val.to_object());

    let _ = JS_SetPrototype(cx, buf_proto.handle().into(), u8_proto.handle().into());
}

/// Read the raw byte slice of a Buffer-like object via the SM typed-array API.
///
/// Accepts any ArrayBufferView (Buffer, Uint8Array, DataView, ...) and returns
/// a `(length, data_ptr)` pair, or `(0, null)` if `obj` isn't a typed array.
/// The returned pointer is only valid until the next GC, so callers must copy
/// the bytes out before any allocation.
// @trace REQ-ENG-005 [entity:Buffer]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn buffer_view_bytes(obj: *mut JSObject) -> (usize, *mut u8) {
    let mut length: usize = 0;
    let mut is_shared = false;
    let mut data: *mut u8 = ::std::ptr::null_mut();
    let unwrapped =
        mozjs_sys::jsapi::JS_GetObjectAsUint8Array(obj, &mut length, &mut is_shared, &mut data);
    if unwrapped.is_null() || data.is_null() {
        (0, ::std::ptr::null_mut())
    } else {
        (length, data)
    }
}

/// @trace REQ-ENG-005 [algorithm:base64_low_byte]
/// Push the base64 alphabet index of `low` (low byte of a UTF-16 code unit)
/// onto `out`. A literal '=' (0x3D) — which can also arise as the low byte
/// of a surrogate code unit — terminates the run and is signalled by
/// returning `false` so the caller can `break`. Bytes outside the alphabet
/// are silently dropped (Node treats them as whitespace).
///
/// @trace REQ-ENG-005 — Lenient decoding. Bun/Node accept BOTH the standard
/// (`+`/`/`) and url-safe (`-`/`_`) alphabets in BOTH base64 and base64url
/// modes (buffer.test.js "lenient decoding accepts both alphabets in the
/// same input"). The `url_safe` flag is kept only for clarity / future
/// strict modes; today both alphabets map identically.
fn push_base64_index(out: &mut Vec<u8>, low: u8, _url_safe: bool) -> bool {
    if low == b'=' {
        return false;
    }
    let idx: i32 = match low {
        b'A'..=b'Z' => (low - b'A') as i32,
        b'a'..=b'z' => (low - b'a' + 26) as i32,
        b'0'..=b'9' => (low - b'0' + 52) as i32,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => -1,
    };
    if idx >= 0 {
        out.push(idx as u8);
    }
    true
}

/// @trace REQ-ENG-005 [algorithm:base64_low_byte]
/// Node.js base64 decoder that reads the LOW BYTE of each UTF-16 code unit
/// of the JSString. This is the canonical Node.js behaviour (V8 stores
/// strings as either latin1 or two-byte; base64 decoding inspects the
/// low byte of each char16_t regardless of storage representation).
///
/// buffer.test.js "two-byte strings decode from the low byte of each code
/// unit" drives:
///   Buffer.from("QUJD\u{1F600}REVG", "base64").toString("latin1") === "ABC"
/// because \uD83D narrows to 0x3D ('=') — the base64 padding terminator —
/// so decoding stops after the first 3 bytes. Units whose low byte is not
/// in the alphabet are silently skipped (Node treats them as whitespace).
///
/// `url_safe` swaps the alphabet for the url-safe variant (`-`/`_`
/// instead of `+`/`/`).
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `js_str` is a valid
/// SM JSString pointer.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn base64_low_byte_decode(
    cx: *mut JSContext,
    js_str: *mut mozjs_sys::jsapi::JSString,
    url_safe: bool,
) -> Vec<u8> {
    use mozjs::jsapi::{
        JS_DeprecatedStringHasLatin1Chars, JS_GetLatin1StringCharsAndLength,
        JS_GetTwoByteStringCharsAndLength,
    };

    // SM strings can be stored either as latin1 (1 byte / char) or two-byte
    // (char16_t). Either way, Node.js base64 decoding reads the LOW BYTE of
    // each UTF-16 code unit, which is identical to the raw byte for a latin1
    // string. We dispatch on the storage kind to avoid inflating latin1
    // strings needlessly.
    //
    // We pass null nogc guards: the documented fast-path that delegates
    // responsibility for not running JS/GC to the caller (see mozjs
    // conversions.rs:604,629 for the same pattern). We honour this by
    // completing the byte copy before returning.
    let mut cleaned: Vec<u8> = Vec::new();
    if JS_DeprecatedStringHasLatin1Chars(js_str) {
        let mut length: usize = 0;
        let chars_ptr =
            JS_GetLatin1StringCharsAndLength(cx, ::std::ptr::null(), js_str, &mut length);
        if chars_ptr.is_null() {
            return Vec::new();
        }
        cleaned.reserve(length);
        for i in 0..length {
            let low = *chars_ptr.add(i);
            if !push_base64_index(&mut cleaned, low, url_safe) {
                break;
            }
        }
    } else {
        let mut length: usize = 0;
        let chars_ptr =
            JS_GetTwoByteStringCharsAndLength(cx, ::std::ptr::null(), js_str, &mut length);
        if chars_ptr.is_null() {
            return Vec::new();
        }
        cleaned.reserve(length);
        for i in 0..length {
            let unit = *chars_ptr.add(i);
            let low = (unit & 0xFF) as u8;
            if !push_base64_index(&mut cleaned, low, url_safe) {
                break;
            }
        }
    }

    // Decode 4 -> 3 bytes from the cleaned indices.
    let n = cleaned.len();
    let mut out: Vec<u8> = Vec::with_capacity(n * 3 / 4);
    let mut i = 0;
    while i + 1 < n {
        let c0 = cleaned[i] as u32;
        let c1 = cleaned[i + 1] as u32;
        let triplet = (c0 << 18) | (c1 << 12);
        out.push(((triplet >> 16) & 0xFF) as u8);
        if i + 2 < n {
            let c2 = cleaned[i + 2] as u32;
            let triplet = triplet | (c2 << 6);
            out.push(((triplet >> 8) & 0xFF) as u8);
            if i + 3 < n {
                let c3 = cleaned[i + 3] as u32;
                let triplet = triplet | c3;
                out.push((triplet & 0xFF) as u8);
            }
        }
        i += 4;
    }
    out
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let obj = create_buffer_object(cx, &[]);
        if !obj.is_null() {
            args.rval().set(mozjs::jsval::ObjectValue(obj));
        } else {
            args.rval().set(UndefinedValue());
        }
        return true;
    }
    let first = *args.get(0).ptr;
    if first.is_string() {
        let s = first.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            let bytes = rust_str.as_bytes();
            let obj = create_buffer_object(cx, bytes);
            if obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            args.rval().set(mozjs::jsval::ObjectValue(obj));
            return true;
        }
    }
    if first.is_int32() {
        let size = first.to_int32().max(0) as usize;
        // `new Buffer(size)` allocates an UNINITIALISED buffer (Node.js
        // semantics, deprecated but supported). SM's JS_NewUint8Array zeroes
        // the backing store, which is stricter than Node.js but never leaks
        // heap data — and it's the same behaviour Buffer.alloc(size) gives.
        // We do not zero-fill a second time; the bytes are already 0.
        let bytes = vec![0u8; size];
        let obj = create_buffer_object(cx, &bytes);
        if obj.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        args.rval().set(mozjs::jsval::ObjectValue(obj));
        return true;
    }
    // Buffer.from-style object/array argument — defer to buffer_from.
    if first.is_object() {
        return buffer_from(cx, argc, vp);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_from(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        // @trace REQ-ENG-005 — Buffer.from() (no args) throws
        // ERR_INVALID_ARG_TYPE per Node.js (test "Buffer,poolSize").
        mozjs::error::throw_type_error(
            cx,
            c"The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object. Received undefined".as_ref(),
        );
        return false;
    }

    let input = *args.get(0).ptr;
    // @trace REQ-ENG-005 — Buffer.from(null/undefined/boolean/number) throws
    // ERR_INVALID_ARG_TYPE (test "Buffer,poolSize" drives Buffer.from(null)).
    if input.is_null() || input.is_undefined() || input.is_boolean() || input.is_number() {
        mozjs::error::throw_type_error(
            cx,
            c"The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object. Received null".as_ref(),
        );
        return false;
    }
    if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        let encoding = if argc >= 2 {
            let enc_val = *args.get(1).ptr;
            if enc_val.is_string() {
                unsafe_jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(enc_val.to_string()))
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        // @trace REQ-ENG-005 — Validate the encoding name. Node.js throws
        // ERR_UNKNOWN_ENCODING (TypeError matching /encoding/) for unknown
        // encodings. buffer.test.js "regression tests from Node.js" drives
        //   Buffer.from("", "buffer")  // "buffer" is not a valid encoding
        // to throw. The empty encoding string defaults to utf8 (Node parity).
        if !encoding.is_empty() {
            const VALID_ENCODINGS: &[&str] = &[
                "utf8",
                "utf-8",
                "ascii",
                "latin1",
                "binary",
                "base64",
                "base64url",
                "hex",
                "ucs2",
                "ucs-2",
                "utf16le",
                "utf-16le",
                "utf16be",
                "utf-16be",
            ];
            let enc_lower = encoding.to_lowercase();
            if !VALID_ENCODINGS.iter().any(|&v| v == enc_lower) {
                let msg = format!("Unknown encoding: {}", encoding);
                let c_msg = ::std::ffi::CString::new(msg)
                    .unwrap_or_else(|_| ::std::ffi::CString::new("Unknown encoding").unwrap());
                mozjs::error::throw_type_error(cx, c_msg.as_ref());
                return false;
            }
        }
        let bytes = if encoding == "hex" {
            // @trace REQ-ENG-005 — Buffer.from(str, "hex") decodes pairs of
            // hex digits and STOPS at the first non-hex character (Node.js
            // behaviour). buffer.test.js "hex input containing byte 0xFF"
            // drives "ab\xff\xffcd" → [0xab] (the \xff stops decoding).
            let sb = s.as_bytes();
            let mut out: Vec<u8> = Vec::with_capacity(sb.len() / 2);
            let mut i = 0;
            while i + 1 < sb.len() {
                let hi = sb[i];
                let lo = sb[i + 1];
                let hv = (hi as char).to_digit(16);
                let lv = (lo as char).to_digit(16);
                match (hv, lv) {
                    (Some(h), Some(l)) => {
                        out.push(((h << 4) | l) as u8);
                        i += 2;
                    }
                    _ => break,
                }
            }
            // @trace REQ-ENG-005 — Node.js discards a single trailing hex
            // digit (odd-length input); it does NOT pad. buffer.test.js
            // "single hex character is discarded" drives Buffer.from("A","hex")
            // to length 0.
            out
        } else if encoding == "base64" {
            // @trace REQ-ENG-005 [algorithm:base64]
            // Node.js parity: base64 decoding reads the LOW BYTE of each
            // UTF-16 code unit of the input string. A two-byte JSString
            // containing surrogate pairs therefore contributes only the
            // low byte of each surrogate — buffer.test.js
            //   "two-byte strings decode from the low byte of each code unit"
            // drives:
            //   Buffer.from("QUJD\u{1F600}REVG", "base64").toString("latin1") === "ABC"
            // because \uD83D narrows to 0x3D ('=') — the canonical base64
            // padding terminator — so decoding stops after "ABC". Units
            // whose low byte is not in the alphabet are silently skipped
            // (Node treats them as whitespace). bun_base64::decode_alloc
            // operates on raw bytes and would instead decode the full
            // UTF-8 form of the input (4 bytes per surrogate) and produce
            // "ABCDEF" — wrong. We hand-roll the low-byte walker.
            let js_str = input.to_string();
            base64_low_byte_decode(cx, js_str, false)
        } else if encoding == "base64url" {
            // @trace REQ-ENG-005 [algorithm:base64]
            // Same low-byte semantics as standard base64, but accept the
            // url-safe alphabet (- and _ instead of + and /). Padding is
            // optional and may be missing; we honour a literal '=' or a
            // surrogate-derived 0x3D low byte as the terminator.
            let js_str = input.to_string();
            base64_low_byte_decode(cx, js_str, true)
        } else if encoding == "utf-16le"
            || encoding == "ucs2"
            || encoding == "ucs-2"
            || encoding == "utf16le"
        {
            // @trace REQ-ENG-005 [algorithm:utf-16le]
            // Node.js semantics: encode each UTF-16 code unit as little-endian
            // 2 bytes. Rust strings are UTF-8, so expand BMP chars to 2 bytes
            // (code units) and surrogate pairs (U+10000+) to 4 bytes.
            let mut out: Vec<u8> = Vec::with_capacity(s.len() * 2);
            for c in s.chars() {
                let code = c as u32;
                if code <= 0xFFFF {
                    out.extend_from_slice(&(code as u16).to_le_bytes());
                } else {
                    // Surrogate pair
                    let v = code - 0x10000;
                    let hi = 0xD800 + (v >> 10) as u16;
                    let lo = 0xDC00 + (v & 0x3FF) as u16;
                    out.extend_from_slice(&hi.to_le_bytes());
                    out.extend_from_slice(&lo.to_le_bytes());
                }
            }
            out
        } else if encoding == "utf-16be"
            || encoding == "utf16be"
            || encoding == "ucs2be"
            || encoding == "ucs-2be"
        {
            let mut out: Vec<u8> = Vec::with_capacity(s.len() * 2);
            for c in s.chars() {
                let code = c as u32;
                if code <= 0xFFFF {
                    out.extend_from_slice(&(code as u16).to_be_bytes());
                } else {
                    let v = code - 0x10000;
                    let hi = 0xD800 + (v >> 10) as u16;
                    let lo = 0xDC00 + (v & 0x3FF) as u16;
                    out.extend_from_slice(&hi.to_be_bytes());
                    out.extend_from_slice(&lo.to_be_bytes());
                }
            }
            out
        } else if encoding == "latin1" || encoding == "binary" || encoding == "ascii" {
            // @trace REQ-ENG-005 [api:Buffer.from] — Latin1/binary/ascii encode
            // each char to a single byte clamped to 0-255. Node.js 'ascii'
            // historically preserves the high bit on encode (latin1 semantics)
            // so that buf.write("\xa4","ascii") round-trips through
            // toString("latin1"). See buffer.test.js "Buffer.from latin1 vs ascii".
            s.chars()
                .map(|c| (c as u32 & 0xFF) as u8)
                .collect::<Vec<u8>>()
        } else {
            // Default: UTF-8 (Node.js semantics for the unspecified / "utf8"
            // / "utf-8" / "" cases). `s.as_bytes()` *is* UTF-8 because Rust
            // strings are UTF-8 — no extra work needed.
            s.as_bytes().to_vec()
        };
        create_buffer_from_bytes(cx, &args, &bytes)
    } else if input.is_object() {
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = input.to_object());
        let obj_handle: mozjs::jsapi::Handle<*mut JSObject> = obj_root.handle().into();
        // @trace REQ-ENG-005 — Functions are not valid Buffer.from inputs.
        // Node.js's `from(value)` only enters the object branch when
        // `typeof value === 'object' && value !== null`; a function has
        // typeof === 'function' and reaches the throw fallback. SM treats
        // functions as objects, so we explicitly reject them here.
        if unsafe { mozjs_sys::jsapi::JS::IsCallable(obj_root.get()) } {
            mozjs::error::throw_type_error(
                cx,
                c"The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object.".as_ref(),
            );
            return false;
        }

        // @trace REQ-ENG-005 — String wrapper objects (new String("test"),
        // new MyString()) convert to the underlying string and recurse into
        // Buffer.from(string). Node.js parity: Buffer.from(new String("test"))
        // === Buffer.from("test"). buffer.test.js "Buffer.from (Node.js test/
        // test-buffer-from.js)" drives this. We probe via the global String
        // constructor's prototype chain — `obj instanceof String`.
        let mut cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let mut is_string_obj = false;
        {
            rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
            if !global.get().is_null() {
                let mut str_ctor_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    global.handle().into(),
                    c"String".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut str_ctor_val,
                    },
                );
                if str_ctor_val.is_object() {
                    rooted!(&in(cx_ref) let str_ctor_root = str_ctor_val.to_object());
                    let mut is_inst = false;
                    rooted!(&in(cx_ref) let input_h = input);
                    mozjs_sys::jsapi::JS_HasInstance(
                        cx,
                        str_ctor_root.handle().into(),
                        input_h.handle().into(),
                        &mut is_inst,
                    );
                    is_string_obj = is_inst;
                }
            }
        }
        if is_string_obj {
            // ToString the object via SM's ToString (handles String wrappers,
            // subclass overrides, etc.) and recurse into the string branch.
            let obj_val = ObjectValue(obj_root.get());
            rooted!(&in(cx_ref) let obj_val_h = obj_val);
            let jsstr = mozjs::rust::ToString(&mut cx_ref, obj_val_h.handle());
            if !jsstr.is_null() {
                let str_val = mozjs::jsval::StringValue(&*jsstr);
                let s = crate::js_to_rust_string(cx, str_val);
                return create_buffer_from_bytes(cx, &args, s.as_bytes());
            }
        }

        // Check if it's an ArrayBuffer using mozjs_sys API
        let is_ab = unsafe { mozjs_sys::jsapi::JS::IsArrayBufferObject(obj_root.get()) };

        // @trace REQ-ENG-005 — Node.js also recognises ArrayBufferView
        // (TypedArray / DataView / Buffer) inputs and shares their backing
        // ArrayBuffer. Buffer.test.js "creating a Buffer from a Uint32Array"
        // drives Buffer.from(ui32). We detect ArrayBuffer views BEFORE the
        // arrayLike fallback so typed-array inputs copy their bytes verbatim
        // (not char-by-char via length).
        let is_view = if !is_ab {
            unsafe { mozjs_sys::jsapi::JS_IsArrayBufferViewObject(obj_root.get()) }
        } else {
            false
        };

        if is_ab {
            // @trace REQ-ENG-005 [api:Buffer.from(arrayBuffer)] — Node parity:
            // the returned Buffer SHARES the source ArrayBuffer (no copy), so
            // `buf.buffer === ab` and `buf.byteOffset` reflects the view. We
            // build the typed-array view directly off the source ArrayBuffer.
            let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
            let mut data_len: usize = 0;
            let mut is_shared = false;
            unsafe {
                mozjs_sys::jsapi::JS::GetArrayBufferLengthAndData(
                    obj_root.get(),
                    &mut data_len,
                    &mut is_shared,
                    &mut data_ptr,
                );
            }

            // @trace REQ-ENG-005 — Node.js coerces byteOffset via ToNumber.
            // Non-numeric / NaN byteOffset defaults to 0. Infinity throws.
            // buffer.test.js "new Buffer() (Node.js test/test-buffer-new.js)"
            // drives Buffer.from(ab, "fhqwhgads") → byteOffset 0,
            // Buffer.from(ab, NaN) → 0, Buffer.from(ab, {}) → 0,
            // Buffer.from(ab, []) → 0, Buffer.from(ab, [1]) → 1,
            // Buffer.from(ab, Infinity) → throw.
            let offset: usize = if argc > 1 {
                let v = *args.get(1).ptr;
                if v.is_int32() {
                    (v.to_int32() as isize).max(0) as usize
                } else if v.is_double() {
                    let d = v.to_double();
                    if d.is_nan() {
                        0
                    } else if d == f64::INFINITY {
                        mozjs::error::throw_range_error(
                            cx,
                            c"Offset is outside the bounds of the DataView".as_ref(),
                        );
                        return false;
                    } else {
                        (d.max(0.0) as isize).max(0) as usize
                    }
                } else {
                    rooted!(&in(cx_ref) let v_root = v);
                    match mozjs::rust::ToNumber(cx, v_root.handle()) {
                        Ok(d) => {
                            if d.is_nan() {
                                0
                            } else if d == f64::INFINITY {
                                mozjs::error::throw_range_error(
                                    cx,
                                    c"Offset is outside the bounds of the DataView".as_ref(),
                                );
                                return false;
                            } else {
                                (d.max(0.0) as isize).max(0) as usize
                            }
                        }
                        Err(_) => return false,
                    }
                }
            } else {
                0
            };

            // @trace REQ-ENG-005 — Node.js validates byteOffset against the
            // ArrayBuffer length and throws RangeError on overflow
            // (buffer.test.js "ParseArrayIndex() should handle full uint32").
            if offset > data_len {
                mozjs::error::throw_range_error(
                    cx,
                    c"Offset is outside the bounds of the DataView".as_ref(),
                );
                return false;
            }

            // @trace REQ-ENG-005 — Node.js coerces length via ToNumber. If the
            // result is NaN, length defaults to 0 (NOT data_len-offset).
            // buffer.test.js "new Buffer() (Node.js test/test-buffer-new.js)"
            // drives Buffer.from(ab, 0, "fhqwhgads") → length 0,
            // Buffer.from(ab, 0, [1]) → length 1, Buffer.from(ab, 0, Infinity) → throw.
            let len: usize = if argc > 2 {
                let v = *args.get(2).ptr;
                if v.is_int32() {
                    (v.to_int32() as isize).max(0) as usize
                } else if v.is_double() {
                    let d = v.to_double();
                    if d.is_nan() {
                        0
                    } else if d == f64::INFINITY {
                        mozjs::error::throw_range_error(
                            cx,
                            c"\"length\" is outside of buffer bounds".as_ref(),
                        );
                        return false;
                    } else {
                        (d.max(0.0) as isize).max(0) as usize
                    }
                } else {
                    // Non-number length: ToNumber coercion. Strings/objects
                    // parse via JS semantics.
                    rooted!(&in(cx_ref) let v_root = v);
                    match mozjs::rust::ToNumber(cx, v_root.handle()) {
                        Ok(d) => {
                            if d.is_nan() {
                                0
                            } else if d == f64::INFINITY {
                                mozjs::error::throw_range_error(
                                    cx,
                                    c"\"length\" is outside of buffer bounds".as_ref(),
                                );
                                return false;
                            } else {
                                (d.max(0.0) as isize).max(0) as usize
                            }
                        }
                        Err(_) => return false,
                    }
                }
            } else {
                data_len.saturating_sub(offset)
            };

            // @trace REQ-ENG-005 — Node.js throws RangeError when offset+len
            // exceeds the source ArrayBuffer's length (explicit length that
            // overflows the available range). buffer.test.js "new Buffer()"
            // drives Buffer.from(ab.buffer, 3, 6) on a 5-byte buffer to throw.
            if offset + len > data_len {
                mozjs::error::throw_range_error(
                    cx,
                    c"\"length\" is outside of buffer bounds".as_ref(),
                );
                return false;
            }

            let end = (offset + len).min(data_len);
            let view_len = end.saturating_sub(offset);

            // @trace REQ-ENG-005 [api:Buffer.from auto-length RAB view] — When
            // the caller does NOT specify an explicit length (only byteOffset),
            // pass length = -1 to SM so the resulting Uint8Array is an
            // auto-length view that tracks the source ArrayBuffer's current
            // byteLength. This is required for resizable ArrayBuffers
            // (ArrayBuffer(maxByteLength)) — `Buffer.from(rab)` followed by
            // `rab.resize(N)` must reflect the new length in `buf.length`,
            // which drives the TOCTOU clamp semantics in copy/fill.
            let view_len_arg: i64 = if argc <= 2 { -1 } else { view_len as i64 };

            // Build a Uint8Array view sharing the source ArrayBuffer.
            let view = unsafe {
                mozjs_sys::jsapi::JS_NewUint8ArrayWithBuffer(
                    cx,
                    obj_root.handle().into(),
                    offset,
                    view_len_arg,
                )
            };
            if view.is_null() {
                return create_buffer_from_bytes(cx, &args, &[]);
            }
            // Rebind the view's prototype to Buffer.prototype so it presents
            // as a Buffer (instanceof checks, _isBuffer, fill/write/etc.).
            set_buffer_proto(cx, view);
            let cx_ref =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref) let view_root = view);
            rooted!(&in(cx_ref) let is_buf = BooleanValue(true));
            JS_DefineProperty(
                cx,
                view_root.handle().into(),
                c"_isBuffer".as_ptr(),
                is_buf.handle().into(),
                0u32,
            );
            args.rval().set(ObjectValue(view));
            true
        } else if is_view {
            // @trace REQ-ENG-005 [api:Buffer.from(TypedArray)] — Node.js
            // creates a Buffer that COPIES the typed-array's ELEMENTS (not
            // raw bytes) and truncates each element to its low byte. So
            // `Buffer.from(new Uint32Array([256, 257]))` → <Buffer 00 01>.
            // buffer.test.js "creating a Buffer from a Uint32Array" drives
            // `ui32.fill(42)` → <Buffer 2a 2a 2a 2a> (4 bytes, not 16).
            let mut view_len: usize = 0;
            let mut view_shared = false;
            let mut view_data: *mut u8 = ::std::ptr::null_mut();
            let view_unwrapped = unsafe {
                mozjs_sys::jsapi::JS_GetObjectAsArrayBufferView(
                    obj_root.get(),
                    &mut view_len,
                    &mut view_shared,
                    &mut view_data,
                )
            };
            if !view_unwrapped.is_null() && !view_data.is_null() {
                // Determine element size from the TypedArray kind. For
                // Uint8Array/Uint8ClampedArray/Int8Array/DataView, element
                // size is 1 (straight byte copy). For wider element types,
                // we walk elements and take the low byte (little-endian).
                let elem_type =
                    unsafe { mozjs_sys::jsapi::JS_GetArrayBufferViewType(obj_root.get()) };
                use mozjs_sys::jsapi::JS::Scalar::Type as ST;
                let (elem_size, count, is_float): (usize, usize, bool) = match elem_type {
                    ST::Int8 | ST::Uint8 | ST::Uint8Clamped => (1, view_len, false),
                    ST::Int16 | ST::Uint16 => (2, view_len / 2, false),
                    ST::Int32 | ST::Uint32 => (4, view_len / 4, false),
                    ST::Float16 => (2, view_len / 2, true),
                    ST::Float32 => (4, view_len / 4, true),
                    ST::Float64 => (8, view_len / 8, true),
                    ST::BigInt64 | ST::BigUint64 => (8, view_len / 8, false),
                    _ => (1, view_len, false), // DataView or unknown → byte copy
                };
                let mut bytes: Vec<u8> = Vec::with_capacity(count);
                for i in 0..count {
                    let base = i * elem_size;
                    let byte = if is_float {
                        let f: f64 = if elem_type == ST::Float32 {
                            let mut buf4 = [0u8; 4];
                            for k in 0..4 {
                                buf4[k] = unsafe { *view_data.add(base + k) };
                            }
                            f32::from_le_bytes(buf4) as f64
                        } else if elem_type == ST::Float16 {
                            // Half-float: expand bits manually.
                            let mut buf2 = [0u8; 2];
                            for k in 0..2 {
                                buf2[k] = unsafe { *view_data.add(base + k) };
                            }
                            let bits = u16::from_le_bytes(buf2);
                            f16_to_f64(bits)
                        } else {
                            let mut buf8 = [0u8; 8];
                            for k in 0..8 {
                                buf8[k] = unsafe { *view_data.add(base + k) };
                            }
                            f64::from_le_bytes(buf8)
                        };
                        if f.is_nan() || f.is_infinite() {
                            0u8
                        } else {
                            let n = f.trunc() as i64;
                            (n & 0xFF) as u8
                        }
                    } else {
                        unsafe { *view_data.add(base) }
                    };
                    bytes.push(byte);
                }
                create_buffer_from_bytes(cx, &args, &bytes)
            } else {
                create_buffer_from_bytes(cx, &args, &[])
            }
        } else {
            // @trace REQ-ENG-005 — Symbol.toPrimitive: Node.js honours an
            // object's [Symbol.toPrimitive]("string") hint and converts the
            // result to a Buffer via the string path. buffer.test.js
            // "Buffer.from (Node.js test/test-buffer-from.js)" drives
            // MyPrimitive with [Symbol.toPrimitive] returning "test".
            //
            // SM's JS::ToString already invokes [Symbol.toPrimitive] with the
            // "string" hint when present, so we probe for the symbol via
            // Object.keys/Symbol enumeration-free path: check if the object
            // (or its prototype) defines a toPrimitive symbol that is a
            // function. We do this in JS to avoid the C++ symbol-API dance.
            let cx_ref_t =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let used_to_prim = false;
            {
                // Use a tiny JS helper to probe + invoke Symbol.toPrimitive.
                let probe_src = r#"(function(obj) {
                    if (obj == null || typeof obj !== 'object') return undefined;
                    var tp = obj[Symbol.toPrimitive];
                    if (typeof tp !== 'function') return undefined;
                    var r = tp.call(obj, 'string');
                    return (typeof r === 'string') ? r : null;
                })"#;
                let c_filename = ZBox::from_bytes("<buffer-from-toprim>".as_bytes());
                let opts = mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr(), 1);
                if !opts.is_null() {
                    let mut src = mozjs::rust::transform_str_to_source_text(probe_src);
                    let mut rval_fn = UndefinedValue();
                    let rval_h = MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval_fn,
                    };
                    mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h);
                    libc::free(opts as *mut _);
                    if rval_fn.is_object() {
                        rooted!(&in(cx_ref_t) let fn_obj = rval_fn.to_object());
                        rooted!(&in(cx_ref_t) let obj_val = ObjectValue(obj_root.get()));
                        let args_arr = mozjs::jsapi::HandleValueArray {
                            length_: 1,
                            elements_: &obj_val.get() as *const Value,
                        };
                        rooted!(&in(cx_ref_t) let null_obj: *mut JSObject = ::std::ptr::null_mut());
                        rooted!(&in(cx_ref_t) let fn_val = ObjectValue(fn_obj.get()));
                        let mut result_val = UndefinedValue();
                        let ok = JS_CallFunctionValue(
                            cx,
                            null_obj.handle().into(),
                            fn_val.handle().into(),
                            &args_arr,
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut result_val,
                            },
                        );
                        if ok && result_val.is_string() {
                            let s = crate::js_to_rust_string(cx, result_val);
                            return create_buffer_from_bytes(cx, &args, s.as_bytes());
                        }
                    }
                }
            }
            let _ = used_to_prim;

            // Array-like or Buffer object
            // @trace REQ-ENG-005 — Node.js accepts an object here only if:
            //   1. It is a Buffer (has Buffer.isBuffer true), OR
            //   2. It is an array-like (has a numeric `.length`), OR
            //   3. It is a JSON-serialized Buffer ({ type: "Buffer", data: [...] })
            // Otherwise it throws TypeError. buffer.test.js "Buffer.from
            // (Node.js test/test-buffer-from.js)" drives Buffer.from({}) /
            // new Boolean(true) / new Number(true) / { valueOf() {...} } to
            // throw.
            let mut type_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj_handle,
                c"type".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut type_val,
                },
            );
            let is_legacy_buffer_blob = type_val.is_string()
                && unsafe_jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(type_val.to_string()))
                    == "Buffer";

            let mut length_val = UndefinedValue();
            let length_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut length_val,
            };
            JS_GetProperty(cx, obj_handle, c"length".as_ptr(), length_handle);
            // @trace REQ-ENG-005 [api:Buffer.from array-like] — Node.js
            // converts `.length` via ToLength/ToNumber so a fractional length
            // like 3.3 truncates to 3, a numeric string like "5" parses to 5,
            // and a non-numeric string like "BAM" yields NaN → length 0.
            // buffer.test.js "check for fractional length args" drives
            // Buffer.from({ length: 3.3 }).length === 3 and
            // Buffer.from({ length: "BAM" }).length === 0.
            let len: usize = if length_val.is_int32() {
                let raw = length_val.to_int32();
                if raw < 0 { 0 } else { raw as usize }
            } else if length_val.is_double() {
                let d = length_val.to_double();
                if d.is_nan() || d < 0.0 {
                    0
                } else {
                    (d.floor() as i64).max(0) as usize
                }
            } else if length_val.is_string() {
                let s = unsafe_jsstr_to_string(
                    cx,
                    ::std::ptr::NonNull::new_unchecked(length_val.to_string()),
                );
                match s.parse::<f64>() {
                    Ok(d) if d.is_finite() && d >= 0.0 => (d.floor() as i64) as usize,
                    _ => 0,
                }
            } else {
                0
            };

            // @trace REQ-ENG-005 — Node.js throws TypeError when the object is
            // neither array-like (no numeric .length) nor a legacy Buffer blob
            // ({type:"Buffer", data:[...]}). buffer.test.js "Buffer.from" drives
            //   Buffer.from({}) // no length
            //   Buffer.from(new Boolean(true)) // no length, no .data
            //   Buffer.from({valueOf(){return null}}) // no length
            //   Buffer.from(new Number(true)) // wraps primitive, no length
            // to throw. An object that IS array-like ({length: 0} etc.) is
            // accepted as an empty Buffer.
            let length_is_undefined = length_val.is_undefined() || length_val.is_null();
            if length_is_undefined && !is_legacy_buffer_blob {
                mozjs::error::throw_type_error(
                    cx,
                    c"The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object.".as_ref(),
                );
                return false;
            }

            let mut bytes = Vec::with_capacity(len);
            for i in 0..len {
                let mut elem = UndefinedValue();
                let elem_handle = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                };
                JS_GetElement(cx, obj_handle, i as u32, elem_handle);
                // @trace REQ-ENG-005 — Node.js converts each element to a byte
                // via ToNumber→ToInt8/Uint8 truncation. buffer.test.js
                // "construction from arrayish object" drives
                //   { 0: "0", 1: "1", 2: "2", 3: "3", length: 4 } → [0,1,2,3].
                // Numbers clamp to 0-255; strings parse to their numeric value;
                // undefined/null/NaN become 0.
                let byte_val: u8 = if elem.is_int32() {
                    let raw = elem.to_int32();
                    (raw & 0xFF) as u8
                } else if elem.is_double() {
                    let d = elem.to_double();
                    if d.is_nan() || d.is_infinite() {
                        0
                    } else {
                        // Node uses ToInt8 semantics on the truncated integer.
                        let n = d.trunc() as i64;
                        (n & 0xFF) as u8
                    }
                } else if elem.is_string() {
                    let s =
                        unsafe_jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(elem.to_string()));
                    match s.trim().parse::<f64>() {
                        Ok(d) if d.is_finite() => {
                            let n = d.trunc() as i64;
                            (n & 0xFF) as u8
                        }
                        _ => 0,
                    }
                } else if elem.is_boolean() {
                    if elem.to_boolean() { 1 } else { 0 }
                } else {
                    // undefined, null, objects → 0 (Node.js coerces to NaN → 0)
                    0
                };
                bytes.push(byte_val);
            }
            // @trace REQ-ENG-005 — Legacy Buffer blob: { type: "Buffer",
            // data: [...] }. Read .data and convert each element to a byte.
            if is_legacy_buffer_blob && bytes.is_empty() {
                let mut data_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    obj_handle,
                    c"data".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut data_val,
                    },
                );
                if data_val.is_object() {
                    let cx_ref_d =
                        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                    rooted!(&in(cx_ref_d) let data_root = data_val.to_object());
                    let mut dlen_val = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        data_root.handle().into(),
                        c"length".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut dlen_val,
                        },
                    );
                    let dlen = if dlen_val.is_int32() {
                        dlen_val.to_int32().max(0) as usize
                    } else {
                        0
                    };
                    let mut data_bytes = Vec::with_capacity(dlen);
                    for i in 0..dlen {
                        let mut elem = UndefinedValue();
                        JS_GetElement(
                            cx,
                            data_root.handle().into(),
                            i as u32,
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut elem,
                            },
                        );
                        data_bytes.push(if elem.is_int32() {
                            (elem.to_int32() & 0xFF) as u8
                        } else if elem.is_double() {
                            let d = elem.to_double();
                            if d.is_nan() || d.is_infinite() {
                                0
                            } else {
                                (d.trunc() as i64 & 0xFF) as u8
                            }
                        } else {
                            0
                        });
                    }
                    return create_buffer_from_bytes(cx, &args, &data_bytes);
                }
            }
            create_buffer_from_bytes(cx, &args, &bytes)
        }
    } else {
        // @trace REQ-ENG-005 — Non-object, non-string input (Symbol, bigint,
        // function, undefined, null already handled above). Node.js throws.
        mozjs::error::throw_type_error(
            cx,
            c"The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object.".as_ref(),
        );
        false
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_buffer_from_bytes(cx: *mut JSContext, args: &CallArgs, bytes: &[u8]) -> bool {
    let obj = create_buffer_object(cx, bytes);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(ObjectValue(obj));
    true
}

/// Build a Buffer instance wrapping the given bytes and return the raw object
/// pointer. The caller owns the returned reference. Returns null on OOM.
///
/// The instance is a real SM `Uint8Array` (typed array backed by an inline
/// ArrayBuffer). This makes `instanceof Uint8Array` true, gives native
/// `length`/indexed element access/`subarray`/`slice`/`set`, and crucially
/// makes `Buffer.allocUnsafe(64 MiB)` an O(1) allocation instead of a
/// per-byte `JS_DefineElement` storm that would take minutes.
///
/// Shared by globals.rs Buffer methods, node_crypto.rs (randomBytes) and
/// node_fs.rs (readFileSync without encoding) so that all paths return real
/// Buffer instances (`Buffer.isBuffer(x) === true`).
// @trace REQ-ENG-005 [entity:Buffer]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn create_buffer_object(cx: *mut JSContext, bytes: &[u8]) -> *mut JSObject {
    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    // Allocate a real Uint8Array. SM initialises the underlying ArrayBuffer to
    // zero, so an empty `bytes` slice yields a clean zero-length typed array.
    let u8_obj = mozjs_sys::jsapi::JS_NewUint8Array(cx, bytes.len());
    if u8_obj.is_null() {
        return ::std::ptr::null_mut();
    }

    // Copy caller-supplied bytes into the typed array's inline buffer. We must
    // root the typed array while we touch its data pointer so a GC cannot move
    // it out from under us. The data pointer is obtained via the SM accessor
    // which requires an `AutoRequireNoGC` guard — we pass a null guard which
    // is the documented fast-path: the caller is responsible for not running
    // JS/GC between `JS_GetUint8ArrayData` and the last use of the pointer,
    // which we honour by writing all bytes before returning.
    rooted!(&in(cx_ref) let buf_obj = u8_obj);

    if !bytes.is_empty() {
        let mut is_shared = false;
        let data_ptr = mozjs_sys::jsapi::JS_GetUint8ArrayData(
            buf_obj.get(),
            &mut is_shared,
            ::std::ptr::null(),
        );
        if !data_ptr.is_null() {
            ::std::ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, bytes.len());
        }
    }

    // Rebind the prototype to Buffer.prototype so Buffer-specific methods
    // (toString/slice/equals/copy/indexOf + the JS-injected helpers like
    // write/fill/readUInt8/etc.) take precedence, while Uint8Array.prototype
    // provides length/indexed access/subarray/etc. transparently.
    set_buffer_proto(cx, buf_obj.get());

    // Internal marker used by Buffer.isBuffer (Node.js-compatible detection).
    rooted!(&in(cx_ref) let is_buf = BooleanValue(true));
    JS_DefineProperty(
        cx,
        buf_obj.handle().into(),
        c"_isBuffer".as_ptr(),
        is_buf.handle().into(),
        0u32,
    );

    buf_obj.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_to_string(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = this.to_object());

    // Decode the encoding argument before reading the typed-array data, so the
    // JS_GetProperty / JS_NewStringCopyZ allocations do not race with the raw
    // data pointer returned by the SM accessor.
    let encoding = if argc > 0 && (*args.get(0).ptr).is_string() {
        unsafe_jsstr_to_string(
            cx,
            ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()),
        )
    } else {
        String::new()
    };
    let enc_lower = encoding.to_lowercase();

    // Honour start/end arguments (Node: toString(enc, start, end)). Default
    // to the full buffer.
    let (len, data_ptr) = buffer_view_bytes(obj_root.get());

    let start = if argc > 1 && (*args.get(1).ptr).is_int32() {
        (*args.get(1).ptr).to_int32().max(0) as usize
    } else {
        0
    };
    let end = if argc > 2 && (*args.get(2).ptr).is_int32() {
        let e = (*args.get(2).ptr).to_int32();
        if e < 0 { 0 } else { e as usize }.min(len)
    } else {
        len
    };
    let (start, end) = if start > end {
        (end, end)
    } else {
        (start, end)
    };

    // Slice the bytes into an owned Vec so the encoding path is free to
    // allocate strings / call into bun_base64 without worrying about the GC
    // moving the typed-array backing store.
    let bytes: Vec<u8> = if data_ptr.is_null() || start >= end {
        Vec::new()
    } else {
        let slice = ::std::slice::from_raw_parts(data_ptr.add(start), end - start);
        slice.to_vec()
    };

    // @trace REQ-ENG-005 [algorithm:utf16le_decode]
    // For ucs2/utf16le output we keep the raw u16 code units so we can build
    // a two-byte SM JSString that preserves ALL code units including lone
    // surrogates (Node.js never validates UTF-16 well-formedness; it just
    // memcpy's the buffer's u16 cells). Rust's String cannot represent
    // unpaired surrogates, so we carry the units separately and let
    // JS_NewStringCopyN rehydrate them as a char16_t JSString.
    let mut utf16_units_out: ::std::option::Option<Vec<u16>> = ::std::option::Option::None;

    let output = match enc_lower.as_str() {
        "" | "utf8" | "utf-8" => String::from_utf8_lossy(&bytes).into_owned(),
        "hex" => bun_core::fmt::bytes_to_hex_lower_string(&bytes),
        "base64" => {
            // @trace REQ-ENG-005 [algorithm:base64]
            let bytes_out = bun_base64::encode_alloc(&bytes);
            ::std::str::from_utf8(&bytes_out).unwrap_or("").to_owned()
        }
        "base64url" => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // bun_base64 has simdutf_encode_url_safe; use it for url-safe encoding.
            let bytes_out = bun_base64::simdutf_encode_url_safe_alloc(&bytes);
            ::std::str::from_utf8(&bytes_out).unwrap_or("").to_owned()
        }
        "binary" | "latin1" => bytes.iter().map(|&b| b as char).collect::<String>(),
        "ascii" => bytes
            .iter()
            .map(|&b| (b & 0x7F) as char)
            .collect::<String>(),
        "ucs2" | "ucs-2" | "utf16le" | "utf-16le" => {
            // @trace REQ-ENG-005 [algorithm:utf16le_decode]
            // Node.js / WHATWG utf16le decoding yields the JS string from
            // raw 16-bit code units — INCLUDING lone surrogates (Node.js
            // never validates well-formedness; it just preserves code units
            // from the buffer byte-for-byte). `char::from_u32` rejects
            // surrogates so a buffer holding `0xD83D 0xDE00` (😀, a
            // surrogate pair) would lose both halves.
            //
            // We stash the units in `utf16_units_out` so the JSString
            // builder below bypasses the UTF-8 path entirely.
            //
            // Reference: Node.js lib/internal/buffer.js + Buffer.prototype.toString
            // 'ucs2' is a straight memcpy-into-UTF-16 — no surrogate validation.
            let mut units: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
            for chunk in bytes.chunks(2) {
                if chunk.len() == 2 {
                    units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
                }
            }
            // Cheap placeholder; real bytes are emitted via units below.
            utf16_units_out = ::std::option::Option::Some(units.clone());
            // Build a UTF-8 fallback for tests that bypass the JSString
            // path (e.g. when SM rejects the two-byte build). Replace any
            // unpaired surrogate with U+FFFD so String stays valid UTF-8.
            String::from_utf16_lossy(&units)
        }
        // @trace REQ-ENG-005 [api:Buffer.toString] — Node.js throws
        // ERR_UNKNOWN_ENCODING for unrecognised encodings (buffer.test.js
        // "invalid encoding"). Only the empty-encoding default falls back
        // to utf8; explicit unknown strings throw.
        other => {
            // Empty / "utf8" / "utf-8" already matched above. Anything else
            // (e.g. "invalid") must throw.
            if other.is_empty() {
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                let msg = format!("Unknown encoding: {}", other);
                let c_msg = ::std::ffi::CString::new(msg)
                    .unwrap_or_else(|_| ::std::ffi::CString::new("Unknown encoding").unwrap());
                mozjs::error::throw_type_error(cx, c_msg.as_ref());
                return false;
            }
        }
    };

    // @trace REQ-ENG-005 — use JS_NewStringCopyUTF8N so multi-byte UTF-8
    // output (ucs2/utf16le/utf8 decoding produces あ-style chars) becomes a
    // proper SM two-byte JSString. JS_NewStringCopyZ treats the buffer as a
    // NUL-terminated C string and re-encodes latin1 chars verbatim, which
    // mangles non-ASCII output (e.g. toString("ucs2") returned mojibake).
    //
    // @trace REQ-ENG-005 [algorithm:utf16le_decode] — when the source
    // encoding was utf16le/ucs2 and the buffer holds lone or paired
    // surrogates, the resulting Rust `String` is NOT valid UTF-8 (Rust
    // rejects isolated surrogates). The encoding path above stashed the raw
    // u16 code units in `utf16_units_out`; we build a two-byte JSString
    // directly via JS_NewStringCopyN (treating each unit as a char16_t).
    // This preserves ALL code units including unpaired surrogates, matching
    // Node.js's byte-faithful ucs2 behaviour.
    let js_str = if let Some(units) = utf16_units_out {
        // Build two-byte string from raw char16_t buffer via the UCString
        // API (preserves ALL code units including unpaired surrogates).
        JS_NewUCStringCopyN(cx, units.as_ptr(), units.len())
    } else {
        let utf8_bytes = output.as_bytes();
        if utf8_bytes.iter().any(|&b| b >= 0x80) {
            let chars = mozjs::conversions::Utf8Chars::from(output.as_str());
            // mozjs_sys expects *mut RawJSContext; our `cx` is *mut JSContext
            // (alias for *mut RawJSContext under mozjs_sys).
            mozjs_sys::jsapi::JS_NewStringCopyUTF8N(
                cx,
                &*chars as *const _ as *const mozjs_sys::jsapi::JS::UTF8Chars,
            )
        } else {
            let c_s = ZBox::from_bytes(utf8_bytes);
            JS_NewStringCopyZ(cx, c_s.as_ptr())
        }
    };
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_alloc(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // @trace REQ-ENG-005 [api:Buffer.alloc] — Node.js requires the `size`
    // argument to be a number (after ToNumber coercion). Objects with
    // valueOf that return a number still pass, but plain objects (no
    // valueOf) and other non-numeric types throw ERR_INVALID_ARG_TYPE.
    // buffer.test.js "alloc() should throw on non-numeric size" drives this.
    let size = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32().max(0) as usize
        } else if v.is_double() {
            v.to_double().max(0.0) as usize
        } else if v.is_boolean() {
            (v.to_boolean() as i32) as usize
        } else if v.is_undefined() || v.is_null() {
            0
        } else {
            // @trace REQ-ENG-005 — Node.js rejects object/string/symbol size
            // (including objects with valueOf) with ERR_INVALID_ARG_TYPE.
            // buffer.test.js "alloc() should throw on non-numeric size".
            mozjs::error::throw_type_error(
                cx,
                c"The \"size\" argument must be of type number.".as_ref(),
            );
            return false;
        }
    } else {
        0
    };

    // @trace REQ-ENG-005 [entity:Buffer] — RangeError guard for huge allocations.
    // Mirrors JSC's MAX_ARRAY_BUFFER_SIZE check: callers that ask for more than
    // the typed-array ceiling get a synchronous RangeError instead of an OOM
    // abort or multi-minute hang while we attempt to materialise 64 GiB of
    // per-byte JS properties.
    if size > MAX_BUFFER_SIZE {
        let msg = format!(
            "Typed arrays are currently limited to {} bytes. To use an array this large, use an ArrayBuffer instead.",
            MAX_BUFFER_SIZE
        );
        let c_msg = ::std::ffi::CString::new(msg)
            .unwrap_or_else(|_| ::std::ffi::CString::new("Buffer size out of range").unwrap());
        mozjs::error::throw_range_error(cx, c_msg.as_ref());
        return false;
    }

    let fill_byte = if argc >= 2 {
        let fill_val = *args.get(1).ptr;
        if fill_val.is_int32() {
            fill_val.to_int32() as u8
        } else if fill_val.is_string() {
            unsafe_jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(fill_val.to_string()))
                .chars()
                .next()
                .unwrap_or('\0') as u8
        } else {
            0
        }
    } else {
        0
    };

    // SM's JS_NewUint8Array zeroes the backing store, so for `fill_byte == 0`
    // (the default Node alloc) we can short-circuit and never materialise a
    // `Vec<u8>` of `size` bytes just to throw it away. This is the path that
    // `Buffer.alloc(64 MiB)` / `Buffer.allocUnsafe(64 MiB)` hit, and skipping
    // the extra allocation is what makes the bun/test/js/node/buffer-concat
    // test (#1 allocates one 64 MiB buffer in <1 ms instead of timing out).
    if fill_byte == 0 {
        let sized = mozjs_sys::jsapi::JS_NewUint8Array(cx, size);
        if sized.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        set_buffer_proto(cx, sized);
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let sized_root = sized);
        rooted!(&in(cx_ref) let is_buf = BooleanValue(true));
        JS_DefineProperty(
            cx,
            sized_root.handle().into(),
            c"_isBuffer".as_ptr(),
            is_buf.handle().into(),
            0u32,
        );
        args.rval().set(mozjs::jsval::ObjectValue(sized));
        return true;
    }

    create_buffer_from_bytes(cx, &args, &vec![fill_byte; size])
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_buffer(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let v = *args.get(0).ptr;
    if !v.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(_cx));
    rooted!(&in(cx_ref) let obj_root = v.to_object());
    let mut marker = UndefinedValue();
    let marker_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut marker,
    };
    JS_GetProperty(
        _cx,
        obj_root.handle().into(),
        c"_isBuffer".as_ptr(),
        marker_handle,
    );
    args.rval().set(mozjs::jsval::BooleanValue(
        marker.is_boolean() && marker.to_boolean(),
    ));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_concat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        return create_buffer_from_bytes(cx, &args, &[]);
    }
    let list_val = *args.get(0).ptr;
    if !list_val.is_object() {
        return create_buffer_from_bytes(cx, &args, &[]);
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let list_root = list_val.to_object());
    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        list_root.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let list_len: usize = if len_val.is_int32() {
        len_val.to_int32().max(0) as usize
    } else if len_val.is_double() {
        let d = len_val.to_double();
        if d.is_finite() && d > 0.0 {
            d as usize
        } else {
            0
        }
    } else {
        0
    };

    // @trace REQ-ENG-005 [algorithm:buffer_concat] — TOCTOU-safe concat.
    //
    // bun/test/js/node/buffer-concat.test.ts exercises a class of bugs where
    // a user-defined getter on the input array detaches or resizes a
    // previously-read buffer via ArrayBuffer.prototype.transfer() /
    // .resize(). Bun's contract is:
    //   - If a buffer's backing store is detached by a later getter,
    //     Buffer.concat throws TypeError (it must not memcpy from a freed
    //     pointer, nor expose uninitialized heap).
    //   - If a resizable buffer shrinks during iteration, the output uses
    //     the *final* (post-getter) length, never the pre-getter length.
    //
    // To meet both requirements we run *all* getters in a first sweep
    // (just reading each element via JS_GetElement — this is what triggers
    // any user-defined getter), then re-read every element's *current*
    // length/data in a second sweep. The post-getter state is what we use
    // for sizing and copying; a typed-array view whose data pointer went
    // null between sweeps was detached, and we throw TypeError.
    #[derive(Clone, Copy)]
    struct ConcatEntry {
        obj: *mut JSObject,
        is_view: bool,
    }
    // GC-trace the element objects so they survive across loop iterations
    // (JS_GetElement triggers user-defined getters which can trigger GC).
    let rooted_vec = mozjs::rust::RootedObjectVectorWrapper::new(cx);
    let mut entries: Vec<ConcatEntry> = Vec::with_capacity(list_len);
    for i in 0..list_len {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            list_root.handle().into(),
            i as u32,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if !elem.is_object() {
            entries.push(ConcatEntry {
                obj: ::std::ptr::null_mut(),
                is_view: false,
            });
            continue;
        }
        let elem_obj = elem.to_object();
        rooted_vec.append(elem_obj);
        // Use a typed-array probe to mark this element. We do not yet read
        // the final length — getters that fire on later indices may still
        // mutate this element's backing store. The probe is only used to
        // distinguish typed-array views (subject to detach/resize
        // semantics) from legacy array inputs.
        let (probe_len, probe_data) = buffer_view_bytes(elem_obj);
        let is_view = probe_len > 0 || !probe_data.is_null();
        entries.push(ConcatEntry {
            obj: elem_obj,
            is_view,
        });
    }

    // Second sweep: read each element's final length & data. Now every
    // getter has run, so this is the truth we copy from.
    let mut element_lengths: Vec<usize> = Vec::with_capacity(list_len);
    let mut element_data: Vec<*mut u8> = Vec::with_capacity(list_len);
    let mut total: usize = 0;
    for entry in entries.iter() {
        if entry.obj.is_null() {
            element_lengths.push(0);
            element_data.push(::std::ptr::null_mut());
            continue;
        }
        if entry.is_view {
            let (cur_len, cur_data) = buffer_view_bytes(entry.obj);
            // Detach detection: a typed-array view whose backing store has
            // been detached reports length 0 and a null data pointer. Bun
            // throws TypeError — without this guard we'd skip the element
            // and potentially expose uninitialized heap in the output.
            if cur_data.is_null() {
                let c_msg = c"Cannot perform Buffer.concat on a detached ArrayBuffer";
                mozjs::error::throw_type_error(cx, c_msg.as_ref());
                return false;
            }
            // @trace REQ-ENG-005 [entity:Buffer] — refuse concat that would
            // overflow MAX_BUFFER_SIZE. `Buffer.concat([huge, huge, ...])` is
            // the common abuse vector (bun/test/js/node/buffer-concat.test.ts
            // allocates 1024 × 64 MiB), so we bail out as soon as the running
            // total crosses the ceiling rather than waiting for OOM.
            if cur_len > MAX_BUFFER_SIZE || total.saturating_add(cur_len) > MAX_BUFFER_SIZE {
                let msg = format!(
                    "Typed arrays are currently limited to {} bytes. To use an array this large, use an ArrayBuffer instead.",
                    MAX_BUFFER_SIZE
                );
                let c_msg = ::std::ffi::CString::new(msg).unwrap_or_else(|_| {
                    ::std::ffi::CString::new("Buffer.concat total length out of range").unwrap()
                });
                mozjs::error::throw_range_error(cx, c_msg.as_ref());
                return false;
            }
            element_lengths.push(cur_len);
            element_data.push(cur_data);
            total = total.saturating_add(cur_len);
        } else {
            // Legacy non-typed-array input: read the JS `length` property
            // and copy element-by-element. No detach semantics apply.
            rooted!(&in(cx_ref) let entry_root = entry.obj);
            let mut blen = UndefinedValue();
            JS_GetProperty(
                cx,
                entry_root.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut blen,
                },
            );
            let b_len = if blen.is_int32() {
                blen.to_int32().max(0) as usize
            } else if blen.is_double() {
                let d = blen.to_double();
                if d.is_finite() && d > 0.0 {
                    d as usize
                } else {
                    0
                }
            } else {
                0
            };
            if b_len > MAX_BUFFER_SIZE || total.saturating_add(b_len) > MAX_BUFFER_SIZE {
                let msg = format!(
                    "Typed arrays are currently limited to {} bytes. To use an array this large, use an ArrayBuffer instead.",
                    MAX_BUFFER_SIZE
                );
                let c_msg = ::std::ffi::CString::new(msg).unwrap_or_else(|_| {
                    ::std::ffi::CString::new("Buffer.concat total length out of range").unwrap()
                });
                mozjs::error::throw_range_error(cx, c_msg.as_ref());
                return false;
            }
            element_lengths.push(b_len);
            element_data.push(::std::ptr::null_mut());
            total = total.saturating_add(b_len);
        }
    }

    // @trace REQ-ENG-005 [algorithm:buffer_concat]
    // Node.js: when totalLength is provided, the result is truncated to
    // totalLength bytes (or zero-filled if totalLength > sum). When
    // omitted, the result length is the sum of list element lengths.
    // Validation per Node.js: negative total → RangeError; non-numeric
    // total → TypeError (buffer.test.js "Buffer.concat" drives both).
    let mut target_total = total;
    if argc >= 2 {
        let tl_val = *args.get(1).ptr;
        if tl_val.is_int32() {
            let n = tl_val.to_int32();
            if n < 0 {
                mozjs::error::throw_range_error(
                    cx,
                    c"\"totalLength\" must be a non-negative integer".as_ref(),
                );
                return false;
            }
            target_total = n as usize;
        } else if tl_val.is_double() {
            let d = tl_val.to_double();
            if !d.is_finite() || d < 0.0 {
                mozjs::error::throw_range_error(
                    cx,
                    c"\"totalLength\" must be a non-negative integer".as_ref(),
                );
                return false;
            }
            target_total = d as usize;
        } else if tl_val.is_undefined() || tl_val.is_null() {
            // undefined/null → use sum of lengths (Node treats as omitted).
        } else {
            // Strings, booleans, objects → ERR_INVALID_ARG_TYPE (TypeError).
            mozjs::error::throw_type_error(
                cx,
                c"\"totalLength\" must be a non-negative integer".as_ref(),
            );
            return false;
        }
        if target_total > MAX_BUFFER_SIZE {
            let msg = format!(
                "Typed arrays are currently limited to {} bytes. To use an array this large, use an ArrayBuffer instead.",
                MAX_BUFFER_SIZE
            );
            let c_msg = ::std::ffi::CString::new(msg).unwrap_or_else(|_| {
                ::std::ffi::CString::new("Buffer.concat totalLength out of range").unwrap()
            });
            mozjs::error::throw_range_error(cx, c_msg.as_ref());
            return false;
        }
    }

    let mut all_bytes = vec![0u8; target_total];
    let mut cursor: usize = 0;
    for (i, entry) in entries.iter().enumerate() {
        if cursor >= target_total {
            break;
        }
        if entry.obj.is_null() {
            continue;
        }
        let b_len = *element_lengths.get(i).unwrap_or(&0);
        if b_len == 0 {
            continue;
        }
        let copy_len = b_len.min(target_total.saturating_sub(cursor));
        if copy_len == 0 {
            cursor = cursor.saturating_add(b_len);
            continue;
        }
        let data = *element_data.get(i).unwrap_or(&::std::ptr::null_mut());
        if !data.is_null() {
            ::std::ptr::copy_nonoverlapping(data, all_bytes.as_mut_ptr().add(cursor), copy_len);
        } else {
            rooted!(&in(cx_ref) let entry_root = entry.obj);
            for j in 0..copy_len {
                let mut byte_val = UndefinedValue();
                JS_GetElement(
                    cx,
                    entry_root.handle().into(),
                    j as u32,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut byte_val,
                    },
                );
                all_bytes[cursor + j] = if byte_val.is_int32() {
                    byte_val.to_int32() as u8
                } else {
                    0
                };
            }
        }
        cursor = cursor.saturating_add(b_len);
    }
    create_buffer_from_bytes(cx, &args, &all_bytes)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_slice(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = this.to_object());

    // @trace REQ-ENG-005 [api:Buffer.slice detach] — Node.js throws TypeError
    // when the buffer's backing ArrayBuffer has been detached (e.g. via
    // structuredClone transfer). Detect via the ArrayBuffer.byteLength === 0
    // convention (SM surfaces a detached ArrayBuffer as byteLength 0).
    // buffer.test.js "slice() on detached buffer throws TypeError" drives this.
    {
        let mut ab_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"buffer".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ab_val,
            },
        );
        if ab_val.is_object() {
            rooted!(&in(cx_ref) let ab_root = ab_val.to_object());
            let mut byte_len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                ab_root.handle().into(),
                c"byteLength".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut byte_len_val,
                },
            );
            let byte_len = if byte_len_val.is_int32() {
                byte_len_val.to_int32()
            } else {
                -1
            };
            if byte_len == 0 {
                mozjs::error::throw_type_error(
                    cx,
                    c"Cannot perform %TypedArray.prototype%.slice on a detached ArrayBuffer"
                        .as_ref(),
                );
                return false;
            }
        }
    }

    // Resolve length defensively: prefer the typed-array byte length (O(1)),
    // fall back to the JS `length` property for legacy callers.
    let (ta_len, ta_data) = buffer_view_bytes(obj_root.get());
    let len = if ta_len > 0 {
        ta_len
    } else {
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        if len_val.is_int32() {
            len_val.to_int32() as usize
        } else {
            0
        }
    };

    // @trace REQ-ENG-005 [api:Buffer.slice] — Node.js coerces start/end via
    // ToInteger (handles strings like "0"/"-5", -0 → 0, NaN → 0, Infinity →
    // len). Negative offsets count from the end. buffer.test.js "Buffer.compare"
    // exercises buf.slice("0","1"), buf.slice("-5","10"), buf.slice("-10","-0"),
    // buf.slice("111") (→ empty), buf.slice(0,-0) (→ empty since -0 coerces
    // to +0 and end<start clamps).
    let to_int_offset = |idx: u32| -> i64 {
        if argc <= idx {
            // Missing argument: start defaults to 0, end defaults to +∞ → len.
            return if idx == 0 { 0 } else { i64::MAX };
        }
        let v = *args.get(idx).ptr;
        if v.is_int32() {
            return v.to_int32() as i64;
        }
        if v.is_double() {
            let d = v.to_double();
            if d.is_nan() {
                return 0;
            }
            if d.is_infinite() {
                return if d > 0.0 { i64::MAX } else { i64::MIN };
            }
            return d.trunc() as i64;
        }
        if v.is_string() {
            let s = unsafe_jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(v.to_string()));
            // Node.js uses ToInteger(string) — empty / non-numeric → 0,
            // "-5" → -5, "111" → 111, "-0" → 0 (but distinguishes -0 in
            // sign? JS Number("-0") is -0; trunc() yields 0).
            let s = s.trim();
            if s.is_empty() {
                return 0;
            }
            match s.parse::<f64>() {
                Ok(d) => {
                    if d.is_nan() {
                        return 0;
                    }
                    return d.trunc() as i64;
                }
                Err(_) => return 0,
            }
        }
        if v.is_undefined() || v.is_null() {
            return if idx == 1 { i64::MAX } else { 0 };
        }
        0
    };
    let s_raw = to_int_offset(0);
    let e_raw = to_int_offset(1);
    // -0 detection: JS Number("-0") is negative zero; treat as 0 here but
    // downstream we still want end<start clamping to apply when both are 0.
    // Negative offsets count from end.
    let start_i = if s_raw < 0 {
        (len as i64 + s_raw).max(0)
    } else {
        s_raw.min(len as i64)
    };
    let end_i = if e_raw < 0 {
        (len as i64 + e_raw).max(0)
    } else {
        e_raw.min(len as i64)
    };
    let start = start_i.max(0) as usize;
    let slice_end = (end_i.max(0) as usize).min(len);
    // If end < start, clamp to empty slice (Node semantics: empty).
    let slice_end = slice_end.max(start);
    // @trace REQ-ENG-005 [api:Buffer.slice shares backing ArrayBuffer] —
    // Node.js Buffer.prototype.slice SHARES the underlying ArrayBuffer (it
    // returns a view into the same memory, NOT a copy). buffer.test.js
    // "only top level parent propagates from a non-pooled instance" drives
    //   const c = b.slice(0, 4); const d = c.slice(0, 2);
    //   expect(c.parent).toBe(d.parent);
    // We construct a Uint8Array view via JS_NewUint8ArrayWithBuffer when the
    // source is a real TypedArray with a backing ArrayBuffer.
    if !ta_data.is_null() && ta_len > 0 {
        // Walk to the underlying ArrayBuffer via the obj's buffer slot.
        let mut ab_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"buffer".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ab_val,
            },
        );
        let mut base_byte_offset: usize = 0;
        if ab_val.is_object() {
            rooted!(&in(cx_ref) let ab_root = ab_val.to_object());
            // Source's own byteOffset — the new view is offset by this plus `start`.
            let mut bo_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj_root.handle().into(),
                c"byteOffset".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut bo_val,
                },
            );
            if bo_val.is_int32() {
                base_byte_offset = (bo_val.to_int32().max(0)) as usize;
            }
            let view_len: i64 = (slice_end - start) as i64;
            let new_offset = base_byte_offset + start;
            let view = mozjs_sys::jsapi::JS_NewUint8ArrayWithBuffer(
                cx,
                ab_root.handle().into(),
                new_offset,
                view_len,
            );
            if !view.is_null() {
                set_buffer_proto(cx, view);
                let cx_ref =
                    mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                rooted!(&in(cx_ref) let view_root = view);
                rooted!(&in(cx_ref) let is_buf = BooleanValue(true));
                JS_DefineProperty(
                    cx,
                    view_root.handle().into(),
                    c"_isBuffer".as_ptr(),
                    is_buf.handle().into(),
                    0u32,
                );
                args.rval().set(ObjectValue(view));
                return true;
            }
        }
        // Fallback: copy bytes if we couldn't share.
        let bytes: Vec<u8> =
            ::std::slice::from_raw_parts(ta_data.add(start), slice_end - start).to_vec();
        return create_buffer_from_bytes(cx, &args, &bytes);
    }
    let bytes: Vec<u8> = {
        // Legacy property-storage path (kept for non-typed-array Buffer-likes
        // handed in by callers that predate the typed-array refactor).
        let mut v = Vec::new();
        for i in start..slice_end {
            let mut byte_val = UndefinedValue();
            JS_GetElement(
                cx,
                obj_root.handle().into(),
                i as u32,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut byte_val,
                },
            );
            v.push(if byte_val.is_int32() {
                byte_val.to_int32() as u8
            } else {
                0
            });
        }
        v
    };
    create_buffer_from_bytes(cx, &args, &bytes)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_copy(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    // @trace REQ-ENG-005 [api:Buffer.copy] — Node.js throws ERR_INVALID_ARG_TYPE
    // (TypeError) when `target` is missing or not a Buffer/Uint8Array. Test
    // "Buffer,poolSize" drives Buffer.allocUnsafe(10).copy().
    if !this.is_object() || argc == 0 || !(*args.get(0).ptr).is_object() {
        mozjs::error::throw_type_error(cx, c"The \"target\" argument must be an instance of ArrayBufferView. Received type undefined".as_ref());
        return false;
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let src_root = this.to_object());

    // Node.js Buffer#copy(target, targetStart, sourceStart, sourceEnd)
    let target_val = *args.get(0).ptr;
    if !target_val.is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    rooted!(&in(cx_ref) let tgt_root = target_val.to_object());

    // @trace REQ-ENG-005 [api:Buffer.copy TOCTOU] — Node.js evaluates the
    // numeric arguments left-to-right via ToNumber, which invokes user
    // valueOf callbacks. Those callbacks can detach or resize the source /
    // target backing ArrayBuffer (ArrayBuffer.prototype.transfer / resize).
    // The TOCTOU contract (see buffer-copy-fill-detach.test.ts):
    //
    //   • targetStart is range-checked BEFORE sourceStart's valueOf runs —
    //     `copy(target, -1, {valueOf: throws})` throws ERR_OUT_OF_RANGE
    //     without invoking sourceStart's valueOf.
    //   • After every user callback has run, re-read src_len / tgt_len. A
    //     detached buffer yields length 0 → copy returns 0 (no crash). A
    //     resized-down buffer yields a smaller logical length → copy clamps
    //     sourceEnd / sourceStart / targetStart to the new length and never
    //     reads past the post-resize boundary (no OOB read).
    //   • sourceStart is bounds-checked against the PRE-sourceEnd-coercion
    //     source length: `copy(target, 0, 100, {valueOf: resize-to-50})`
    //     keeps sourceStart=100 valid against the original length 1024 and
    //     yields 0 bytes copied (Node parity).

    // Step 1: ToNumber(targetStart). Default 0. Triggers user valueOf.
    let tgt_start_raw = if argc > 1 && !(*args.get(1).ptr).is_undefined() {
        let h = mozjs::rust::HandleValue::from_marked_location(args.get(1).ptr);
        match unsafe { mozjs::rust::ToNumber(cx, h) } {
            Ok(n) => n,
            Err(()) => return false,
        }
    } else {
        0.0
    };
    // Node validates targetStart < 0 BEFORE coercing sourceStart (test parity).
    if !(tgt_start_raw.is_finite()) || tgt_start_raw < 0.0 {
        let _ = throw_error_with_code(
            cx,
            true,
            "ERR_OUT_OF_RANGE",
            "The value of \"targetStart\" is out of range. It must be >= 0",
        );
        return false;
    }

    // Step 2: ToNumber(sourceStart). Default 0.
    let src_start_raw = if argc > 2 && !(*args.get(2).ptr).is_undefined() {
        let h = mozjs::rust::HandleValue::from_marked_location(args.get(2).ptr);
        match unsafe { mozjs::rust::ToNumber(cx, h) } {
            Ok(n) => n,
            Err(()) => return false,
        }
    } else {
        0.0
    };

    // Read the PRE-sourceEnd-coercion source length — Node bounds-checks
    // sourceStart against this length, NOT the post-sourceEnd-coercion
    // length (test "sourceStart primitive stays valid when sourceEnd valueOf
    // shrinks source" drives this).
    let (src_len_pre, _src_data_pre) = buffer_view_bytes(src_root.get());
    if !src_start_raw.is_finite() || src_start_raw < 0.0 || src_start_raw > src_len_pre as f64 {
        let _ = throw_error_with_code(
            cx,
            true,
            "ERR_OUT_OF_RANGE",
            "The value of \"sourceStart\" is out of range.",
        );
        return false;
    }

    // Step 3: ToNumber(sourceEnd). Default = pre-coercion src_len.
    let src_end_raw = if argc > 3 && !(*args.get(3).ptr).is_undefined() {
        let h = mozjs::rust::HandleValue::from_marked_location(args.get(3).ptr);
        match unsafe { mozjs::rust::ToNumber(cx, h) } {
            Ok(n) => n,
            Err(()) => return false,
        }
    } else {
        src_len_pre as f64
    };

    // @trace REQ-ENG-005 [TOCTOU] — Re-read lengths AFTER all user callbacks
    // have run. detach → (0, null); resize-down → smaller logical length.
    let (src_len, src_data) = buffer_view_bytes(src_root.get());
    let (tgt_len, tgt_data) = buffer_view_bytes(tgt_root.get());

    // Coerced numbers → integer indices. Negative sourceEnd → 0.
    let tgt_start = tgt_start_raw as usize;
    let mut src_start = src_start_raw as usize;
    let mut src_end = if !src_end_raw.is_finite() {
        src_len
    } else if src_end_raw < 0.0 {
        0
    } else {
        src_end_raw as usize
    };
    // Clamp to current (post-resize) lengths.
    if src_end > src_len {
        src_end = src_len;
    }
    if src_start > src_len {
        src_start = src_len;
    }

    // Detach / empty: either side reports length 0 → no bytes copied.
    // Node parity: copy returns 0 when either side is detached.
    if src_len == 0 || tgt_len == 0 {
        args.rval().set(Int32Value(0));
        return true;
    }

    if src_start >= src_end {
        args.rval().set(Int32Value(0));
        return true;
    }

    let copy_len_src = src_end - src_start;
    let copy_len_tgt = tgt_len.saturating_sub(tgt_start);
    let copy_len = copy_len_src.min(copy_len_tgt);

    if copy_len == 0 {
        args.rval().set(Int32Value(0));
        return true;
    }

    if !src_data.is_null() && !tgt_data.is_null() {
        // Fast path: both sides are Typed arrays → memcpy.
        ::std::ptr::copy_nonoverlapping(src_data.add(src_start), tgt_data.add(tgt_start), copy_len);
    } else {
        // Legacy per-element copy. Node writes from source to target at
        // targetStart + i, NOT at i (canonical Node semantics).
        for i in 0..copy_len {
            let mut byte_val = UndefinedValue();
            JS_GetElement(
                cx,
                src_root.handle().into(),
                (src_start + i) as u32,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut byte_val,
                },
            );
            let b = if byte_val.is_int32() {
                byte_val.to_int32() as u8
            } else {
                0
            };
            rooted!(&in(cx_ref) let b_root = Int32Value(b as i32));
            JS_SetElement(
                cx,
                tgt_root.handle().into(),
                (tgt_start + i) as u32,
                b_root.handle().into(),
            );
        }
    }
    args.rval().set(Int32Value(copy_len as i32));
    true
}

// @trace REQ-ENG-005 — Construct a TypeError or RangeError, attach a Node.js
// `.code` property (e.g. "ERR_OUT_OF_RANGE", "ERR_UNKNOWN_ENCODING"), set it
// as the pending exception, and return false for use in `extern "C"` hooks.
// Mirrors the node_fs.rs pending-exception pattern.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_error_with_code(cx: *mut JSContext, range: bool, code: &str, msg: &str) -> bool {
    let c_msg = ::std::ffi::CString::new(msg)
        .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
    if range {
        mozjs::error::throw_range_error(cx, c_msg.as_ref());
    } else {
        mozjs::error::throw_type_error(cx, c_msg.as_ref());
    }
    if JS_IsExceptionPending(cx) {
        rooted!(in(cx) let mut exn = UndefinedValue());
        JS_GetPendingException(cx, exn.handle_mut().into());
        let exn_val = exn.get();
        if !exn_val.is_undefined() && exn_val.is_object() {
            let cx_ref_err =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref_err) let exn_root = exn_val.to_object());
            let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(code.as_bytes()).as_ptr());
            if !code_str.is_null() {
                let code_val = StringValue(&*code_str);
                rooted!(&in(cx_ref_err) let code_r = code_val);
                JS_DefineProperty(
                    cx,
                    exn_root.handle().into(),
                    c"code".as_ptr(),
                    code_r.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                JS_SetPendingException(
                    cx,
                    exn.handle().into(),
                    ExceptionStackBehavior::DoNotCapture,
                );
            }
        }
    }
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_equals(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() || argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let src_root = this.to_object());
    let (src_len, src_data) = buffer_view_bytes(src_root.get());

    let other_val = *args.get(0).ptr;
    if !other_val.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    rooted!(&in(cx_ref) let tgt_root = other_val.to_object());
    let (tgt_len, tgt_data) = buffer_view_bytes(tgt_root.get());

    if src_len != tgt_len {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }

    if !src_data.is_null() && !tgt_data.is_null() {
        // memcmp fast path for two typed-array backed buffers.
        let equal = ::std::slice::from_raw_parts(src_data, src_len)
            == ::std::slice::from_raw_parts(tgt_data, tgt_len);
        args.rval().set(mozjs::jsval::BooleanValue(equal));
        return true;
    }

    // Legacy per-element fallback.
    for i in 0..src_len {
        let mut a_val = UndefinedValue();
        JS_GetElement(
            cx,
            src_root.handle().into(),
            i as u32,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut a_val,
            },
        );
        let mut b_val = UndefinedValue();
        JS_GetElement(
            cx,
            tgt_root.handle().into(),
            i as u32,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut b_val,
            },
        );
        let a = if a_val.is_int32() {
            a_val.to_int32() as u8
        } else {
            0
        };
        let b = if b_val.is_int32() {
            b_val.to_int32() as u8
        } else {
            0
        };
        if a != b {
            args.rval().set(mozjs::jsval::BooleanValue(false));
            return true;
        }
    }
    args.rval().set(mozjs::jsval::BooleanValue(true));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_index_of(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() || argc == 0 {
        args.rval().set(Int32Value(-1));
        return true;
    }

    let mut cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = this.to_object());

    let (_buf_len, data_ptr) = buffer_view_bytes(obj_root.get());
    if data_ptr.is_null() {
        // Fallback to JS-level access for non-typed-array Buffer-likes.
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        // Reuse the legacy implementation on this slow path.
        let len = if len_val.is_int32() {
            len_val.to_int32() as usize
        } else {
            0
        };
        return buffer_index_of_legacy(cx, &args, argc, obj_root.handle().into(), len);
    }

    // @trace REQ-ENG-005 [api:Buffer.indexOf detach semantics] — Node.js:
    // byteOffset is coerced via ToInteger which invokes valueOf callbacks.
    // The user's valueOf may detach the backing ArrayBuffer (ArrayBuffer.transfer),
    // in which case the haystack must be treated as length 0 → indexOf returns -1.
    // We coerce BEFORE reading bytes so the detach is observable, then re-read
    // the view to detect it.
    //
    // @trace REQ-ENG-005 — encoding-as-2nd-arg overload: when the needle is a
    // string and the second argument is a string, it's an encoding (not a
    // byteOffset). buffer.test.js "indexOf(value, encoding) is unchanged by
    // the lastIndexOf fix" drives b.indexOf("hello", "utf16le"). Detect this
    // case BEFORE the ToInt32 coercion so we don't treat "utf16le" as offset 0.
    let encoding_arg_at_pos1 =
        argc >= 2 && (*args.get(0).ptr).is_string() && (*args.get(1).ptr).is_string();
    let byte_offset: i64 = if argc >= 2 && !encoding_arg_at_pos1 {
        let off_val = *args.get(1).ptr;
        rooted!(&in(cx_ref) let off_root = off_val);
        match mozjs::rust::ToInt32(cx, off_root.handle()) {
            Ok(v) => v as i64,
            Err(_) => {
                // Coercion threw — propagate.
                return false;
            }
        }
    } else {
        0
    };
    let byte_offset_raw = byte_offset;

    // Re-read the buffer view AFTER coercion — if valueOf detached the
    // backing store, data_ptr is now null and length is 0.
    let (buf_len_post, data_ptr_post) = buffer_view_bytes(obj_root.get());
    let detached = data_ptr_post.is_null() || buf_len_post == 0;

    // @trace REQ-ENG-005 — Node parity: a negative byteOffset is interpreted
    // as an offset from the END of the buffer (offset = max(0, len+offset)).
    // buffer.test.js "indexOf/lastIndexOf with an explicit byteOffset" drives
    // `b.indexOf("hello", -5)` on "hello world hello" (len=17) to start the
    // search at index 12 (the second "hello"), NOT at index 0. Computed
    // against the post-coercion length so a valueOf that resized the buffer
    // is honoured.
    let byte_offset = if byte_offset_raw < 0 {
        let from_end = buf_len_post as i64 + byte_offset_raw;
        if from_end < 0 { 0 } else { from_end as usize }
    } else {
        byte_offset_raw as usize
    };

    // String encoding argument coercion (ToString invokes toString callbacks
    // which may also detach the buffer).
    let encoding: ::std::string::String = if encoding_arg_at_pos1 {
        // encoding-as-2nd-arg overload: arg[1] is the encoding string.
        let enc_val = *args.get(1).ptr;
        rooted!(&in(cx_ref) let enc_root = enc_val);
        let enc_str_ptr = mozjs::rust::ToString(&mut cx_ref, enc_root.handle());
        if !enc_str_ptr.is_null() {
            unsafe_jsstr_to_string(cx, NonNull::new_unchecked(enc_str_ptr)).to_lowercase()
        } else {
            return false;
        }
    } else if argc >= 3 {
        let enc_val = *args.get(2).ptr;
        rooted!(&in(cx_ref) let enc_root = enc_val);
        let enc_str_ptr = mozjs::rust::ToString(&mut cx_ref, enc_root.handle());
        if !enc_str_ptr.is_null() {
            unsafe_jsstr_to_string(cx, NonNull::new_unchecked(enc_str_ptr)).to_lowercase()
        } else {
            // ToString threw.
            return false;
        }
    } else {
        "utf8".to_string()
    };

    // Re-read again after encoding toString coercion.
    let (buf_len_final, data_ptr_final) = buffer_view_bytes(obj_root.get());
    let detached_final = data_ptr_final.is_null() || buf_len_final == 0;

    if detached || detached_final {
        // Haystack detached → treat as length 0 → indexOf returns -1.
        args.rval().set(Int32Value(-1));
        return true;
    }

    let buf_len = buf_len_final;
    let bytes = ::std::slice::from_raw_parts(data_ptr_final, buf_len);
    let search_val = *args.get(0).ptr;
    let _ = buf_len; // silence unused warning if buffer was re-read above

    // @trace REQ-ENG-005 — Number needle: ToNumber coercion so a double like
    // 99.9 or an out-of-range integer like 256+99=355 is masked to its low
    // byte (Node parity — buffer.test.js "indexOf" drives b.indexOf(99.9) and
    // b.indexOf(256 + 99) both to match 'c'=99). We accept any numeric
    // primitive (int32 / double) and also coerce non-primitive numbers via
    // ToNumber so valueOf side effects run. IMPORTANT: a Buffer / Uint8Array
    // is also an object but is treated as a needle buffer (see below), NOT as
    // a number — `ToNumber(buffer)` returns NaN, which would mask to 0 and
    // spuriously match every zero byte.
    let search_is_buffer_like = search_val.is_object() && {
        let cx_ref_s = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref_s) let obj_v_root = search_val.to_object());
        let mut _len: usize = 0;
        let mut _shared = false;
        let mut _data: *mut u8 = ::std::ptr::null_mut();
        let unwrapped = mozjs_sys::jsapi::JS_GetObjectAsUint8Array(
            obj_v_root.get(),
            &mut _len,
            &mut _shared,
            &mut _data,
        );
        !unwrapped.is_null()
    };
    let needle_num: Option<f64> = if search_val.is_number() {
        Some(if search_val.is_int32() {
            search_val.to_int32() as f64
        } else {
            search_val.to_double()
        })
    } else if search_val.is_object() && !search_is_buffer_like {
        // Plain object with valueOf: try ToNumber. Buffer / Uint8Array is
        // excluded — those are handled as a byte needle below.
        rooted!(&in(cx_ref) let search_root = search_val);
        match mozjs::rust::ToNumber(cx, search_root.handle()) {
            Ok(n) => Some(n),
            Err(_) => return false,
        }
    } else {
        None
    };
    if let Some(num) = needle_num {
        // Node parity: indexOf(99.9) → 99 & 0xff = 99; indexOf(355) → 355 & 0xff = 99.
        let n = num.trunc();
        let needle = if n < 0.0 { 0 } else { (n as i64 & 0xFF) as u8 };
        if byte_offset > buf_len {
            args.rval().set(Int32Value(-1));
            return true;
        }
        for (idx, &b) in bytes[byte_offset..].iter().enumerate() {
            if b == needle {
                args.rval().set(Int32Value((byte_offset + idx) as i32));
                return true;
            }
        }
        args.rval().set(Int32Value(-1));
        return true;
    }
    // Buffer / Uint8Array needle: byte-exact substring search.
    if search_is_buffer_like {
        let cx_ref_n = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref_n) let needle_root = search_val.to_object());
        let (needle_len, needle_data) = buffer_view_bytes(needle_root.get());
        if needle_len == 0 {
            args.rval().set(DoubleValue(byte_offset as f64));
            return true;
        }
        if byte_offset > buf_len || needle_len > buf_len - byte_offset {
            args.rval().set(Int32Value(-1));
            return true;
        }
        let needle = ::std::slice::from_raw_parts(needle_data, needle_len);
        for i in byte_offset..=(buf_len - needle_len) {
            if bytes[i..i + needle_len] == *needle {
                args.rval().set(Int32Value(i as i32));
                return true;
            }
        }
        args.rval().set(Int32Value(-1));
        return true;
    }
    if search_val.is_string() {
        let js_str = search_val.to_string();
        let needle_str = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(js_str));
        // encoding was coerced above via ToString (which may detach).
        let needle: Vec<u8> = match encoding.as_str() {
            "utf8" | "utf-8" | "" => needle_str.bytes().collect(),
            "ucs2" | "ucs-2" | "utf16le" | "utf-16le" => {
                let mut out = Vec::with_capacity(needle_str.len() * 2);
                for c in needle_str.chars() {
                    let code = c as u32;
                    if code <= 0xFFFF {
                        out.extend_from_slice(&(code as u16).to_le_bytes());
                    } else {
                        let v = code - 0x10000;
                        out.extend_from_slice(&(0xD800 + (v >> 10) as u16).to_le_bytes());
                        out.extend_from_slice(&(0xDC00 + (v & 0x3FF) as u16).to_le_bytes());
                    }
                }
                out
            }
            "latin1" | "binary" | "ascii" => needle_str
                .chars()
                .map(|c| (c as u32 & 0xFF) as u8)
                .collect(),
            "hex" => {
                let mut out = Vec::new();
                let chars: Vec<char> = needle_str.chars().collect();
                let mut i = 0;
                while i + 1 < chars.len() {
                    let hi = chars[i].to_digit(16).unwrap_or(0);
                    let lo = chars[i + 1].to_digit(16).unwrap_or(0);
                    out.push(((hi << 4) | lo) as u8);
                    i += 2;
                }
                out
            }
            "base64" => bun_base64::decode_alloc(needle_str.as_bytes()).unwrap_or_default(),
            "base64url" => bun_base64::decode_alloc(needle_str.as_bytes()).unwrap_or_default(),
            _ => needle_str.bytes().collect(),
        };
        if needle.is_empty() {
            // Node: empty needle matches at byte_offset.
            args.rval().set(DoubleValue(byte_offset as f64));
            return true;
        }
        if needle.len() > buf_len || byte_offset > buf_len - needle.len() {
            args.rval().set(Int32Value(-1));
            return true;
        }
        // memchr-like scan: O(N*M) worst case but with a tight inner loop.
        'outer: for i in byte_offset..=(buf_len - needle.len()) {
            for (j, &nbyte) in needle.iter().enumerate() {
                if bytes[i + j] != nbyte {
                    continue 'outer;
                }
            }
            args.rval().set(Int32Value(i as i32));
            return true;
        }
    } else if search_val.is_object() {
        // Buffer/Uint8Array needle — use the typed-array view bytes directly.
        // @trace REQ-ENG-005 [api:Buffer.indexOf detach needle] — Node.js:
        // if the needle was detached via the byteOffset/encoding callbacks,
        // treat it as length 0 → matches at byte_offset (empty needle).
        let cx_ref_nd = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref_nd) let needle_root = search_val.to_object());
        let (n_len, n_data) = buffer_view_bytes(needle_root.get());
        if n_data.is_null() || n_len == 0 {
            args.rval().set(DoubleValue(byte_offset as f64));
            return true;
        }
        if n_len > buf_len || byte_offset > buf_len - n_len {
            args.rval().set(Int32Value(-1));
            return true;
        }
        let needle = ::std::slice::from_raw_parts(n_data, n_len);
        'outer2: for i in byte_offset..=(buf_len - n_len) {
            for (j, &nbyte) in needle.iter().enumerate() {
                if bytes[i + j] != nbyte {
                    continue 'outer2;
                }
            }
            args.rval().set(Int32Value(i as i32));
            return true;
        }
    }
    args.rval().set(Int32Value(-1));
    true
}

/// Legacy per-element indexOf, used only when `this` is not a typed array
/// (kept for any non-Buffer array-like callers that predate the typed-array
/// refactor). Mirrors the previous implementation verbatim.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn buffer_index_of_legacy(
    cx: *mut JSContext,
    args: &CallArgs,
    argc: u32,
    obj_h: Handle<*mut JSObject>,
    buf_len: usize,
) -> bool {
    let byte_offset = if argc >= 2 {
        let off_val = *args.get(1).ptr;
        if off_val.is_int32() {
            off_val.to_int32().max(0) as usize
        } else {
            0
        }
    } else {
        0
    };

    let search_val = *args.get(0).ptr;
    if search_val.is_int32() {
        let needle = search_val.to_int32() as u8;
        for i in byte_offset..buf_len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                obj_h,
                i as u32,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            if elem.is_int32() && elem.to_int32() as u8 == needle {
                args.rval().set(Int32Value(i as i32));
                return true;
            }
        }
    } else if search_val.is_string() {
        let js_str = search_val.to_string();
        let needle_str = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(js_str));
        // @trace REQ-ENG-005 [api:Buffer.indexOf/lastIndexOf] — Node.js
        // honours the optional encoding argument (positional idx 2 for
        // indexOf, idx 2 for lastIndexOf): encode the needle string under
        // that encoding before scanning. Default is utf8.
        let encoding = if argc >= 3 && (*args.get(2).ptr).is_string() {
            unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(2).ptr).to_string()))
                .to_lowercase()
        } else {
            "utf8".to_string()
        };
        let needle: Vec<u8> = match encoding.as_str() {
            "utf8" | "utf-8" | "" => needle_str.bytes().collect(),
            "ucs2" | "ucs-2" | "utf16le" | "utf-16le" => {
                let mut out = Vec::with_capacity(needle_str.len() * 2);
                for c in needle_str.chars() {
                    let code = c as u32;
                    if code <= 0xFFFF {
                        out.extend_from_slice(&(code as u16).to_le_bytes());
                    } else {
                        let v = code - 0x10000;
                        out.extend_from_slice(&(0xD800 + (v >> 10) as u16).to_le_bytes());
                        out.extend_from_slice(&(0xDC00 + (v & 0x3FF) as u16).to_le_bytes());
                    }
                }
                out
            }
            "latin1" | "binary" | "ascii" => needle_str
                .chars()
                .map(|c| (c as u32 & 0xFF) as u8)
                .collect(),
            "hex" => {
                let mut out = Vec::new();
                let chars: Vec<char> = needle_str.chars().collect();
                let mut i = 0;
                while i + 1 < chars.len() {
                    let hi = chars[i].to_digit(16).unwrap_or(0);
                    let lo = chars[i + 1].to_digit(16).unwrap_or(0);
                    out.push(((hi << 4) | lo) as u8);
                    i += 2;
                }
                out
            }
            "base64" => bun_base64::decode_alloc(needle_str.as_bytes()).unwrap_or_default(),
            "base64url" => bun_base64::decode_alloc(needle_str.as_bytes()).unwrap_or_default(),
            _ => needle_str.bytes().collect(),
        };
        if needle.is_empty() || needle.len() > buf_len {
            args.rval().set(DoubleValue(byte_offset as f64));
            return true;
        }
        'outer: for i in byte_offset..=(buf_len - needle.len()) {
            for (j, &nbyte) in needle.iter().enumerate() {
                let mut elem = UndefinedValue();
                JS_GetElement(
                    cx,
                    obj_h,
                    (i + j) as u32,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                );
                let b = if elem.is_int32() {
                    elem.to_int32() as u8
                } else {
                    0
                };
                if b != nbyte {
                    continue 'outer;
                }
            }
            args.rval().set(Int32Value(i as i32));
            return true;
        }
    }
    args.rval().set(Int32Value(-1));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_is_encoding(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let valid = [
        "utf8",
        "utf-8",
        "ascii",
        "latin1",
        "binary",
        "base64",
        "base64url",
        "hex",
        "ucs2",
        "ucs-2",
        "utf16le",
        "utf-16le",
    ];
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let enc_val = *args.get(0).ptr;
    if !enc_val.is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let enc_str = unsafe_jsstr_to_string(_cx, ::std::ptr::NonNull::new_unchecked(enc_val.to_string()));
    let is_valid = valid.iter().any(|&v| v == enc_str.to_lowercase());
    args.rval().set(mozjs::jsval::BooleanValue(is_valid));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_byte_length(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        // @trace REQ-ENG-005 — Buffer.byteLength() (no args) throws
        // ERR_INVALID_ARG_TYPE (a TypeError) per Node.js.
        mozjs::error::throw_type_error(
            cx,
            c"The \"string\", \"Buffer\", or \"TypedArray\" argument must be of type string or an instance of Buffer, TypedArray, or DataView. Received undefined".as_ref(),
        );
        return false;
    }
    let input = *args.get(0).ptr;
    // @trace REQ-ENG-005 [api:Buffer.byteLength] — Node.js honours the
    // optional encoding argument: utf8/ascii/latin1/hex count via the
    // encoded byte length, ucs2/utf16le is 2 bytes per code unit (4 per
    // surrogate pair), base64/base64url is the decoded length. Default
    // encoding is utf8.
    let encoding = if argc >= 2 && (*args.get(1).ptr).is_string() {
        unsafe_jsstr_to_string(
            cx,
            ::std::ptr::NonNull::new_unchecked((*args.get(1).ptr).to_string()),
        )
        .to_lowercase()
    } else {
        "utf8".to_string()
    };
    if input.is_string() {
        let s = crate::js_to_rust_string(cx, input);
        let n: i32 = match encoding.as_str() {
            "hex" => {
                // Each pair of hex digits = 1 byte; odd-length rounds up by
                // one (Node.js pads a leading 0). Non-hex chars contribute 0.
                let mut bytes = 0usize;
                let mut pair = false;
                for c in s.chars() {
                    if c.is_ascii_hexdigit() {
                        if pair {
                            bytes += 1;
                            pair = false;
                        } else {
                            pair = true;
                        }
                    }
                }
                (if pair { bytes + 1 } else { bytes }) as i32
            }
            "ucs2" | "ucs-2" | "utf16le" | "utf-16le" | "utf16be" | "utf-16be" => {
                let mut n = 0usize;
                for c in s.chars() {
                    let code = c as u32;
                    if code <= 0xFFFF {
                        n += 2;
                    } else {
                        n += 4;
                    }
                }
                n as i32
            }
            "base64" | "base64url" => {
                // Strip whitespace + (for url) map chars, then count decoded
                // length per RFC 4648 (4 chars → 3 bytes, with padding).
                let stripped: String = s.chars().filter(|c| !c.is_whitespace()).collect();
                let canonical: String = if encoding == "base64url" {
                    let mut t: String = stripped
                        .chars()
                        .map(|c| match c {
                            '-' => '+',
                            '_' => '/',
                            _ => c,
                        })
                        .collect();
                    while t.len() % 4 != 0 {
                        t.push('=');
                    }
                    t
                } else {
                    stripped
                };
                if canonical.is_empty() {
                    0
                } else {
                    // @trace REQ-ENG-005 — RFC 4648 decoded length: only the
                    // non-padding characters contribute (each group of 4
                    // non-pad chars → 3 bytes); pad chars are stripped first.
                    // Node parity: "aaaa==" (4 non-pad + 2 pad) → 3 bytes,
                    // "bHNr…cw==" (34 non-pad + 2 pad) → 25 bytes,
                    // "aGkk" (4 non-pad) → 3 bytes.
                    let non_pad = canonical.chars().filter(|&c| c != '=').count();
                    (non_pad as i32 * 3) / 4
                }
            }
            // @trace REQ-ENG-005 — latin1/binary/ascii → 1 byte per code unit
            // (chars().count(), NOT UTF-8 byte length). utf8/utf-8/"" → UTF-8
            // byte length (s.len()). buffer.test.js "Buffer.byteLength()"
            // drives "Il était tué" latin1 → 12 (12 code units) vs utf8 → 14.
            "latin1" | "binary" | "ascii" => s.chars().count() as i32,
            // utf8 / utf-8 / "" → UTF-8 byte length.
            _ => s.len() as i32,
        };
        args.rval().set(Int32Value(n));
    } else if input.is_object() {
        // @trace REQ-ENG-005 — ArrayBuffer / TypedArray / Buffer / DataView
        // → byte length. Non-view plain objects (e.g. {}) must still throw
        // ERR_INVALID_ARG_TYPE (test "Buffer.byteLength()").
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = input.to_object());
        let is_ab = mozjs_sys::jsapi::JS::IsArrayBufferObject(obj_root.get());
        let is_view = unsafe { mozjs_sys::jsapi::JS_IsArrayBufferViewObject(obj_root.get()) };
        if !is_ab && !is_view {
            mozjs::error::throw_type_error(
                cx,
                c"The \"string\", \"Buffer\", or \"TypedArray\" argument must be of type string or an instance of Buffer, TypedArray, or DataView. Received [object Object]".as_ref(),
            );
            return false;
        }
        // @trace REQ-ENG-005 — ArrayBuffer → byteLength via the JS
        // ArrayBuffer.prototype.byteLength accessor; TypedArray / Buffer /
        // DataView → use the typed-array view bytes. buffer.test.js
        // "Buffer.byteLength()" drives ArrayBuffer(8) → 8, Int8Array(8) → 8,
        // Buffer.from([…]) → its length.
        let n: usize = if is_ab {
            let mut bl_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj_root.handle().into(),
                c"byteLength".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut bl_val,
                },
            );
            if bl_val.is_int32() {
                bl_val.to_int32().max(0) as usize
            } else {
                0
            }
        } else {
            // @trace REQ-ENG-005 — Any ArrayBuffer view (TypedArray /
            // DataView / Buffer) — read its `.byteLength` property directly
            // rather than going through JS_GetObjectAsUint8Array (which only
            // recognises Uint8Array and returns 0 for Int8Array etc.).
            let mut bl_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj_root.handle().into(),
                c"byteLength".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut bl_val,
                },
            );
            if bl_val.is_int32() {
                bl_val.to_int32().max(0) as usize
            } else {
                0
            }
        };
        args.rval().set(DoubleValue(n as f64));
    } else {
        // @trace REQ-ENG-005 — Node.js throws ERR_INVALID_ARG_TYPE (a
        // TypeError) for non-string/non-ArrayBufferView/non-ArrayBuffer
        // input. Test "Buffer.byteLength()" drives 32/NaN/{}/().
        mozjs::error::throw_type_error(
            cx,
            c"The \"string\", \"Buffer\", or \"TypedArray\" argument must be of type string or an instance of Buffer, TypedArray, or DataView. Received ".as_ref(),
        );
        return false;
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn buffer_compare(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(Int32Value(0));
        return true;
    }

    let read_bytes = |obj: *mut JSObject| -> (::std::vec::Vec<u8>, usize) {
        // Fast path: typed-array backed → memcpy.
        let (n, ptr) = buffer_view_bytes(obj);
        if !ptr.is_null() {
            let slice = ::std::slice::from_raw_parts(ptr, n);
            return (slice.to_vec(), n);
        }
        // Slow fallback: legacy per-element read.
        let cx_ref_c = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref_c) let obj_root = obj);
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() {
            len_val.to_int32() as usize
        } else {
            0
        };
        let mut bytes = ::std::vec::Vec::with_capacity(len);
        for i in 0..len {
            let mut v = UndefinedValue();
            JS_GetElement(
                cx,
                obj_root.handle().into(),
                i as u32,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            );
            bytes.push(if v.is_int32() { v.to_int32() as u8 } else { 0 });
        }
        (bytes, len)
    };

    let a_val = *args.get(0).ptr;
    let b_val = *args.get(1).ptr;
    // @trace REQ-ENG-006 [api:Buffer.compare] — Node.js throws ERR_INVALID_ARG_TYPE
    // ("The \"buf1\", \"buf2\" arguments must be of type Uint8Array") when either
    // argument is not a Buffer/Uint8Array. Mirror that here so the static
    // `Buffer.compare(x, nonBuffer)` path matches prototype semantics.
    if !a_val.is_object() || !b_val.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"The \"buf1\", \"buf2\" arguments must be of type Uint8Array or Buffer".as_ptr(),
        );
        return false;
    }
    let cx_ref_cmp = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref_cmp) let a_root = a_val.to_object());
    rooted!(&in(cx_ref_cmp) let b_root = b_val.to_object());
    let (a_bytes, _) = read_bytes(a_root.handle().get());
    let (b_bytes, _) = read_bytes(b_root.handle().get());
    args.rval().set(Int32Value(a_bytes.cmp(&b_bytes) as i32));
    true
}

pub fn install_crypto_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let crypto_obj = JS_NewPlainObject(cx));
        if crypto_obj.get().is_null() {
            return;
        }

        JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"randomUUID".as_ptr(),
            Some(crypto_random_uuid),
            0,
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineFunction(
            cx,
            crypto_obj.handle(),
            c"getRandomValues".as_ptr(),
            Some(crypto_get_random_values),
            1,
            JSPROP_ENUMERATE as u32,
        );

        {
            rooted!(&in(cx) let subtle_obj = JS_NewPlainObject(cx));
            if !subtle_obj.get().is_null() {
                JS_DefineFunction(
                    cx,
                    subtle_obj.handle(),
                    c"digest".as_ptr(),
                    Some(crypto_subtle_digest),
                    2,
                    JSPROP_ENUMERATE as u32,
                );
                JS_DefineProperty3(
                    cx,
                    crypto_obj.handle(),
                    c"subtle".as_ptr(),
                    subtle_obj.handle(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        JS_DefineProperty3(
            cx,
            global,
            c"crypto".as_ptr(),
            crypto_obj.handle(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_random_uuid(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // Generate UUID v4 using BoringSSL CSPRNG (16 random bytes, then set version/variant bits)
    let mut bytes = [0u8; 16];
    bao_crypto::random::rand_bytes(&mut bytes).unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
    let uuid = format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    );
    let c_uuid = ZBox::from_bytes(uuid.as_bytes());
    let js_str = JS_NewStringCopyZ(_cx, c_uuid.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_get_random_values(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_val = *args.get(0).ptr;
    if !arr_val.is_object() {
        args.rval().set(arr_val);
        return true;
    }
    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let arr_root = arr_val.to_object());
    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        arr_root.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let len = if len_val.is_int32() {
        len_val.to_int32().max(0) as usize
    } else {
        0
    };

    let mut buf = vec![0u8; len];
    bao_crypto::random::rand_bytes(&mut buf).unwrap();
    for (i, &byte) in buf.iter().enumerate() {
        rooted!(&in(cx_ref) let byte_root = Int32Value(byte as i32));
        JS_SetElement(
            cx,
            arr_root.handle().into(),
            i as u32,
            byte_root.handle().into(),
        );
    }
    args.rval().set(arr_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn crypto_subtle_digest(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(
            cx,
            c"crypto.subtle.digest requires algorithm and data".as_ptr(),
        );
        return false;
    }

    let algo_val = *args.get(0).ptr;
    let algo = if algo_val.is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked(algo_val.to_string())).to_lowercase()
    } else {
        "sha-256".to_string()
    };

    let data_val = *args.get(1).ptr;
    let bytes = if data_val.is_object() {
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = data_val.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() {
            len_val.to_int32().max(0) as usize
        } else {
            0
        };
        let mut v = Vec::with_capacity(len);
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                obj_root.handle().into(),
                i as u32,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            v.push(if elem.is_int32() {
                elem.to_int32() as u8
            } else {
                0
            });
        }
        v
    } else if data_val.is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked(data_val.to_string())).into_bytes()
    } else {
        Vec::new()
    };

    let hash = match algo.as_str() {
        "sha-1" | "sha1" => {
            let mut h = bun_sha_hmac::SHA1::init();
            h.update(&bytes);
            let mut out = [0u8; bun_sha_hmac::SHA1::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha-256" | "sha256" => {
            let mut h = bun_sha_hmac::SHA256::init();
            h.update(&bytes);
            let mut out = [0u8; bun_sha_hmac::SHA256::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha-384" | "sha384" => {
            let mut h = bun_sha_hmac::SHA384::init();
            h.update(&bytes);
            let mut out = [0u8; bun_sha_hmac::SHA384::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        "sha-512" | "sha512" => {
            let mut h = bun_sha_hmac::SHA512::init();
            h.update(&bytes);
            let mut out = [0u8; bun_sha_hmac::SHA512::DIGEST];
            h.r#final(&mut out);
            out.to_vec()
        }
        _ => {
            let msg = format!("Unsupported algorithm: {}", algo);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let arr_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let arr_root = arr_obj);
    let lv = DoubleValue(hash.len() as f64);
    rooted!(&in(cx_ref) let lv_r = lv);
    JS_DefineProperty(
        cx,
        arr_root.handle().into(),
        c"length".as_ptr(),
        lv_r.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    for (i, &byte) in hash.iter().enumerate() {
        let v = Int32Value(byte as i32);
        rooted!(&in(cx_ref) let v_r = v);
        JS_DefineElement(
            cx,
            arr_root.handle().into(),
            i as u32,
            v_r.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr_obj));
    true
}

pub fn install_structured_clone(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx,
            global,
            c"structuredClone".as_ptr(),
            ::std::option::Option::Some(structured_clone_fn),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn structured_clone_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;

    // @trace REQ-ENG-005 [api:structuredClone transfer] — Node.js's
    // structuredClone(value, { transfer: [ab] }) DETACHES every ArrayBuffer in
    // the transfer list (their byteLength becomes 0 and any TypedArray view
    // throws on subsequent access). buffer.test.js "slice() on detached buffer
    // throws TypeError" and "subarray() on detached buffer throws TypeError"
    // drive this via structuredClone(ab, { transfer: [ab] }).
    //
    // We handle the transfer list BEFORE cloning so the source buffers are
    // detached even if clone is a no-op. For ArrayBuffer inputs we then
    // return a fresh ArrayBuffer copy of the original bytes.
    if argc >= 2 && val.is_object() {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let cx_ref =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref) let opts_root = opts.to_object());
            let mut transfer_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_root.handle().into(),
                c"transfer".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut transfer_val,
                },
            );
            if transfer_val.is_object() {
                rooted!(&in(cx_ref) let transfer_root = transfer_val.to_object());
                let mut length_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    transfer_root.handle().into(),
                    c"length".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut length_val,
                    },
                );
                let list_len: i64 = if length_val.is_int32() {
                    length_val.to_int32() as i64
                } else if length_val.is_double() {
                    length_val.to_double() as i64
                } else {
                    0
                };
                rooted!(&in(cx_ref) let val_obj_root = if val.is_object() { val.to_object() } else { ::std::ptr::null_mut() });
                // For each transfer-list item: read its current byteLength,
                // create a fresh ArrayBuffer of equal size (the "clone"), and
                // detach the source via SM's JS::DetachArrayBuffer. If the
                // item is the value being cloned, the clone becomes our
                // return value.
                let mut result_for_cloned: ::std::option::Option<*mut JSObject> = None;
                for i in 0..list_len {
                    let mut item = UndefinedValue();
                    JS_GetElement(
                        cx,
                        transfer_root.handle().into(),
                        i as u32,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut item,
                        },
                    );
                    if item.is_object() {
                        rooted!(&in(cx_ref) let item_root = item.to_object());
                        // Only ArrayBuffers are transferable per spec.
                        let is_ab =
                            mozjs_sys::jsapi::JS::IsArrayBufferObject(item_root.handle().get());
                        if is_ab {
                            // Read length + data so we can build a clone.
                            let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
                            let mut data_len: usize = 0;
                            let mut is_shared = false;
                            mozjs_sys::jsapi::JS::GetArrayBufferLengthAndData(
                                item_root.handle().get(),
                                &mut data_len,
                                &mut is_shared,
                                &mut data_ptr,
                            );
                            // Copy bytes out so they survive detachment.
                            let bytes_copy: Vec<u8> = if !data_ptr.is_null() && data_len > 0 {
                                ::std::slice::from_raw_parts(data_ptr, data_len).to_vec()
                            } else {
                                Vec::new()
                            };
                            // Detach source ArrayBuffer.
                            mozjs_sys::jsapi::JS::DetachArrayBuffer(cx, item_root.handle().into());
                            // If this item is the value being cloned, build
                            // a clone ArrayBuffer and use it as the return.
                            let is_top = if val_obj_root.get().is_null() {
                                false
                            } else {
                                val_obj_root.handle().get() == item_root.handle().get()
                            };
                            if is_top {
                                let clone =
                                    mozjs_sys::jsapi::JS::NewArrayBuffer(cx, bytes_copy.len());
                                if !clone.is_null() {
                                    if !bytes_copy.is_empty() {
                                        let mut clone_shared = false;
                                        let clone_data = mozjs_sys::jsapi::JS::GetArrayBufferData(
                                            clone,
                                            &mut clone_shared,
                                            ::std::ptr::null(),
                                        );
                                        if !clone_data.is_null() {
                                            ::std::ptr::copy_nonoverlapping(
                                                bytes_copy.as_ptr(),
                                                clone_data,
                                                bytes_copy.len(),
                                            );
                                        }
                                    }
                                    result_for_cloned = Some(clone);
                                }
                            }
                        }
                    }
                }
                // If we cloned the top-level value, return the clone directly.
                if let Some(clone) = result_for_cloned {
                    args.rval().set(ObjectValue(clone));
                    return true;
                }
            }
        }
    }

    let val = *args.get(0).ptr;

    if val.is_undefined()
        || val.is_null()
        || val.is_boolean()
        || val.is_int32()
        || val.is_double()
        || val.is_string()
    {
        args.rval().set(val);
        return true;
    }

    if val.is_object() {
        let cx_ref = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let obj_root = val.to_object());

        let mut ctor_name = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root.handle().into(),
            c"constructor".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctor_name,
            },
        );
        if ctor_name.is_object() {
            rooted!(&in(cx_ref) let ctor_root = ctor_name.to_object());
            let mut name_val = UndefinedValue();
            JS_GetProperty(
                cx,
                ctor_root.handle().into(),
                c"name".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut name_val,
                },
            );
            if name_val.is_string() {
                let name = crate::js_to_rust_string(cx, name_val);
                match name.as_str() {
                    "Date" => {
                        let mut time_val = UndefinedValue();
                        JS_GetProperty(
                            cx,
                            obj_root.handle().into(),
                            c"getTime".as_ptr(),
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut time_val,
                            },
                        );
                        if time_val.is_object() {
                            rooted!(&in(cx_ref) let gt_root = ObjectValue(time_val.to_object()));
                            let global = CurrentGlobalOrNull(cx);
                            if !global.is_null() {
                                rooted!(&in(cx_ref) let global_root = global);
                                let mut ms_rval = UndefinedValue();
                                JS_CallFunctionValue(
                                    cx,
                                    obj_root.handle().into(),
                                    gt_root.handle().into(),
                                    &HandleValueArray::empty(),
                                    MutableHandle::<Value> {
                                        _phantom_0: ::std::marker::PhantomData,
                                        ptr: &mut ms_rval,
                                    },
                                );
                                let ms = if ms_rval.is_double() {
                                    ms_rval.to_double()
                                } else if ms_rval.is_int32() {
                                    ms_rval.to_int32() as f64
                                } else {
                                    0.0
                                };
                                let src = format!("new Date({})", ms);
                                let mut eval_rval = UndefinedValue();
                                let eval_opts =
                                    mozjs::glue::NewCompileOptions(cx, c"clone".as_ptr(), 1);
                                if !eval_opts.is_null() {
                                    let mut src_text =
                                        mozjs::rust::transform_str_to_source_text(&src);
                                    JS::Evaluate2(
                                        cx,
                                        eval_opts,
                                        &mut src_text,
                                        MutableHandle::<Value> {
                                            _phantom_0: ::std::marker::PhantomData,
                                            ptr: &mut eval_rval,
                                        },
                                    );
                                    libc::free(eval_opts as *mut _);
                                }
                                args.rval().set(eval_rval);
                                return true;
                            }
                        }
                    }
                    "RegExp" => {
                        let mut source_val = UndefinedValue();
                        JS_GetProperty(
                            cx,
                            obj_root.handle().into(),
                            c"source".as_ptr(),
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut source_val,
                            },
                        );
                        let mut flags_val = UndefinedValue();
                        JS_GetProperty(
                            cx,
                            obj_root.handle().into(),
                            c"flags".as_ptr(),
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut flags_val,
                            },
                        );
                        let source = if source_val.is_string() {
                            crate::js_to_rust_string(cx, source_val)
                        } else {
                            "".to_string()
                        };
                        let flags = if flags_val.is_string() {
                            crate::js_to_rust_string(cx, flags_val)
                        } else {
                            "".to_string()
                        };
                        let src = format!(
                            "new RegExp(\"{}\", \"{}\")",
                            source.replace('\\', "\\\\").replace('"', "\\\""),
                            flags
                        );
                        let mut eval_rval = UndefinedValue();
                        let eval_opts = mozjs::glue::NewCompileOptions(cx, c"clone".as_ptr(), 1);
                        if !eval_opts.is_null() {
                            let mut src_text = mozjs::rust::transform_str_to_source_text(&src);
                            JS::Evaluate2(
                                cx,
                                eval_opts,
                                &mut src_text,
                                MutableHandle::<Value> {
                                    _phantom_0: ::std::marker::PhantomData,
                                    ptr: &mut eval_rval,
                                },
                            );
                            libc::free(eval_opts as *mut _);
                        }
                        args.rval().set(eval_rval);
                        return true;
                    }
                    _ => {}
                }
            }
        }

        let mut json_rval = UndefinedValue();
        let json_rval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut json_rval,
        };
        let json_src = mozjs::rust::transform_str_to_source_text(
            "(function(o){try{return JSON.parse(JSON.stringify(o))}catch(e){return o}})",
        );
        let json_opts = mozjs::glue::NewCompileOptions(cx, c"json_clone".as_ptr(), 1);
        if !json_opts.is_null() {
            let mut json_fn_val = UndefinedValue();
            JS::Evaluate2(
                cx,
                json_opts,
                &mut ::std::mem::MaybeUninit::new(json_src).assume_init(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut json_fn_val,
                },
            );
            libc::free(json_opts as *mut _);
            if json_fn_val.is_object() {
                let global = CurrentGlobalOrNull(cx);
                if !global.is_null() {
                    rooted!(&in(cx_ref) let global_root = global);
                    rooted!(&in(cx_ref) let fn_root = ObjectValue(json_fn_val.to_object()));
                    rooted!(&in(cx_ref) let obj_val_rooted = ObjectValue(obj_root.get()));
                    let obj_arg = HandleValueArray {
                        length_: 1,
                        elements_: &obj_val_rooted.get() as *const Value,
                    };
                    JS_CallFunctionValue(
                        cx,
                        global_root.handle().into(),
                        fn_root.handle().into(),
                        &obj_arg,
                        json_rval_h,
                    );
                    args.rval().set(json_rval);
                    return true;
                }
            }
        }
    }

    args.rval().set(val);
    true
}

pub fn install_assert_strict(cx: &mut mozjs::context::JSContext) {
    crate::require::cache_assert_strict(cx);
}

pub fn install_web_api_constructors(
    cx: &mut mozjs::context::JSContext,
    _global: mozjs::rust::Handle<*mut JSObject>,
) {
    let src = r#"
var _g = globalThis;

// AbortController + AbortSignal
if (typeof _g.AbortController === 'undefined') {
  _g.AbortSignal = function AbortSignal() {
    this.aborted = false;
    this.reason = undefined;
    this._listeners = [];
  };
  _g.AbortSignal.prototype.addEventListener = function(type, fn) {
    if (type === 'abort') this._listeners.push(fn);
  };
  _g.AbortSignal.prototype.removeEventListener = function(type, fn) {
    if (type === 'abort') {
      var idx = this._listeners.indexOf(fn);
      if (idx !== -1) this._listeners.splice(idx, 1);
    }
  };
  _g.AbortController = function AbortController() {
    var signal = new _g.AbortSignal();
    this.signal = signal;
    this.abort = function(reason) {
      signal.aborted = true;
      signal.reason = reason || new Error('The operation was aborted');
      for (var i = 0; i < signal._listeners.length; i++) {
        signal._listeners[i]({ type: 'abort', target: signal });
      }
    };
  };
}

// @trace REQ-ENG-005 [entity:Blob] — Web Blob (size/type/arrayBuffer/text/slice/stream).
// Backed by Uint8Array chunks so non-ASCII UTF-8 round-trips correctly.
// A process-global registry (_bao_blob_registry) keeps Blob references alive
// for URL.createObjectURL/resolveObjectURL/revokeObjectURL (Node.js buffer.resolveObjectURL).
_g._bao_blob_registry = (typeof _g._bao_blob_registry !== 'undefined') ? _g._bao_blob_registry : new Map();
_g._bao_blob_counter = (typeof _g._bao_blob_counter !== 'undefined') ? _g._bao_blob_counter : 0;

function _bao_blob_collect_parts(parts) {
  var chunks = [];
  var size = 0;
  parts = parts || [];
  for (var i = 0; i < parts.length; i++) {
    var p = parts[i];
    if (p == null) continue;
    var bytes;
    if (typeof p === 'string') {
      // Encode as UTF-8 to match the WHATWG Blob spec.
      bytes = new TextEncoder().encode(p);
    } else if (p instanceof ArrayBuffer) {
      bytes = new Uint8Array(p.slice(0));
    } else if (ArrayBuffer.isView(p)) {
      // Typed array view — copy its byte slice (handles byteOffset/byteLength).
      var view = new Uint8Array(p.buffer, p.byteOffset, p.byteLength);
      bytes = new Uint8Array(view.length);
      bytes.set(view);
    } else if (p && typeof p === 'object' && typeof p.size === 'number' && typeof p.arrayBuffer === 'function') {
      // Blob-ish — defer (synchronous ctor cannot await). Snapshot eagerly to
      // keep _parts simple: this is rare and matches Node's eager concatenation
      // for the common Blob(Blob[]) case.
      var ab = p.arrayBuffer();
      // For simplicity assume resolved Blob; if it returned a Promise, encode
      // empty (Bun's runtime path covers the async case for fetch).
      if (typeof ab.then !== 'function') {
        var v = new Uint8Array(ab);
        bytes = new Uint8Array(v.length);
        bytes.set(v);
      } else {
        bytes = new Uint8Array(0);
      }
    } else {
      bytes = new Uint8Array(0);
    }
    chunks.push(bytes);
    size += bytes.length;
  }
  return { chunks: chunks, size: size };
}

function _bao_blob_concat(self) {
  var total = self.size;
  var out = new Uint8Array(total);
  var offset = 0;
  for (var i = 0; i < self._chunks.length; i++) {
    out.set(self._chunks[i], offset);
    offset += self._chunks[i].length;
  }
  return out;
}

if (typeof _g.Blob === 'undefined') {
  _g.Blob = function Blob(parts, options) {
    if (!(this instanceof _g.Blob)) return new _g.Blob(parts, options);
    options = options || {};
    var collected = _bao_blob_collect_parts(parts);
    this._chunks = collected.chunks;
    this.size = collected.size;
    // Normalise type per WHATWG: lowercased ASCII; ignore invalid.
    var t = (typeof options.type === 'string') ? options.type : '';
    this.type = t.toLowerCase();
  };
  _g.Blob.prototype.arrayBuffer = function() {
    return Promise.resolve(_bao_blob_concat(this).buffer);
  };
  _g.Blob.prototype.text = function() {
    var bytes = _bao_blob_concat(this);
    // TextDecoder default is UTF-8.
    return Promise.resolve(new TextDecoder().decode(bytes));
  };
  _g.Blob.prototype.slice = function(start, end, contentType) {
    var size = this.size;
    var relStart = (start === undefined) ? 0 : (start | 0);
    if (relStart < 0) relStart = Math.max(size + relStart, 0);
    else relStart = Math.min(relStart, size);
    var relEnd = (end === undefined) ? size : (end | 0);
    if (relEnd < 0) relEnd = Math.max(size + relEnd, 0);
    else relEnd = Math.min(relEnd, size);
    var span = Math.max(relEnd - relStart, 0);
    var out = new Uint8Array(span);
    var outOff = 0;
    var cur = 0;
    for (var j = 0; j < this._chunks.length && outOff < span; j++) {
      var ch2 = this._chunks[j];
      var next = cur + ch2.length;
      if (next <= relStart) { cur = next; continue; }
      var ls = Math.max(relStart - cur, 0);
      var le = Math.min(relEnd - cur, ch2.length);
      var taken2 = ch2.subarray(ls, le);
      out.set(taken2, outOff);
      outOff += taken2.length;
      cur = next;
    }
    var b = new _g.Blob([], { type: contentType });
    b._chunks = [out];
    b.size = span;
    return b;
  };
  _g.Blob.prototype.stream = function() {
    var bytes = _bao_blob_concat(this);
    var rs = new ReadableStream({
      start: function(controller) { controller.enqueue(bytes); controller.close(); }
    });
    return rs;
  };
}

// @trace REQ-ENG-005 [api:URL.createObjectURL/revokeObjectURL] — Blob URL
// registry. URL.createObjectURL(blob) returns "blob:<origin>/<uuid>" and
// stores the Blob; resolveObjectURL(url) / fetch("blob:...") can retrieve it.
// URL is defined later (node_url.rs) as JSPROP_PERMANENT — use defineProperty
// fallback so static methods land on the constructor even if install order
// changes.
function _bao_install_blob_url_statics() {
  if (typeof _g.URL !== 'function') return false;
  if (_g.URL.hasOwnProperty('createObjectURL')) return true;
  var desc = {
    createObjectURL: function createObjectURL(blob) {
      if (blob == null || typeof blob !== 'object' || typeof blob.size !== 'number' || typeof blob.arrayBuffer !== 'function') {
        throw new TypeError("Failed to execute 'createObjectURL' on 'URL': parameter 1 is not of type 'Blob'.");
      }
      _g._bao_blob_counter = (_g._bao_blob_counter | 0) + 1;
      // WHATWG: "blob:" + origin + "/" + UUID. Use a stable counter under a
      // null origin (servo/browser contexts use real origins; CLI uses null).
      var origin = 'null';
      var id = 'blob:' + origin + '/' + Date.now().toString(36) + '-' + _g._bao_blob_counter.toString(36);
      _g._bao_blob_registry.set(id, blob);
      return id;
    },
    revokeObjectURL: function revokeObjectURL(id) {
      if (typeof id !== 'string') return;
      _g._bao_blob_registry.delete(id);
    }
  };
  try {
    Object.defineProperty(_g.URL, 'createObjectURL', { value: desc.createObjectURL, writable: true, configurable: true, enumerable: false });
    Object.defineProperty(_g.URL, 'revokeObjectURL', { value: desc.revokeObjectURL, writable: true, configurable: true, enumerable: false });
  } catch (e) { return false; }
  // Mirror on globalThis for `URL.createObjectURL` and `URL.revokeObjectURL`
  // both already point to the same constructor — no second global needed.
  return true;
}
_bao_install_blob_url_statics();
// Re-attempt after node_url::install has run (URL constructor is registered
// later than web_api_constructors). node_buffer::install invokes this hook
// again — see _bao_run_blob_url_statics() below.
_g._bao_run_blob_url_statics = _bao_install_blob_url_statics;

// File extends Blob
if (typeof _g.File === 'undefined') {
  _g.File = function File(parts, name, options) {
    _g.Blob.call(this, parts, options);
    this.name = name || '';
    this.lastModified = (options && options.lastModified) || Date.now();
  };
  _g.File.prototype = Object.create(_g.Blob.prototype);
  _g.File.prototype.constructor = _g.File;
}

// FormData
if (typeof _g.FormData === 'undefined') {
  _g.FormData = function FormData() {
    this._data = [];
  };
  _g.FormData.prototype.append = function(name, value, filename) {
    this._data.push({ name: name, value: value, filename: filename });
  };
  _g.FormData.prototype.get = function(name) {
    for (var i = 0; i < this._data.length; i++) {
      if (this._data[i].name === name) return this._data[i].value;
    }
    return null;
  };
  _g.FormData.prototype.getAll = function(name) {
    var result = [];
    for (var i = 0; i < this._data.length; i++) {
      if (this._data[i].name === name) result.push(this._data[i].value);
    }
    return result;
  };
  _g.FormData.prototype.has = function(name) {
    for (var i = 0; i < this._data.length; i++) {
      if (this._data[i].name === name) return true;
    }
    return false;
  };
  _g.FormData.prototype.delete = function(name) {
    this._data = this._data.filter(function(entry) { return entry.name !== name; });
  };
  _g.FormData.prototype.set = function(name, value, filename) {
    var found = false;
    for (var i = 0; i < this._data.length; i++) {
      if (this._data[i].name === name) {
        if (!found) { this._data[i] = { name: name, value: value, filename: filename }; found = true; }
        else { this._data.splice(i, 1); i--; }
      }
    }
    if (!found) this._data.push({ name: name, value: value, filename: filename });
  };
}

// @trace REQ-ENG-006 [api:DOMException] — Web/Node.js global DOMException.
// Constructor: new DOMException(message?, nameOrOptions?) — name defaults
// to "Error", code defaults to 0 (or the matching numeric constant for a
// known name). Inherits from Error so `instanceof Error` is true.
if (typeof _g.DOMException === 'undefined') {
  var _domCodeMap = {
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
    InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8,
    NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11,
    SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, ValidationError: 16, TypeMismatchError: 17,
    SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21,
    QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24,
    DataCloneError: 25,
  };
  function DOMException(message, options) {
    // @trace REQ-ENG-006 — DOMException constructor.
    //
    // Spec: returns an instance whose [[Prototype]] is DOMException.prototype
    // (so `new DOMException(...) instanceof DOMException` is true). The
    // previous implementation discarded `this` when `Error.call(this)`
    // happened to return a fresh Error object, leaving the result with
    // Error.prototype as its [[Prototype]] instead of DOMException.prototype.
    //
    // Fix: install Error's own .stack/.message on `this` (which carries the
    // correct prototype chain), but never reassign `this` to the Error
    // object returned by Error.call.
    if (!(this instanceof DOMException)) {
      // Allow DOMException() without `new` to behave like `new DOMException()`.
      var obj = Object.create(DOMException.prototype);
      return DOMException.apply(obj, arguments);
    }
    // Use Error's machinery for the .stack backtrace, but keep `this`.
    try {
      Error.call(this, message);
    } catch (_e) { /* Error.call may throw on some subclasses; ignore */ }
    this.message = (message === undefined) ? '' : String(message);
    var name = 'Error';
    var cause;
    if (typeof options === 'string') {
      name = options;
    } else if (options && typeof options === 'object') {
      if (options.name !== undefined) name = String(options.name);
      if ('cause' in options) cause = options.cause;
    }
    this.name = name;
    this.code = _domCodeMap[name] || 0;
    if (cause !== undefined) this.cause = cause;
    return this;
  }
  DOMException.prototype = Object.create(Error.prototype);
  DOMException.prototype.constructor = DOMException;
  DOMException.prototype.name = 'Error';
  // Standard error-code constants (static properties).
  DOMException.INDEX_SIZE_ERR = 1;
  DOMException.DOMSTRING_SIZE_ERR = 2;
  DOMException.HIERARCHY_REQUEST_ERR = 3;
  DOMException.WRONG_DOCUMENT_ERR = 4;
  DOMException.INVALID_CHARACTER_ERR = 5;
  DOMException.NO_DATA_ALLOWED_ERR = 6;
  DOMException.NO_MODIFICATION_ALLOWED_ERR = 7;
  DOMException.NOT_FOUND_ERR = 8;
  DOMException.NOT_SUPPORTED_ERR = 9;
  DOMException.INUSE_ATTRIBUTE_ERR = 10;
  DOMException.INVALID_STATE_ERR = 11;
  DOMException.SYNTAX_ERR = 12;
  DOMException.INVALID_MODIFICATION_ERR = 13;
  DOMException.NAMESPACE_ERR = 14;
  DOMException.INVALID_ACCESS_ERR = 15;
  DOMException.VALIDATION_ERR = 16;
  DOMException.TYPE_MISMATCH_ERR = 17;
  DOMException.SECURITY_ERR = 18;
  DOMException.NETWORK_ERR = 19;
  DOMException.ABORT_ERR = 20;
  DOMException.URL_MISMATCH_ERR = 21;
  DOMException.QUOTA_EXCEEDED_ERR = 22;
  DOMException.TIMEOUT_ERR = 23;
  DOMException.INVALID_NODE_TYPE_ERR = 24;
  DOMException.DATA_CLONE_ERR = 25;
  _g.DOMException = DOMException;
}

// DOMParser — lightweight HTML parser for CLI mode.
// Browser mode: servo provides window.DOMParser natively (no override needed).
// CLI mode: parses HTML via native __dom_parse(html, mimeType) if available,
// otherwise falls back to a minimal document object with querySelector etc.
if (typeof _g.DOMParser === 'undefined') {
  function DOMParser() {}
  DOMParser.prototype.parseFromString = function(html, mimeType) {
    mimeType = mimeType || 'text/html';
    // Try native Rust binding (html5ever-based) if available.
    if (typeof __dom_parse === 'function') {
      var result = __dom_parse(html, mimeType);
      if (result != null) return result;
    }
    // Fallback: return a minimal document object.
    // Supports text/html and application/xhtml+xml.
    if (mimeType === 'text/xml' || mimeType === 'application/xml') {
      // XML parsing not supported in fallback — return empty document.
      return {
        nodeType: 9,
        documentElement: null,
        body: { innerHTML: '', children: [], childNodes: [] },
        head: { children: [], childNodes: [] },
        querySelector: function() { return null; },
        querySelectorAll: function() { return []; },
        getElementsByTagName: function() { return []; },
        getElementById: function() { return null; },
        getElementsByClassName: function() { return []; },
        childNodes: [],
        createElement: function(tag) {
          return {
            nodeType: 1, tagName: tag.toUpperCase(), children: [], childNodes: [],
            attributes: {}, innerHTML: '', textContent: '',
            appendChild: function(c) { this.children.push(c); this.childNodes.push(c); return c; },
            setAttribute: function(n, v) { this.attributes[n] = v; },
            getAttribute: function(n) { return this.attributes.hasOwnProperty(n) ? this.attributes[n] : null; },
            querySelector: function() { return null; },
            querySelectorAll: function() { return []; }
          };
        },
        createTextNode: function(t) { return { nodeType: 3, textContent: t }; }
      };
    }
    // HTML fallback: parse basic structure with regex-based extraction.
    var titleMatch = (typeof html === 'string') ? html.match(/<title[^>]*>([\s\S]*?)<\/title>/i) : null;
    var title = titleMatch ? titleMatch[1] : '';
    var bodyMatch = (typeof html === 'string') ? html.match(/<body[^>]*>([\s\S]*?)<\/body>/i) : null;
    var bodyHTML = bodyMatch ? bodyMatch[1] : (typeof html === 'string' ? html : '');
    var headMatch = (typeof html === 'string') ? html.match(/<head[^>]*>([\s\S]*?)<\/head>/i) : null;
    var headHTML = headMatch ? headMatch[1] : '';
    function _makeElement(tag, inner) {
      return {
        nodeType: 1, tagName: tag.toUpperCase(), children: [], childNodes: [],
        attributes: {}, innerHTML: inner || '', textContent: (inner || '').replace(/<[^>]*>/g, ''),
        appendChild: function(c) { this.children.push(c); this.childNodes.push(c); return c; },
        setAttribute: function(n, v) { this.attributes[n] = v; },
        getAttribute: function(n) { return this.attributes.hasOwnProperty(n) ? this.attributes[n] : null; },
        querySelector: function() { return null; },
        querySelectorAll: function() { return []; }
      };
    }
    var bodyEl = _makeElement('body', bodyHTML);
    var headEl = _makeElement('head', headHTML);
    if (title) {
      var titleEl = _makeElement('title', title);
      titleEl.textContent = title;
      headEl.children.push(titleEl);
      headEl.childNodes.push(titleEl);
    }
    var htmlEl = _makeElement('html', '');
    htmlEl.children = [headEl, bodyEl];
    htmlEl.childNodes = [headEl, bodyEl];
    return {
      nodeType: 9,
      documentElement: htmlEl,
      body: bodyEl,
      head: headEl,
      title: title,
      querySelector: function(sel) {
        // Minimal: support 'body', 'head', 'title', 'html' tag selectors.
        var s = (sel || '').toLowerCase();
        if (s === 'html') return htmlEl;
        if (s === 'head') return headEl;
        if (s === 'body') return bodyEl;
        if (s === 'title') { return title ? headEl.querySelector('title') || null : null; }
        return null;
      },
      querySelectorAll: function(sel) {
        var r = this.querySelector(sel);
        return r ? [r] : [];
      },
      getElementsByTagName: function(tag) {
        var t = (tag || '').toUpperCase();
        if (t === 'HTML') return [htmlEl];
        if (t === 'HEAD') return [headEl];
        if (t === 'BODY') return [bodyEl];
        if (t === 'TITLE' && title) return headEl.children.filter(function(c) { return c.tagName === 'TITLE'; });
        return [];
      },
      getElementById: function() { return null; },
      getElementsByClassName: function() { return []; },
      childNodes: [htmlEl],
      createElement: function(tag) { return _makeElement(tag, ''); },
      createTextNode: function(t) { return { nodeType: 3, textContent: t }; }
    };
  };
  _g.DOMParser = DOMParser;
}

// History API — session history management.
// Browser mode: servo provides window.history natively (no override needed).
// CLI mode: implements pushState/replaceState/back/forward/go with in-memory stack.
if (typeof _g.History === 'undefined') {
  function History() {
    this._states = [null];
    this._urls = [(typeof location !== 'undefined' && location.href) ? location.href : ''];
    this._index = 0;
  }
  Object.defineProperty(History.prototype, 'length', {
    get: function() { return this._states.length; },
    configurable: true, enumerable: true
  });
  Object.defineProperty(History.prototype, 'state', {
    get: function() { return this._states[this._index]; },
    configurable: true, enumerable: true
  });
  Object.defineProperty(History.prototype, 'scrollRestoration', {
    get: function() { return 'auto'; },
    set: function() {},
    configurable: true, enumerable: true
  });
  History.prototype.pushState = function(state, title, url) {
    this._states = this._states.slice(0, this._index + 1);
    this._urls = this._urls.slice(0, this._index + 1);
    this._states.push(state);
    this._urls.push(url || '');
    this._index = this._states.length - 1;
  };
  History.prototype.replaceState = function(state, title, url) {
    this._states[this._index] = state;
    if (url) this._urls[this._index] = url;
  };
  History.prototype.back = function() {
    if (this._index > 0) { this._index--; this._dispatchPopState(); }
  };
  History.prototype.forward = function() {
    if (this._index < this._states.length - 1) { this._index++; this._dispatchPopState(); }
  };
  History.prototype.go = function(delta) {
    var i = this._index + (delta || 0);
    if (i >= 0 && i < this._states.length) { this._index = i; this._dispatchPopState(); }
  };
  History.prototype._dispatchPopState = function() {
    if (typeof this.dispatchEvent === 'function') {
      var PopStateEvent = (typeof _g.PopStateEvent !== 'undefined') ? _g.PopStateEvent : Event;
      this.dispatchEvent(new PopStateEvent('popstate', { state: this.state }));
    } else if (typeof _g.dispatchEvent === 'function') {
      var PopStateEvent2 = (typeof _g.PopStateEvent !== 'undefined') ? _g.PopStateEvent : Event;
      _g.dispatchEvent(new PopStateEvent2('popstate', { state: this.state }));
    }
  };
  _g.History = History;
}
if (typeof _g.history === 'undefined') {
  _g.history = new _g.History();
}

// PopStateEvent — required by History._dispatchPopState.
if (typeof _g.PopStateEvent === 'undefined') {
  function PopStateEvent(type, options) {
    this.type = type;
    this.state = (options && options.state !== undefined) ? options.state : null;
    this.bubbles = !!(options && options.bubbles);
    this.cancelable = !!(options && options.cancelable);
  }
  PopStateEvent.prototype = Object.create(Event.prototype);
  PopStateEvent.prototype.constructor = PopStateEvent;
  _g.PopStateEvent = PopStateEvent;
}
"#;
    unsafe {
        let raw = cx.raw_cx();
        let mut rval = UndefinedValue();
        let opts = mozjs::glue::NewCompileOptions(raw, c"web_api_constructors".as_ptr(), 1);
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

/// Install __filename / __dirname on a target object (REQ-SEC-002 parameter injection).
///
/// Same as `install_file_globals_from_cache` but attaches properties to `target`
/// instead of `global`. Used by `create_node_api_scope_values`.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `target` is a
/// valid handle to a JSObject.
unsafe fn install_file_globals_on_target(
    cx: &mut mozjs::context::JSContext,
    target: mozjs::rust::Handle<*mut JSObject>,
) {
    let (filename, dirname) = FILE_GLOBALS.with(|f| f.borrow().clone());
    let raw = cx.raw_cx();
    if let Some(fn_str) = filename {
        let c_fn = ZBox::from_bytes(fn_str.as_bytes());
        let js_str = JS_NewStringCopyZ(raw, c_fn.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx) let v = StringValue(&*js_str));
            JS_DefineProperty(
                raw,
                target.into(),
                c"__filename".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    if let Some(dir_str) = dirname {
        let c_dir = ZBox::from_bytes(dir_str.as_bytes());
        let js_str = JS_NewStringCopyZ(raw, c_dir.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx) let v = StringValue(&*js_str));
            JS_DefineProperty(
                raw,
                target.into(),
                c"__dirname".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
}

/// Create Node API scope values for privileged evaluate_js (REQ-SEC-002).
///
/// Creates a temporary scope object on `global` with a randomized name
/// (e.g., `__bao_7f3a9c2e`) and puts all Node API values into it.
/// The IIFE wrapper in wrap_privileged_script then:
/// 1. Extracts the scope object: `var __scope = globalThis[scopeName]`
/// 2. Deletes the scope: `delete globalThis[scopeName]`
/// 3. Deletes global Buffer: `delete globalThis.Buffer`
/// 4. Passes scope values as function parameters to the user script
///
/// The scope name is randomized per-call to prevent adversarial enumeration.
/// `__bao_setEnv`/`__bao_delEnv` are installed on the scope object (not global)
/// and passed into the process.env Proxy as factory parameters, eliminating
/// them from the global surface entirely.
///
/// # Safety
///
/// Caller must ensure `cx` is a valid JSContext pointer and `global` is a
/// valid handle to the global object. This function is called from servo's
/// `register_script_thread_callback`, which runs on the script thread.
pub unsafe fn create_node_api_scope_values(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
    scope_name: &str,
) {
    // Step 1: Create scope object
    rooted!(&in(cx) let scope_obj = JS_NewPlainObject(cx));
    if scope_obj.get().is_null() {
        return;
    }

    // Step 2: Install all Node API values on the scope object
    // (not on global — they'll be passed as IIFE parameters)
    crate::bun_api::install_bun_on_target(cx, scope_obj.handle());
    crate::bun_api::install_process_on_target(cx, scope_obj.handle(), global);
    crate::require::install_require_on_target(cx, scope_obj.handle());
    install_module_on_target(cx, scope_obj.handle(), global);
    install_file_globals_on_target(cx, scope_obj.handle());

    // Buffer is special: the prototype JS eval needs Buffer on global
    // to work correctly. So we install Buffer globally first, then copy
    // the reference into the scope. The IIFE wrapper will delete
    // globalThis.Buffer after extracting the scope.
    install_buffer_global(cx, global);

    // Copy the Buffer reference from global into scope
    {
        let mut buffer_val = UndefinedValue();
        let buffer_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut buffer_val,
        };
        JS_GetProperty(cx.raw_cx(), global.into(), c"Buffer".as_ptr(), buffer_h);
        if buffer_val.is_object() {
            rooted!(&in(cx) let buffer_obj = buffer_val.to_object());
            JS_DefineProperty3(
                cx,
                scope_obj.handle(),
                c"Buffer".as_ptr(),
                buffer_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Step 3: Attach scope object to global with the randomized name.
    // flags=0 means: non-enumerable, configurable, writable (SpiderMonkey defaults).
    // The randomized name prevents adversarial enumeration — page JS cannot
    // guess `__bao_7f3a9c2e` since it's generated after page JS runs.
    let scope_name_c = ZBox::from_bytes(scope_name.as_bytes());
    JS_DefineProperty3(
        cx,
        global,
        scope_name_c.as_ptr(),
        scope_obj.handle(),
        0u32, // non-enumerable, configurable (default), writable (default)
    );
}

// ── Unit tests for globals pure Rust data/logic ───────────────────────
// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_globals_default_empty() {
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert!(fg.0.is_none());
            assert!(fg.1.is_none());
        });
    }

    #[test]
    fn set_file_globals_updates_main() {
        set_file_globals(Some("/app/index.js".to_string()), Some("/app".to_string()));
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert_eq!(fg.0.as_deref(), Some("/app/index.js"));
            assert_eq!(fg.1.as_deref(), Some("/app"));
        });
        set_file_globals(None, None);
    }

    #[test]
    fn set_file_globals_different_paths() {
        set_file_globals(
            Some("/home/user/project/src/main.ts".to_string()),
            Some("/home/user/project/src".to_string()),
        );
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert!(fg.0.as_ref().unwrap().contains("main.ts"));
            assert!(fg.1.as_ref().unwrap().contains("src"));
        });
        set_file_globals(None, None);
    }

    #[test]
    fn set_file_globals_idempotent() {
        set_file_globals(Some("/a/b.js".to_string()), Some("/a".to_string()));
        set_file_globals(Some("/a/b.js".to_string()), Some("/a".to_string()));
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert_eq!(fg.0.as_deref(), Some("/a/b.js"));
        });
        set_file_globals(None, None);
    }

    #[test]
    fn file_globals_path_with_spaces() {
        set_file_globals(
            Some("/path with spaces/app.js".to_string()),
            Some("/path with spaces".to_string()),
        );
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert_eq!(fg.0.as_deref(), Some("/path with spaces/app.js"));
        });
        set_file_globals(None, None);
    }

    #[test]
    fn file_globals_unicode_path() {
        set_file_globals(
            Some("/用户/项目/app.js".to_string()),
            Some("/用户/项目".to_string()),
        );
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert_eq!(fg.0.as_deref(), Some("/用户/项目/app.js"));
        });
        set_file_globals(None, None);
    }

    #[test]
    fn file_globals_partial_set_filename_only() {
        set_file_globals(Some("/only/file.js".to_string()), None);
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert_eq!(fg.0.as_deref(), Some("/only/file.js"));
            assert!(fg.1.is_none());
        });
        set_file_globals(None, None);
    }

    #[test]
    fn file_globals_partial_set_dirname_only() {
        set_file_globals(None, Some("/only/dir".to_string()));
        FILE_GLOBALS.with(|fg| {
            let fg = fg.borrow();
            assert!(fg.0.is_none());
            assert_eq!(fg.1.as_deref(), Some("/only/dir"));
        });
        set_file_globals(None, None);
    }

    // ── REQ-SEC-003: API split structural verification ────────────────────
    // @trace TEST-SEC-003 [req:REQ-SEC-001,REQ-SEC-002,REQ-SEC-003] [level:unit]
    //
    // These tests verify the CODE STRUCTURE guarantees that Node APIs are
    // not on the page global. Runtime verification requires servo and is
    // tested in bao_browser/tests/security_sandbox_tests.rs.

    /// Verify install_web_apis is a separate function from install_node_apis.
    /// REQ-SEC-003: The two must be distinct so browser pages get Web APIs only.
    #[test]
    fn web_apis_and_node_apis_are_separate_functions() {
        let _ = install_web_apis
            as unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>);
        let _ = install_node_apis
            as unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>);
        let _ = install_all
            as unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>);
    }

    /// Verify install_all is a distinct function (not aliased to either sub-function).
    /// REQ-SEC-003: install_all must call BOTH functions for CLI mode.
    #[test]
    fn install_all_is_distinct_from_sub_functions() {
        let all_ptr = install_all as *const () as usize;
        let web_ptr = install_web_apis as *const () as usize;
        let node_ptr = install_node_apis as *const () as usize;
        assert_ne!(
            all_ptr, web_ptr,
            "install_all must not be aliased to install_web_apis"
        );
        assert_ne!(
            all_ptr, node_ptr,
            "install_all must not be aliased to install_node_apis"
        );
    }

    /// Verify install_web_apis does NOT call any Node API installer.
    /// install_node_apis does NOT call any Web-only API installer.
    /// REQ-SEC-003: Static source analysis to prevent accidental re-merging.
    #[test]
    fn web_apis_excludes_node_api_installers() {
        let source = include_str!("globals.rs");

        // Find install_web_apis function body (between install_web_apis and install_node_apis)
        let web_start = source
            .find("pub unsafe fn install_web_apis")
            .expect("install_web_apis function not found in source");
        let web_end = source[web_start..]
            .find("pub unsafe fn install_node_apis")
            .expect("install_node_apis function not found after install_web_apis");
        let web_body = &source[web_start..web_start + web_end];

        // Node API installers that must NOT appear in install_web_apis
        let node_installers = [
            "bun_api::install_bun_global",
            "bun_api::install_process_global",
            "install_buffer_global",
            "require::install_require",
            "install_module_global",
            "node_events::install",
            "node_fs::install",
            "node_crypto::install",
            "node_http::install",
            "node_http2::install",
            "node_https::install",
            "node_os::install",
            "node_child_process::install",
            "node_stream::install",
            "node_zlib::install",
            "node_net::install",
            "node_dns::install",
            "node_buffer::install",
            "node_tty::install",
            "node_vm::install",
            "node_module::install",
            "node_querystring::install",
            "node_perf_hooks::install",
            "node_timers_module::install",
            "node_readline::install",
            "node_tls::install",
            "install_assert_strict",
            "install_file_globals_from_cache",
            "bun_test::install_bun_test",
        ];

        for installer in &node_installers {
            assert!(
                !web_body.contains(installer),
                "REQ-SEC-003 REGRESSION: install_web_apis contains Node API installer: {}",
                installer
            );
        }

        // Web API installers that MUST appear in install_web_apis
        let web_installers = [
            "fetch_api::install_fetch_global",
            "fetch_api::install_response_constructor",
            "fetch_api::install_headers_constructor",
            "fetch_api::install_request_constructor",
            "timers::install_timer_globals",
            "web_api::install_performance",
            "web_api::install_websocket_constructor",
            "install_crypto_global",
            "web_api::install_web_encodings",
            "web_api::install_atob_btoa",
            "web_api::install_queue_microtask",
            "install_structured_clone",
            "install_web_api_constructors",
        ];

        for installer in &web_installers {
            assert!(
                web_body.contains(installer),
                "REQ-SEC-003 REGRESSION: install_web_apis missing web API installer: {}",
                installer
            );
        }
    }

    /// MAX_BUFFER_SIZE matches the typed-array ceiling (4 GiB - 1). Above this
    /// Buffer.allocUnsafe / Buffer.concat must throw RangeError rather than
    /// attempting to allocate (which would OOM or hang on the per-byte property
    /// storage used by the bao Buffer implementation).
    // @trace REQ-ENG-005 [entity:Buffer]
    #[test]
    fn max_buffer_size_matches_typed_array_ceiling() {
        assert_eq!(MAX_BUFFER_SIZE, (1usize << 32) - 1);
        // Sanity: 64 MiB fits (the bun buffer-concat test allocates one).
        assert!(1024 * 1024 * 64 < MAX_BUFFER_SIZE);
        // 64 GiB exceeds the ceiling (the bun buffer-concat test does this via
        // concat-with-1024-elements).
        assert!((1024usize * 1024 * 1024 * 64) > MAX_BUFFER_SIZE);
    }
}
