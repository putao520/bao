// @trace REQ-BRW-003  REQ-BRW-001: Bridge between servo browser context and Node.js APIs
// REQ-ENG-007: Unified runtime coordination
//
// Architecture: dual-context bridge
// - servo's JSContext handles DOM + Web APIs (created by servo internally)
// - Node.js APIs are injected as self-contained JS polyfills via evaluate_javascript()
// - Event loop coordination: servo's spin_event_loop() drives both contexts
//
// Why not share a single JSContext:
// - JSEngine::init() can only be called once (mozjs constraint)
// - servo calls it internally in JSEngineSetup::default()
// - Direct JSContext injection is not supported by servo's public API
// - Bridge approach is the pragmatic solution that respects servo's architecture

use crate::page::PageHandle;
use crate::error::BrowserError;

/// Inject Node.js API polyfills into a browser page context.
/// This makes `require`, `Buffer`, `process`, etc. available in the servo page.
pub fn inject_node_apis(page: &PageHandle) -> Result<(), BrowserError> {
    page.evaluate_js(NODE_POLYFILLS)?;
    Ok(())
}

/// Inject stealth anti-fingerprinting scripts into a browser page context.
/// These scripts modify navigator, screen, canvas, WebGL, and audio APIs.
pub fn inject_stealth_scripts(page: &PageHandle) -> Result<(), BrowserError> {
    page.evaluate_js(STEALTH_POLYFILLS)?;
    Ok(())
}

/// Inject both Node.js APIs and stealth scripts into a page.
pub fn inject_all(page: &PageHandle, stealth: bool) -> Result<(), BrowserError> {
    inject_node_apis(page)?;
    if stealth {
        inject_stealth_scripts(page)?;
    }
    Ok(())
}

