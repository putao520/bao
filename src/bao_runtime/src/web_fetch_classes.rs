// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch]
// Enhanced Fetch API classes: Headers, Request, Response
//
// These are pure-JS implementations of the WHATWG Fetch spec classes,
// installed via JS::Evaluate on the SpiderMonkey global. They replace
// the earlier minimal Rust-native constructors in fetch_api.rs with
// full-featured JS classes that match the Web API surface.
//
// ## Design decisions
//
// 1. Pure JS over Rust-native: The WHATWG Headers/Request/Response APIs
//    have many methods (forEach, entries, keys, values, clone, etc.)
//    that are tedious to implement one-by-one in Rust via mozjs FFI.
//    A JS IIFE is more maintainable and matches how Bun/Deno implement
//    these in their JS layer.
//
// 2. Headers uses a Map internally: This gives case-insensitive lookup
//    (via lowercased keys) and preserves insertion order for iteration.
//
// 3. Request/Response body storage: Bodies are stored as _bodyText (string)
//    or _bodyBytes (Uint8Array) private slots. The text/json/arrayBuffer/blob
//    methods return Promises per the WHATWG spec.

use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

/// Install the enhanced Fetch API classes (Headers, Request, Response)
/// on the global object. Called from `globals::install_web_apis`.
pub fn install_fetch_classes(
    cx: &mut mozjs::context::JSContext,
    _global: mozjs::rust::Handle<*mut JSObject>,
) {
    let src = r#"
(function() {
  var _g = globalThis;

  // ═══════════════════════════════════════════════════════════════
  // Headers — WHATWG Fetch spec
  // ═══════════════════════════════════════════════════════════════
  // Internal storage: a Map with lowercased header names as keys.
  // Values are arrays of strings (to support multiple values per name
  // for append()). get() returns the first value joined by ", " per spec.
  // set() replaces all values with a single value.

  var _bao_headers_normalise = function _bao_headers_normalise(name) {
    return String(name).toLowerCase();
  };

  var _bao_headers_value_normalise = function _bao_headers_value_normalise(value) {
    return String(value).trim();
  };

  var _bao_headers_fill = function _bao_headers_fill(headers, init) {
    if (init == null) return;
    if (typeof init === 'object') {
      // Sequence of [name, value] pairs
      if (typeof init[Symbol.iterator] === 'function' && !Array.isArray(init) && !(init instanceof _g.Headers)) {
        var iter = init[Symbol.iterator]();
        var step;
        while (!(step = iter.next()).done) {
          var pair = step.value;
          if (!pair || typeof pair[Symbol.iterator] !== 'function') {
            throw new TypeError('Header pairs must be iterable');
          }
          var pIter = pair[Symbol.iterator]();
          var p1 = pIter.next();
          var p2 = pIter.next();
          headers.append(p1.value, p2.value);
        }
      } else if (init instanceof _g.Headers) {
        // Copy from another Headers instance
        init._map.forEach(function(values, name) {
          for (var i = 0; i < values.length; i++) {
            headers.append(name, values[i]);
          }
        });
      } else {
        // Plain object
        var keys = Object.keys(init);
        for (var i = 0; i < keys.length; i++) {
          headers.append(keys[i], init[keys[i]]);
        }
      }
    }
  };

  _g.Headers = function Headers(init) {
    if (!(this instanceof _g.Headers)) return new _g.Headers(init);
    this._map = new Map();
    _bao_headers_fill(this, init);
  };

  _g.Headers.prototype.get = function get(name) {
    var n = _bao_headers_normalise(name);
    var values = this._map.get(n);
    if (values === undefined) return null;
    return values.join(', ');
  };

  _g.Headers.prototype.set = function set(name, value) {
    var n = _bao_headers_normalise(name);
    var v = _bao_headers_value_normalise(value);
    this._map.set(n, [v]);
  };

  _g.Headers.prototype.append = function append(name, value) {
    var n = _bao_headers_normalise(name);
    var v = _bao_headers_value_normalise(value);
    var existing = this._map.get(n);
    if (existing === undefined) {
      this._map.set(n, [v]);
    } else {
      existing.push(v);
    }
  };

  _g.Headers.prototype.has = function has(name) {
    return this._map.has(_bao_headers_normalise(name));
  };

  _g.Headers.prototype.delete = function _delete(name) {
    this._map.delete(_bao_headers_normalise(name));
  };

  _g.Headers.prototype.forEach = function forEach(callback, thisArg) {
    var self = this;
    this._map.forEach(function(values, name) {
      callback.call(thisArg, values.join(', '), name, self);
    });
  };

  _g.Headers.prototype.entries = function entries() {
    var pairs = [];
    this._map.forEach(function(values, name) {
      pairs.push([name, values.join(', ')]);
    });
    var idx = 0;
    return {
      next: function() {
        if (idx < pairs.length) return { value: pairs[idx++], done: false };
        return { value: undefined, done: true };
      },
      // Make iterable
      '@@iterator': function() { return this; }
    };
  };

  _g.Headers.prototype.keys = function keys() {
    var names = [];
    this._map.forEach(function(_, name) { names.push(name); });
    var idx = 0;
    return {
      next: function() {
        if (idx < names.length) return { value: names[idx++], done: false };
        return { value: undefined, done: true };
      }
    };
  };

  _g.Headers.prototype.values = function values() {
    var vals = [];
    this._map.forEach(function(values) { vals.push(values.join(', ')); });
    var idx = 0;
    return {
      next: function() {
        if (idx < vals.length) return { value: vals[idx++], done: false };
        return { value: undefined, done: true };
      }
    };
  };

  // Make Headers iterable (for..of)
  _g.Headers.prototype[Symbol.iterator] = _g.Headers.prototype.entries;

  // ═══════════════════════════════════════════════════════════════
  // Request — WHATWG Fetch spec
  // ═══════════════════════════════════════════════════════════════

  var _bao_valid_methods = /^(GET|HEAD|POST|PUT|DELETE|OPTIONS|PATCH|CONNECT|TRACE)$/i;

  var _bao_request_redirect_modes = ['follow', 'error', 'manual'];
  var _bao_request_modes = ['navigate', 'same-origin', 'no-cors', 'cors'];
  var _bao_request_credentials = ['omit', 'same-origin', 'include'];
  var _bao_request_cache = ['default', 'no-store', 'reload', 'no-cache', 'force-cache', 'only-if-cached'];

  _g.Request = function Request(input, init) {
    if (!(this instanceof _g.Request)) return new _g.Request(input, init);
    init = init || {};

    // Handle Request object as input
    if (input instanceof _g.Request) {
      this.url = input.url;
      this.method = init.method || input.method;
      this.headers = new _g.Headers(init.headers || input.headers);
      this._bodySource = init.body !== undefined ? init.body : input._bodySource;
      this.redirect = init.redirect || input.redirect;
      this.mode = init.mode || input.mode;
      this.credentials = init.credentials || input.credentials;
      this.cache = init.cache || input.cache;
      this._signal = init.signal || input._signal;
      this.referrer = input.referrer || '';
      this.referrerPolicy = input.referrerPolicy || '';
      this.integrity = input.integrity || '';
      this.destination = input.destination || '';
    } else {
      // String URL
      this.url = String(input);
      this.method = (init.method && _bao_valid_methods.test(init.method))
        ? init.method.toUpperCase()
        : 'GET';
      this.headers = new _g.Headers(init.headers);
      this._bodySource = init.body;
      this.redirect = (_bao_request_redirect_modes.indexOf(init.redirect) !== -1)
        ? init.redirect : 'follow';
      this.mode = (_bao_request_modes.indexOf(init.mode) !== -1)
        ? init.mode : 'cors';
      this.credentials = (_bao_request_credentials.indexOf(init.credentials) !== -1)
        ? init.credentials : 'same-origin';
      this.cache = (_bao_request_cache.indexOf(init.cache) !== -1)
        ? init.cache : 'default';
      this._signal = init.signal || null;
      this.referrer = init.referrer || '';
      this.referrerPolicy = init.referrerPolicy || '';
      this.integrity = init.integrity || '';
      this.destination = init.destination || '';
    }

    // Body consumed flag
    this._bodyUsed = false;

    // Store body text for text()/json() consumption
    if (this._bodySource !== undefined && this._bodySource !== null) {
      if (typeof this._bodySource === 'string') {
        this._bodyText = this._bodySource;
      } else if (this._bodySource instanceof ArrayBuffer) {
        this._bodyBytes = new Uint8Array(this._bodySource);
      } else if (ArrayBuffer.isView(this._bodySource)) {
        this._bodyBytes = new Uint8Array(
          this._bodySource.buffer,
          this._bodySource.byteOffset,
          this._bodySource.byteLength
        );
      } else if (this._bodySource instanceof _g.Blob) {
        // Blob body — will be read lazily
        this._bodyBlob = this._bodySource;
      } else if (typeof this._bodySource === 'object' && typeof this._bodySource.text === 'function') {
        // Blob-like
        this._bodyBlob = this._bodySource;
      }
    }

    // Signal getter
    Object.defineProperty(this, 'signal', {
      get: function() {
        if (!this._signal) {
          this._signal = new _g.AbortController().signal;
        }
        return this._signal;
      },
      enumerable: true
    });

    // bodyUsed getter
    Object.defineProperty(this, 'bodyUsed', {
      get: function() { return this._bodyUsed; },
      enumerable: true
    });

    // body getter (returns null or ReadableStream)
    Object.defineProperty(this, 'body', {
      get: function() {
        if (this._bodyUsed) return null;
        if (this._bodySource == null) return null;
        // Return a simple ReadableStream-like wrapper
        var bytes;
        if (this._bodyBytes) bytes = this._bodyBytes;
        else if (this._bodyText) bytes = new TextEncoder().encode(this._bodyText);
        else return null;
        return new ReadableStream({
          start: function(controller) { controller.enqueue(bytes); controller.close(); }
        });
      },
      enumerable: true
    });
  };

  _g.Request.prototype.text = function text() {
    this._bodyUsed = true;
    if (this._bodyText !== undefined) return Promise.resolve(this._bodyText);
    if (this._bodyBytes) return Promise.resolve(new TextDecoder().decode(this._bodyBytes));
    if (this._bodyBlob) return this._bodyBlob.text();
    return Promise.resolve('');
  };

  _g.Request.prototype.json = function json() {
    return this.text().then(function(t) { return JSON.parse(t); });
  };

  _g.Request.prototype.arrayBuffer = function arrayBuffer() {
    this._bodyUsed = true;
    if (this._bodyBytes) return Promise.resolve(this._bodyBytes.buffer.slice(0));
    if (this._bodyText !== undefined) return Promise.resolve(new TextEncoder().encode(this._bodyText).buffer);
    if (this._bodyBlob) return this._bodyBlob.arrayBuffer();
    return Promise.resolve(new ArrayBuffer(0));
  };

  _g.Request.prototype.blob = function blob() {
    this._bodyUsed = true;
    if (this._bodyBlob) return Promise.resolve(this._bodyBlob);
    if (this._bodyBytes) return Promise.resolve(new _g.Blob([this._bodyBytes]));
    if (this._bodyText !== undefined) return Promise.resolve(new _g.Blob([this._bodyText]));
    return Promise.resolve(new _g.Blob());
  };

  _g.Request.prototype.clone = function clone() {
    return new _g.Request(this);
  };

  // ═══════════════════════════════════════════════════════════════
  // Response — WHATWG Fetch spec
  // ═══════════════════════════════════════════════════════════════

  _g.Response = function Response(body, init) {
    if (!(this instanceof _g.Response)) return new _g.Response(body, init);
    init = init || {};

    this.type = 'default';
    this.url = init.url || '';
    this.redirected = false;
    this.status = (typeof init.status === 'number') ? init.status : 200;
    this.ok = this.status >= 200 && this.status < 300;
    this.statusText = init.statusText || '';
    this.headers = new _g.Headers(init.headers);

    // Body storage
    this._bodyUsed = false;
    this._bodySource = body;

    if (body !== undefined && body !== null) {
      if (typeof body === 'string') {
        this._bodyText = body;
      } else if (body instanceof ArrayBuffer) {
        this._bodyBytes = new Uint8Array(body);
      } else if (ArrayBuffer.isView(body)) {
        this._bodyBytes = new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
      } else if (body instanceof _g.Blob) {
        this._bodyBlob = body;
      } else if (typeof body === 'object' && typeof body.text === 'function') {
        this._bodyBlob = body;
      }
    }

    // bodyUsed getter
    Object.defineProperty(this, 'bodyUsed', {
      get: function() { return this._bodyUsed; },
      enumerable: true
    });

    // body getter
    Object.defineProperty(this, 'body', {
      get: function() {
        if (this._bodyUsed) return null;
        if (this._bodySource == null) return null;
        var bytes;
        if (this._bodyBytes) bytes = this._bodyBytes;
        else if (this._bodyText) bytes = new TextEncoder().encode(this._bodyText);
        else return null;
        return new ReadableStream({
          start: function(controller) { controller.enqueue(bytes); controller.close(); }
        });
      },
      enumerable: true
    });
  };

  _g.Response.prototype.text = function text() {
    this._bodyUsed = true;
    if (this._bodyText !== undefined) return Promise.resolve(this._bodyText);
    if (this._bodyBytes) return Promise.resolve(new TextDecoder().decode(this._bodyBytes));
    if (this._bodyBlob) return this._bodyBlob.text();
    return Promise.resolve('');
  };

  _g.Response.prototype.json = function json() {
    return this.text().then(function(t) { return JSON.parse(t); });
  };

  _g.Response.prototype.arrayBuffer = function arrayBuffer() {
    this._bodyUsed = true;
    if (this._bodyBytes) return Promise.resolve(this._bodyBytes.buffer.slice(0));
    if (this._bodyText !== undefined) return Promise.resolve(new TextEncoder().encode(this._bodyText).buffer);
    if (this._bodyBlob) return this._bodyBlob.arrayBuffer();
    return Promise.resolve(new ArrayBuffer(0));
  };

  _g.Response.prototype.blob = function blob() {
    this._bodyUsed = true;
    if (this._bodyBlob) return Promise.resolve(this._bodyBlob);
    if (this._bodyBytes) return Promise.resolve(new _g.Blob([this._bodyBytes], { type: this.headers.get('content-type') || '' }));
    if (this._bodyText !== undefined) return Promise.resolve(new _g.Blob([this._bodyText], { type: this.headers.get('content-type') || '' }));
    return Promise.resolve(new _g.Blob());
  };

  _g.Response.prototype.clone = function clone() {
    if (this._bodyUsed) {
      throw new TypeError('Cannot clone a used Response');
    }
    var cloned = new _g.Response(this._bodySource, {
      status: this.status,
      statusText: this.statusText,
      headers: this.headers,
      url: this.url
    });
    cloned.type = this.type;
    cloned.redirected = this.redirected;
    cloned.ok = this.ok;
    // Copy internal body storage
    if (this._bodyText !== undefined) cloned._bodyText = this._bodyText;
    if (this._bodyBytes) cloned._bodyBytes = this._bodyBytes;
    if (this._bodyBlob) cloned._bodyBlob = this._bodyBlob;
    return cloned;
  };

  // Static: Response.error()
  _g.Response.error = function error() {
    var resp = new _g.Response(null, { status: 0, statusText: '' });
    resp.type = 'error';
    resp.ok = false;
    return resp;
  };

  // Static: Response.redirect(url, status)
  _g.Response.redirect = function redirect(url, status) {
    var redirectStatuses = [301, 302, 303, 307, 308];
    if (arguments.length < 1) throw new TypeError('Response.redirect requires a URL');
    var s = (typeof status === 'number') ? status : 302;
    if (redirectStatuses.indexOf(s) === -1) {
      throw new RangeError('Invalid status code for redirect: ' + s);
    }
    var resp = new _g.Response(null, {
      status: s,
      headers: { 'Location': String(url) }
    });
    resp.type = 'opaqueredirect';
    resp.ok = false;
    resp.redirected = true;
    return resp;
  };

  // Static: Response.json(body, init)
  _g.Response.json = function json(body, init) {
    init = init || {};
    var jsonStr = (body === undefined) ? 'null' : JSON.stringify(body);
    var headers = new _g.Headers(init.headers);
    if (!headers.has('content-type')) {
      headers.set('content-type', 'application/json');
    }
    var resp = new _g.Response(jsonStr, {
      status: init.status || 200,
      statusText: init.statusText || '',
      headers: headers
    });
    return resp;
  };

})();
"#;
    unsafe {
        let raw = cx.raw_cx();
        let mut rval = UndefinedValue();
        let opts = mozjs::glue::NewCompileOptions(
            raw,
            c"fetch_classes".as_ptr(),
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
