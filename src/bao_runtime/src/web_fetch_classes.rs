// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch]
// Full WHATWG Fetch API classes: Headers, Request, Response.
//
// Pure-JS implementations installed via JS::Evaluate on the SpiderMonkey
// global (globals::install_web_apis). These are the live Headers/Request/
// Response constructors; fetch_api::fetch_fn consumes their instance shape
// (url/method/headers/_bodyText/_bodyBytes/_bodyBlob) when given a Request
// object as input.
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
//
// 4. Method handling: the constructor validates init.method against the
//    bun_http::Method table (IANA method registry) and THROWS on unknown
//    tokens — no silent GET fallback. WHATWG permits arbitrary method
//    tokens, but the Rust wire layer (AsyncHTTP takes the closed Method
//    enum) cannot serialize them; an explicit error is honest where a
//    silent fallback would misroute the request.

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
    if (init instanceof _g.Headers) {
      // Copy from another Headers instance
      init._map.forEach(function(values, name) {
        for (var i = 0; i < values.length; i++) {
          headers.append(name, values[i]);
        }
      });
      return;
    }
    // Sequence of [name, value] pairs — arrays and any iterable (Headers
    // fill per WHATWG: sequence<sequence<ByteString>>). Plain objects fall
    // through to the record branch below.
    var isIterable = typeof init[Symbol.iterator] === 'function';
    if (isIterable) {
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
      return;
    }
    // Plain object
    var keys = Object.keys(init);
    for (var i = 0; i < keys.length; i++) {
      headers.append(keys[i], init[keys[i]]);
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

  // Method table mirroring bun_http::Method (the IANA HTTP method registry
  // tokens the Rust wire layer can serialize). A method outside this table
  // throws instead of silently falling back to GET — the wire layer has no
  // arbitrary-token passthrough, and a silent GET would misroute the request
  // (e.g. a WebDAV PROPFIND answered by a GET handler).
  var _bao_method_table = 'ACL BIND CHECKOUT CONNECT COPY DELETE GET HEAD LINK LOCK M-SEARCH MERGE MKACTIVITY MKADDRESSBOOK MKCALENDAR MKCOL MOVE NOTIFY OPTIONS PATCH POST PROPFIND PROPPATCH PURGE PUT QUERY REBIND REPORT SEARCH SOURCE SUBSCRIBE TRACE UNBIND UNLINK UNLOCK UNSUBSCRIBE'.split(' ');
  var _bao_method_set = Object.create(null);
  for (var _mi = 0; _mi < _bao_method_table.length; _mi++) {
    _bao_method_set[_bao_method_table[_mi]] = true;
  }

  var _bao_normalise_method = function _bao_normalise_method(m) {
    var up = String(m).toUpperCase();
    if (!_bao_method_set[up]) {
      throw new TypeError('Failed to construct \'Request\': HTTP method "' + up + '" is not supported by the Bao HTTP wire layer (supported: IANA method registry tokens such as GET/POST/PROPFIND/REPORT).');
    }
    return up;
  };

  // URLSearchParams body → application/x-www-form-urlencoded string.
  // Structural probe: the runtime's URLSearchParams is a native constructor
  // without a resolvable .prototype, so `instanceof` throws
  // (JSMSG_BAD_PROTOTYPE: "'prototype' property ... is not an object") and
  // .constructor identity resolves to Object. The method surface
  // append+getAll+entries+forEach is the discriminator — FormData ALSO has
  // all four since gaining its WHATWG iteration surface, so any classifier
  // MUST probe FormData (_bao_is_formdata) BEFORE this predicate; Map lacks
  // append/getAll, Blob lacks all four.
  var _bao_is_urlsearchparams = function _bao_is_urlsearchparams(v) {
    return !!v && typeof v === 'object'
      && typeof v.append === 'function'
      && typeof v.getAll === 'function'
      && typeof v.entries === 'function'
      && typeof v.forEach === 'function'
      && !Array.isArray(v._data);
  };

  // FormData body — the live object is parked on _bodyFormData; the native
  // fetch layer (fetch_api.rs extract_formdata_multipart) serializes it to
  // multipart/form-data at send time (the boundary is generated there).
  var _bao_is_formdata = function _bao_is_formdata(v) {
    if (typeof _g.FormData === 'function' && v instanceof _g.FormData) return true;
    return v && typeof v === 'object' && Array.isArray(v._data) && typeof v.getAll === 'function';
  };

  var _bao_is_blob = function _bao_is_blob(v) {
    if (typeof _g.Blob === 'function' && v instanceof _g.Blob) return true;
    return v && typeof v === 'object' && typeof v.size === 'number' && typeof v.arrayBuffer === 'function';
  };

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
      this.method = (init.method !== undefined) ? _bao_normalise_method(init.method) : input.method;
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
      this.method = (init.method !== undefined) ? _bao_normalise_method(init.method) : 'GET';
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
      } else if (_bao_is_formdata(this._bodySource)) {
        // BEFORE the URLSearchParams probe: FormData's WHATWG iteration
        // surface (entries/forEach) also satisfies _bao_is_urlsearchparams.
        // The live object is parked here; fetch() serializes it (boundary
        // generated at send time by the native multipart encoder).
        this._bodyFormData = this._bodySource;
      } else if (_bao_is_urlsearchparams(this._bodySource)) {
        // Serialize eagerly so fetch(Request) can read _bodyText synchronously.
        this._bodyText = this._bodySource.toString();
        if (!this.headers.has('content-type')) {
          this.headers.set('content-type', 'application/x-www-form-urlencoded;charset=UTF-8');
        }
      } else if (_bao_is_blob(this._bodySource)) {
        // Blob body — will be read lazily
        this._bodyBlob = this._bodySource;
      } else {
        throw new TypeError('Failed to construct \'Request\': unsupported body type ' + Object.prototype.toString.call(this._bodySource) + ' (expected string / ArrayBuffer / typed array / Blob / URLSearchParams).');
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
    if (this._bodyFormData) {
      return Promise.reject(new TypeError('Request body is FormData: multipart consumption via text()/json()/arrayBuffer()/blob() is not wired (fetch() sends FormData bodies).'));
    }
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
      } else if (_bao_is_formdata(body)) {
        // BEFORE the URLSearchParams probe (FormData's iteration surface
        // satisfies it). Fail-closed: Response-side multipart consumption
        // (text()/json()) is not wired; only fetch/Request send paths
        // serialize FormData. A toString() fallback would corrupt the body.
        throw new TypeError('Failed to construct \'Response\': FormData response bodies are not supported (multipart serialization is wired for fetch/Request send paths only).');
      } else if (_bao_is_urlsearchparams(body)) {
        this._bodyText = body.toString();
      } else if (typeof body === 'object' && typeof body.text === 'function') {
        this._bodyBlob = body;
      }
    }

    // bodyUsed getter
    Object.defineProperty(this, 'bodyUsed', {
      get: function() { return this._bodyUsed; },
      enumerable: true
    });

    // body getter — streaming source branch first: `_bodyStreamSource` is a
    // native holder (installed by fetch_async's streaming resolve) whose
    // pull/cancel feed a WHATWG ReadableStream. The strategy is
    // `{ highWaterMark: 1 }` with the DEFAULT count size (one-chunk
    // lookahead): this port's `_readableStreamDefaultControllerRead`
    // empty-queue branch adds the read request WITHOUT invoking the
    // controller's pull steps, so a `{ highWaterMark: 0 }` stream stalls
    // after its first parked pull. With count-hwm 1 the invariant "a read
    // against an empty queue always has a pull in flight" holds (every
    // dequeue re-fills the one-chunk lookahead), and an UNREAD stream keeps
    // at most the initial lookahead queued — no parked pulls — so the
    // fetch-side park (unobserved staging ≥ high-water) still engages.
    // The constructed stream is CACHED on the instance: the WHATWG body
    // getter must return the same stream object on every access (a fresh
    // stream per access would double-consume a live body).
    Object.defineProperty(this, 'body', {
      get: function() {
        if (this._bodyStreamSource) {
          if (!this._bodyStream) {
            var src = this._bodyStreamSource;
            this._bodyStream = new ReadableStream({
              pull: function(controller) {
                return __baoFetchBodyPull(src).then(function(r) {
                  if (r.done) controller.close();
                  else controller.enqueue(r.value);
                });
              },
              cancel: function(reason) {
                return __baoFetchBodyCancel(src);
              }
            }, { highWaterMark: 1 });
          }
          return this._bodyStream;
        }
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

  // Streaming-body reader pump: drains the native pull stream into an
  // accumulated result. `collect` receives each Uint8Array chunk and the
  // accumulated state; returns the final state at done.
  var _bao_drain_stream = function _bao_drain_stream(response, collect, init) {
    var reader = response.body.getReader();
    var acc = init;
    function pump() {
      return reader.read().then(function(r) {
        if (r.done) return acc;
        acc = collect(acc, r.value);
        return pump();
      });
    }
    return pump();
  };

  _g.Response.prototype.text = function text() {
    if (this._bodyStreamSource) {
      if (this._bodyUsed) return Promise.reject(new TypeError('Body is unusable'));
      this._bodyUsed = true;
      var dec = new TextDecoder();
      var self = this;
      return _bao_drain_stream(self, function(parts, chunk) {
        parts.push(dec.decode(chunk, { stream: true }));
        return parts;
      }, []).then(function(parts) {
        var tail = dec.decode();
        if (tail) parts.push(tail);
        return parts.join('');
      });
    }
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
    if (this._bodyStreamSource) {
      if (this._bodyUsed) return Promise.reject(new TypeError('Body is unusable'));
      this._bodyUsed = true;
      return _bao_drain_stream(this, function(chunks, chunk) {
        chunks.push(chunk);
        return chunks;
      }, []).then(function(chunks) {
        var total = 0;
        for (var i = 0; i < chunks.length; i++) total += chunks[i].byteLength;
        var out = new Uint8Array(total);
        var off = 0;
        for (var j = 0; j < chunks.length; j++) {
          out.set(chunks[j], off);
          off += chunks[j].byteLength;
        }
        return out.buffer;
      });
    }
    this._bodyUsed = true;
    if (this._bodyBytes) return Promise.resolve(this._bodyBytes.buffer.slice(0));
    if (this._bodyText !== undefined) return Promise.resolve(new TextEncoder().encode(this._bodyText).buffer);
    if (this._bodyBlob) return this._bodyBlob.arrayBuffer();
    return Promise.resolve(new ArrayBuffer(0));
  };

  _g.Response.prototype.blob = function blob() {
    if (this._bodyStreamSource) {
      var type = this.headers.get('content-type') || '';
      return this.arrayBuffer().then(function(buf) {
        return new _g.Blob([new Uint8Array(buf)], { type: type });
      });
    }
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
    if (this._bodyStreamSource) {
      // A streaming body is a single-consumer live transport stream — it
      // cannot be duplicated (WHATWG clone would tee, which needs two
      // independent consumers of one socket).
      throw new TypeError('Cannot clone a Response with a streaming body');
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
        // Same-phase WebCrypto/SSE/storage surfaces — installed here (the tail
        // of the web-API phase) so crypto.subtle lands after
        // globals::install_crypto_global created the shared subtle object, and
        // EventSource/localStorage see fetch/timers already installed.
        crate::web_api::install_crypto_subtle(cx, _global);
        crate::web_api::install_event_source(cx, _global);
        crate::web_api::install_local_storage(cx, _global);

        let raw = cx.raw_cx();
        let mut rval = UndefinedValue();
        let opts = mozjs::glue::NewCompileOptions(
            raw,
            c"fetch_classes".as_ptr(),
            1,
        );
        if !opts.is_null() {
            let mut src_text = mozjs::rust::transform_str_to_source_text(src);
            // BCE (P0 browser startup panic, servo error.rs:74): a failed
            // evaluation returns false WITH the thrown exception pending on
            // the context. This install runs on servo's ScriptThread inside
            // page init; an unconsumed pending exception detonates servo's
            // `assert!(!JS_IsExceptionPending)` in `throw_dom_exception` on
            // the next error path, killing the ScriptThread (browser dies at
            // startup, CDP never listens). The classes this blob fails to
            // install are absent; the failure itself is a handled outcome —
            // consume the exception, never leak it into servo's loop.
            let evaluated = mozjs_sys::jsapi::JS::Evaluate2(
                raw,
                opts,
                &mut src_text,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            if !evaluated {
                JS_ClearPendingException(raw);
            }
            libc::free(opts as *mut _);
        }
    }
}
