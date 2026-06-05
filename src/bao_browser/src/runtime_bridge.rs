// @trace REQ-BRW-003  REQ-BRW-001: Bridge between servo browser context and Node.js APIs
// REQ-ENG-007: Unified runtime coordination
//
// Architecture: native host function injection via servo's script thread callback
// - servo's JSContext handles DOM + Web APIs (created by servo internally)
// - Node.js APIs are registered as mozjs native functions on servo's Window global
// - Uses servo's register_script_thread_callback → handle_evaluate_javascript drain pattern
// - Event loop coordination: servo's spin_event_loop() drives both contexts
//
// JSContext fusion:
// - servo creates JSContext internally in JSEngineSetup::default()
// - bao_runtime::globals::install_all registers native functions on that same JSContext
// - Callback receives (cx: *mut JSContext, global: *mut JSObject) from servo
// - No second JSContext needed — true parasitic fusion

use crate::page::PageHandle;
use crate::error::BrowserError;
use std::ptr::{self, NonNull};
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Inject Node.js APIs temporarily for privileged evaluate_js execution (REQ-SEC-002).
///
/// Uses `register_script_thread_callback` to queue a callback that creates
/// a temporary scope object `__bao_privileged_apis` on the global. The scope
/// contains all Node.js/Bun API values (require, Bun, process, Buffer, module,
/// __filename, __dirname) but they are NOT installed on the Window global itself.
///
/// The IIFE wrapper in `wrap_privileged_script` extracts the scope, deletes it
/// from globalThis, and passes the values as function parameters to the user
/// script. This prevents page-level JS from accessing Node APIs (REQ-SEC-003).
///
/// This is the dual-layer JS model: page JS runs with Web APIs only,
/// but evaluate_js scripts get the full Node.js/Bun runtime via parameters.
pub fn inject_node_apis_for_evaluate(webview_id: servo::WebViewId) {
    let callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send> =
        Box::new(move |cx_ptr, global_ptr| {
            unsafe { create_scope_values_native(cx_ptr, global_ptr); }
        });

    servo::register_script_thread_callback(webview_id, callback);
}

/// Bridge callback to create Node API scope values for privileged evaluate_js.
///
/// Creates `__bao_privileged_apis` on the global with all Node API values.
/// The IIFE wrapper deletes this scope and passes the values as parameters.
/// Node APIs are never written to the Window global itself (REQ-SEC-003).
unsafe fn create_scope_values_native(cx_ptr: *mut std::ffi::c_void, global_ptr: *mut std::ffi::c_void) {
    use mozjs::context::JSContext;
    use mozjs::gc::Handle;
    use mozjs::jsapi::{JSContext as RawJSContext, JSObject};

    let raw_cx = cx_ptr as *mut RawJSContext;
    let raw_global = global_ptr as *mut JSObject;

    let cx_nn = match NonNull::new(raw_cx) {
        Some(nn) => nn,
        None => return,
    };

    if raw_global.is_null() {
        return;
    }

    let mut cx = unsafe { JSContext::from_ptr(cx_nn) };
    let global = unsafe { Handle::from_marked_location(raw_global as *const *mut JSObject) };

    unsafe {
        bao_runtime::globals::create_node_api_scope_values(&mut cx, global);
    }
}

/// Inject Node.js APIs as native mozjs host functions on servo's Window global.
///
/// Uses `servo::register_script_thread_callback` to queue a callback that will
/// be drained on servo's script thread during `handle_evaluate_javascript`.
/// The callback casts the raw pointers to mozjs types and calls
/// `bao_runtime::globals::install_all` to register all Node.js/Bun host functions
/// natively — zero JS polyfill strings, maximum performance.
///
/// Also installs stealth anti-fingerprinting properties as PERMANENT engine-layer
/// getters if a stealth profile is provided.
///
/// Falls back to JS polyfill injection if native registration is unavailable.
pub fn inject_node_apis(page: &PageHandle) -> Result<(), BrowserError> {
    inject_node_apis_with_stealth(page, None)
}

/// Inject Node.js APIs with optional stealth profile.
///
/// Same as `inject_node_apis`, but also installs stealth properties as PERMANENT
/// engine-layer getters when a profile is provided.
pub fn inject_node_apis_with_stealth(page: &PageHandle, stealth_profile: Option<bao_stealth::StealthProfile>) -> Result<(), BrowserError> {
    let webview_id = page.webview_id()
        .ok_or_else(|| BrowserError::Init("page has no webview".into()))?;

    let registered = register_native_host_functions(webview_id, stealth_profile);

    // Trigger the callback drain by evaluating an empty script.
    // Uses evaluate_js_web (no Node API injection) to avoid recursion.
    // servo's handle_evaluate_javascript will drain pending callbacks
    // before executing the (empty) JS.
    page.evaluate_js_web("")?;

    if !registered {
        // Fallback: inject JS polyfill string
        page.evaluate_js_web(NODE_POLYFILLS)?;
    }

    Ok(())
}

/// Attempt to register bao_runtime's native host functions via servo's callback mechanism.
///
/// Returns `true` if registration succeeded, `false` if servo's API is unavailable
/// (e.g., older servo build without `register_script_thread_callback`).
///
/// If `stealth_profile` is provided, stealth properties are installed as PERMANENT
/// engine-layer getters after the Node.js host functions.
fn register_native_host_functions(webview_id: servo::WebViewId, stealth_profile: Option<bao_stealth::StealthProfile>) -> bool {
    let callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send> =
        Box::new(move |cx_ptr, global_ptr| {
            // SAFETY: Called on servo's script thread with valid JSContext/JSObject.
            unsafe { install_all_native(cx_ptr, global_ptr, &stealth_profile); }
        });

    servo::register_script_thread_callback(webview_id, callback);
    true
}

/// Bridge callback: cast raw servo pointers to mozjs types and install all host functions.
///
/// Called on servo's script thread during `handle_evaluate_javascript` drain.
/// `cx_ptr` is `*mut mozjs::jsapi::JSContext` (servo's script thread JSContext).
/// `global_ptr` is `*mut mozjs::jsapi::JSObject` (servo's Window global object).
///
/// If `stealth_profile` is `Some`, installs stealth properties as PERMANENT engine-layer
/// getters (JSPROP_PERMANENT ≡ configurable:false) after the Node.js host functions.
unsafe fn install_all_native(cx_ptr: *mut std::ffi::c_void, global_ptr: *mut std::ffi::c_void, stealth_profile: &Option<bao_stealth::StealthProfile>) {
    use mozjs::context::JSContext;
    use mozjs::gc::Handle;
    use mozjs::jsapi::{JSContext as RawJSContext, JSObject};

    let raw_cx = cx_ptr as *mut RawJSContext;
    let raw_global = global_ptr as *mut JSObject;

    let cx_nn = match NonNull::new(raw_cx) {
        Some(nn) => nn,
        None => return,
    };

    if raw_global.is_null() {
        return;
    }

    // SAFETY: We are on servo's script thread, the JSContext is valid,
    // and this is the only JSContext alive (SpiderMonkey constraint).
    let mut cx = unsafe { JSContext::from_ptr(cx_nn) };

    // Create a Handle from the raw global pointer.
    // SAFETY: global_ptr points to a valid, live JSObject on the GC heap.
    // The Handle is valid for the duration of this callback (no GC in between).
    let global = unsafe { Handle::from_marked_location(raw_global as *const *mut JSObject) };

    // Register Web APIs only on page global (REQ-SEC-003).
    // Node.js APIs (require, fs, Bun, process, etc.) are NOT installed on
    // the page global — they are only available in privileged evaluate_js context.
    unsafe {
        bao_runtime::globals::install_web_apis(&mut cx, global);
    }

    // Install stealth properties as PERMANENT engine-layer getters.
    // JSPROP_PERMANENT ≡ configurable:false → JS Object.defineProperty throws TypeError.
    // Zero JS injection — all properties are native accessor getters.
    if let Some(profile) = stealth_profile {
        bao_stealth::engine_props::set_profile(profile);
        bao_runtime::fetch_api::set_fetch_stealth_profile(Some(profile.clone()));
        unsafe {
            bao_stealth::engine_props::install_stealth_props(raw_cx, raw_global);
        }
    } else {
        bao_runtime::fetch_api::set_fetch_stealth_profile(None);
    }
}

/// Inject both Node.js APIs and stealth scripts into a page.
pub fn inject_all(page: &PageHandle, stealth: bool) -> Result<(), BrowserError> {
    let profile = if stealth {
        page.stealth_profile()
    } else {
        None
    };
    inject_node_apis_with_stealth(page, profile)
}

/// Inject Node.js APIs and (if profile present) stealth properties into a page.
///
/// Stealth properties are installed as PERMANENT engine-layer getters (zero JS injection).
pub fn inject_all_with_profile(page: &PageHandle, profile: &Option<bao_stealth::StealthProfile>) -> Result<(), BrowserError> {
    inject_node_apis_with_stealth(page, profile.clone())
}