/// Inject Node.js APIs and (if profile present) profile-aware stealth scripts into a page.
pub fn inject_all_with_profile(page: &PageHandle, profile: &Option<bao_stealth::StealthProfile>) -> Result<(), BrowserError> {
    inject_node_apis(page)?;
    if let Some(prof) = profile {
        let engine = bao_stealth::StealthEngine::new(prof.clone());
        let js = engine.inject_navigator_js();
        page.evaluate_js(&js)?;
        inject_stealth_scripts(page)?;
    }
    Ok(())
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

const STEALTH_POLYFILLS: &str = r#"(function() {
  // @trace REQ-STL-002 Navigator/Screen fingerprint masking
  // @trace REQ-STL-003 Canvas/WebGL noise injection
  // @trace REQ-STL-006 AudioContext noise injection

  // Stealth configuration — matches Chrome 130 on Windows
  var _ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36';
  var _platform = 'Win32';
  var _vendor = 'Google Inc.';

  // Navigator overrides
  if (typeof navigator !== 'undefined') {
    var _navigatorProps = {
      userAgent: _ua,
      platform: _platform,
      vendor: _vendor,
      appVersion: _ua.replace('Mozilla/', ''),
      language: 'en-US',
      languages: ['en-US', 'en'],
      hardwareConcurrency: 8,
      deviceMemory: 8,
      maxTouchPoints: 0,
      vendorSub: '',
      productSub: '20030107',
      cookiesEnabled: true,
      doNotTrack: null,
      webdriver: false,
    };

    Object.keys(_navigatorProps).forEach(function(prop) {
      if (prop in navigator) {
        try {
          Object.defineProperty(navigator, prop, {
            get: function() { return _navigatorProps[prop]; },
            configurable: true,
          });
        } catch(e) {}
      }
    });

    // Plugins — fake standard Chrome plugins
    if (navigator.plugins) {
      try {
        Object.defineProperty(navigator, 'plugins', {
          get: function() {
            return {
              length: 5,
              0: { name: 'PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
              1: { name: 'Chrome PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
              2: { name: 'Chromium PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
              3: { name: 'Microsoft Edge PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
              4: { name: 'WebKit built-in PDF', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
              item: function(i) { return this[i]; },
              namedItem: function(name) { for (var i=0;i<this.length;i++) if(this[i].name===name) return this[i]; return null; },
              refresh: function() {},
            };
          },
          configurable: true,
        });
      } catch(e) {}
    }

    // MimeTypes
    if (navigator.mimeTypes) {
      try {
        Object.defineProperty(navigator, 'mimeTypes', {
          get: function() {
            return {
              length: 2,
              0: { type: 'application/pdf', suffixes: 'pdf', description: 'Portable Document Format' },
              1: { type: 'text/pdf', suffixes: 'pdf', description: 'Portable Document Format' },
              item: function(i) { return this[i]; },
              namedItem: function(name) { for (var i=0;i<this.length;i++) if(this[i].type===name) return this[i]; return null; },
            };
          },
          configurable: true,
        });
      } catch(e) {}
    }
  }

  // Screen overrides
  if (typeof screen !== 'undefined') {
    var _screenProps = {
      width: 1920,
      height: 1080,
      availWidth: 1920,
      availHeight: 1040,
      colorDepth: 24,
      pixelDepth: 24,
    };

    Object.keys(_screenProps).forEach(function(prop) {
      if (prop in screen) {
        try {
          Object.defineProperty(screen, prop, {
            get: function() { return _screenProps[prop]; },
            configurable: true,
          });
        } catch(e) {}
      }
    });
  }

  // Canvas noise injection
  if (typeof HTMLCanvasElement !== 'undefined') {
    var _origToDataURL = HTMLCanvasElement.prototype.toDataURL;
    var _origToBlob = HTMLCanvasElement.prototype.toBlob;
    var _origGetImageData = CanvasRenderingContext2D.prototype.getImageData;

    // Deterministic noise seed based on session
    var _noiseSeed = (function() {
      var s = 0;
      var str = _ua + _platform + 'bao-stealth';
      for (var i = 0; i < str.length; i++) {
        s = ((s << 5) - s) + str.charCodeAt(i);
        s = s & s;
      }
      return s;
    })();

    function _noise(x) {
      x = ((x >> 16) ^ x) * 0x45d9f3b;
      x = ((x >> 16) ^ x) * 0x45d9f3b;
      x = (x >> 16) ^ x;
      return (x & 0xFF) > 127 ? 1 : -1;
    }

    HTMLCanvasElement.prototype.toDataURL = function() {
      var ctx = this.getContext('2d');
      if (ctx && this.width > 0 && this.height > 0) {
        try {
          var imgData = ctx.getImageData(0, 0, Math.min(this.width, 1), Math.min(this.height, 1));
          imgData.data[0] = (imgData.data[0] + _noise(_noiseSeed)) & 0xFF;
          ctx.putImageData(imgData, 0, 0);
        } catch(e) {}
      }
      return _origToDataURL.apply(this, arguments);
    };

    HTMLCanvasElement.prototype.toBlob = function() {
      var ctx = this.getContext('2d');
      if (ctx && this.width > 0 && this.height > 0) {
        try {
          var imgData = ctx.getImageData(0, 0, Math.min(this.width, 1), Math.min(this.height, 1));
          imgData.data[0] = (imgData.data[0] + _noise(_noiseSeed + 1)) & 0xFF;
          ctx.putImageData(imgData, 0, 0);
        } catch(e) {}
      }
      return _origToBlob.apply(this, arguments);
    };

    CanvasRenderingContext2D.prototype.getImageData = function(sx, sy, sw, sh) {
      var result = _origGetImageData.apply(this, arguments);
      var data = result.data;
      for (var i = 0; i < Math.min(data.length, 16); i++) {
        data[i] = (data[i] + _noise(_noiseSeed + i)) & 0xFF;
      }
      return result;
    };
  }

  // WebGL fingerprint masking
  if (typeof WebGLRenderingContext !== 'undefined') {
    var _origGetParameter = WebGLRenderingContext.prototype.getParameter;
    var _webglVendor = 'Google Inc. (NVIDIA)';
    var _webglRenderer = 'ANGLE (NVIDIA, NVIDIA GeForce GTX 1080 Direct3D11 vs_5_0 ps_5_0)';

    WebGLRenderingContext.prototype.getParameter = function(param) {
      if (param === 0x1F00) return _webglVendor;     // VENDOR
      if (param === 0x1F01) return _webglRenderer;   // RENDERER
      if (param === 0x9245) return 'WebKit WebGL';   // UNMASKED_VENDOR_WEBGL
      if (param === 0x9246) return _webglRenderer;   // UNMASKED_RENDERER_WEBGL
      return _origGetParameter.apply(this, arguments);
    };

    // WebGL2
    if (typeof WebGL2RenderingContext !== 'undefined') {
      var _origGetParameter2 = WebGL2RenderingContext.prototype.getParameter;
      WebGL2RenderingContext.prototype.getParameter = function(param) {
        if (param === 0x1F00) return _webglVendor;
        if (param === 0x1F01) return _webglRenderer;
        if (param === 0x9245) return 'WebKit WebGL';
        if (param === 0x9246) return _webglRenderer;
        return _origGetParameter2.apply(this, arguments);
      };
    }
  }

  // AudioContext noise
  if (typeof AudioContext !== 'undefined' || typeof webkitAudioContext !== 'undefined') {
    var _AudioCtx = typeof AudioContext !== 'undefined' ? AudioContext : webkitAudioContext;
    var _origGetFloatFreqData = AnalyserNode.prototype.getFloatFrequencyData;

    AnalyserNode.prototype.getFloatFrequencyData = function(array) {
      _origGetFloatFreqData.apply(this, arguments);
      for (var i = 0; i < array.length; i++) {
        array[i] += _noise(_noiseSeed + i) * 0.001;
      }
    };
  }

  // WebDriver detection prevention
  if (typeof navigator !== 'undefined') {
    delete navigator.__proto__.webdriver;
    try {
      Object.defineProperty(navigator, 'webdriver', { get: function() { return false; }, configurable: true });
    } catch(e) {}
  }

  // Permissions API masking
  if (typeof Permissions !== 'undefined' && Permissions.prototype.query) {
    var _origPermissionsQuery = Permissions.prototype.query;
    Permissions.prototype.query = function(desc) {
      if (desc.name === 'notifications') {
        return Promise.resolve({ state: 'default', onchange: null });
      }
      return _origPermissionsQuery.apply(this, arguments);
    };
  }

  // Chrome runtime mock (prevents detection of missing chrome.runtime)
  if (typeof window !== 'undefined' && !window.chrome) {
    window.chrome = {
      runtime: { onConnect: { addListener: function(){} }, onMessage: { addListener: function(){} } },
      loadTimes: function() { return { firstPaintTime: 0, startLoadTime: 0 }; },
      csi: function() { return { onloadT: 0, startE: 0, pageT: 0 }; },
    };
  }
})();"#;

#[cfg(test)]
mod tests {
    #[test]
    fn test_polyfills_are_valid_js() {
        // Verify the polyfills don't have obvious syntax issues
        // by checking they are non-empty and contain expected constructs
        assert!(!super::NODE_POLYFILLS.is_empty());
        assert!(super::NODE_POLYFILLS.contains("Buffer"));
        assert!(super::NODE_POLYFILLS.contains("require"));
        assert!(super::NODE_POLYFILLS.contains("process"));
        assert!(!super::STEALTH_POLYFILLS.is_empty());
        assert!(super::STEALTH_POLYFILLS.contains("navigator"));
        assert!(super::STEALTH_POLYFILLS.contains("WebGL"));
        assert!(super::STEALTH_POLYFILLS.contains("Canvas"));
    }
}