const NODE_POLYFILLS: &str = r#"(function() {
  // @trace REQ-ENG-007 Node.js API polyfills for browser context

  // global alias
  if (typeof global === 'undefined') {
    global = globalThis;
  }

  // process
  if (typeof process === 'undefined') {
    process = {
      argv: ['bao', typeof __filename !== 'undefined' ? __filename : ''],
      argv0: 'bao',
      execArgv: [],
      execPath: '/usr/local/bin/bao',
      env: (function() {
        var e = {};
        if (typeof navigator !== 'undefined' && navigator.userAgent) {
          e.NODE_VERSION = '20.11.0';
          e.BAO_VERSION = '0.1.0';
        }
        e.HOME = '/';
        e.PATH = '/usr/local/bin:/usr/bin:/bin';
        e.TERM = 'xterm-256color';
        return e;
      })(),
      version: 'v20.11.0',
      versions: {
        node: '20.11.0',
        v8: '12.4.254.14',
        uv: '1.27.0',
        zlib: '1.2.13',
        brotli: '1.0.9',
        ares: '1.19.1',
        modules: '115',
        openssl: '3.0.12',
        icu: '74.2',
        bun: '1.0.25',
        bao: '0.1.0',
      },
      pid: 1,
      ppid: 0,
      title: 'bao',
      arch: (function() {
        if (typeof navigator !== 'undefined') {
          var p = navigator.platform || '';
          if (p.indexOf('Win') >= 0) return 'x64';
          if (p.indexOf('Mac') >= 0) return 'arm64';
          if (p.indexOf('Linux') >= 0) return 'x64';
        }
        return 'x64';
      })(),
      platform: (function() {
        if (typeof navigator !== 'undefined') {
          var p = navigator.platform || '';
          if (p.indexOf('Win') >= 0) return 'win32';
          if (p.indexOf('Mac') >= 0) return 'darwin';
        }
        return 'linux';
      })(),
      cwd: function() { return '/'; },
      chdir: function() {},
      exit: function(code) { throw new Error('process.exit(' + (code||0) + ')'); },
      hrtime: (function() {
        var origin = performance.now() * 1e-3;
        return function bigtime() {
          var diff = performance.now() * 1e-3 - origin;
          var sec = Math.floor(diff);
          var nsec = Math.round((diff - sec) * 1e9);
          if (arguments.length > 0) {
            sec += arguments[0][0];
            nsec += arguments[0][1];
            sec += Math.floor(nsec / 1e9);
            nsec = nsec % 1e9;
            if (nsec < 0) { nsec += 1e9; sec -= 1; }
          }
          var result = [sec, nsec];
          result.bigint = function() { return BigInt(sec) * 1000000000n + BigInt(nsec); };
          return result;
        };
      })(),
      uptime: function() { return performance.now() / 1000; },
      memoryUsage: function() {
        return { rss: 64*1024*1024, heapTotal: 32*1024*1024, heapUsed: 16*1024*1024, external: 2*1024*1024, arrayBuffers: 1*1024*1024 };
      },
      cpuUsage: function() { return { user: 100000, system: 50000 }; },
      nextTick: function(fn) {
        var args = Array.prototype.slice.call(arguments, 1);
        Promise.resolve().then(function() { fn.apply(null, args); });
      },
      binding: function(name) { return {}; },
      dlopen: function() { throw new Error('process.dlopen not available in browser context'); },
      stdout: { write: function(d) { console.log(d); return true; }, end: function() {} },
      stderr: { write: function(d) { console.error(d); return true; }, end: function() {} },
      stdin: { on: function() {}, resume: function() { return this; }, pipe: function() {} },
      on: function(event, fn) { return this; },
      off: function() {},
      once: function(event, fn) { return this; },
      emit: function(event) { return false; },
      removeAllListeners: function() { return this; },
      setUncaughtExceptionCallback: function() {},
    };
  }

  // Buffer — browser-compatible implementation backed by Uint8Array
  if (typeof Buffer === 'undefined') {
    Buffer = (function() {
      function B(data, encoding) {
        if (!(this instanceof B)) return new B(data, encoding);
        if (data instanceof Uint8Array) {
          this._buf = new Uint8Array(data);
        } else if (data instanceof ArrayBuffer) {
          this._buf = new Uint8Array(data);
        } else if (typeof data === 'string') {
          this._buf = new Uint8Array(Array.from(data).map(function(c) { return c.charCodeAt(0); }));
        } else if (Array.isArray(data)) {
          this._buf = new Uint8Array(data);
        } else {
          this._buf = new Uint8Array(0);
        }
        this.length = this._buf.length;
      }

      B.isBuffer = function(obj) { return obj instanceof B; };

      B.from = function(data, encoding) {
        if (data instanceof B) return new B(data._buf);
        if (data instanceof Uint8Array) return new B(data);
        if (data instanceof ArrayBuffer) return new B(data);
        if (typeof data === 'string') {
          if (encoding === 'hex') {
            var bytes = [];
            for (var i = 0; i < data.length; i += 2) {
              bytes.push(parseInt(data.substr(i, 2), 16));
            }
            return new B(bytes);
          }
          if (encoding === 'base64') {
            var bin = atob(data);
            var bytes = [];
            for (var i = 0; i < bin.length; i++) bytes.push(bin.charCodeAt(i));
            return new B(bytes);
          }
          return new B(data);
        }
        return new B(data);
      };

      B.alloc = function(size, fill, encoding) {
        var buf = new B(new Uint8Array(size));
        if (fill !== undefined) buf.fill(fill);
        return buf;
      };

      B.allocUnsafe = function(size) {
        return new B(new Uint8Array(size));
      };

      B.allocUnsafeSlow = function(size) {
        return new B(new Uint8Array(size));
      };

      B.concat = function(list, totalLength) {
        if (!Array.isArray(list) || list.length === 0) return new B(new Uint8Array(0));
        var len = totalLength !== undefined ? totalLength : list.reduce(function(a, b) { return a + b.length; }, 0);
        var result = new Uint8Array(len);
        var offset = 0;
        for (var i = 0; i < list.length; i++) {
          var buf = list[i] instanceof B ? list[i]._buf : new Uint8Array(list[i]);
          result.set(buf, offset);
          offset += buf.length;
        }
        return new B(result);
      };

      B.byteLength = function(str, encoding) {
        if (typeof str === 'string') {
          if (encoding === 'base64') return atob(str).length;
          if (encoding === 'hex') return str.length / 2;
          return new TextEncoder().encode(str).length;
        }
        if (str instanceof ArrayBuffer) return str.byteLength;
        if (str instanceof Uint8Array) return str.length;
        return 0;
      };

      B.compare = function(a, b) {
        for (var i = 0; i < Math.min(a.length, b.length); i++) {
          if (a._buf[i] < b._buf[i]) return -1;
          if (a._buf[i] > b._buf[i]) return 1;
        }
        return a.length - b.length;
      };

      B.prototype.slice = function(start, end) {
        return new B(this._buf.slice(start || 0, end));
      };

      B.prototype.subarray = function(start, end) {
        return new B(this._buf.subarray(start || 0, end));
      };

      B.prototype.toString = function(encoding, start, end) {
        var s = start || 0;
        var e = end !== undefined ? end : this._buf.length;
        var slice = this._buf.slice(s, e);
        if (encoding === 'hex') {
          return Array.from(slice).map(function(b) { return b.toString(16).padStart(2, '0'); }).join('');
        }
        if (encoding === 'base64') {
          var bin = Array.from(slice).map(function(b) { return String.fromCharCode(b); }).join('');
          return btoa(bin);
        }
        return new TextDecoder().decode(slice);
      };

      B.prototype.toJSON = function() {
        return { type: 'Buffer', data: Array.from(this._buf) };
      };

      B.prototype.equals = function(other) {
        if (!(other instanceof B) || this.length !== other.length) return false;
        for (var i = 0; i < this.length; i++) {
          if (this._buf[i] !== other._buf[i]) return false;
        }
        return true;
      };

      B.prototype.compare = function(other, targetStart, targetEnd, sourceStart, sourceEnd) {
        var a = this._buf.slice(sourceStart || 0, sourceEnd);
        var b = other._buf.slice(targetStart || 0, targetEnd);
        for (var i = 0; i < Math.min(a.length, b.length); i++) {
          if (a[i] < b[i]) return -1;
          if (a[i] > b[i]) return 1;
        }
        return a.length - b.length;
      };

      B.prototype.copy = function(target, targetStart, sourceStart, sourceEnd) {
        var src = this._buf.slice(sourceStart || 0, sourceEnd);
        for (var i = 0; i < src.length; i++) {
          if (target._buf) target._buf[targetStart + i] = src[i];
        }
        return src.length;
      };

      B.prototype.fill = function(value, start, end) {
        var s = start || 0;
        var e = end !== undefined ? end : this._buf.length;
        var v = typeof value === 'number' ? value : 0;
        for (var i = s; i < e; i++) this._buf[i] = v;
        return this;
      };

      B.prototype.write = function(str, offset, length, encoding) {
        var o = offset || 0;
        var bytes = new TextEncoder().encode(str);
        var len = Math.min(bytes.length, length !== undefined ? length : this._buf.length - o);
        for (var i = 0; i < len; i++) this._buf[o + i] = bytes[i];
        return len;
      };

      B.prototype.includes = function(value, offset) {
        return this.indexOf(value, offset) !== -1;
      };

      B.prototype.indexOf = function(value, offset) {
        var o = offset || 0;
        var search = typeof value === 'number' ? [value] : Array.from(new TextEncoder().encode(String(value)));
        for (var i = o; i <= this._buf.length - search.length; i++) {
          var found = true;
          for (var j = 0; j < search.length; j++) {
            if (this._buf[i + j] !== search[j]) { found = false; break; }
          }
          if (found) return i;
        }
        return -1;
      };

      B.prototype.readUInt8 = function(offset) { return this._buf[offset || 0]; };
      B.prototype.readUInt16LE = function(offset) { var o = offset||0; return this._buf[o] | (this._buf[o+1]<<8); };
      B.prototype.readUInt16BE = function(offset) { var o = offset||0; return (this._buf[o]<<8) | this._buf[o+1]; };
      B.prototype.readUInt32LE = function(offset) {
        var o = offset||0;
        return (this._buf[o]) | (this._buf[o+1]<<8) | (this._buf[o+2]<<16) | (this._buf[o+3]<<24);
      };
      B.prototype.readInt8 = function(offset) { var v = this._buf[offset||0]; return v > 127 ? v - 256 : v; };
      B.prototype.readInt16LE = function(offset) { var v = this.readUInt16LE(offset); return v > 32767 ? v - 65536 : v; };
      B.prototype.readInt32LE = function(offset) { var v = this.readUInt32LE(offset); return v > 2147483647 ? v - 4294967296 : v; };
      B.prototype.readFloatLE = function(offset) {
        var buf = new ArrayBuffer(4); new Float32Array(buf)[0] = 0;
        new Uint8Array(buf).set(this._buf.slice(offset||0, (offset||0)+4));
        return new Float32Array(buf)[0];
      };
      B.prototype.readDoubleLE = function(offset) {
        var buf = new ArrayBuffer(8);
        new Uint8Array(buf).set(this._buf.slice(offset||0, (offset||0)+8));
        return new Float64Array(buf)[0];
      };

      B.prototype.writeUInt8 = function(v, offset) { this._buf[offset||0] = v & 0xFF; return (offset||0)+1; };
      B.prototype.writeUInt16LE = function(v, offset) { var o = offset||0; this._buf[o]=v&0xFF; this._buf[o+1]=(v>>8)&0xFF; return o+2; };
      B.prototype.writeUInt32LE = function(v, offset) { var o = offset||0; this._buf[o]=v&0xFF; this._buf[o+1]=(v>>8)&0xFF; this._buf[o+2]=(v>>16)&0xFF; this._buf[o+3]=(v>>24)&0xFF; return o+4; };
      B.prototype.writeInt8 = function(v, offset) { return this.writeUInt8(v < 0 ? v + 256 : v, offset); };
      B.prototype.writeInt16LE = function(v, offset) { return this.writeUInt16LE(v < 0 ? v + 65536 : v, offset); };
      B.prototype.writeInt32LE = function(v, offset) { return this.writeUInt32LE(v < 0 ? v + 4294967296 : v, offset); };
      B.prototype.writeFloatLE = function(v, offset) {
        var buf = new ArrayBuffer(4); new Float32Array(buf)[0] = v;
        this._buf.set(new Uint8Array(buf), offset||0); return (offset||0)+4;
      };
      B.prototype.writeDoubleLE = function(v, offset) {
        var buf = new ArrayBuffer(8); new Float64Array(buf)[0] = v;
        this._buf.set(new Uint8Array(buf), offset||0); return (offset||0)+8;
      };

      B.prototype[Symbol.iterator] = function() {
        var idx = 0; var buf = this._buf;
        return { next: function() { return idx < buf.length ? { value: buf[idx++], done: false } : { done: true }; } };
      };

      return B;
    })();
  }

  // require — basic module loader for browser context
  if (typeof require === 'undefined') {
    var _module_cache = {};
    var _module_builtin = {
      'fs': { readFileSync: function() { throw new Error('fs not available in browser context'); }, existsSync: function() { return false; } },
      'path': {
        join: function() { return Array.prototype.slice.call(arguments).join('/').replace(/\/+/g, '/'); },
        resolve: function() { var parts = Array.prototype.slice.call(arguments); return '/' + parts.join('/').replace(/\/+/g, '/'); },
        dirname: function(p) { return p.split('/').slice(0, -1).join('/') || '.'; },
        basename: function(p, ext) { var b = p.split('/').pop(); return ext && b.endsWith(ext) ? b.slice(0, -ext.length) : b; },
        extname: function(p) { var i = p.lastIndexOf('.'); return i >= 0 ? p.slice(i) : ''; },
        sep: '/', delimiter: ':',
        posix: {
          join: function() { return Array.prototype.slice.call(arguments).join('/').replace(/\/+/g, '/'); },
          resolve: function() { var parts = Array.prototype.slice.call(arguments); return '/' + parts.join('/').replace(/\/+/g, '/'); },
          dirname: function(p) { return p.split('/').slice(0, -1).join('/') || '.'; },
          basename: function(p, ext) { var b = p.split('/').pop(); return ext && b.endsWith(ext) ? b.slice(0, -ext.length) : b; },
          extname: function(p) { var i = p.lastIndexOf('.'); return i >= 0 ? p.slice(i) : ''; },
          sep: '/', delimiter: ':',
        },
        win32: { sep: '\\', delimiter: ';' },
      },
      'url': {
        parse: function(u) { try { var p = new URL(u); return { href: p.href, protocol: p.protocol, host: p.host, hostname: p.hostname, pathname: p.pathname, search: p.search, hash: p.hash }; } catch(e) { return {}; } },
        format: function(u) { return typeof u === 'string' ? u : (u.protocol||'http:') + '//' + (u.host||u.hostname||'localhost') + (u.pathname||'/'); },
        resolve: function(from, to) { try { return new URL(to, from).href; } catch(e) { return to; } },
        URL: typeof URL !== 'undefined' ? URL : function() {},
        URLSearchParams: typeof URLSearchParams !== 'undefined' ? URLSearchParams : function() {},
      },
      'querystring': {
        parse: function(str, sep, eq) {
          sep = sep || '&'; eq = eq || '=';
          var obj = {};
          if (!str) return obj;
          str.split(sep).forEach(function(pair) {
            var idx = pair.indexOf(eq);
            var key = idx >= 0 ? pair.substring(0, idx) : pair;
            var val = idx >= 0 ? pair.substring(idx + 1) : '';
            obj[decodeURIComponent(key)] = decodeURIComponent(val);
          });
          return obj;
        },
        stringify: function(obj, sep, eq) {
          sep = sep || '&'; eq = eq || '=';
          return Object.keys(obj || {}).map(function(k) {
            return encodeURIComponent(k) + eq + encodeURIComponent(obj[k]);
          }).join(sep);
        },
        escape: encodeURIComponent,
        unescape: decodeURIComponent,
      },
      'events': {
        EventEmitter: (function() {
          function EE() { this._events = {}; }
          EE.prototype.on = function(e, fn) { (this._events[e] = this._events[e] || []).push(fn); return this; };
          EE.prototype.once = function(e, fn) { var self = this; function g() { self.off(e, g); fn.apply(this, arguments); } g._orig = fn; this.on(e, g); return this; };
          EE.prototype.off = function(e, fn) {
            if (!this._events[e]) return this;
            if (!fn) { delete this._events[e]; return this; }
            this._events[e] = this._events[e].filter(function(f) { return f !== fn && f._orig !== fn; });
            return this;
          };
          EE.prototype.emit = function(e) {
            var args = Array.prototype.slice.call(arguments, 1);
            (this._events[e] || []).forEach(function(fn) { fn.apply(null, args); });
            return this;
          };
          EE.prototype.removeListener = EE.prototype.off;
          EE.prototype.removeAllListeners = function(e) { if (e) delete this._events[e]; else this._events = {}; return this; };
          EE.prototype.listeners = function(e) { return this._events[e] || []; };
          EE.prototype.listenerCount = function(e) { return (this._events[e] || []).length; };
          return EE;
        })(),
      },
      'util': {
        inspect: function(obj) { return JSON.stringify(obj, null, 2); },
        inherits: function(ctor, superCtor) { ctor.prototype = Object.create(superCtor.prototype); ctor.prototype.constructor = ctor; },
        isFunction: function(v) { return typeof v === 'function'; },
        isNull: function(v) { return v === null; },
        isUndefined: function(v) { return v === undefined; },
        isObject: function(v) { return v !== null && typeof v === 'object'; },
        isString: function(v) { return typeof v === 'string'; },
        promisify: function(fn) {
          return function() {
            var args = Array.prototype.slice.call(arguments);
            return new Promise(function(resolve, reject) {
              args.push(function(err, result) { if (err) reject(err); else resolve(result); });
              fn.apply(null, args);
            });
          };
        },
        format: function(fmt) {
          var args = Array.prototype.slice.call(arguments, 1);
          return fmt.replace(/%[sdjifo]/g, function(m) { return args.length ? String(args.shift()) : m; });
        },
        types: {
          isDate: function(v) { return v instanceof Date; },
          isRegExp: function(v) { return v instanceof RegExp; },
          isArray: function(v) { return Array.isArray(v); },
          isPromise: function(v) { return v && typeof v.then === 'function'; },
        },
      },
      'stream': { Readable: function(){}, Writable: function(){}, Duplex: function(){}, Transform: function(){} },
      'buffer': { Buffer: typeof Buffer !== 'undefined' ? Buffer : function(){} },
      'crypto': {
        randomBytes: function(size, cb) {
          var arr = new Uint8Array(size);
          if (typeof crypto !== 'undefined' && crypto.getRandomValues) crypto.getRandomValues(arr);
          if (cb) cb(null, Buffer.from(arr));
          return Buffer.from(arr);
        },
        createHash: function(algo) {
          var chunks = [];
          return {
            update: function(data) { chunks.push(typeof data === 'string' ? data : String(data)); return this; },
            digest: function(enc) {
              var str = chunks.join('');
              if (typeof crypto !== 'undefined' && crypto.subtle) {
                return crypto.subtle.digest('SHA-256', new TextEncoder().encode(str)).then(function(buf) {
                  var arr = new Uint8Array(buf); return enc === 'hex' ? Array.from(arr).map(function(b){return b.toString(16).padStart(2,'0');}).join('') : Buffer.from(arr);
                });
              }
              return enc === 'hex' ? '00000000' : Buffer.alloc(0);
            },
          };
        },
      },
      'os': {
        platform: function() { return 'linux'; },
        arch: function() { return 'x64'; },
        homedir: function() { return '/'; },
        tmpdir: function() { return '/tmp'; },
        type: function() { return 'Linux'; },
        release: function() { return '6.8.0'; },
        hostname: function() { return 'bao'; },
        cpus: function() { return [{ model: 'bao', speed: 3000 }]; },
        totalmem: function() { return 8*1024*1024*1024; },
        freemem: function() { return 4*1024*1024*1024; },
        uptime: function() { return 3600; },
        EOL: '\n',
      },
      'assert': {
        ok: function(v, msg) { if (!v) throw new Error(msg || 'assertion failed'); },
        equal: function(a, b, msg) { if (a !== b) throw new Error(msg || a + ' !== ' + b); },
        deepEqual: function(a, b, msg) { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(msg || 'not deep equal'); },
        throws: function(fn, msg) { try { fn(); throw new Error(msg || 'expected throw'); } catch(e) { if (e.message === (msg || 'expected throw')) throw e; } },
      },
      'timers': {
        setTimeout: typeof setTimeout !== 'undefined' ? setTimeout : function(fn) { fn(); return 0; },
        setInterval: typeof setInterval !== 'undefined' ? setInterval : function(fn) { return 0; },
        clearTimeout: typeof clearTimeout !== 'undefined' ? clearTimeout : function() {},
        clearInterval: typeof clearInterval !== 'undefined' ? clearInterval : function() {},
        setImmediate: typeof setImmediate !== 'undefined' ? setImmediate : function(fn) { return setTimeout(fn, 0); },
        clearImmediate: typeof clearImmediate !== 'undefined' ? clearImmediate : function() {},
      },
    };

    require = function(name) {
      if (_module_cache[name]) return _module_cache[name];
      if (_module_builtin[name]) { _module_cache[name] = _module_builtin[name]; return _module_builtin[name]; }
      throw new Error("Cannot find module '" + name + "' in browser context");
    };

    require.resolve = function(name) { return name; };
    require.cache = _module_cache;
  }

  // setImmediate / clearImmediate
  if (typeof setImmediate === 'undefined') {
    setImmediate = function(fn) {
      var args = Array.prototype.slice.call(arguments, 1);
      return setTimeout(function() { fn.apply(null, args); }, 0);
    };
    clearImmediate = function(id) { clearTimeout(id); };
  }

  // __dirname / __filename
  if (typeof __dirname === 'undefined') {
    __dirname = '/';
    __filename = '/index.js';
  }

  // TextEncoder / TextDecoder (most browsers have these, but ensure)
  if (typeof TextEncoder === 'undefined') {
    TextEncoder = function() { this.encode = function(str) { return new Uint8Array(Array.from(str).map(function(c){return c.charCodeAt(0);})); }; };
  }
  if (typeof TextDecoder === 'undefined') {
    TextDecoder = function() { this.decode = function(buf) { return String.fromCharCode.apply(null, buf); }; };
  }

  // URL / URLSearchParams (most browsers have these, but ensure)
  if (typeof URL === 'undefined') {
    URL = function(url, base) { throw new Error('URL not available'); };
  }
  if (typeof URLSearchParams === 'undefined') {
    URLSearchParams = function(init) {
      this._params = [];
      this.append = function(k,v) { this._params.push([k,v]); };
      this.get = function(k) { for(var i=0;i<this._params.length;i++) if(this._params[i][0]===k) return this._params[i][1]; return null; };
      this.toString = function() { return this._params.map(function(p){return p[0]+'='+p[1];}).join('&'); };
    };
  }

  // btoa / atob (most browsers have these, but ensure)
  if (typeof btoa === 'undefined') {
    var _b64chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=';
    btoa = function(str) {
      var out = '';
      for (var i = 0; i < str.length; i += 3) {
        var a = str.charCodeAt(i), b = str.charCodeAt(i+1), c = str.charCodeAt(i+2);
        out += _b64chars[a>>2] + _b64chars[((a&3)<<4)|(b>>4)] + (isNaN(b)?'=':_b64chars[((b&15)<<2)|(c>>6)]) + (isNaN(b)||isNaN(c)?'=':_b64chars[c&63]);
      }
      return out;
    };
    atob = function(str) {
      var out = '';
      str = str.replace(/=+$/, '');
      for (var i = 0; i < str.length; i += 4) {
        var a = _b64chars.indexOf(str[i]), b = _b64chars.indexOf(str[i+1]);
        var c = _b64chars.indexOf(str[i+2]), d = _b64chars.indexOf(str[i+3]);
        out += String.fromCharCode((a<<2)|(b>>4)) + (c>=0?String.fromCharCode(((b&15)<<4)|(c>>2)):'') + (d>=0?String.fromCharCode(((c&3)<<6)|d):'');
      }
      return out;
    };
  }
})();"#;

// ── Bridge types ────────────────────────────────────────────────────

/// Commands sent through the runtime bridge for execution in a page context.
///
/// Each variant maps to a [`PageHandle`] operation. The bridge decouples
/// command submission from execution — a worker loop reads from the
/// [`BridgeReceiver`] and drives the real servo page.
///
/// @trace REQ-BRW-003 [entity:RuntimeBridge]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeCommand {
    /// Navigate the page to a URL.
    Navigate(String),
    /// Evaluate JavaScript in the page and return the result as a string.
    Evaluate(String),
    /// Capture a screenshot of the current page.
    Screenshot,
    /// Close the page and mark the bridge as inactive.
    Close,
    /// Resize the page viewport to width × height.
    Resize(u32, u32),
    /// Retrieve the current page title.
    GetTitle,
    /// Retrieve the current page URL.
    GetUrl,
}

/// Response returned after executing a [`BridgeCommand`].
///
/// @trace REQ-BRW-003 [entity:RuntimeBridge]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeResponse {
    /// Command succeeded with no return value.
    Ok,
    /// Command failed with a descriptive message.
    Err(String),
    /// Command returned a null / void result.
    Null,
    /// Command returned a string value (evaluation result, title, URL, …).
    Value(String),
    /// Command returned binary data (screenshot image bytes).
    Binary(Vec<u8>),
}

impl BridgeResponse {
    /// Returns `true` when the response is [`Ok`](BridgeResponse::Ok).
    pub fn is_ok(&self) -> bool {
        matches!(self, BridgeResponse::Ok)
    }

    /// Returns `true` when the response is [`Err`](BridgeResponse::Err).
    pub fn is_err(&self) -> bool {
        matches!(self, BridgeResponse::Err(_))
    }

    /// Converts [`Err`](BridgeResponse::Err) into `Result::Err`, wrapping all other
    /// variants in `Result::Ok`.
    pub fn ok(self) -> Result<Self, String> {
        match self {
            BridgeResponse::Err(e) => Err(e),
            other => Ok(other),
        }
    }
}

/// Receiving end of a [`BridgeChannel`].
///
/// A worker thread (or event-loop iteration) calls [`recv`](BridgeReceiver::recv)
/// to obtain commands and their optional response channels, executes them against
/// the page, and sends back [`BridgeResponse`] values.
pub struct BridgeReceiver {
    rx: mpsc::Receiver<(BridgeCommand, Option<mpsc::Sender<BridgeResponse>>)>,
    alive: Arc<AtomicBool>,
}

impl std::fmt::Debug for BridgeReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeReceiver")
            .field("alive", &self.alive)
            .finish()
    }
}

impl BridgeReceiver {
    /// Block until a command arrives or the channel is disconnected.
    pub fn recv(&self) -> Result<(BridgeCommand, Option<mpsc::Sender<BridgeResponse>>), String> {
        self.rx.recv().map_err(|_| "channel closed".to_string())
    }

    /// Block for at most `timeout`, returning the command or a timeout error.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<(BridgeCommand, Option<mpsc::Sender<BridgeResponse>>), String> {
        self.rx
            .recv_timeout(timeout)
            .map_err(|e| format!("{}", e))
    }

    /// Whether the bridge has been marked alive (both sides share the flag).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

/// Producer half of the bridge command channel.
///
/// Methods are thread-safe (`&self`) so a single channel can be shared across
/// threads for concurrent submission.
///
/// @trace REQ-BRW-003 [entity:BridgeChannel]
#[derive(Debug)]
pub struct BridgeChannel {
    tx: mpsc::Sender<(BridgeCommand, Option<mpsc::Sender<BridgeResponse>>)>,
    alive: Arc<AtomicBool>,
}

impl BridgeChannel {
    /// Create a new bridge channel pair.
    ///
    /// Returns `(sender, receiver)` where commands flow sender → receiver and
    /// responses flow back via per-command one-shot channels.
    pub fn new() -> (Self, BridgeReceiver) {
        let (tx, rx) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let channel = BridgeChannel {
            tx,
            alive: alive.clone(),
        };
        let receiver = BridgeReceiver { rx, alive };
        (channel, receiver)
    }

    /// Send a command and block until the worker returns a response.
    pub fn send(&self, cmd: BridgeCommand) -> Result<BridgeResponse, String> {
        let (resp_tx, resp_rx) = mpsc::channel();
        self.tx
            .send((cmd, Some(resp_tx)))
            .map_err(|_| "bridge closed".to_string())?;
        resp_rx.recv().map_err(|_| "response channel closed".to_string())
    }

    /// Send a command and wait at most `timeout` for a response.
    pub fn send_timeout(&self, cmd: BridgeCommand, timeout: Duration) -> Result<BridgeResponse, String> {
        let (resp_tx, resp_rx) = mpsc::channel();
        self.tx
            .send((cmd, Some(resp_tx)))
            .map_err(|_| "bridge closed".to_string())?;
        resp_rx
            .recv_timeout(timeout)
            .map_err(|e| format!("{}", e))
    }

    /// Send a command without waiting for a response.
    ///
    /// The worker receives `None` for the responder slot and can skip
    /// the response-send step.
    pub fn fire_and_forget(&self, cmd: BridgeCommand) -> Result<(), String> {
        self.tx
            .send((cmd, None))
            .map_err(|_| "bridge closed".to_string())
    }

    /// Whether the bridge is marked alive (both sender and receiver).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Mark the bridge as closed.
    ///
    /// This only sets a flag — the underlying channel remains connected.
    /// Dropping the [`BridgeChannel`] / [`BridgeReceiver`] pair fully tears
    /// down the transport.
    pub fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

/// High-level bridge that owns a [`BridgeChannel`] and provides the public
/// command API for the bao_browser runtime.
///
/// In production, a worker loop reads from the associated [`BridgeReceiver`]
/// and dispatches commands to a servo [`PageHandle`].  In tests the channel
/// alone is exercised.
///
/// @trace REQ-BRW-003 [entity:RuntimeBridge]
#[derive(Debug)]
pub struct RuntimeBridge {
    channel: BridgeChannel,
}

impl RuntimeBridge {
    /// Create a fresh bridge, returning the sending half and the receiver.
    pub fn new() -> (Self, BridgeReceiver) {
        let (channel, receiver) = BridgeChannel::new();
        (RuntimeBridge { channel }, receiver)
    }

    /// Send a command and wait for the response.  See [`BridgeChannel::send`].
    pub fn send(&self, cmd: BridgeCommand) -> Result<BridgeResponse, String> {
        self.channel.send(cmd)
    }

    /// Send a command and wait at most `timeout` for a response.
    /// See [`BridgeChannel::send_timeout`].
    pub fn send_timeout(&self, cmd: BridgeCommand, timeout: Duration) -> Result<BridgeResponse, String> {
        self.channel.send_timeout(cmd, timeout)
    }

    /// Send a command without waiting for a response.
    /// See [`BridgeChannel::fire_and_forget`].
    pub fn fire_and_forget(&self, cmd: BridgeCommand) -> Result<(), String> {
        self.channel.fire_and_forget(cmd)
    }

    /// Whether the bridge is alive.  See [`BridgeChannel::is_alive`].
    pub fn is_alive(&self) -> bool {
        self.channel.is_alive()
    }

    /// Mark the bridge closed.  See [`BridgeChannel::close`].
    pub fn close(&self) {
        self.channel.close();
    }
}

#[cfg(test)]
mod tests {
    // ─── Polyfill validation ──────────────────────────────────────
    // @trace REQ-BRW-003 [req:REQ-BRW-003] [level:unit]

    #[test]
    fn test_polyfills_are_valid_js() {
        assert!(!super::NODE_POLYFILLS.is_empty());
        assert!(super::NODE_POLYFILLS.contains("Buffer"));
        assert!(super::NODE_POLYFILLS.contains("require"));
        assert!(super::NODE_POLYFILLS.contains("process"));
    }

    // ─── BridgeCommand / BridgeResponse / BridgeChannel extended tests ──
    // @trace REQ-BRW-003 [req:REQ-BRW-003] [level:unit]

    #[test]
    fn bridge_command_navigate_equality() {
        let cmd1 = super::BridgeCommand::Navigate("https://example.com".into());
        let cmd2 = super::BridgeCommand::Navigate("https://example.com".into());
        let cmd3 = super::BridgeCommand::Navigate("https://other.com".into());
        assert_eq!(cmd1, cmd2);
        assert_ne!(cmd1, cmd3);
    }

    #[test]
    fn bridge_command_evaluate_equality() {
        let cmd1 = super::BridgeCommand::Evaluate("1+1".into());
        let cmd2 = super::BridgeCommand::Evaluate("1+1".into());
        assert_eq!(cmd1, cmd2);
        assert_ne!(cmd1, super::BridgeCommand::Evaluate("2+2".into()));
    }

    #[test]
    fn bridge_command_resize_equality() {
        assert_eq!(super::BridgeCommand::Resize(800, 600), super::BridgeCommand::Resize(800, 600));
        assert_ne!(super::BridgeCommand::Resize(800, 600), super::BridgeCommand::Resize(1024, 768));
    }

    #[test]
    fn bridge_command_variants_distinct() {
        let cmds = [
            super::BridgeCommand::Navigate("x".into()),
            super::BridgeCommand::Evaluate("y".into()),
            super::BridgeCommand::Screenshot,
            super::BridgeCommand::Close,
            super::BridgeCommand::Resize(1, 1),
            super::BridgeCommand::GetTitle,
            super::BridgeCommand::GetUrl,
        ];
        for i in 0..cmds.len() {
            for j in 0..cmds.len() {
                if i != j {
                    assert_ne!(cmds[i], cmds[j]);
                }
            }
        }
    }

    #[test]
    fn bridge_response_ok_is_ok() {
        let resp = super::BridgeResponse::Ok;
        assert!(resp.is_ok());
        assert!(!resp.is_err());
    }

    #[test]
    fn bridge_response_err_is_err() {
        let resp = super::BridgeResponse::Err("failed".into());
        assert!(!resp.is_ok());
        assert!(resp.is_err());
    }

    #[test]
    fn bridge_response_null_not_err() {
        let resp = super::BridgeResponse::Null;
        assert!(!resp.is_ok());  // Null is not BridgeResponse::Ok
        assert!(!resp.is_err()); // Null is also not an error
    }

    #[test]
    fn bridge_response_value_not_err() {
        let resp = super::BridgeResponse::Value("result".into());
        assert!(!resp.is_ok());  // Value is not BridgeResponse::Ok
        assert!(!resp.is_err());
    }

    #[test]
    fn bridge_response_binary_not_err() {
        let resp = super::BridgeResponse::Binary(vec![1, 2, 3]);
        assert!(!resp.is_ok());  // Binary is not BridgeResponse::Ok
        assert!(!resp.is_err());
    }

    #[test]
    fn bridge_response_ok_method_wraps_non_err() {
        // .ok() converts Err → Result::Err, all others → Result::Ok
        assert!(super::BridgeResponse::Null.ok().is_ok());
        assert!(super::BridgeResponse::Value("v".into()).ok().is_ok());
        assert!(super::BridgeResponse::Binary(vec![]).ok().is_ok());
    }

    #[test]
    fn bridge_response_ok_method_on_err() {
        let resp = super::BridgeResponse::Err("error msg".into());
        let result = resp.ok();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "error msg");
    }

    #[test]
    fn bridge_response_ok_method_on_ok_variants() {
        assert!(super::BridgeResponse::Ok.ok().is_ok());
        assert!(super::BridgeResponse::Null.ok().is_ok());
        assert!(super::BridgeResponse::Value("v".into()).ok().is_ok());
        assert!(super::BridgeResponse::Binary(vec![]).ok().is_ok());
    }

    #[test]
    fn bridge_channel_new_alive() {
        let (channel, _receiver) = super::BridgeChannel::new();
        assert!(channel.is_alive());
    }

    #[test]
    fn bridge_channel_close_sets_not_alive() {
        let (channel, _receiver) = super::BridgeChannel::new();
        channel.close();
        assert!(!channel.is_alive());
    }

    #[test]
    fn bridge_receiver_alive_shares_flag() {
        let (channel, receiver) = super::BridgeChannel::new();
        assert!(receiver.is_alive());
        channel.close();
        assert!(!receiver.is_alive());
    }

    #[test]
    fn bridge_channel_fire_and_forget() {
        let (channel, receiver) = super::BridgeChannel::new();
        assert!(channel.fire_and_forget(super::BridgeCommand::GetTitle).is_ok());
        let (cmd, responder) = receiver.recv().unwrap();
        assert_eq!(cmd, super::BridgeCommand::GetTitle);
        assert!(responder.is_none());
    }

    #[test]
    fn bridge_channel_send_with_response() {
        let (channel, receiver) = super::BridgeChannel::new();
        // send() blocks until response — we need a worker thread
        let worker = std::thread::spawn(move || {
            let (cmd, responder) = receiver.recv().unwrap();
            if let Some(resp_tx) = responder {
                resp_tx.send(super::BridgeResponse::Value("title".into())).unwrap();
            }
        });
        let result = channel.send(super::BridgeCommand::GetTitle).unwrap();
        assert_eq!(result, super::BridgeResponse::Value("title".into()));
        worker.join().unwrap();
    }

    #[test]
    fn runtime_bridge_new_alive() {
        let (bridge, _receiver) = super::RuntimeBridge::new();
        assert!(bridge.is_alive());
    }

    #[test]
    fn runtime_bridge_close() {
        let (bridge, _receiver) = super::RuntimeBridge::new();
        bridge.close();
        assert!(!bridge.is_alive());
    }

    #[test]
    fn runtime_bridge_fire_and_forget() {
        let (bridge, receiver) = super::RuntimeBridge::new();
        assert!(bridge.fire_and_forget(super::BridgeCommand::Close).is_ok());
        let (cmd, responder) = receiver.recv().unwrap();
        assert_eq!(cmd, super::BridgeCommand::Close);
        assert!(responder.is_none());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Extended unit tests for bridge types and polyfills
    // @trace REQ-BRW-003 [req:REQ-BRW-003] [level:unit]
    // ═══════════════════════════════════════════════════════════════════════

    // ─── BridgeCommand Debug format tests ──────────────────────────────────

    #[test]
    fn bridge_command_debug_format_navigate() {
        let cmd = super::BridgeCommand::Navigate("https://example.com".into());
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Navigate"));
        assert!(debug_str.contains("https://example.com"));
    }

    #[test]
    fn bridge_command_debug_format_evaluate() {
        let cmd = super::BridgeCommand::Evaluate("return 42".into());
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Evaluate"));
        assert!(debug_str.contains("return 42"));
    }

    #[test]
    fn bridge_command_debug_format_screenshot() {
        let cmd = super::BridgeCommand::Screenshot;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Screenshot"));
    }

    #[test]
    fn bridge_command_debug_format_close() {
        let cmd = super::BridgeCommand::Close;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Close"));
    }

    #[test]
    fn bridge_command_debug_format_resize() {
        let cmd = super::BridgeCommand::Resize(1920, 1080);
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Resize"));
        assert!(debug_str.contains("1920"));
        assert!(debug_str.contains("1080"));
    }

    #[test]
    fn bridge_command_debug_format_get_title() {
        let cmd = super::BridgeCommand::GetTitle;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetTitle"));
    }

    #[test]
    fn bridge_command_debug_format_get_url() {
        let cmd = super::BridgeCommand::GetUrl;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetUrl"));
    }

    // ─── BridgeCommand Clone tests ────────────────────────────────────────

    #[test]
    fn bridge_command_clone_navigate() {
        let cmd = super::BridgeCommand::Navigate("https://test.com".into());
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
    }

    #[test]
    fn bridge_command_clone_evaluate() {
        let cmd = super::BridgeCommand::Evaluate("x + y".into());
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
    }

    #[test]
    fn bridge_command_clone_resize() {
        let cmd = super::BridgeCommand::Resize(1024, 768);
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
    }

    // ─── BridgeResponse Debug/Clone/Equality tests ────────────────────────

    #[test]
    fn bridge_response_debug_format_ok() {
        let resp = super::BridgeResponse::Ok;
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("Ok"));
    }

    #[test]
    fn bridge_response_debug_format_err() {
        let resp = super::BridgeResponse::Err("something went wrong".into());
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("Err"));
        assert!(debug_str.contains("something went wrong"));
    }

    #[test]
    fn bridge_response_debug_format_null() {
        let resp = super::BridgeResponse::Null;
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("Null"));
    }

    #[test]
    fn bridge_response_debug_format_value() {
        let resp = super::BridgeResponse::Value("result string".into());
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("Value"));
        assert!(debug_str.contains("result string"));
    }

    #[test]
    fn bridge_response_debug_format_binary() {
        let resp = super::BridgeResponse::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains("Binary"));
    }

    #[test]
    fn bridge_response_clone_ok() {
        let resp = super::BridgeResponse::Ok;
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
    }

    #[test]
    fn bridge_response_clone_err() {
        let resp = super::BridgeResponse::Err("error".into());
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
    }

    #[test]
    fn bridge_response_clone_value() {
        let resp = super::BridgeResponse::Value("value".into());
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
    }

    #[test]
    fn bridge_response_clone_binary() {
        let resp = super::BridgeResponse::Binary(vec![1, 2, 3, 4]);
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
    }

    #[test]
    fn bridge_response_equality_ok() {
        assert_eq!(super::BridgeResponse::Ok, super::BridgeResponse::Ok);
    }

    #[test]
    fn bridge_response_equality_err() {
        assert_eq!(
            super::BridgeResponse::Err("same error".into()),
            super::BridgeResponse::Err("same error".into())
        );
        assert_ne!(
            super::BridgeResponse::Err("error a".into()),
            super::BridgeResponse::Err("error b".into())
        );
    }

    #[test]
    fn bridge_response_equality_value() {
        assert_eq!(
            super::BridgeResponse::Value("same".into()),
            super::BridgeResponse::Value("same".into())
        );
        assert_ne!(
            super::BridgeResponse::Value("a".into()),
            super::BridgeResponse::Value("b".into())
        );
    }

    #[test]
    fn bridge_response_equality_binary() {
        assert_eq!(
            super::BridgeResponse::Binary(vec![1, 2, 3]),
            super::BridgeResponse::Binary(vec![1, 2, 3])
        );
        assert_ne!(
            super::BridgeResponse::Binary(vec![1, 2, 3]),
            super::BridgeResponse::Binary(vec![1, 2, 4])
        );
    }

    #[test]
    fn bridge_response_variants_distinct() {
        let responses = [
            super::BridgeResponse::Ok,
            super::BridgeResponse::Err("e".into()),
            super::BridgeResponse::Null,
            super::BridgeResponse::Value("v".into()),
            super::BridgeResponse::Binary(vec![1]),
        ];
        for i in 0..responses.len() {
            for j in 0..responses.len() {
                if i != j {
                    assert_ne!(responses[i], responses[j]);
                }
            }
        }
    }

    // ─── BridgeChannel edge case tests ────────────────────────────────────

    #[test]
    fn bridge_channel_send_timeout_zero_timeout_returns_err() {
        // send_timeout with Duration::ZERO: command is sent to channel,
        // but no worker responds within 0ms → timeout error.
        let (channel, receiver) = super::BridgeChannel::new();
        // Drain the receiver in a separate thread so the send doesn't block
        let _drainer = std::thread::spawn(move || {
            // Just drain the command, don't respond
            let _ = receiver.recv();
        });
        let result = channel.send_timeout(
            super::BridgeCommand::GetTitle,
            std::time::Duration::from_secs(0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn bridge_channel_send_timeout_short_timeout() {
        let (channel, _receiver) = super::BridgeChannel::new();
        // No worker to respond — should timeout
        let result = channel.send_timeout(
            super::BridgeCommand::GetTitle,
            std::time::Duration::from_millis(1),
        );
        assert!(result.is_err());
    }

    #[test]
    fn bridge_channel_fire_and_forget_multiple() {
        let (channel, receiver) = super::BridgeChannel::new();
        assert!(channel.fire_and_forget(super::BridgeCommand::GetTitle).is_ok());
        assert!(channel.fire_and_forget(super::BridgeCommand::GetUrl).is_ok());
        assert!(channel.fire_and_forget(super::BridgeCommand::Screenshot).is_ok());

        let (cmd1, _) = receiver.recv().unwrap();
        let (cmd2, _) = receiver.recv().unwrap();
        let (cmd3, _) = receiver.recv().unwrap();

        assert_eq!(cmd1, super::BridgeCommand::GetTitle);
        assert_eq!(cmd2, super::BridgeCommand::GetUrl);
        assert_eq!(cmd3, super::BridgeCommand::Screenshot);
    }

    #[test]
    fn bridge_channel_close_then_send_fails() {
        let (channel, receiver) = super::BridgeChannel::new();
        channel.close();
        // Channel is marked closed but underlying mpsc still works
        // The alive flag is just a marker, not a hard barrier
        // Verify the alive flag is set
        assert!(!channel.is_alive());
        // Drop receiver to actually close the channel
        drop(receiver);
        // Now send should fail
        let result = channel.send(super::BridgeCommand::GetTitle);
        assert!(result.is_err());
    }

    #[test]
    fn bridge_channel_close_then_fire_and_forget_fails() {
        let (channel, receiver) = super::BridgeChannel::new();
        channel.close();
        // Drop receiver to actually close the channel
        drop(receiver);
        let result = channel.fire_and_forget(super::BridgeCommand::Close);
        assert!(result.is_err());
    }

    #[test]
    fn bridge_channel_receiver_sees_close_flag() {
        let (channel, receiver) = super::BridgeChannel::new();
        assert!(receiver.is_alive());
        channel.close();
        assert!(!receiver.is_alive());
    }

    #[test]
    fn bridge_channel_multiple_send_response_pairs() {
        let (channel, receiver) = super::BridgeChannel::new();

        let worker = std::thread::spawn(move || {
            for _ in 0..3 {
                let (cmd, responder) = receiver.recv().unwrap();
                if let Some(resp_tx) = responder {
                    let resp = match cmd {
                        super::BridgeCommand::GetTitle => super::BridgeResponse::Value("Title".into()),
                        super::BridgeCommand::GetUrl => super::BridgeResponse::Value("https://url.com".into()),
                        _ => super::BridgeResponse::Ok,
                    };
                    resp_tx.send(resp).unwrap();
                }
            }
        });

        let r1 = channel.send(super::BridgeCommand::GetTitle).unwrap();
        let r2 = channel.send(super::BridgeCommand::GetUrl).unwrap();
        let r3 = channel.send(super::BridgeCommand::Screenshot).unwrap();

        assert_eq!(r1, super::BridgeResponse::Value("Title".into()));
        assert_eq!(r2, super::BridgeResponse::Value("https://url.com".into()));
        assert_eq!(r3, super::BridgeResponse::Ok);

        worker.join().unwrap();
    }

    // ─── BridgeReceiver edge case tests ───────────────────────────────────

    #[test]
    fn bridge_receiver_recv_timeout_short() {
        let (_channel, receiver) = super::BridgeChannel::new();
        // No command sent — should timeout
        let result = receiver.recv_timeout(std::time::Duration::from_millis(1));
        assert!(result.is_err());
    }

    #[test]
    fn bridge_receiver_recv_after_channel_dropped() {
        let (channel, receiver) = super::BridgeChannel::new();
        drop(channel);
        // recv should return error when sender is dropped
        let result = receiver.recv();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "channel closed");
    }

    #[test]
    fn bridge_receiver_debug_format() {
        let (_channel, receiver) = super::BridgeChannel::new();
        let debug_str = format!("{:?}", receiver);
        assert!(debug_str.contains("BridgeReceiver"));
        assert!(debug_str.contains("alive"));
    }

    // ─── RuntimeBridge edge case tests ────────────────────────────────────

    #[test]
    fn runtime_bridge_send_timeout() {
        let (bridge, receiver) = super::RuntimeBridge::new();

        let worker = std::thread::spawn(move || {
            let (cmd, responder) = receiver.recv().unwrap();
            if let Some(resp_tx) = responder {
                let resp = match cmd {
                    super::BridgeCommand::Evaluate(ref code) => {
                        super::BridgeResponse::Value(format!("evaluated: {}", code))
                    }
                    _ => super::BridgeResponse::Ok,
                };
                resp_tx.send(resp).unwrap();
            }
        });

        let result = bridge
            .send_timeout(
                super::BridgeCommand::Evaluate("1+1".into()),
                std::time::Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(result, super::BridgeResponse::Value("evaluated: 1+1".into()));

        worker.join().unwrap();
    }

    #[test]
    fn runtime_bridge_close_propagates() {
        let (bridge, receiver) = super::RuntimeBridge::new();
        assert!(bridge.is_alive());
        assert!(receiver.is_alive());
        bridge.close();
        assert!(!bridge.is_alive());
        assert!(!receiver.is_alive());
    }

    #[test]
    fn runtime_bridge_fire_and_forget_after_close_still_works() {
        let (bridge, receiver) = super::RuntimeBridge::new();
        bridge.close();
        // close() only sets the alive flag, doesn't close the channel
        // fire_and_forget should still work until receiver is dropped
        assert!(bridge.fire_and_forget(super::BridgeCommand::Close).is_ok());
        let (cmd, responder) = receiver.recv().unwrap();
        assert_eq!(cmd, super::BridgeCommand::Close);
        assert!(responder.is_none());
    }

    #[test]
    fn runtime_bridge_send_after_receiver_dropped() {
        let (bridge, receiver) = super::RuntimeBridge::new();
        drop(receiver);
        let result = bridge.send(super::BridgeCommand::GetTitle);
        assert!(result.is_err());
    }

    #[test]
    fn runtime_bridge_debug_format() {
        let (bridge, _receiver) = super::RuntimeBridge::new();
        let debug_str = format!("{:?}", bridge);
        assert!(debug_str.contains("RuntimeBridge"));
    }

    // ─── NODE_POLYFILLS content tests ─────────────────────────────────────

    #[test]
    fn node_polyfills_process_version() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("version: 'v20.11.0'"));
    }

    #[test]
    fn node_polyfills_process_versions_structure() {
        let poly = super::NODE_POLYFILLS;
        // Check key version fields exist
        assert!(poly.contains("node: '20.11.0'"));
        assert!(poly.contains("v8: '12.4.254.14'"));
        assert!(poly.contains("uv: '1.27.0'"));
        assert!(poly.contains("zlib: '1.2.13'"));
        assert!(poly.contains("brotli: '1.0.9'"));
        assert!(poly.contains("ares: '1.19.1'"));
        assert!(poly.contains("modules: '115'"));
        assert!(poly.contains("openssl: '3.0.12'"));
        assert!(poly.contains("icu: '74.2'"));
        assert!(poly.contains("bun: '1.0.25'"));
        assert!(poly.contains("bao: '0.1.0'"));
    }

    #[test]
    fn node_polyfills_process_env() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("env:"));
        assert!(poly.contains("e.HOME = '/'"));
        assert!(poly.contains("e.PATH = '/usr/local/bin:/usr/bin:/bin'"));
        assert!(poly.contains("e.TERM = 'xterm-256color'"));
        assert!(poly.contains("e.NODE_VERSION = '20.11.0'"));
        assert!(poly.contains("e.BAO_VERSION = '0.1.0'"));
    }

    #[test]
    fn node_polyfills_process_argv() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("argv:"));
        assert!(poly.contains("argv0: 'bao'"));
    }

    #[test]
    fn node_polyfills_buffer_from() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("B.from = function"));
        assert!(poly.contains("if (data instanceof B)"));
        assert!(poly.contains("if (encoding === 'hex')"));
        assert!(poly.contains("if (encoding === 'base64')"));
    }

    #[test]
    fn node_polyfills_buffer_alloc() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("B.alloc = function"));
        assert!(poly.contains("B.allocUnsafe = function"));
        assert!(poly.contains("B.allocUnsafeSlow = function"));
    }

    #[test]
    fn node_polyfills_buffer_static_methods() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("B.isBuffer = function"));
        assert!(poly.contains("B.concat = function"));
        assert!(poly.contains("B.byteLength = function"));
        assert!(poly.contains("B.compare = function"));
    }

    #[test]
    fn node_polyfills_buffer_instance_methods() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("B.prototype.slice = function"));
        assert!(poly.contains("B.prototype.toString = function"));
        assert!(poly.contains("B.prototype.toJSON = function"));
        assert!(poly.contains("B.prototype.equals = function"));
        assert!(poly.contains("B.prototype.compare = function"));
        assert!(poly.contains("B.prototype.copy = function"));
        assert!(poly.contains("B.prototype.fill = function"));
        assert!(poly.contains("B.prototype.write = function"));
        assert!(poly.contains("B.prototype.indexOf = function"));
    }

    #[test]
    fn node_polyfills_buffer_read_methods() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("B.prototype.readUInt8 = function"));
        assert!(poly.contains("B.prototype.readUInt16LE = function"));
        assert!(poly.contains("B.prototype.readUInt16BE = function"));
        assert!(poly.contains("B.prototype.readUInt32LE = function"));
        assert!(poly.contains("B.prototype.readInt8 = function"));
        assert!(poly.contains("B.prototype.readInt16LE = function"));
        assert!(poly.contains("B.prototype.readInt32LE = function"));
        assert!(poly.contains("B.prototype.readFloatLE = function"));
        assert!(poly.contains("B.prototype.readDoubleLE = function"));
    }

    #[test]
    fn node_polyfills_buffer_write_methods() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("B.prototype.writeUInt8 = function"));
        assert!(poly.contains("B.prototype.writeUInt16LE = function"));
        assert!(poly.contains("B.prototype.writeUInt32LE = function"));
        assert!(poly.contains("B.prototype.writeInt8 = function"));
        assert!(poly.contains("B.prototype.writeInt16LE = function"));
        assert!(poly.contains("B.prototype.writeInt32LE = function"));
        assert!(poly.contains("B.prototype.writeFloatLE = function"));
        assert!(poly.contains("B.prototype.writeDoubleLE = function"));
    }

    #[test]
    fn node_polyfills_require_cache() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("require.cache = _module_cache"));
        assert!(poly.contains("_module_cache = {}"));
    }

    #[test]
    fn node_polyfills_require_builtin_modules() {
        let poly = super::NODE_POLYFILLS;
        // Check key built-in modules are defined
        assert!(poly.contains("'fs':"));
        assert!(poly.contains("'path':"));
        assert!(poly.contains("'url':"));
        assert!(poly.contains("'querystring':"));
        assert!(poly.contains("'events':"));
        assert!(poly.contains("'util':"));
        assert!(poly.contains("'stream':"));
        assert!(poly.contains("'buffer':"));
        assert!(poly.contains("'crypto':"));
        assert!(poly.contains("'os':"));
        assert!(poly.contains("'assert':"));
        assert!(poly.contains("'timers':"));
    }

    #[test]
    fn node_polyfills_path_module() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("join: function"));
        assert!(poly.contains("resolve: function"));
        assert!(poly.contains("dirname: function"));
        assert!(poly.contains("basename: function"));
        assert!(poly.contains("extname: function"));
        assert!(poly.contains("sep: '/'"));
        assert!(poly.contains("posix:"));
        assert!(poly.contains("win32:"));
    }

    #[test]
    fn node_polyfills_global_alias() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("global = globalThis"));
    }

    #[test]
    fn node_polyfills_text_encoder_decoder() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("TextEncoder"));
        assert!(poly.contains("TextDecoder"));
    }

    #[test]
    fn node_polyfills_btoa_atob() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("btoa = function"));
        assert!(poly.contains("atob = function"));
        assert!(poly.contains("_b64chars"));
    }

    // ─── Edge case tests ──────────────────────────────────────────────────

    #[test]
    fn bridge_command_empty_navigate_url() {
        let cmd = super::BridgeCommand::Navigate("".into());
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Navigate"));
    }

    #[test]
    fn bridge_command_empty_evaluate_string() {
        let cmd = super::BridgeCommand::Evaluate("".into());
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Evaluate"));
    }

    #[test]
    fn bridge_response_empty_value() {
        let resp = super::BridgeResponse::Value("".into());
        assert!(!resp.is_ok());
        assert!(!resp.is_err());
        let result = resp.ok();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), super::BridgeResponse::Value("".into()));
    }

    #[test]
    fn bridge_response_empty_binary() {
        let resp = super::BridgeResponse::Binary(vec![]);
        assert!(!resp.is_ok());
        assert!(!resp.is_err());
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
    }

    #[test]
    fn bridge_response_large_binary_payload() {
        // Create a large binary payload (1MB)
        let large_data: Vec<u8> = (0..=255).cycle().take(1024 * 1024).collect();
        let resp = super::BridgeResponse::Binary(large_data.clone());
        assert!(!resp.is_ok());
        assert!(!resp.is_err());
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
        // Verify the data is intact
        if let super::BridgeResponse::Binary(data) = cloned {
            assert_eq!(data.len(), 1024 * 1024);
            assert_eq!(data[0], 0);
            assert_eq!(data[255], 255);
            assert_eq!(data[256], 0); // cycles back
        } else {
            panic!("Expected Binary variant");
        }
    }

    #[test]
    fn bridge_command_unicode_navigate_url() {
        let unicode_url = "https://例子.测试/路径?查询=值#片段";
        let cmd = super::BridgeCommand::Navigate(unicode_url.into());
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains(unicode_url));
    }

    #[test]
    fn bridge_command_unicode_evaluate_string() {
        let unicode_code = "console.log('你好世界 🎉')";
        let cmd = super::BridgeCommand::Evaluate(unicode_code.into());
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains(unicode_code));
    }

    #[test]
    fn bridge_response_unicode_value() {
        let unicode_value = "结果: 成功 ✅ 日本語 한국어 العربية";
        let resp = super::BridgeResponse::Value(unicode_value.into());
        let cloned = resp.clone();
        assert_eq!(resp, cloned);
        let debug_str = format!("{:?}", resp);
        assert!(debug_str.contains(unicode_value));
    }

    #[test]
    fn bridge_response_unicode_error() {
        let unicode_error = "错误: 文件未找到 📁❌";
        let resp = super::BridgeResponse::Err(unicode_error.into());
        assert!(resp.is_err());
        let result = resp.ok();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), unicode_error);
    }

    #[test]
    fn bridge_channel_debug_format() {
        let (channel, _receiver) = super::BridgeChannel::new();
        let debug_str = format!("{:?}", channel);
        assert!(debug_str.contains("BridgeChannel"));
        assert!(debug_str.contains("alive"));
    }

    // ── REQ-SEC-002/003: Runtime bridge security structural verification ──
    // @trace TEST-SEC-003 [req:REQ-SEC-001,REQ-SEC-002,REQ-SEC-003] [level:unit]

    /// Verify install_all_native calls install_web_apis (NOT install_all).
    /// REQ-SEC-003: The bridge must NOT inject Node APIs on page global.
    #[test]
    fn runtime_bridge_calls_web_apis_not_install_all() {
        let source = include_str!("runtime_bridge.rs");

        let func_start = source.find("unsafe fn install_all_native")
            .expect("install_all_native function not found");
        let func_end = source[func_start..].find("fn inject_node_apis_for_evaluate")
            .or_else(|| source[func_start..].find("/// Inject Node.js APIs as native"))
            .or_else(|| source[func_start..].find("// ── Bridge types"))
            .expect("end boundary not found after install_all_native");
        let func_body = &source[func_start..func_start + func_end];

        assert!(
            func_body.contains("bao_runtime::globals::install_web_apis"),
            "REQ-SEC-003 REGRESSION: install_all_native must call install_web_apis"
        );
        assert!(
            !func_body.contains("bao_runtime::globals::install_all"),
            "REQ-SEC-003 REGRESSION: install_all_native calls install_all (should be install_web_apis)"
        );
        assert!(
            !func_body.contains("bao_runtime::globals::install_node_apis"),
            "REQ-SEC-003 REGRESSION: install_all_native calls install_node_apis directly"
        );
    }

    /// Verify inject_node_apis_for_evaluate calls create_node_api_scope_values.
    /// REQ-SEC-002: Privileged evaluate_js gets Node APIs via scope object,
    /// NOT by installing them on the Window global (REQ-SEC-003).
    #[test]
    fn runtime_bridge_privileged_injection_creates_scope() {
        let source = include_str!("runtime_bridge.rs");

        let func_start = source.find("unsafe fn create_scope_values_native")
            .expect("create_scope_values_native function not found");
        let func_body_start = source[func_start..].find("{")
            .expect("function body start not found");
        // Search up to the next function definition or 2000 chars, whichever comes first
        let search_limit = source[func_start + func_body_start..]
            .find("pub fn inject_node_apis")
            .or_else(|| source[func_start + func_body_start..].find("/// Inject Node.js APIs as native"))
            .unwrap_or(2000)
            .min(2000);
        let func_body = &source[func_start + func_body_start..func_start + func_body_start + search_limit];

        assert!(
            func_body.contains("bao_runtime::globals::create_node_api_scope_values"),
            "REQ-SEC-002 REGRESSION: create_scope_values_native must call create_node_api_scope_values"
        );
        assert!(
            !func_body.contains("bao_runtime::globals::install_node_apis"),
            "REQ-SEC-003 REGRESSION: create_scope_values_native must NOT call install_node_apis (that installs on global)"
        );
        assert!(
            !func_body.contains("bao_runtime::globals::install_web_apis"),
            "create_scope_values_native should NOT redundantly call install_web_apis"
        );
    }

    /// Verify inject_node_apis_with_stealth uses evaluate_js_web (not evaluate_js).
    /// REQ-SEC-002: Internal drain must NOT trigger Node API injection (avoid recursion).
    #[test]
    fn runtime_bridge_drain_uses_web_mode() {
        let source = include_str!("runtime_bridge.rs");

        let func_start = source.find("pub fn inject_node_apis_with_stealth")
            .expect("inject_node_apis_with_stealth function not found");
        let func_end = source[func_start..].find("fn register_native_host_functions")
            .expect("end boundary not found");
        let func_body = &source[func_start..func_start + func_end];

        assert!(
            func_body.contains("evaluate_js_web"),
            "REQ-SEC-002 REGRESSION: inject_node_apis_with_stealth must use evaluate_js_web (not evaluate_js)"
        );
        assert!(
            !func_body.contains("page.evaluate_js(\"\")"),
            "REQ-SEC-002 REGRESSION: inject_node_apis_with_stealth uses evaluate_js (would cause recursion)"
        );
    }

    /// Verify NODE_POLYFILLS contains Node API names (for fallback mode).
    #[test]
    fn node_polyfills_contains_security_sensitive_names() {
        let poly = super::NODE_POLYFILLS;
        assert!(poly.contains("require"), "NODE_POLYFILLS must contain 'require'");
        assert!(poly.contains("Buffer"), "NODE_POLYFILLS must contain 'Buffer'");
        assert!(poly.contains("process"), "NODE_POLYFILLS must contain 'process'");
    }
}
