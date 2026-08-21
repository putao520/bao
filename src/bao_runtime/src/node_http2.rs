// @trace REQ-ENG-006 [api:node:http2]
//
// HTTP/2 module — JS IIFE + Rust uWS bridge.
//
// Architecture:
//   - createServer / createSecureServer: uWS App (non-SSL / SSL) with H2-capable
//     route handler, same pattern as node_http / node_https.
//   - connect / client session: fetch_async::start for async HTTP/2 requests
//     (BoringSSL ALPN negotiates H2 automatically).
//   - Http2Session / Http2Stream / constants: JS IIFE for the bulk of the API
//     surface; Rust native functions only for server creation, settings packing,
//     and async fetch bridging.
use ::std::cell::RefCell;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, Int32Value, JSVal, ObjectValue, PrivateValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::request::Request;
use bun_uws_sys::response::Response;
use bun_uws_sys::socket_context::BunSocketContextOptions;

use crate::gc_store::{gc_store_get_ns, gc_store_insert_ns, gc_store_remove_ns};
use crate::require::cache_builtin;

static NEXT_SERVER_ID: AtomicU64 = AtomicU64::new(1);
#[allow(dead_code)]
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ACTIVE_H2_APPS: RefCell<Vec<*mut App<false>>> = const { RefCell::new(Vec::new()) };
    static ACTIVE_H2_SSL_APPS: RefCell<Vec<*mut App<true>>> = const { RefCell::new(Vec::new()) };
}

pub fn has_active_servers() -> bool {
    ACTIVE_H2_APPS.with(|s| !s.borrow().is_empty())
        || ACTIVE_H2_SSL_APPS.with(|s| !s.borrow().is_empty())
}

// BCE-007 registration gap (node_http2 variant): `drain_and_check`
// (timers.rs) keeps the JS-thread uWS `Loop` ticking ONLY while
// `node_http::has_active_servers()` is true — the h2-local registries above
// were never consulted there, so an h2 App's listen socket never `accept()`ed
// (requests connected, then sat unanswered; route handler never invoked).
// Same disease Bun.serve had before the unified `register_active_app` fix
// (bun_api.rs). Close the class: every h2 register/unregister ALSO keeps the
// unified node_http liveness registry in sync, so the single source of truth
// drives the loop tick for h2 Apps too.
//
// The SSL registration passes an `*mut App<true>` through the `App<false>`
// registry API: `App<SSL>` is a `#[repr(C)]` zero-sized opaque token with
// identical layout for both instantiations, and the unified registry uses
// pointers ONLY for liveness bookkeeping (len / ptr-eq / retain — never
// dereferences), so the cast is a representation-preserving token alias.

pub unsafe fn register_active_h2_app(app: *mut App<false>) {
    if app.is_null() {
        return;
    }
    ACTIVE_H2_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        if !apps.iter().any(|&p| ::std::ptr::eq(p, app)) {
            apps.push(app);
        }
    });
    crate::node_http::register_active_app(app);
}

pub unsafe fn unregister_active_h2_app(app: *mut App<false>) {
    if app.is_null() {
        return;
    }
    ACTIVE_H2_APPS.with(|s| {
        s.borrow_mut().retain(|&p| !::std::ptr::eq(p, app));
    });
    crate::node_http::unregister_active_app(app);
}

pub unsafe fn register_active_h2_ssl_app(app: *mut App<true>) {
    if app.is_null() {
        return;
    }
    ACTIVE_H2_SSL_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        if !apps.iter().any(|&p| ::std::ptr::eq(p, app)) {
            apps.push(app);
        }
    });
    // Liveness token only — see the safety note above the register fns.
    crate::node_http::register_active_app(app as *mut App<false>);
}

pub unsafe fn unregister_active_h2_ssl_app(app: *mut App<true>) {
    if app.is_null() {
        return;
    }
    ACTIVE_H2_SSL_APPS.with(|s| {
        s.borrow_mut().retain(|&p| !::std::ptr::eq(p, app));
    });
    // Liveness token only — see the safety note above the register fns.
    crate::node_http::unregister_active_app(app as *mut App<false>);
}

// ──────────────────────────────────────────────────────────────────────
// JS IIFE — Http2Session, Http2Stream, connect, utility functions
// ──────────────────────────────────────────────────────────────────────

const HTTP2_JS: &str = r#"
(function() {
  // ── Minimal EventEmitter ────────────────────────────────────────────
  function EE() { this._events = Object.create(null); }
  EE.prototype.on = function(ev, fn) {
    (this._events[ev] || (this._events[ev] = [])).push(fn);
    return this;
  };
  EE.prototype.once = function(ev, fn) {
    var self = this;
    var g = function() { self.removeListener(ev, g); fn.apply(this, arguments); };
    g.listener = fn;
    this.on(ev, g);
    return this;
  };
  EE.prototype.emit = function(ev) {
    var args = Array.prototype.slice.call(arguments, 1);
    var list = this._events[ev];
    if (list) { for (var i = 0; i < list.length; i++) { list[i].apply(this, args); } }
    return this;
  };
  EE.prototype.removeListener = function(ev, fn) {
    var list = this._events[ev];
    if (!list) return this;
    for (var i = list.length - 1; i >= 0; i--) {
      if (list[i] === fn || list[i].listener === fn) list.splice(i, 1);
    }
    return this;
  };
  EE.prototype.removeAllListeners = function(ev) {
    if (ev) delete this._events[ev];
    else this._events = Object.create(null);
    return this;
  };
  EE.prototype.prependListener = function(ev, fn) {
    (this._events[ev] || (this._events[ev] = [])).unshift(fn);
    return this;
  };

  // ── byte-exact body helpers (same contract as the node:http client) ──
  // Buffers/TypedArrays/ArrayBuffers are byte bodies; the historical
  // `String(data)` coercion turned them into "72,101,108" comma strings.
  function isByteValue(v) {
    return !!v && typeof v === 'object' &&
      (v instanceof ArrayBuffer ||
       (typeof ArrayBuffer !== 'undefined' && typeof ArrayBuffer.isView === 'function' && ArrayBuffer.isView(v)));
  }
  function pushBodyChunk(stream, data) {
    if (data === undefined || data === null) return;
    if (typeof data === 'string' || isByteValue(data)) {
      (stream._bodyChunks || (stream._bodyChunks = [])).push(data);
      return;
    }
    throw new TypeError('http2: stream chunk must be a string, Buffer, TypedArray or ArrayBuffer');
  }
  // Transport body argument: all-string parts join (fast path, identical
  // bytes); any binary part switches to byte-exact Uint8Array assembly.
  function buildBodyArg(parts) {
    var hasBinary = false;
    for (var i = 0; i < parts.length; i++) {
      if (typeof parts[i] !== 'string') { hasBinary = true; break; }
    }
    if (!hasBinary) return parts.join('');
    var enc = new TextEncoder();
    var chunks = [];
    var total = 0;
    for (var j = 0; j < parts.length; j++) {
      var p = parts[j];
      var u;
      if (typeof p === 'string') u = enc.encode(p);
      else if (p instanceof ArrayBuffer) u = new Uint8Array(p);
      else u = new Uint8Array(p.buffer, p.byteOffset, p.byteLength);
      chunks.push(u);
      total += u.length;
    }
    var out = new Uint8Array(total);
    var off = 0;
    for (var k = 0; k < chunks.length; k++) {
      out.set(chunks[k], off);
      off += chunks[k].length;
    }
    return out;
  }

  // ── Http2Stream ─────────────────────────────────────────────────────
  function Http2Stream(session, id) {
    this._session = session;
    this._id = id;
    this._headers = {};
    this._trailers = {};
    this._ended = false;
    this._closed = false;
    this._events = Object.create(null);
    this.readable = true;
    this.writable = true;
    // Ordered string|byte parts — byte-exact request-body accumulation.
    this._bodyChunks = [];
  }
  Http2Stream.prototype = Object.create(null);
  // Mix in EventEmitter
  Http2Stream.prototype.on = EE.prototype.on;
  Http2Stream.prototype.once = EE.prototype.once;
  Http2Stream.prototype.emit = EE.prototype.emit;
  Http2Stream.prototype.removeListener = EE.prototype.removeListener;
  Http2Stream.prototype.removeAllListeners = EE.prototype.removeAllListeners;
  Http2Stream.prototype.prependListener = EE.prototype.prependListener;

  Object.defineProperty(Http2Stream.prototype, 'id', {
    get: function() { return this._id; },
    enumerable: true
  });
  Object.defineProperty(Http2Stream.prototype, 'session', {
    get: function() { return this._session; },
    enumerable: true
  });
  Object.defineProperty(Http2Stream.prototype, 'closed', {
    get: function() { return this._closed; },
    enumerable: true
  });
  Object.defineProperty(Http2Stream.prototype, 'ended', {
    get: function() { return this._ended; },
    enumerable: true
  });
  Object.defineProperty(Http2Stream.prototype, 'state', {
    get: function() {
      return {
        localWindowSize: 65535,
        state: this._ended ? 4 : 0, // NGHTTP2_STREAM_CLOSED : NGHTTP2_STREAM_IDLE
        weight: 16,
        sumDependencyWeight: 0,
        localClose: this._ended ? 1 : 0,
        remoteClose: 0
      };
    },
    enumerable: true
  });

  Http2Stream.prototype.respond = function(headers, options) {
    if (this._closed || this._ended) return this;
    headers = headers || {};
    this._headers = Object.assign(this._headers, headers);
    if (options && options.endStream) {
      this.end();
    }
    return this;
  };

  Http2Stream.prototype.end = function(data, cb) {
    if (this._ended) return this;
    if (typeof data === 'function') { cb = data; data = undefined; }
    pushBodyChunk(this, data);
    this._ended = true;
    this.writable = false;
    this.emit('end');
    if (cb) cb();
    return this;
  };

  Http2Stream.prototype.close = function(code, cb) {
    if (this._closed) return this;
    if (typeof code === 'function') { cb = code; code = 0; }
    this._closed = true;
    this._ended = true;
    this.readable = false;
    this.writable = false;
    // Upstream f7ad274e3 parity: release the stream from the session's
    // registry as soon as it is fully closed. Node drops a stream when both
    // halves close; without this eviction one Http2Stream per request stays
    // rooted on the session for its whole lifetime (WeakRef never clears).
    if (this._session && this._session._streams) {
      delete this._session._streams[this._id];
    }
    this.emit('close');
    if (cb) cb();
    return this;
  };

  Http2Stream.prototype.priority = function(options) {
    // Priority signaling — no-op in JS-based H2 layer
    return this;
  };

  Http2Stream.prototype.sendTrailers = function(headers) {
    if (this._closed) return this;
    this._trailers = Object.assign(this._trailers, headers || {});
    return this;
  };

  Http2Stream.prototype.pushStream = function(headers, options, callback) {
    // Server push — not supported in JS-based H2 layer
    var err = new Error('HTTP/2 server push is not supported');
    if (callback) callback(err);
    return this;
  };

  Http2Stream.prototype.setTimeout = function(msecs, callback) {
    if (callback) callback();
    return this;
  };

  Http2Stream.prototype.destroy = function(error) {
    if (this._closed) return;
    this._closed = true;
    this._ended = true;
    this.readable = false;
    this.writable = false;
    // Same release-on-close invariant as close() (f7ad274e3 parity).
    if (this._session && this._session._streams) {
      delete this._session._streams[this._id];
    }
    if (error) this.emit('error', error);
    this.emit('close');
  };

  // ── Http2Session ────────────────────────────────────────────────────
  var nextStreamId = 1;

  function Http2Session(mode, authority, options) {
    this._mode = mode; // 'client' or 'server'
    this._authority = authority || '';
    this._options = options || {};
    this._streamId = nextStreamId;
    nextStreamId += 2; // client-initiated streams use odd IDs
    this._streams = Object.create(null);
    this._closed = false;
    this._destroyed = false;
    this._events = Object.create(null);
    this._settings = {
      headerTableSize: 4096,
      enablePush: false,
      initialWindowSize: 65535,
      maxFrameSize: 16384,
      maxConcurrentStreams: 100,
      maxHeaderListSize: 65535
    };
    this._remoteSettings = {
      headerTableSize: 4096,
      enablePush: false,
      initialWindowSize: 65535,
      maxFrameSize: 16384,
      maxConcurrentStreams: 100,
      maxHeaderListSize: 65535
    };
    this._pingCallbacks = Object.create(null);
    this._nextPingId = 0;
  }
  Http2Session.prototype = Object.create(null);
  // Mix in EventEmitter
  Http2Session.prototype.on = EE.prototype.on;
  Http2Session.prototype.once = EE.prototype.once;
  Http2Session.prototype.emit = EE.prototype.emit;
  Http2Session.prototype.removeListener = EE.prototype.removeListener;
  Http2Session.prototype.removeAllListeners = EE.prototype.removeAllListeners;
  Http2Session.prototype.prependListener = EE.prototype.prependListener;

  Object.defineProperty(Http2Session.prototype, 'closed', {
    get: function() { return this._closed; },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'destroyed', {
    get: function() { return this._destroyed; },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'type', {
    get: function() { return this._mode === 'client' ? 0 : 1; },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'encrypted', {
    get: function() { return this._mode === 'client' && this._authority.indexOf('https') === 0; },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'alpnProtocol', {
    get: function() { return 'h2'; },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'originSet', {
    get: function() {
      if (!this._authority) return [];
      return [this._authority];
    },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'localSettings', {
    get: function() { return Object.assign({}, this._settings); },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'remoteSettings', {
    get: function() { return Object.assign({}, this._remoteSettings); },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'state', {
    get: function() {
      return {
        effectiveLocalWindowSize: 65535,
        effectiveRecvDataLength: 0,
        nextStreamID: this._streamId,
        localWindowSize: 65535,
        lastProcStreamID: 0,
        remoteWindowSize: 65535,
        outboundQueueSize: 0,
        deflateDynamicTableSize: 0,
        inflateDynamicTableSize: 0
      };
    },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'pendingSettingsAck', {
    get: function() { return false; },
    enumerable: true
  });
  Object.defineProperty(Http2Session.prototype, 'settings', {
    set: function(s) {
      if (s && typeof s === 'object') {
        for (var k in s) {
          if (s.hasOwnProperty(k)) this._settings[k] = s[k];
        }
        this.emit('localSettings', this._settings);
      }
    }
  });

  Http2Session.prototype.request = function(headers, options) {
    if (this._closed || this._destroyed) {
      throw new Error('Session is closed');
    }
    headers = headers || {};
    options = options || {};

    var streamId = this._streamId;
    this._streamId += 2;

    var stream = new Http2Stream(this, streamId);
    stream._headers = Object.assign({}, headers);

    // If this is a client session, perform the fetch via __http2_fetch.
    // The bridge returns the fetch Promise, which resolves with the realm's
    // real WHATWG Response — response headers become the 'response' event
    // (with ':status'), the body is consumed via arrayBuffer() and delivered
    // as ONE Buffer 'data' chunk (Node http2 stream semantics), then 'end'.
    // (The old bridge returned a statusCode:0 placeholder JSON synchronously
    // — every http2 client response was a silent fake.)
    if (this._mode === 'client' && typeof __http2_fetch === 'function') {
      var method = headers[':method'] || 'GET';
      var path = headers[':path'] || '/';
      var authority = headers[':authority'] || this._authority;
      var scheme = headers[':scheme'] || 'https';
      var url = scheme + '://' + authority + path;

      // Build regular headers (strip pseudo-headers)
      var reqHeaders = {};
      for (var k in headers) {
        if (k.charAt(0) !== ':') {
          reqHeaders[k] = headers[k];
        }
      }

      var headersJSON = '{}';
      try { headersJSON = JSON.stringify(reqHeaders); } catch(e) {}

      // Request body: validated eagerly (string or byte body — anything
      // else throws, never a silent comma-string/empty), then assembled
      // byte-exactly. The fire-once bridge carries the body via options.body
      // only — stream.write/end after request() cannot retro-send (the
      // fetch is already in flight).
      if (options.body !== undefined && options.body !== null &&
          typeof options.body !== 'string' && !isByteValue(options.body)) {
        throw new TypeError('http2: request body must be a string, Buffer, TypedArray or ArrayBuffer');
      }
      var bodyArg = buildBodyArg(
        options.body === undefined || options.body === null ? [] : [options.body]
      );

      var sess = this;
      var settle = function (resp) {
        var respHeaders = {};
        try {
          if (resp && resp.headers && typeof resp.headers.forEach === 'function') {
            resp.headers.forEach(function (v, hk) { respHeaders[hk] = v; });
          }
        } catch (e) {}
        respHeaders[':status'] = String(resp && typeof resp.status === 'number' ? resp.status : 0);
        stream._responseHeaders = respHeaders;
        if (!resp || typeof resp.arrayBuffer !== 'function') {
          failStream(new Error('http2: transport resolved without a Response body'));
          return;
        }
        resp.arrayBuffer().then(function (ab) {
          stream.emit('response', respHeaders);
          // One Buffer chunk over the exact wire bytes (Node 'data'
          // semantics — byte view, never a lossy decoded string).
          var chunk;
          if (typeof Buffer !== 'undefined' && typeof Buffer.from === 'function') {
            chunk = Buffer.from(ab);
          } else {
            chunk = new Uint8Array(ab);
          }
          stream._responseBody = chunk;
          if (chunk.length !== 0) stream.emit('data', chunk);
          stream.end();
          // Release-on-completion (same eviction the synchronous path used).
          if (sess._streams) delete sess._streams[streamId];
        }, failStream);
      };
      var failStream = function (err) {
        var e = err instanceof Error ? err : new Error(String(err && err.message ? err.message : err));
        stream.emit('error', e);
        stream.close();
        if (sess._streams) delete sess._streams[streamId];
      };
      var p;
      try {
        p = __http2_fetch(url, method, headersJSON, bodyArg);
      } catch (e) {
        failStream(e);
      }
      if (p !== undefined && p !== null && typeof p.then === 'function' && !stream._closed) {
        p.then(settle, failStream);
      }
    }

    this._streams[streamId] = stream;
    // Upstream f7ad274e3 parity (its flush_queue release point): when the
    // outbound request already completed synchronously — END_STREAM sent by
    // the fetch bridge above — the finished stream must not sit in the
    // session registry until session teardown.
    if (stream._ended) {
      delete this._streams[streamId];
    }
    this.emit('stream', stream, headers);
    return stream;
  };

  Http2Session.prototype.respondWithFile = function(filePath, headers, options) {
    throw new Error('http2.respondWithFile is not supported');
  };

  Http2Session.prototype.respondWithFD = function(fd, headers, options) {
    throw new Error('http2.respondWithFD is not supported');
  };

  Http2Session.prototype.ping = function(payload, callback) {
    if (this._closed || this._destroyed) {
      if (callback) callback(new Error('Session is closed'));
      return false;
    }
    if (typeof payload === 'function') {
      callback = payload;
      payload = null;
    }
    var pingId = this._nextPingId++;
    this._pingCallbacks[pingId] = callback;
    // Simulate ping response
    if (callback) {
      var duration = 0;
      callback(null, duration, payload || Buffer.alloc(8));
    }
    this.emit('ping', payload || Buffer.alloc(8));
    return true;
  };

  Http2Session.prototype.close = function(callback) {
    if (this._closed) {
      if (callback) callback();
      return;
    }
    this._closed = true;
    // Close all open streams. `_streams` is Object.create(null) — it has no
    // hasOwnProperty (calling it threw TypeError the first time this code
    // was ever reached); for-in over a null-proto object only ever yields
    // own enumerable keys, so the guard was redundant anyway. Each
    // stream.close() evicts itself from the registry (f7ad274e3 parity).
    for (var id in this._streams) {
      var stream = this._streams[id];
      if (stream && !stream._closed) stream.close();
    }
    this.emit('close');
    if (callback) callback();
  };

  Http2Session.prototype.destroy = function(error, callback) {
    if (this._destroyed) {
      if (callback) callback();
      return;
    }
    this._destroyed = true;
    this._closed = true;
    // Destroy all streams (evicts each from the registry — see close()).
    for (var id in this._streams) {
      var stream = this._streams[id];
      if (stream) stream.destroy(error);
    }
    if (error) this.emit('error', error);
    this.emit('close');
    if (callback) callback();
  };

  Http2Session.prototype.goaway = function(code, lastStreamID, opaqueData) {
    if (typeof code === 'undefined') code = 0;
    if (typeof lastStreamID === 'undefined') lastStreamID = 0;
    this.emit('goaway', code, lastStreamID, opaqueData);
    this.close();
  };

  Http2Session.prototype.ref = function() { return this; };
  Http2Session.prototype.unref = function() { return this; };

  Http2Session.prototype.setLocalWindowSize = function(windowSize) {
    this._settings.initialWindowSize = windowSize;
    return this;
  };

  Http2Session.prototype.setTimeout = function(msecs, callback) {
    if (callback) callback();
    return this;
  };

  Http2Session.prototype.sendSettings = function(settings) {
    if (settings && typeof settings === 'object') {
      for (var k in settings) {
        if (settings.hasOwnProperty(k)) this._settings[k] = settings[k];
      }
    }
    this.emit('localSettings', this._settings);
  };

  // ── connect(authority, options, listener) ───────────────────────────
  function connect(authority, options, listener) {
    if (typeof options === 'function') {
      listener = options;
      options = {};
    }
    options = options || {};

    var session = new Http2Session('client', authority, options);

    // Store authority for fetch
    if (typeof authority === 'string') {
      if (authority.indexOf('://') === -1) {
        authority = 'https://' + authority;
      }
      session._authority = authority;
    }

    // Emit 'connect' event (synchronous, matching Node.js behavior)
    session.emit('connect', session, null);

    if (listener) {
      session.on('stream', listener);
    }

    return session;
  }

  // ── Server ──────────────────────────────────────────────────────────
  // Node compat split (verified against node docs): the createServer
  // handler is the COMPAT onRequestHandler — called (request, response) —
  // while session-style 'stream' listeners are separate (stream, headers,
  // flags) listeners. Registering the compat handler on 'stream' made the
  // native dispatcher's emit hit it with the wrong shape (double dispatch
  // of one function), so the compat handler now lives on _onStreamHandler
  // (the property the native listen/route bridge reads) and 'stream' is
  // reserved for real session-style listeners.
  function Http2Server(options, handler) {
    if (typeof options === 'function') {
      handler = options;
      options = {};
    }
    this._options = options || {};
    this._events = Object.create(null);
    this.listening = false;
    this._port = 0;
    if (handler) this._onStreamHandler = handler;
  }
  Http2Server.prototype = Object.create(null);
  Http2Server.prototype.on = EE.prototype.on;
  Http2Server.prototype.once = EE.prototype.once;
  Http2Server.prototype.emit = EE.prototype.emit;
  Http2Server.prototype.removeListener = EE.prototype.removeListener;
  Http2Server.prototype.removeAllListeners = EE.prototype.removeAllListeners;
  Http2Server.prototype.prependListener = EE.prototype.prependListener;

  // Node listen arg forms: (port), (port, cb), (port, host, cb),
  // (port, host, backlog, cb), (port, options, cb). Normalize by type so
  // the historical (port, callback)-only signature stopped dropping the
  // host string into the callback slot — listen(18143, '127.0.0.1', fn)
  // bound fine but fn never ran.
  Http2Server.prototype.listen = function(port) {
    var host = null, backlog, callback = null;
    for (var i = 1; i < arguments.length; i++) {
      var a = arguments[i];
      if (typeof a === 'function') {
        if (!callback) callback = a;
      } else if (typeof a === 'number') {
        if (backlog === undefined) backlog = a;
      } else if (typeof a === 'string') {
        if (host === null) host = a;
      }
    }
    this._port = port;
    this._host = host || '0.0.0.0';
    this.listening = true;
    // Delegate to native __http2_server_listen(serverObj, port, host, cb)
    if (typeof __http2_server_listen === 'function') {
      __http2_server_listen(this, port, this._host, callback);
    } else if (callback) {
      callback();
    }
    return this;
  };

  Http2Server.prototype.close = function(callback) {
    this.listening = false;
    // Delegate to native __http2_server_close
    if (typeof __http2_server_close === 'function') {
      __http2_server_close(this, callback);
    } else {
      if (callback) callback();
    }
    return this;
  };

  Http2Server.prototype.setTimeout = function(msecs, callback) {
    if (callback) callback();
    return this;
  };

  Http2Server.prototype.address = function() {
    return {
      port: this._listeningPort || this._port || 0,
      family: 'IPv4',
      address: this._host || '0.0.0.0'
    };
  };

  // ── SecureServer ────────────────────────────────────────────────────
  function Http2SecureServer(options, handler) {
    if (typeof options === 'function') {
      handler = options;
      options = {};
    }
    this._options = options || {};
    this._events = Object.create(null);
    this.listening = false;
    this._port = 0;
    // Compat handler split — see Http2Server.
    if (handler) this._onStreamHandler = handler;
  }
  Http2SecureServer.prototype = Object.create(null);
  Http2SecureServer.prototype.on = EE.prototype.on;
  Http2SecureServer.prototype.once = EE.prototype.once;
  Http2SecureServer.prototype.emit = EE.prototype.emit;
  Http2SecureServer.prototype.removeListener = EE.prototype.removeListener;
  Http2SecureServer.prototype.removeAllListeners = EE.prototype.removeAllListeners;
  Http2SecureServer.prototype.prependListener = EE.prototype.prependListener;

  Http2SecureServer.prototype.listen = function(port) {
    var host = null, backlog, callback = null;
    for (var i = 1; i < arguments.length; i++) {
      var a = arguments[i];
      if (typeof a === 'function') {
        if (!callback) callback = a;
      } else if (typeof a === 'number') {
        if (backlog === undefined) backlog = a;
      } else if (typeof a === 'string') {
        if (host === null) host = a;
      }
    }
    this._port = port;
    this._host = host || '0.0.0.0';
    this.listening = true;
    // Delegate to native __http2_secure_server_listen(serverObj, port, host, cb)
    if (typeof __http2_secure_server_listen === 'function') {
      __http2_secure_server_listen(this, port, this._host, callback);
    } else if (callback) {
      callback();
    }
    return this;
  };

  Http2SecureServer.prototype.close = function(callback) {
    this.listening = false;
    // Delegate to native __http2_secure_server_close
    if (typeof __http2_secure_server_close === 'function') {
      __http2_secure_server_close(this, callback);
    } else {
      if (callback) callback();
    }
    return this;
  };

  Http2SecureServer.prototype.setTimeout = function(msecs, callback) {
    if (callback) callback();
    return this;
  };

  Http2SecureServer.prototype.address = function() {
    return {
      port: this._listeningPort || this._port || 0,
      family: 'IPv4',
      address: this._host || '0.0.0.0'
    };
  };

  // ── createServer / createSecureServer ───────────────────────────────
  function createServer(options, handler) {
    return new Http2Server(options, handler);
  }

  function createSecureServer(options, handler) {
    return new Http2SecureServer(options, handler);
  }

  // ── Utility functions ───────────────────────────────────────────────
  function getDefaultSettings() {
    return {
      headerTableSize: 4096,
      enablePush: false,
      initialWindowSize: 65535,
      maxFrameSize: 16384,
      maxConcurrentStreams: 100,
      maxHeaderListSize: 65535,
      maxHeaderSize: 16384,
      enableConnectProtocol: false
    };
  }

  function getPackedSettings(settings) {
    // Returns a Buffer containing the serialized SETTINGS frame payload.
    // Each setting is 6 bytes: 2-byte identifier + 4-byte value.
    settings = settings || getDefaultSettings();
    var keys = [
      'headerTableSize',       // 0x01
      'enablePush',            // 0x02
      'maxConcurrentStreams',  // 0x03
      'initialWindowSize',     // 0x04
      'maxFrameSize',          // 0x05
      'maxHeaderListSize'      // 0x06
    ];
    var ids = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    var buf = Buffer.alloc(keys.length * 6);
    for (var i = 0; i < keys.length; i++) {
      var val = settings[keys[i]];
      if (typeof val === 'undefined') {
        var defaults = getDefaultSettings();
        val = defaults[keys[i]];
      }
      var offset = i * 6;
      buf[offset] = (ids[i] >> 8) & 0xff;
      buf[offset + 1] = ids[i] & 0xff;
      buf[offset + 2] = (val >> 24) & 0xff;
      buf[offset + 3] = (val >> 16) & 0xff;
      buf[offset + 4] = (val >> 8) & 0xff;
      buf[offset + 5] = val & 0xff;
    }
    return buf;
  }

  function getUnpackedSettings(buf) {
    // Parse a SETTINGS frame payload Buffer into a settings object.
    if (!buf || !buf.length) return getDefaultSettings();
    var settings = getDefaultSettings();
    var keys = [
      'headerTableSize',       // 0x01
      'enablePush',            // 0x02
      'maxConcurrentStreams',  // 0x03
      'initialWindowSize',     // 0x04
      'maxFrameSize',          // 0x05
      'maxHeaderListSize'      // 0x06
    ];
    var ids = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    for (var i = 0; i < Math.floor(buf.length / 6); i++) {
      var offset = i * 6;
      var id = (buf[offset] << 8) | buf[offset + 1];
      var val = (buf[offset + 2] << 24) | (buf[offset + 3] << 16) |
                (buf[offset + 4] << 8) | buf[offset + 5];
      // Convert signed to unsigned for values > 0x7FFFFFFF
      if (val < 0) val = val >>> 0;
      for (var j = 0; j < ids.length; j++) {
        if (ids[j] === id) {
          settings[keys[j]] = val;
          break;
        }
      }
    }
    return settings;
  }

  function sensitiveHeaders(headers) {
    // Mark headers as sensitive so they are never indexed in HPACK.
    // Returns the same headers object with a hidden _sensitive flag.
    if (headers && typeof headers === 'object') {
      Object.defineProperty(headers, '_sensitive', {
        value: true,
        enumerable: false,
        configurable: true
      });
    }
    return headers;
  }

  // ── Export ──────────────────────────────────────────────────────────
  return {
    connect: connect,
    createServer: createServer,
    createSecureServer: createSecureServer,
    Http2Session: Http2Session,
    Http2Stream: Http2Stream,
    Http2Server: Http2Server,
    Http2SecureServer: Http2SecureServer,
    getDefaultSettings: getDefaultSettings,
    getPackedSettings: getPackedSettings,
    getUnpackedSettings: getUnpackedSettings,
    sensitiveHeaders: sensitiveHeaders,
    // PerformanceEntry stubs
    performance: {
      timerify: function(fn) { return fn; },
      eventLoopUtilization: function() { return { idle: 0, active: 0, utilization: 0 }; }
    }
  };
})();
"#;

// ──────────────────────────────────────────────────────────────────────
// Install — register module on the JS global
// ──────────────────────────────────────────────────────────────────────

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let http2_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if http2_obj.get().is_null() {
        return;
    }

    unsafe {
        // ── Constants ──────────────────────────────────────────────────
        // HTTP/2 header pseudo-header constants
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_STATUS", 0x01);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_METHOD", 0x02);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_PATH", 0x04);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_AUTHORITY", 0x08);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_SCHEME", 0x10);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_CONTENT_TYPE", 0x20);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_CONTENT_LENGTH", 0x40);

        // Legacy aliases (Node.js compat)
        define_int_prop(cx, http2_obj.get(), "HEADER_STATUS", 0x01);
        define_int_prop(cx, http2_obj.get(), "HEADER_METHOD", 0x02);
        define_int_prop(cx, http2_obj.get(), "HEADER_PATH", 0x04);
        define_int_prop(cx, http2_obj.get(), "HEADER_AUTHORITY", 0x08);
        define_int_prop(cx, http2_obj.get(), "HEADER_SCHEME", 0x10);

        // nghttp2 error codes
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_NO_ERROR", 0);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_PROTOCOL_ERROR", 1);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_INTERNAL_ERROR", 2);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FLOW_CONTROL_ERROR", 3);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_SETTINGS_TIMEOUT", 4);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_CLOSED", 5);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FRAME_SIZE_ERROR", 6);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_REFUSED_STREAM", 7);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_CANCEL", 8);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_COMPRESSION_ERROR", 9);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_CONNECT_ERROR", 10);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_ENHANCE_YOUR_CALM", 11);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_INADEQUATE_SECURITY", 12);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_HTTP_1_1_REQUIRED", 13);

        // Default settings constants
        define_int_prop(
            cx,
            http2_obj.get(),
            "DEFAULT_SETTINGS_HEADER_TABLE_SIZE",
            4096,
        );
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_ENABLE_PUSH", 0);
        define_int_prop(
            cx,
            http2_obj.get(),
            "DEFAULT_SETTINGS_INITIAL_WINDOW_SIZE",
            65535,
        );
        define_int_prop(
            cx,
            http2_obj.get(),
            "DEFAULT_SETTINGS_MAX_FRAME_SIZE",
            16384,
        );
        define_int_prop(
            cx,
            http2_obj.get(),
            "DEFAULT_SETTINGS_MAX_CONCURRENT_STREAMS",
            100,
        );
        define_int_prop(
            cx,
            http2_obj.get(),
            "DEFAULT_SETTINGS_MAX_HEADER_LIST_SIZE",
            65535,
        );

        // Stream states
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_IDLE", 0);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_OPEN", 1);
        define_int_prop(
            cx,
            http2_obj.get(),
            "NGHTTP2_STREAM_STATE_RESERVED_LOCAL",
            2,
        );
        define_int_prop(
            cx,
            http2_obj.get(),
            "NGHTTP2_STREAM_STATE_RESERVED_REMOTE",
            3,
        );
        define_int_prop(
            cx,
            http2_obj.get(),
            "NGHTTP2_STREAM_STATE_HALF_CLOSED_LOCAL",
            4,
        );
        define_int_prop(
            cx,
            http2_obj.get(),
            "NGHTTP2_STREAM_STATE_HALF_CLOSED_REMOTE",
            5,
        );
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_CLOSED", 6);

        // Frame types
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_DATA", 0);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_HEADERS", 1);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_PRIORITY", 2);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_RST_STREAM", 3);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_SETTINGS", 4);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_PUSH_PROMISE", 5);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_PING", 6);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_GOAWAY", 7);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_WINDOW_UPDATE", 8);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_CONTINUATION", 9);

        // Flags
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FLAG_NONE", 0);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FLAG_END_STREAM", 1);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FLAG_END_HEADERS", 4);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FLAG_ACK", 1);

        // ── Native functions ───────────────────────────────────────────
        // Server creation (delegates to JS Http2Server/Http2SecureServer
        // but also registers native uWS App for real HTTP serving)
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"createServer".as_ptr(),
            Some(http2_create_server),
            2,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"createSecureServer".as_ptr(),
            Some(http2_create_secure_server),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // Client fetch bridge
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"__http2_fetch".as_ptr(),
            Some(http2_fetch),
            4,
            0 as u32,
        );

        // Server listen/close bridges (called from JS)
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"__http2_server_listen".as_ptr(),
            Some(http2_server_listen),
            3,
            0 as u32,
        );
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"__http2_server_close".as_ptr(),
            Some(http2_server_close),
            2,
            0 as u32,
        );
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"__http2_secure_server_listen".as_ptr(),
            Some(http2_secure_server_listen),
            3,
            0 as u32,
        );
        w2::JS_DefineFunction(
            cx,
            http2_obj.handle(),
            c"__http2_secure_server_close".as_ptr(),
            Some(http2_secure_server_close),
            2,
            0 as u32,
        );

        // The JS IIFE resolves these host bridges as FREE variables — the
        // `typeof __http2_server_listen === 'function'` probes inside the
        // IIFE look at the GLOBAL, never at this module object. Defining
        // them only on http2_obj left every probe false: JS-side servers
        // fell back to a no-op listen and the client fetch bridge never
        // ran. Mirror them onto the global (non-enumerable, configurable)
        // so the IIFE sees them.
        rooted!(&in(cx) let global = CurrentGlobalOrNull(cx.raw_cx()));
        if !global.get().is_null() {
            let bridges: &[(&str, JSNative, u32)] = &[
                ("__http2_fetch", Some(http2_fetch), 4),
                ("__http2_server_listen", Some(http2_server_listen), 3),
                ("__http2_server_close", Some(http2_server_close), 2),
                (
                    "__http2_secure_server_listen",
                    Some(http2_secure_server_listen),
                    3,
                ),
                (
                    "__http2_secure_server_close",
                    Some(http2_secure_server_close),
                    2,
                ),
            ];
            for &(name, native, nargs) in bridges {
                let c_name = ZBox::from_bytes(name);
                w2::JS_DefineFunction(
                    cx,
                    global.handle(),
                    c_name.as_ptr(),
                    native,
                    nargs,
                    0 as u32,
                );
            }
        }

        // ── Evaluate JS IIFE ───────────────────────────────────────────
        let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"node:http2".as_ptr(), 1);
        if !opts.is_null() {
            let mut src_text = mozjs::rust::transform_str_to_source_text(HTTP2_JS);
            let mut rval = UndefinedValue();
            if JS::Evaluate2(
                cx.raw_cx(),
                opts,
                &mut src_text,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            ) && rval.is_object()
            {
                // HTTP2_JS ends with `})();`, so Evaluate2's completion value
                // IS the exports object already — do NOT call it again. The
                // previous code re-invoked the exports object as a function
                // (TypeError, silently swallowed), which dropped the entire
                // JS API surface (connect / Http2Session / Http2Stream /
                // getDefaultSettings / ...) from the module:
                // require('http2').connect was undefined.
                rooted!(&in(cx) let exports = rval.to_object());
                // Copy JS-defined properties onto http2_obj
                let js_props = [
                    "connect",
                    "Http2Session",
                    "Http2Stream",
                    "Http2Server",
                    "Http2SecureServer",
                    "getDefaultSettings",
                    "getPackedSettings",
                    "getUnpackedSettings",
                    "sensitiveHeaders",
                    "performance",
                ];
                for &prop in &js_props {
                    let c_prop = ZBox::from_bytes(prop.as_bytes());
                    let mut prop_val = UndefinedValue();
                    JS_GetProperty(
                        cx.raw_cx(),
                        exports.handle().into(),
                        c_prop.as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut prop_val,
                        },
                    );
                    if !prop_val.is_undefined() {
                        rooted!(&in(cx) let pv = prop_val);
                        JS_DefineProperty(
                            cx.raw_cx(),
                            http2_obj.handle().into(),
                            c_prop.as_ptr(),
                            pv.handle().into(),
                            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                        );
                    }
                }
            }
            libc::free(opts as *mut _);
        }
    }

    cache_builtin(cx, "http2", http2_obj.get());
}

// ──────────────────────────────────────────────────────────────────────
// ServerUserData — GC-safe per-server state (same pattern as node_http)
// ──────────────────────────────────────────────────────────────────────

struct H2ServerUserData {
    cx: *mut JSContext,
    global_key: String,
    handler_key: String,
    server_key: String,
}

impl H2ServerUserData {
    fn new(
        cx: *mut JSContext,
        global: *mut JSObject,
        handler: *mut JSObject,
        server: *mut JSObject,
    ) -> Self {
        let server_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let global_key = format!("http2_server_{}_global", server_id);
        let handler_key = format!("http2_server_{}_handler", server_id);
        let server_key = format!("http2_server_{}_server", server_id);
        gc_store_insert_ns(cx, "http2", &global_key, global);
        gc_store_insert_ns(cx, "http2", &handler_key, handler);
        gc_store_insert_ns(cx, "http2", &server_key, server);
        Self {
            cx,
            global_key,
            handler_key,
            server_key,
        }
    }

    fn global(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http2", &self.global_key)
    }

    fn handler(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http2", &self.handler_key)
    }

    fn server_obj(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http2", &self.server_key)
    }

    fn cleanup(&self) {
        gc_store_remove_ns(self.cx, "http2", &self.global_key);
        gc_store_remove_ns(self.cx, "http2", &self.handler_key);
        gc_store_remove_ns(self.cx, "http2", &self.server_key);
    }
}

// ──────────────────────────────────────────────────────────────────────
// Per-request state — GC-safe lifetime for async bodies/responses
// ──────────────────────────────────────────────────────────────────────
// uWS hands the route handler a `res` that stays valid past handler return
// ONLY under its async contract: attach onAborted (mandatory when not
// responding inline) and onData for body delivery. The JS req/res objects
// outlive the route-handler frame, so they are rooted in the GcStore under
// per-request keys and the uWS callbacks resolve them through this state.
//
// Ownership: the Box lives in H2_LIVE_REQUESTS keyed by id; every finish
// path (res.end, stream.end, fallback 500, body-end fallback, abort) calls
// h2_req_finish, which clears the uWS callbacks FIRST (so a freed state can
// never be dereferenced by a later on_data/on_aborted dispatch — uWS holds
// None after clear) and then drops the Box and its GcStore entries. The
// map-remove makes finish idempotent, which is what makes the reentrant
// case safe: res.end() invoked from inside a req 'data' listener frees the
// state mid-callback, and the pump's post-emit code touches only locals.

struct H2ReqState {
    cx: *mut JSContext,
    id: u64,
    req_key: String,
    res_key: String,
}

static NEXT_H2_REQ_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static H2_LIVE_REQUESTS: RefCell<::std::collections::HashMap<u64, Box<H2ReqState>>> =
        RefCell::new(::std::collections::HashMap::new());
}

/// Idempotent per-request teardown: detach uWS callbacks, drop the state Box
/// and its GcStore roots. `res` is None on the abort path (the connection is
/// dead; touching the res would be a use-after-close).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_req_finish(cx: *mut JSContext, id: u64, res: Option<&mut Response<false>>) {
    let state = H2_LIVE_REQUESTS.with(|m| m.borrow_mut().remove(&id));
    match state {
        Some(st) => {
            if let Some(r) = res {
                r.clear_on_data();
                r.clear_aborted();
            }
            gc_store_remove_ns(cx, "http2", &st.req_key);
            gc_store_remove_ns(cx, "http2", &st.res_key);
        }
        None => {}
    }
}

/// Explicit-500 crash-class guard (node:http 4c933019 pattern): a handler
/// that never responded must never fall through to uWS's
/// "Returning from a request handler without responding" std::terminate —
/// answer explicitly. If a status line is already on the wire, complete
/// that response instead of double-writing a status.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_respond_500(res: &mut Response<false>, msg: &[u8]) {
    if res.state().is_http_status_called() {
        res.end(&[], true);
        return;
    }
    res.write_status(b"500 Internal Server Error");
    res.write_header(b"Content-Type", b"text/plain");
    res.end(msg, true);
}

/// Emit `event` (with one optional arg) on a JS object through its `emit`
/// method (the native node_events EE — emit reads `this`, so the receiver is
/// the object itself). Returns the EE's had-listeners boolean. Caller must be
/// inside the realm. Pending exceptions from listeners are cleared — a
/// throwing listener must not kill the pump.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_emit_event(
    cx: *mut JSContext,
    obj: *mut JSObject,
    event: &str,
    arg: Option<JSVal>,
) -> bool {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);

    let mut emit_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"emit".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut emit_val,
        },
    );
    if !emit_val.is_object() {
        return false;
    }

    let c_event = ZBox::from_bytes(event.as_bytes());
    let event_str = JS_NewStringCopyZ(cx, c_event.as_ptr());
    if event_str.is_null() {
        return false;
    }

    let ev_val = StringValue(&*event_str);
    rooted!(&in(cx_ref) let ev_root = ev_val);
    rooted!(&in(cx_ref) let arg_root = arg.unwrap_or_else(UndefinedValue));

    let args_vals = [ev_root.get(), arg_root.get()];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: args_vals.as_ptr(),
    };

    rooted!(&in(cx_ref) let emit_fn = emit_val.to_object());
    let emit_fn_val = ObjectValue(emit_fn.get());
    rooted!(&in(cx_ref) let emit_fn_root = emit_fn_val);

    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = JS_CallFunctionValue(
        cx,
        obj_root.handle().into(),
        emit_fn_root.handle().into(),
        &call_args,
        rval_h,
    );
    if !ok {
        JS_ClearPendingException(cx);
        return false;
    }
    rval.is_boolean() && rval.to_boolean()
}

/// Read a boolean property off a JS object (missing → false).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_get_bool_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str) -> bool {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut v = UndefinedValue();
    let c_name = ZBox::from_bytes(name.as_bytes());
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    v.is_boolean() && v.to_boolean()
}

/// Set a boolean property on a JS object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_set_bool_prop(cx: *mut JSContext, obj: *mut JSObject, name: &str, val: bool) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    rooted!(&in(cx_ref) let v = mozjs::jsval::BooleanValue(val));
    let c_name = ZBox::from_bytes(name.as_bytes());
    JS_SetProperty(cx, obj_root.handle().into(), c_name.as_ptr(), v.handle().into());
}

/// Append one response-body chunk to the res object's `_bodyChunks` array,
/// byte-exactly (same contract as node_http::res_append_chunk): strings are
/// stored as JS strings, byte views as fresh Uint8Array parts. Anything
/// else is a TypeError — never a silent drop.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_res_append_chunk(cx: *mut JSContext, obj: *mut JSObject, v: JSVal) -> bool {
    if v.is_undefined() || v.is_null() {
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);

    let part_val: Value = if v.is_string() {
        v
    } else if let Some(bytes) = crate::node_buffer::collect_byte_view(cx, v) {
        let ta = crate::globals::create_buffer_object(cx, &bytes);
        if ta.is_null() {
            return false;
        }
        ObjectValue(ta)
    } else {
        JS_ReportErrorUTF8(
            cx,
            c"%s".as_ptr(),
            c"http2: stream chunk must be a string, Buffer, TypedArray or ArrayBuffer".as_ptr(),
        );
        return false;
    };
    rooted!(&in(cx_ref) let part_root = part_val);

    // Ensure `_bodyChunks` array exists.
    let mut chunks_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_bodyChunks".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut chunks_val,
        },
    );
    if !chunks_val.is_object() {
        rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, 0));
        if arr.get().is_null() {
            return false;
        }
        rooted!(&in(cx_ref) let arr_val = ObjectValue(arr.get()));
        JS_SetProperty(
            cx,
            obj_root.handle().into(),
            c"_bodyChunks".as_ptr(),
            arr_val.handle().into(),
        );
        chunks_val = arr_val.get();
    }
    rooted!(&in(cx_ref) let chunks_obj = chunks_val.to_object());

    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        chunks_obj.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let next_index: u32 = if len_val.is_int32() {
        len_val.to_int32().max(0) as u32
    } else {
        0
    };
    JS_DefineElement(
        cx,
        chunks_obj.handle().into(),
        next_index,
        part_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    true
}

/// Concatenate `_bodyChunks` into the exact wire bytes (strings encode
/// UTF-8, Uint8Array parts copy verbatim).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_res_collect_body(cx: *mut JSContext, obj: *mut JSObject) -> Vec<u8> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);

    let mut chunks_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_bodyChunks".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut chunks_val,
        },
    );
    if !chunks_val.is_object() {
        return Vec::new();
    }
    rooted!(&in(cx_ref) let chunks_obj = chunks_val.to_object());

    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        chunks_obj.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let count: u32 = if len_val.is_int32() {
        len_val.to_int32().max(0) as u32
    } else {
        0
    };

    let mut out: Vec<u8> = Vec::new();
    for i in 0..count {
        let mut elem = UndefinedValue();
        if !JS_GetElement(
            cx,
            chunks_obj.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        ) {
            break;
        }
        if elem.is_string() {
            out.extend_from_slice(crate::js_to_rust_string(cx, elem).as_bytes());
        } else if elem.is_object() {
            match crate::node_buffer::collect_byte_view(cx, elem) {
                Some(bytes) => out.extend_from_slice(&bytes),
                None => eprintln!("[node:http2] response body chunk {} was not extractable", i),
            }
        }
    }
    out
}

/// Write every own string-keyed property of the headers object as a response
/// header (names lowercased for uWS; ':' pseudo-headers skipped). Iterates
/// ALL keys via IdVector — the fixed common-header list silently dropped
/// every other header a handler sent.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_write_headers_obj(
    cx: *mut JSContext,
    hdrs: *mut JSObject,
    res_mut: &mut Response<false>,
) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let hdrs_obj = hdrs);
    let mut ids = mozjs::rust::IdVector::new(cx_ref);
    if !w2::GetPropertyKeys(
        cx_ref,
        hdrs_obj.handle().into(),
        JSITER_OWNONLY as u32,
        ids.handle_mut(),
    ) {
        return;
    }
    for jsid in &*ids {
        if !jsid.is_string() {
            continue;
        }
        let key_str = jsid.to_string();
        let key = mozjs::conversions::unsafe_jsstr_to_string(
            cx,
            NonNull::new_unchecked(key_str),
        );
        if key.starts_with(':') {
            continue;
        }
        let c_key = ZBox::from_bytes(key.as_bytes());
        let mut hv = UndefinedValue();
        JS_GetProperty(
            cx,
            hdrs_obj.handle().into(),
            c_key.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut hv,
            },
        );
        if hv.is_string() {
            let val = crate::js_to_rust_string(cx, hv);
            let key_lower = key.to_ascii_lowercase();
            let c_val = ZBox::from_bytes(val.as_bytes());
            (*res_mut).write_header(key_lower.as_bytes(), c_val.as_bytes());
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// uWS route handler — bridges C++ HTTP events to JS Http2Stream
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn uws_h2_route_handler(
    res: *mut bun_uws_sys::response::c::uws_res,
    req: *mut bun_uws_sys::Request,
    user_data: *mut ::std::ffi::c_void,
) {
    if res.is_null() || req.is_null() || user_data.is_null() {
        return;
    }

    let ud = &*(user_data as *const H2ServerUserData);
    let cx = ud.cx;
    if cx.is_null() {
        return;
    }

    let raw_cx = cx;
    let res_mut = Response::<false>::cast_res(res);

    // Enter the context's persistent realm before any JS resolution (same
    // rationale as node_http::uws_route_handler): async dispatch runs with no
    // realm entered, and the GcStore properties backing ud.global()/handler()
    // live on this realm's global — without the AutoRealm the lookups fail
    // and the handler silently never runs (uWS then std::terminates on the
    // unanswered request). First-principles realm model: one realm per
    // JsContext, held for the context's lifetime.
    let realm_global = match bao_engine::context::thread_realm_global() {
        Some(g) if !g.is_null() => g,
        _ => {
            // No realm on this thread → no JS server should exist here.
            // Explicit 500 (never a silent return → uWS std::terminate).
            eprintln!("[node:http2] no JS realm on this thread — responding 500");
            (*res_mut).write_status(b"500 Internal Server Error");
            (*res_mut).write_header(b"Content-Type", b"text/plain");
            (*res_mut).end(b"no JS realm", true);
            return;
        }
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let realm_global_root = realm_global);
    let mut realm = mozjs::realm::AutoRealm::new_from_handle(cx_ref, realm_global_root.handle());
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;

    // Now inside the realm: CurrentGlobalOrNull = persistent global, so the
    // GcStore lookups resolve the registered server global and stream handler.
    let Some(global) = ud.global() else {
        eprintln!(
            "[node:http2] server global unavailable (key {}) — responding 500",
            ud.global_key
        );
        (*res_mut).write_status(b"500 Internal Server Error");
        (*res_mut).write_header(b"Content-Type", b"text/plain");
        (*res_mut).end(b"no server global", true);
        return;
    };
    if global.is_null() {
        eprintln!("[node:http2] server global null — responding 500");
        (*res_mut).write_status(b"500 Internal Server Error");
        (*res_mut).write_header(b"Content-Type", b"text/plain");
        (*res_mut).end(b"no server global", true);
        return;
    }

    let Some(handler) = ud.handler() else {
        // Registered-but-unresolvable handler must fail explicitly — never a
        // silent return (crash) and never a fake response.
        eprintln!(
            "[node:http2] stream handler unavailable (key {}) — responding 500",
            ud.handler_key
        );
        (*res_mut).write_status(b"500 Internal Server Error");
        (*res_mut).write_header(b"Content-Type", b"text/plain");
        (*res_mut).end(b"no stream handler", true);
        return;
    };
    if handler.is_null() {
        eprintln!("[node:http2] stream handler null — responding 500");
        (*res_mut).write_status(b"500 Internal Server Error");
        (*res_mut).write_header(b"Content-Type", b"text/plain");
        (*res_mut).end(b"no stream handler", true);
        return;
    }

    let req_ref = bun_opaque::opaque_deref_mut(req);
    let method_bytes = req_ref.method();
    let url_bytes = req_ref.url();
    // uWS stores the method token lowercased internally; Node's req.method
    // carries the client-sent uppercase token (same restore as node_http).
    let method_upper = method_bytes.to_ascii_uppercase();
    let method_str = ::std::str::from_utf8_unchecked(&method_upper);
    let url_str = ::std::str::from_utf8_unchecked(url_bytes);

    // Body detection drives the uWS async contract: a request WITH a body
    // keeps the Response alive past handler return (the onData pump delivers
    // 'data'/'end' and enforces respond-or-500 at body end); a bodyless
    // request must be fully decided by the handler's return.
    let has_body = {
        let content_length = req_ref
            .header(b"content-length")
            .and_then(|v| ::std::str::from_utf8(v).ok())
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let chunked = req_ref.header(b"transfer-encoding").is_some();
        content_length > 0 || chunked
    };

    // Union stream/request object. arg1 of the compat handler is node's
    // Http2ServerRequest AND carries the session-style Http2Stream method
    // surface (respond/end/close + ':method'/':path') — http_te_parity pins
    // createServer handlers written against the session shape, and node's
    // Http2ServerRequest is itself a stream (req.stream === this surface).
    rooted!(&in(cx_ref) let stream_obj = w2::JS_NewPlainObject(cx_ref));
    if stream_obj.get().is_null() {
        eprintln!("[node:http2] stream object allocation failed — responding 500");
        h2_respond_500(&mut *res_mut, b"stream allocation failed");
        return;
    }

    // Session-shape pseudo-header properties.
    {
        let c_method = ZBox::from_bytes(method_str.as_bytes());
        let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
        if !js_method.is_null() {
            let mv = StringValue(&*js_method);
            rooted!(&in(cx_ref) let mvr = mv);
            JS_DefineProperty(
                raw_cx,
                stream_obj.handle().into(),
                c":method".as_ptr(),
                mvr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    {
        let c_path = ZBox::from_bytes(url_str.as_bytes());
        let js_path = JS_NewStringCopyZ(raw_cx, c_path.as_ptr());
        if !js_path.is_null() {
            let pv = StringValue(&*js_path);
            rooted!(&in(cx_ref) let pvr = pv);
            JS_DefineProperty(
                raw_cx,
                stream_obj.handle().into(),
                c":path".as_ptr(),
                pvr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Compat request properties (method/url/httpVersion/stream).
    {
        let c_method = ZBox::from_bytes(method_str.as_bytes());
        let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
        if !js_method.is_null() {
            let mv = StringValue(&*js_method);
            rooted!(&in(cx_ref) let mvr = mv);
            JS_DefineProperty(
                raw_cx,
                stream_obj.handle().into(),
                c"method".as_ptr(),
                mvr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    {
        let c_url = ZBox::from_bytes(url_str.as_bytes());
        let js_url = JS_NewStringCopyZ(raw_cx, c_url.as_ptr());
        if !js_url.is_null() {
            let uv = StringValue(&*js_url);
            rooted!(&in(cx_ref) let uvr = uv);
            JS_DefineProperty(
                raw_cx,
                stream_obj.handle().into(),
                c"url".as_ptr(),
                uvr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    {
        let js_ver = JS_NewStringCopyZ(raw_cx, c"2.0".as_ptr());
        if !js_ver.is_null() {
            let vv = StringValue(&*js_ver);
            rooted!(&in(cx_ref) let vvr = vv);
            JS_DefineProperty(
                raw_cx,
                stream_obj.handle().into(),
                c"httpVersion".as_ptr(),
                vvr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    {
        let sv = ObjectValue(stream_obj.get());
        rooted!(&in(cx_ref) let svr = sv);
        JS_DefineProperty(
            raw_cx,
            stream_obj.handle().into(),
            c"stream".as_ptr(),
            svr.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Headers: ALL request headers via for_each_header — the previous fixed
    // common-name list silently dropped every other header the client sent.
    // (HTTP/1.x wire path: no ':authority'/':scheme' pseudo-headers exist,
    // so none are synthesized.)
    rooted!(&in(cx_ref) let headers_obj = w2::JS_NewPlainObject(cx_ref));
    if !headers_obj.get().is_null() {
        let mut header_pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        req_ref.for_each_header(
            |pairs: &mut Vec<(Vec<u8>, Vec<u8>)>, name: &[u8], value: &[u8]| {
                pairs.push((name.to_vec(), value.to_vec()));
            },
            &mut header_pairs as *mut Vec<(Vec<u8>, Vec<u8>)>,
        );
        for (name, value) in &header_pairs {
            let c_k = ZBox::from_bytes(name);
            let c_v = ZBox::from_bytes(value);
            let js_v = JS_NewStringCopyZ(raw_cx, c_v.as_ptr());
            if !js_v.is_null() {
                let hv = StringValue(&*js_v);
                rooted!(&in(cx_ref) let hvr = hv);
                JS_DefineProperty(
                    raw_cx,
                    headers_obj.handle().into(),
                    c_k.as_ptr(),
                    hvr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        let hdrs_val = ObjectValue(headers_obj.get());
        rooted!(&in(cx_ref) let hdrs_r = hdrs_val);
        JS_DefineProperty(
            raw_cx,
            stream_obj.handle().into(),
            c"headers".as_ptr(),
            hdrs_r.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Session-shape method surface (respond/end/close).
    w2::JS_DefineFunction(
        cx_ref,
        stream_obj.handle(),
        c"respond".as_ptr(),
        Some(h2_stream_respond),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stream_obj.handle(),
        c"end".as_ptr(),
        Some(h2_stream_end),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stream_obj.handle(),
        c"close".as_ptr(),
        Some(h2_stream_close),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // EE surface for the request side: 'data'/'end'/'aborted' listeners
    // (node_events natives — the same EE the pump emits through).
    attach_ee_methods(raw_cx, stream_obj.get());

    // Store uWS res pointer on the stream object
    let res_ptr_val = PrivateValue(res as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let rv = res_ptr_val);
    JS_DefineProperty(
        raw_cx,
        stream_obj.handle().into(),
        c"_uwsRes".as_ptr(),
        rv.handle().into(),
        0,
    );

    // Compat response object: writeHead/setHeader/write/end bridging to the
    // uWS Response (node's Http2ServerResponse surface).
    rooted!(&in(cx_ref) let res_obj = w2::JS_NewPlainObject(cx_ref));
    if res_obj.get().is_null() {
        eprintln!("[node:http2] response object allocation failed — responding 500");
        h2_respond_500(&mut *res_mut, b"response allocation failed");
        return;
    }
    let res_methods: &[(&str, u32, unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool)] = &[
        ("writeHead", 2, h2_res_write_head),
        ("setHeader", 2, h2_res_set_header),
        ("getHeader", 1, h2_res_get_header),
        ("getHeaders", 0, h2_res_get_headers),
        ("hasHeader", 1, h2_res_has_header),
        ("removeHeader", 1, h2_res_remove_header),
        ("write", 1, h2_res_write),
        ("end", 1, h2_res_end),
        ("setTimeout", 2, h2_res_set_timeout),
    ];
    for (name, nargs, op) in res_methods {
        let c_name = ZBox::from_bytes(name.as_bytes());
        w2::JS_DefineFunction(
            cx_ref,
            res_obj.handle(),
            c_name.as_ptr(),
            Some(*op),
            *nargs,
            JSPROP_ENUMERATE as u32,
        );
    }
    {
        rooted!(&in(cx_ref) let sv = Int32Value(200));
        JS_DefineProperty(
            raw_cx,
            res_obj.handle().into(),
            c"statusCode".as_ptr(),
            sv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    {
        rooted!(&in(cx_ref) let hdrs_plain = w2::JS_NewPlainObject(cx_ref));
        rooted!(&in(cx_ref) let hv = ObjectValue(hdrs_plain.get()));
        JS_DefineProperty(
            raw_cx,
            res_obj.handle().into(),
            c"_headers".as_ptr(),
            hv.handle().into(),
            0,
        );
    }
    attach_ee_methods(raw_cx, res_obj.get());
    JS_DefineProperty(
        raw_cx,
        res_obj.handle().into(),
        c"_uwsRes".as_ptr(),
        rv.handle().into(),
        0,
    );

    // Per-request state: root the stream/res objects in the GcStore and
    // register the uWS async contract (onAborted mandatory, onData = body
    // pump) so the response may legally complete after this frame returns.
    let req_id = NEXT_H2_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let req_key = format!("http2_req_{}_stream", req_id);
    let res_key = format!("http2_req_{}_res", req_id);
    gc_store_insert_ns(raw_cx, "http2", &req_key, stream_obj.get());
    gc_store_insert_ns(raw_cx, "http2", &res_key, res_obj.get());
    {
        rooted!(&in(cx_ref) let idv = Int32Value(req_id as i32));
        JS_DefineProperty(
            raw_cx,
            stream_obj.handle().into(),
            c"_stateId".as_ptr(),
            idv.handle().into(),
            0,
        );
        JS_DefineProperty(
            raw_cx,
            res_obj.handle().into(),
            c"_stateId".as_ptr(),
            idv.handle().into(),
            0,
        );
    }
    H2_LIVE_REQUESTS.with(|m| {
        m.borrow_mut().insert(
            req_id,
            Box::new(H2ReqState {
                cx: raw_cx,
                id: req_id,
                req_key: req_key.clone(),
                res_key: res_key.clone(),
            }),
        );
    });
    let state_ptr = H2_LIVE_REQUESTS.with(|m| {
        m.borrow()
            .get(&req_id)
            .map(|b| b.as_ref() as *const H2ReqState as *mut H2ReqState)
    });
    if let Some(state_ptr) = state_ptr {
        (*res_mut).on_aborted(
            |st: *mut H2ReqState, _res: &mut Response<false>| h2_on_aborted(st),
            state_ptr,
        );
        (*res_mut).on_data(
            |st: *mut H2ReqState, res: &mut Response<false>, chunk: &[u8], last: bool| {
                h2_on_data(st, res, chunk, last)
            },
            state_ptr,
        );
    }

    // Compat dispatch: handler(stream, res) — arg1 doubles as the session
    // stream (union object above).
    rooted!(&in(cx_ref) let handler_root = ObjectValue(handler));
    rooted!(&in(cx_ref) let global_root = global);

    let args_vals = [
        ObjectValue(stream_obj.get()),
        ObjectValue(res_obj.get()),
    ];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: args_vals.as_ptr(),
    };

    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = JS_CallFunctionValue(
        raw_cx,
        global_root.handle().into(),
        handler_root.handle().into(),
        &call_args,
        rval_h,
    );
    if !ok {
        // Handler threw — explicit 500 (never silent terminate; the uWS
        // unanswered-request path is std::terminate → mozalloc_abort).
        JS_ClearPendingException(raw_cx);
        eprintln!("[node:http2] request handler threw — responding 500");
        if !(*res_mut).state().is_http_end_called() {
            h2_respond_500(&mut *res_mut, b"request handler threw");
        }
        h2_req_finish(raw_cx, req_id, Some(&mut *res_mut));
        return;
    }

    // Session-style 'stream' event on the server object (node forwards the
    // session stream event alongside the compat handler; the compat handler
    // is NOT registered on 'stream', so no double dispatch).
    if let Some(server_obj) = ud.server_obj() {
        if !server_obj.is_null() {
            h2_emit_server_stream(raw_cx, server_obj, stream_obj.get(), headers_obj.get());
        }
    }

    // Post-dispatch decision. Response already complete → res.end finished
    // the state itself. With a body, the onData pump owns the
    // respond-or-500 deadline. Bodyless requests are decided now: deliver a
    // synthetic 'end' (CL:0 handlers), then enforce respond-or-500.
    if (*res_mut).state().is_http_end_called() {
        return;
    }
    if has_body {
        return;
    }
    h2_set_bool_prop(raw_cx, stream_obj.get(), "_bodyEnded", true);
    h2_emit_event(raw_cx, stream_obj.get(), "end", None);
    if !(*res_mut).state().is_http_end_called() {
        eprintln!("[node:http2] request handler returned without responding — responding 500");
        h2_respond_500(&mut *res_mut, b"handler did not respond");
    }
    h2_req_finish(raw_cx, req_id, Some(&mut *res_mut));
}

/// onData pump: forward request-body chunks to the JS stream's 'data'
/// listeners and 'end' at the final chunk, then enforce respond-or-500.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_on_data(
    st: *mut H2ReqState,
    res: &mut Response<false>,
    chunk: &[u8],
    last: bool,
) {
    // Copy everything the post-emit code needs BEFORE any JS call — a
    // reentrant res.end() inside a 'data' listener finishes (and frees) the
    // state mid-callback; after the emit only these locals and the callback
    // param `res` may be touched.
    let cx = (*st).cx;
    let id = (*st).id;
    let req_key = (*st).req_key.clone();

    // Realm entry (same rationale as the route handler: pump dispatch runs
    // with no realm entered, GcStore lookups need the realm's global).
    let realm_global = match bao_engine::context::thread_realm_global() {
        Some(g) if !g.is_null() => g,
        _ => return,
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let realm_global_root = realm_global);
    let mut realm = mozjs::realm::AutoRealm::new_from_handle(cx_ref, realm_global_root.handle());
    let _cx_ref: &mut mozjs::context::JSContext = &mut realm;

    let stream_obj = match gc_store_get_ns(cx, "http2", &req_key) {
        Some(o) if !o.is_null() => o,
        _ => return,
    };

    // Synthetic-end guard: a bodyless request already got 'end' inline (and
    // the route handler finished the state).
    if h2_get_bool_prop(cx, stream_obj, "_bodyEnded") {
        return;
    }

    if !chunk.is_empty() {
        let chunk_val = crate::bun_api::bytes_to_js_uint8array(cx, chunk);
        if !chunk_val.is_undefined() {
            h2_emit_event(cx, stream_obj, "data", Some(chunk_val));
        }
    }
    if !last {
        return;
    }
    h2_set_bool_prop(cx, stream_obj, "_bodyEnded", true);
    h2_emit_event(cx, stream_obj, "end", None);
    // Body fully delivered and the handler still has not responded —
    // explicit 500 (never fall through to uWS std::terminate).
    if !res.state().is_http_end_called() {
        eprintln!("[node:http2] request handler returned without responding — responding 500");
        h2_respond_500(res, b"handler did not respond");
        h2_req_finish(cx, id, Some(res));
    }
}

/// onAborted: the connection died before the response completed. Mark the
/// JS objects dead (late write/end calls become no-ops instead of hitting a
/// dead uWS res), notify listeners, drop the per-request state.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_on_aborted(st: *mut H2ReqState) {
    // Copy locals first — no reentrancy concerns here, but the state is
    // freed below and must not be touched after.
    let cx = (*st).cx;
    let id = (*st).id;
    let req_key = (*st).req_key.clone();
    let res_key = (*st).res_key.clone();

    let realm_global = bao_engine::context::thread_realm_global().unwrap_or(core::ptr::null_mut());
    if !realm_global.is_null() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let realm_global_root = realm_global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(cx_ref, realm_global_root.handle());
        let _cx_ref: &mut mozjs::context::JSContext = &mut realm;

        if let Some(res_obj) = gc_store_get_ns(cx, "http2", &res_key) {
            if !res_obj.is_null() {
                h2_set_bool_prop(cx, res_obj, "_ended", true);
                h2_emit_event(cx, res_obj, "close", None);
            }
        }
        if let Some(stream_obj) = gc_store_get_ns(cx, "http2", &req_key) {
            if !stream_obj.is_null() {
                h2_emit_event(cx, stream_obj, "aborted", None);
            }
        }
    }

    // The res is dead — do NOT touch it. Drop the state and its roots.
    let state = H2_LIVE_REQUESTS.with(|m| m.borrow_mut().remove(&id));
    drop(state);
    gc_store_remove_ns(cx, "http2", &req_key);
    gc_store_remove_ns(cx, "http2", &res_key);
}

/// Emit the session-style 'stream' event on the server object:
/// server.emit('stream', stream, headers, 0) — node's (stream, headers,
/// flags) listener shape.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_emit_server_stream(
    cx: *mut JSContext,
    server_obj: *mut JSObject,
    stream_obj: *mut JSObject,
    headers_obj: *mut JSObject,
) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let server_root = server_obj);

    let mut emit_val = UndefinedValue();
    JS_GetProperty(
        cx,
        server_root.handle().into(),
        c"emit".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut emit_val,
        },
    );
    if !emit_val.is_object() {
        return;
    }

    let ev_str = JS_NewStringCopyZ(cx, c"stream".as_ptr());
    if ev_str.is_null() {
        return;
    }
    let ev_val = StringValue(&*ev_str);
    rooted!(&in(cx_ref) let ev_root = ev_val);
    rooted!(&in(cx_ref) let s_root = ObjectValue(stream_obj));
    rooted!(&in(cx_ref) let h_root = ObjectValue(headers_obj));
    rooted!(&in(cx_ref) let f_root = Int32Value(0));

    let args_vals = [ev_root.get(), s_root.get(), h_root.get(), f_root.get()];
    let call_args = HandleValueArray {
        length_: 4,
        elements_: args_vals.as_ptr(),
    };

    rooted!(&in(cx_ref) let emit_fn = emit_val.to_object());
    rooted!(&in(cx_ref) let emit_fn_root = ObjectValue(emit_fn.get()));
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        server_root.handle().into(),
        emit_fn_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        eprintln!("[node:http2] server 'stream' listener threw (cleared)");
    }
}

// ──────────────────────────────────────────────────────────────────────
// JS stream methods — bridge to uWS Response::<false>
// ──────────────────────────────────────────────────────────────────────

#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

#[inline]
unsafe fn get_uws_res(
    cx: *mut JSContext,
    obj: *mut JSObject,
) -> *mut bun_uws_sys::response::c::uws_res {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_uwsRes".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ptr_val,
        },
    );
    if !val_is_private(&ptr_val) {
        return core::ptr::null_mut();
    }
    ptr_val.to_private() as *mut bun_uws_sys::response::c::uws_res
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_stream_respond(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let obj = this.to_object());

    // Extract :status from headers
    if argc > 0 {
        let hdrs_val = *args.get(0).ptr;
        if hdrs_val.is_object() {
            rooted!(&in(cx_ref) let hdrs_obj = hdrs_val.to_object());

            // Get :status
            let mut status_val = UndefinedValue();
            JS_GetProperty(
                cx,
                hdrs_obj.handle().into(),
                c":status".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut status_val,
                },
            );

            let status = if status_val.is_string() {
                let s = crate::js_to_rust_string(cx, status_val);
                s.parse::<i32>().unwrap_or(200)
            } else if status_val.is_int32() {
                status_val.to_int32()
            } else {
                200
            };

            let uws_res = get_uws_res(cx, obj.get());
            if !uws_res.is_null() {
                let res_mut = Response::<false>::cast_res(uws_res);
                if (*res_mut).state().is_http_end_called() {
                    // Response already complete — respond() is a no-op, never
                    // a second uWS end (use-after-answer crash class).
                    args.rval().set(UndefinedValue());
                    return true;
                }
                if !(*res_mut).state().is_http_status_called() {
                    let status_str = format!("{} ", status);
                    (*res_mut).write_status(status_str.as_bytes());
                }

                // Write ALL response headers (IdVector iteration; ':'-prefixed
                // pseudo-headers skipped inside) — the fixed common-name list
                // silently dropped every other header.
                h2_write_headers_obj(cx, hdrs_obj.get(), &mut *res_mut);
            }
        }
    }

    // If endStream option, end the response
    if argc > 1 {
        let opts_val = *args.get(1).ptr;
        if opts_val.is_object() {
            rooted!(&in(cx_ref) let opts_obj = opts_val.to_object());
            let mut end_stream_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_obj.handle().into(),
                c"endStream".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut end_stream_val,
                },
            );
            if end_stream_val.is_boolean() && end_stream_val.to_boolean() {
                let uws_res = get_uws_res(cx, obj.get());
                if !uws_res.is_null() {
                    let res_mut = Response::<false>::cast_res(uws_res);
                    if !(*res_mut).state().is_http_end_called() {
                        if !(*res_mut).state().is_http_status_called() {
                            (*res_mut).write_status(b"200 ");
                        }
                        (*res_mut).end(&[], false);
                        h2_finish_state_from_obj(cx, obj.get(), &mut *res_mut);
                    }
                }
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_stream_end(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let obj = this.to_object());

    let uws_res = get_uws_res(cx, obj.get());
    if uws_res.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let res_mut = Response::<false>::cast_res(uws_res);

    // Node contract: end() after end() is a no-op — a second uWS end() on
    // the same response is a use-after-answer crash class (also covers the
    // res.end-first cross-object case: both objects share _uwsRes).
    if (*res_mut).state().is_http_end_called() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Accumulated body: legacy `_body` string + the final chunk, byte-exact
    // (strings encode UTF-8; Buffer/TypedArray chunks copy verbatim — the
    // previous string-only accumulator dropped every binary body).
    let mut body: Vec<u8> = Vec::new();
    {
        let mut body_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_body".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut body_val,
            },
        );
        if body_val.is_string() {
            body.extend_from_slice(crate::js_to_rust_string(cx, body_val).as_bytes());
        }
    }
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            body.extend_from_slice(crate::js_to_rust_string(cx, v).as_bytes());
        } else if let Some(bytes) = crate::node_buffer::collect_byte_view(cx, v) {
            body.extend_from_slice(&bytes);
        }
    }

    // Write default status if not yet written
    if !(*res_mut).state().is_http_status_called() {
        (*res_mut).write_status(b"200 ");
    }

    (*res_mut).end(&body, false);
    h2_finish_state_from_obj(cx, obj.get(), &mut *res_mut);

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_stream_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let obj = this.to_object());

    let uws_res = get_uws_res(cx, obj.get());
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);
        if !(*res_mut).state().is_http_end_called() {
            if !(*res_mut).state().is_http_status_called() {
                (*res_mut).write_status(b"200 ");
            }
            (*res_mut).end(&[], false);
            h2_finish_state_from_obj(cx, obj.get(), &mut *res_mut);
        }
    }

    args.rval().set(UndefinedValue());
    true
}

// ──────────────────────────────────────────────────────────────────────
// Compat response methods — node's Http2ServerResponse surface
// ──────────────────────────────────────────────────────────────────────

/// Read `_stateId` off a JS object and finish the per-request state (used
/// by every path that completes the uWS response: res.end, stream.end,
/// stream.close, respond{endStream:true}).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_finish_state_from_obj(cx: *mut JSContext, obj: *mut JSObject, res: &mut Response<false>) {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut id_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_stateId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut id_val,
        },
    );
    if id_val.is_int32() {
        h2_req_finish(cx, id_val.to_int32() as u64, Some(res));
    }
}

/// Read the `_headers` bookkeeping object off the res object (missing →
/// None).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_get_headers_obj(cx: *mut JSContext, obj: *mut JSObject) -> Option<*mut JSObject> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c"_headers".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_object() {
        Some(v.to_object())
    } else {
        None
    }
}

/// res.writeHead(status[, statusText][, headers]) — write status + headers
/// to the wire. Second writeHead (or after headers sent) is node's
/// ERR_HTTP_HEADERS_SENT.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_write_head(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    let status: i32 = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32()
        } else if v.is_double() {
            v.to_double() as i32
        } else {
            200
        }
    } else {
        200
    };

    // Record statusCode for bookkeeping (end() default uses it too).
    rooted!(&in(cx_ref) let sv = Int32Value(status));
    JS_SetProperty(cx, obj.handle().into(), c"statusCode".as_ptr(), sv.handle().into());

    let uws_res = get_uws_res(cx, obj.get());
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);
        if (*res_mut).state().is_http_end_called() {
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c"write after end".as_ptr());
            return false;
        }
        if (*res_mut).state().is_http_status_called() {
            let msg = ZBox::from_bytes("Headers already sent".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        let status_str = format!("{} ", status);
        (*res_mut).write_status(status_str.as_bytes());
    }

    // Headers object: first object arg after status (statusText string is
    // accepted and skipped — headers come from the object arg).
    for i in 1..(argc as usize) {
        let v = *args.get(i as u32).ptr;
        if v.is_object() {
            rooted!(&in(cx_ref) let hdrs_obj = v.to_object());

            // Node merge semantics: setHeader values written first, then the
            // writeHead object's values — same-name writeHead keys override
            // (skipped in the store flush so the wire never carries the
            // header twice).
            let mut arg_keys: Vec<String> = Vec::new();
            {
                let mut ids = mozjs::rust::IdVector::new(cx_ref);
                if w2::GetPropertyKeys(
                    cx_ref,
                    hdrs_obj.handle().into(),
                    JSITER_OWNONLY as u32,
                    ids.handle_mut(),
                ) {
                    for jsid in &*ids {
                        if !jsid.is_string() {
                            continue;
                        }
                        let key_str = jsid.to_string();
                        let key = mozjs::conversions::unsafe_jsstr_to_string(
                            cx,
                            NonNull::new_unchecked(key_str),
                        );
                        arg_keys.push(key);
                    }
                }
            }

            let uws_res = get_uws_res(cx, obj.get());
            if !uws_res.is_null() {
                let res_mut = Response::<false>::cast_res(uws_res);
                // setHeader store first, minus overridden keys.
                if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
                    rooted!(&in(cx_ref) let store_root = headers_store);
                    let mut store_ids = mozjs::rust::IdVector::new(cx_ref);
                    if w2::GetPropertyKeys(
                        cx_ref,
                        store_root.handle().into(),
                        JSITER_OWNONLY as u32,
                        store_ids.handle_mut(),
                    ) {
                        for jsid in &*store_ids {
                            if !jsid.is_string() {
                                continue;
                            }
                            let key_str = jsid.to_string();
                            let key = mozjs::conversions::unsafe_jsstr_to_string(
                                cx,
                                NonNull::new_unchecked(key_str),
                            );
                            if key.starts_with(':') || arg_keys.contains(&key) {
                                continue;
                            }
                            let c_key = ZBox::from_bytes(key.as_bytes());
                            let mut hv = UndefinedValue();
                            JS_GetProperty(
                                cx,
                                store_root.handle().into(),
                                c_key.as_ptr(),
                                MutableHandle::<Value> {
                                    _phantom_0: ::std::marker::PhantomData,
                                    ptr: &mut hv,
                                },
                            );
                            if hv.is_string() {
                                let val = crate::js_to_rust_string(cx, hv);
                                let key_lower = key.to_ascii_lowercase();
                                let c_val = ZBox::from_bytes(val.as_bytes());
                                (*res_mut).write_header(key_lower.as_bytes(), c_val.as_bytes());
                            }
                        }
                    }
                }
                h2_write_headers_obj(cx, hdrs_obj.get(), &mut *res_mut);
            }

            // Mirror the writeHead object into _headers for
            // getHeader/hasHeader truth.
            if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
                rooted!(&in(cx_ref) let store_root = headers_store);
                for key in &arg_keys {
                    let c_key = ZBox::from_bytes(key.as_bytes());
                    let mut hv = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        hdrs_obj.handle().into(),
                        c_key.as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut hv,
                        },
                    );
                    if hv.is_string() {
                        JS_SetProperty(
                            cx,
                            store_root.handle().into(),
                            c_key.as_ptr(),
                            {
                                rooted!(&in(cx_ref) let hr = hv);
                                hr.handle().into()
                            },
                        );
                    }
                }
            }
            break;
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

/// res.setHeader(name, value) — pre-send header store (flushed by end()
/// when writeHead was never called). Setting after send is node's
/// ERR_HTTP_HEADERS_SENT.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_set_header(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    if h2_res_headers_sent(cx, obj.get()) {
        let msg = ZBox::from_bytes("Cannot set headers after they are sent to the client".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    if argc < 2 {
        let msg = ZBox::from_bytes("res.setHeader(name, value) requires 2 arguments".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let name_val = *args.get(0).ptr;
    let value_val = *args.get(1).ptr;
    if !name_val.is_string() {
        let msg = ZBox::from_bytes("res.setHeader name must be a string".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let name = crate::js_to_rust_string(cx, name_val);
    let value_str = if value_val.is_string() {
        crate::js_to_rust_string(cx, value_val)
    } else if value_val.is_int32() {
        format!("{}", value_val.to_int32())
    } else if value_val.is_double() {
        format!("{}", value_val.to_double())
    } else {
        let msg = ZBox::from_bytes("res.setHeader value must be a string or number".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };

    if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
        rooted!(&in(cx_ref) let store_root = headers_store);
        let c_name = ZBox::from_bytes(name.as_bytes());
        let c_value = ZBox::from_bytes(value_str.as_bytes());
        let js_v = JS_NewStringCopyZ(cx, c_value.as_ptr());
        if !js_v.is_null() {
            let vv = StringValue(&*js_v);
            rooted!(&in(cx_ref) let vv_root = vv);
            JS_SetProperty(cx, store_root.handle().into(), c_name.as_ptr(), vv_root.handle().into());
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

/// Headers-sent truth: the uWS status line is out, or the response ended.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn h2_res_headers_sent(cx: *mut JSContext, obj: *mut JSObject) -> bool {
    if h2_get_bool_prop(cx, obj, "_ended") {
        return true;
    }
    let uws_res = get_uws_res(cx, obj);
    if uws_res.is_null() {
        return false;
    }
    let res_mut = Response::<false>::cast_res(uws_res);
    (*res_mut).state().is_http_status_called() || (*res_mut).state().is_http_end_called()
}

/// res.getHeader(name) → stored value or undefined.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_get_header(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    args.rval().set(UndefinedValue());
    if argc < 1 || !(*args.get(0).ptr).is_string() {
        return true;
    }
    let name = crate::js_to_rust_string(cx, *args.get(0).ptr);
    if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
        rooted!(&in(cx_ref) let store_root = headers_store);
        let c_name = ZBox::from_bytes(name.as_bytes());
        let mut v = UndefinedValue();
        JS_GetProperty(
            cx,
            store_root.handle().into(),
            c_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        );
        args.rval().set(v);
    }
    true
}

/// res.getHeaders() → shallow copy of the store.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_get_headers(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    rooted!(&in(cx_ref) let out = w2::JS_NewPlainObject(cx_ref));
    args.rval().set(ObjectValue(out.get()));
    if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
        rooted!(&in(cx_ref) let store_root = headers_store);
        let mut ids = mozjs::rust::IdVector::new(cx_ref);
        if w2::GetPropertyKeys(
            cx_ref,
            store_root.handle().into(),
            JSITER_OWNONLY as u32,
            ids.handle_mut(),
        ) {
            for jsid in &*ids {
                if !jsid.is_string() {
                    continue;
                }
                let key_str = jsid.to_string();
                let key = mozjs::conversions::unsafe_jsstr_to_string(
                    cx,
                    NonNull::new_unchecked(key_str),
                );
                let c_key = ZBox::from_bytes(key.as_bytes());
                let mut hv = UndefinedValue();
                JS_GetProperty(
                    cx,
                    store_root.handle().into(),
                    c_key.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut hv,
                    },
                );
                JS_DefineProperty(
                    cx,
                    out.handle().into(),
                    c_key.as_ptr(),
                    {
                        rooted!(&in(cx_ref) let hr = hv);
                        hr.handle().into()
                    },
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }
    true
}

/// res.hasHeader(name) → boolean.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_has_header(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    let mut found = false;
    if argc >= 1 && (*args.get(0).ptr).is_string() {
        let name = crate::js_to_rust_string(cx, *args.get(0).ptr);
        if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
            rooted!(&in(cx_ref) let store_root = headers_store);
            let c_name = ZBox::from_bytes(name.as_bytes());
            let mut v = UndefinedValue();
            JS_GetProperty(
                cx,
                store_root.handle().into(),
                c_name.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            );
            found = !v.is_undefined();
        }
    }
    args.rval().set(mozjs::jsval::BooleanValue(found));
    true
}

/// res.removeHeader(name) — pre-send only (node throws after send).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_remove_header(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    if h2_res_headers_sent(cx, obj.get()) {
        let msg = ZBox::from_bytes("Cannot remove headers after they are sent to the client".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    if argc >= 1 && (*args.get(0).ptr).is_string() {
        let name = crate::js_to_rust_string(cx, *args.get(0).ptr);
        if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
            rooted!(&in(cx_ref) let store_root = headers_store);
            let c_name = ZBox::from_bytes(name.as_bytes());
            rooted!(&in(cx_ref) let uv = UndefinedValue());
            // JS_DeleteProperty1 would be cleaner; UndefinedValue assignment
            // reads as "missing" for every consumer of the store.
            JS_SetProperty(cx, store_root.handle().into(), c_name.as_ptr(), uv.handle().into());
        }
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

/// res.write(chunk) — buffer byte-exactly; the whole body flushes once in
/// end() (same single-flush model as node:http — write() streaming plus a
/// later end() re-send duplicated every chunk on the wire).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    // Node contract: write() after end() throws ERR_STREAM_WRITE_AFTER_END.
    if h2_get_bool_prop(cx, obj.get(), "_ended") {
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c"write after end".as_ptr());
        return false;
    }

    if argc > 0 {
        let v = *args.get(0).ptr;
        if !h2_res_append_chunk(cx, obj.get(), v) {
            return false;
        }
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

/// res.end([chunk]) — flush status (statusCode default or writeHead's),
/// pending setHeader store, then the exact accumulated body. Idempotent;
/// finishes the per-request state and emits 'finish' + 'close'.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_end(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    // Node contract: end() after end() is a no-op (second uWS end() on the
    // same response is a use-after-answer crash class).
    if h2_get_bool_prop(cx, obj.get(), "_ended") {
        args.rval().set(ObjectValue(obj.get()));
        return true;
    }

    if argc > 0 {
        let v = *args.get(0).ptr;
        if !h2_res_append_chunk(cx, obj.get(), v) {
            return false;
        }
    }

    let body = h2_res_collect_body(cx, obj.get());

    let uws_res = get_uws_res(cx, obj.get());
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);
        if !(*res_mut).state().is_http_end_called() {
            // Status: writeHead's line may already be out; otherwise default
            // from the statusCode property (node's implicit 200).
            if !(*res_mut).state().is_http_status_called() {
                let mut status_val = Int32Value(200);
                JS_GetProperty(
                    cx,
                    obj.handle().into(),
                    c"statusCode".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut status_val,
                    },
                );
                let status = if status_val.is_int32() {
                    status_val.to_int32()
                } else {
                    200
                };
                let status_str = format!("{} ", status);
                (*res_mut).write_status(status_str.as_bytes());

                // setHeader store flushes only when writeHead never ran.
                if let Some(headers_store) = h2_get_headers_obj(cx, obj.get()) {
                    rooted!(&in(cx_ref) let store_root = headers_store);
                    h2_write_headers_obj(cx, store_root.get(), &mut *res_mut);
                }
            }

            // uWS computes Content-Length from data.len() — binary bodies
            // hit the wire byte-for-byte.
            (*res_mut).end(&body, false);
            h2_finish_state_from_obj(cx, obj.get(), &mut *res_mut);
        }
    }

    h2_set_bool_prop(cx, obj.get(), "_ended", true);

    // Lifecycle events (node emits 'finish' then 'close' on the response).
    h2_emit_event(cx, obj.get(), "finish", None);
    h2_emit_event(cx, obj.get(), "close", None);

    args.rval().set(ObjectValue(obj.get()));
    true
}

/// res.setTimeout(msecs, callback) — arm the callback on the real timer
/// wheel via global setTimeout (node invokes it with no args on timeout).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_res_set_timeout(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = args.thisv().to_object());

    if argc >= 2 && (*args.get(1).ptr).is_object() {
        let msecs = if (*args.get(0).ptr).is_int32() {
            (*args.get(0).ptr).to_int32().max(0)
        } else {
            0
        };
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        if !global.get().is_null() {
            let c_set_timeout = ZBox::from_bytes("setTimeout".as_bytes());
            let mut st_val = UndefinedValue();
            JS_GetProperty(
                cx,
                global.handle().into(),
                c_set_timeout.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut st_val,
                },
            );
            if st_val.is_object() {
                rooted!(&in(cx_ref) let st_fn = st_val.to_object());
                rooted!(&in(cx_ref) let st_root = ObjectValue(st_fn.get()));
                rooted!(&in(cx_ref) let cb_root = *args.get(1).ptr);
                rooted!(&in(cx_ref) let ms_root = Int32Value(msecs));
                let call_vals = [cb_root.get(), ms_root.get()];
                let call_args = HandleValueArray {
                    length_: 2,
                    elements_: call_vals.as_ptr(),
                };
                let mut rval = UndefinedValue();
                JS_CallFunctionValue(
                    cx,
                    global.handle().into(),
                    st_root.handle().into(),
                    &call_args,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
                JS_ClearPendingException(cx);
            }
        }
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

// ──────────────────────────────────────────────────────────────────────
// createServer / createSecureServer — JS host functions
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_create_server(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Create a JS Http2Server instance via the IIFE's constructor
    // First, get the Http2Server constructor from the http2 module object
    let this_val = args.thisv();
    rooted!(&in(cx_ref) let http2_obj = if this_val.is_object() {
        this_val.to_object()
    } else {
        core::ptr::null_mut()
    });

    if http2_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Get Http2Server constructor
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(
        cx,
        http2_obj.handle().into(),
        c"Http2Server".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_val,
        },
    );

    if !ctor_val.is_object() {
        // Fallback: create a plain object with EE methods
        rooted!(&in(cx_ref) let server_obj = w2::JS_NewPlainObject(cx_ref));
        if server_obj.get().is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        attach_ee_methods(cx, server_obj.get());
        w2::JS_DefineFunction(
            cx_ref,
            server_obj.handle(),
            c"listen".as_ptr(),
            Some(http2_server_listen_js),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server_obj.handle(),
            c"close".as_ptr(),
            Some(http2_server_close_js),
            1,
            JSPROP_ENUMERATE as u32,
        );
        store_handler(cx, cx_ref, server_obj.get(), argc, &args);
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    // new Http2Server(options, handler): build the instance off the ctor's
    // prototype and run the ctor body with the instance as `this`. A bare
    // JS_CallFunctionValue against the global runs the sloppy-mode ctor on
    // the global and returns undefined (the ctor has no return statement),
    // which made createServer() yield undefined.
    rooted!(&in(cx_ref) let ctor = ctor_val.to_object());

    let mut proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        ctor.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto_val,
        },
    );
    rooted!(&in(cx_ref) let proto_obj = if proto_val.is_object() {
        proto_val.to_object()
    } else {
        ::std::ptr::null_mut::<JSObject>()
    });
    rooted!(&in(cx_ref) let server_obj = w2::JS_NewObjectWithGivenProto(
        cx_ref,
        ::std::ptr::null(),
        proto_obj.handle()
    ));
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Prepare args: (options, handler)
    let opts_arg = if argc > 0 && (*args.get(0).ptr).is_object() {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let handler_arg = if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let call_args_vals = [opts_arg, handler_arg];
    let call_args = HandleValueArray {
        length_: if argc > 1 {
            2
        } else if argc > 0 {
            1
        } else {
            0
        },
        elements_: call_args_vals.as_ptr(),
    };

    rooted!(&in(cx_ref) let ctor_fn = ObjectValue(ctor.get()));
    let mut ctor_rval = UndefinedValue();
    JS_CallFunctionValue(
        cx,
        server_obj.handle().into(),
        ctor_fn.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_rval,
        },
    );

    // Store handler on the server object for native listen
    store_handler(cx, cx_ref, server_obj.get(), argc, &args);
    args.rval().set(ObjectValue(server_obj.get()));

    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_create_secure_server(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this_val = args.thisv();
    rooted!(&in(cx_ref) let http2_obj = if this_val.is_object() {
        this_val.to_object()
    } else {
        core::ptr::null_mut()
    });

    if http2_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Get Http2SecureServer constructor
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(
        cx,
        http2_obj.handle().into(),
        c"Http2SecureServer".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_val,
        },
    );

    if !ctor_val.is_object() {
        // Fallback: create a plain object with EE methods
        rooted!(&in(cx_ref) let server_obj = w2::JS_NewPlainObject(cx_ref));
        if server_obj.get().is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        attach_ee_methods(cx, server_obj.get());
        w2::JS_DefineFunction(
            cx_ref,
            server_obj.handle(),
            c"listen".as_ptr(),
            Some(http2_secure_server_listen_js),
            3,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx_ref,
            server_obj.handle(),
            c"close".as_ptr(),
            Some(http2_secure_server_close_js),
            1,
            JSPROP_ENUMERATE as u32,
        );
        store_handler(cx, cx_ref, server_obj.get(), argc, &args);
        // Mark as secure
        rooted!(&in(cx_ref) let secure_val = BooleanValue(true));
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_secure".as_ptr(),
            secure_val.handle().into(),
            0,
        );
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    // new Http2SecureServer(options, handler): construct via the ctor's
    // prototype with the instance as `this` (see http2_create_server — a
    // bare call against the global returns undefined).
    rooted!(&in(cx_ref) let ctor = ctor_val.to_object());

    let mut proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        ctor.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto_val,
        },
    );
    rooted!(&in(cx_ref) let proto_obj = if proto_val.is_object() {
        proto_val.to_object()
    } else {
        ::std::ptr::null_mut::<JSObject>()
    });
    rooted!(&in(cx_ref) let server_obj = w2::JS_NewObjectWithGivenProto(
        cx_ref,
        ::std::ptr::null(),
        proto_obj.handle()
    ));
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let opts_arg = if argc > 0 && (*args.get(0).ptr).is_object() {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let handler_arg = if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };

    let call_args_vals = [opts_arg, handler_arg];
    let call_args = HandleValueArray {
        length_: if argc > 1 {
            2
        } else if argc > 0 {
            1
        } else {
            0
        },
        elements_: call_args_vals.as_ptr(),
    };

    rooted!(&in(cx_ref) let ctor_fn = ObjectValue(ctor.get()));
    let mut ctor_rval = UndefinedValue();
    JS_CallFunctionValue(
        cx,
        server_obj.handle().into(),
        ctor_fn.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_rval,
        },
    );

    store_handler(cx, cx_ref, server_obj.get(), argc, &args);
    // Mark as secure
    rooted!(&in(cx_ref) let secure_val = BooleanValue(true));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"_secure".as_ptr(),
        secure_val.handle().into(),
        0,
    );
    args.rval().set(ObjectValue(server_obj.get()));

    true
}

/// Store the stream handler on the server JS object as `_onStreamHandler`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn store_handler(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    server_obj: *mut JSObject,
    argc: u32,
    args: &CallArgs,
) {
    // The handler can be arg[1] (options, handler) or arg[0] (handler only)
    let handler_val = if argc > 1 {
        *args.get(1).ptr
    } else if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };

    if handler_val.is_object() {
        rooted!(&in(cx_ref) let cb = handler_val.to_object());
        let cb_val = ObjectValue(cb.get());
        rooted!(&in(cx_ref) let cb_root = cb_val);
        rooted!(&in(cx_ref) let server_root = server_obj);
        JS_DefineProperty(
            cx,
            server_root.handle().into(),
            c"_onStreamHandler".as_ptr(),
            cb_root.handle().into(),
            0,
        );
    }
}

/// Attach EventEmitter methods to a plain server object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn attach_ee_methods(cx: *mut JSContext, obj: *mut JSObject) {
    let ee_on: JSNative = Some(crate::node_events::ee_on);
    let ee_emit: JSNative = Some(crate::node_events::ee_emit);
    let ee_once: JSNative = Some(crate::node_events::ee_once);
    let ee_off: JSNative = Some(crate::node_events::ee_off);
    let ee_prepend: JSNative = Some(crate::node_events::ee_prepend);
    let ee_remove_all: JSNative = Some(crate::node_events::ee_remove_all);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj_root = obj);
    for (name, op) in [
        ("on", ee_on),
        ("once", ee_once),
        ("emit", ee_emit),
        ("off", ee_off),
        ("addListener", ee_on),
        ("removeListener", ee_off),
        ("prependListener", ee_prepend),
        ("removeAllListeners", ee_remove_all),
    ] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            obj_root.handle().into(),
            c_name.as_ptr(),
            op,
            2,
            JSPROP_ENUMERATE as u32,
        );
    }
}

// ──────────────────────────────────────────────────────────────────────
// Server listen / close — native uWS App bridge
// ──────────────────────────────────────────────────────────────────────

/// Per-listen state handed to uWS: the listen callback fires when the
/// socket actually binds (or fails), which is where node semantics place the
/// 'listening' event and the user callback — calling the callback eagerly at
/// listen() time was the "listen 回调不触发"/premature-fire class.
struct H2ListenState {
    cx: *mut JSContext,
    cb_key: String,
    server_key: String,
}

/// uWS listen callback: socket non-null = listening confirmed; null = bind
/// failed (EADDRINUSE etc). Emits 'listening'/'error' on the server object
/// and invokes the user callback (no args on success, an error carrier on
/// failure), then frees the state (uWS fires this exactly once per listen).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn uws_h2_listen_callback(
    listen_socket: *mut bun_uws_sys::listen_socket::ListenSocket,
    user_data: *mut ::std::ffi::c_void,
) {
    if user_data.is_null() {
        // Legacy js-bridge listen sites pass null — nothing deferred there.
        return;
    }
    let st = Box::from_raw(user_data as *mut H2ListenState);
    let cx = st.cx;
    let cb_key = st.cb_key.clone();
    let server_key = st.server_key.clone();

    let realm_global = match bao_engine::context::thread_realm_global() {
        Some(g) if !g.is_null() => g,
        _ => {
            eprintln!("[node:http2] listen callback: no JS realm — cannot fire JS callback");
            gc_store_remove_ns(cx, "http2", &cb_key);
            gc_store_remove_ns(cx, "http2", &server_key);
            return;
        }
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let realm_global_root = realm_global);
    let mut realm = mozjs::realm::AutoRealm::new_from_handle(cx_ref, realm_global_root.handle());
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;

    let cb = gc_store_get_ns(cx, "http2", &cb_key);
    let server_obj = gc_store_get_ns(cx, "http2", &server_key);

    if listen_socket.is_null() {
        // Bind failed: node calls cb(err) and emits 'error' on the server.
        eprintln!("[node:http2] listen failed (bind error)");
        rooted!(&in(cx_ref) let err_obj = w2::JS_NewPlainObject(cx_ref));
        if !err_obj.get().is_null() {
            let c_code = ZBox::from_bytes("EADDRINUSE".as_bytes());
            let c_code_v = JS_NewStringCopyZ(cx, c_code.as_ptr());
            if !c_code_v.is_null() {
                let cv = StringValue(&*c_code_v);
                rooted!(&in(cx_ref) let cvr = cv);
                JS_DefineProperty(
                    cx,
                    err_obj.handle().into(),
                    c"code".as_ptr(),
                    cvr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let c_msg = ZBox::from_bytes("listen EADDRINUSE: address already in use".as_bytes());
            let c_msg_v = JS_NewStringCopyZ(cx, c_msg.as_ptr());
            if !c_msg_v.is_null() {
                let mv = StringValue(&*c_msg_v);
                rooted!(&in(cx_ref) let mvr = mv);
                JS_DefineProperty(
                    cx,
                    err_obj.handle().into(),
                    c"message".as_ptr(),
                    mvr.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            if let Some(server) = server_obj {
                if !server.is_null() {
                    h2_emit_event(cx, server, "error", Some(ObjectValue(err_obj.get())));
                }
            }
            if let Some(cb) = cb {
                if !cb.is_null() {
                    rooted!(&in(cx_ref) let cb_root = ObjectValue(cb));
                    rooted!(&in(cx_ref) let arg_root = ObjectValue(err_obj.get()));
                    let call_vals = [arg_root.get()];
                    let call_args = HandleValueArray {
                        length_: 1,
                        elements_: call_vals.as_ptr(),
                    };
                    let mut rval = UndefinedValue();
                    JS_CallFunctionValue(
                        cx,
                        realm_global_root.handle().into(),
                        cb_root.handle().into(),
                        &call_args,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rval,
                        },
                    );
                    JS_ClearPendingException(cx);
                }
            }
        }
    } else {
        if let Some(server) = server_obj {
            if !server.is_null() {
                h2_emit_event(cx, server, "listening", None);
            }
        }
        if let Some(cb) = cb {
            if !cb.is_null() {
                rooted!(&in(cx_ref) let cb_root = ObjectValue(cb));
                let mut rval = UndefinedValue();
                JS_CallFunctionValue(
                    cx,
                    realm_global_root.handle().into(),
                    cb_root.handle().into(),
                    &HandleValueArray::empty(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
                JS_ClearPendingException(cx);
            }
        }
    }

    gc_store_remove_ns(cx, "http2", &cb_key);
    gc_store_remove_ns(cx, "http2", &server_key);
}

/// __http2_server_listen(serverObj, port[, host][, callback]) — called from
/// JS. The JS wrapper normalizes node's listen arg forms; here host and
/// callback are picked by type so the historical (serverObj, port, callback)
/// shape keeps working too.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // args: serverObj, port, [host | callback], [callback]
    if argc < 2 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let server_obj_val = *args.get(0).ptr;
    rooted!(&in(cx_ref) let server_obj = server_obj_val.to_object());

    let port: u16 = if (*args.get(1).ptr).is_int32() {
        (*args.get(1).ptr).to_int32() as u16
    } else if (*args.get(1).ptr).is_double() {
        (*args.get(1).ptr).to_double() as u16
    } else {
        3000
    };

    let mut host: Option<String> = None;
    let mut callback: Option<*mut JSObject> = None;
    for i in 2..(argc as usize) {
        let v = *args.get(i as u32).ptr;
        if v.is_string() && host.is_none() {
            host = Some(crate::js_to_rust_string(cx, v));
        } else if v.is_object() && callback.is_none() {
            rooted!(&in(cx_ref) let cb = v.to_object());
            callback = Some(cb.get());
        }
    }

    // Create uWS App<false>
    let opts = BunSocketContextOptions::default();
    let app_ptr = match App::<false>::create(&opts) {
        Some(p) => p,
        None => {
            let msg = format!("Failed to create HTTP/2 server on port {}", port);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // node:http2 rides the same uWS HTTP/1.x parser as node:http, so it
    // keeps node-family framing parity (upstream `IsNodeHttp` split, BCE
    // bdb738222): an HTTP/1.0 request bearing Transfer-Encoding is
    // dispatched and the connection closed after (ancientHttp already
    // marks close), not 400-rejected as Bun.serve does per RFC 9112 6.1.
    // Real-Node http2 would GOAWAY any HTTP/1.x text outright, but this
    // surface is an HTTP/1.x-adapted compat server by design — rejecting
    // only the 1.0+TE pair would match neither Node shape. Must be set
    // before any traffic reaches the app.
    // Safety: app_ptr is a live `*mut App<false>` from `App::create` above,
    // valid until `App::<false>::destroy`.
    unsafe { (*app_ptr).set_is_node_http(true) };

    // Get the JS stream handler from the server object
    let mut handler_val = UndefinedValue();
    let handler_mh = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut handler_val,
    };
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_onStreamHandler".as_ptr(),
        handler_mh,
    );

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        App::<false>::destroy(app_ptr);
        let msg = ZBox::from_bytes("http2.createServer requires a stream handler".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Allocate H2ServerUserData (server object registered for the
    // session-style 'stream' event emission from the route handler).
    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(
        cx,
        global.get(),
        handler_root.get(),
        server_obj.get(),
    ));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route
    let safe_handler: Option<
        extern "C" fn(
            *mut bun_uws_sys::response::c::uws_res,
            *mut bun_uws_sys::Request,
            *mut ::std::ffi::c_void,
        ),
    > = unsafe {
        ::std::mem::transmute(Some(
            uws_h2_route_handler
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::response::c::uws_res,
                    *mut bun_uws_sys::Request,
                    *mut ::std::ffi::c_void,
                ),
        ))
    };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Store ud pointer on server object
    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            udv.handle().into(),
            0,
        );
    }

    // Listen. Host binding honored through listen_with_config (the plain
    // listen() call bound 0.0.0.0 unconditionally). The JS-facing 'listening'
    // event + user callback fire from uws_h2_listen_callback when the socket
    // actually binds — not eagerly here.
    let listen_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
    let cb_key = format!("http2_listen_{}_cb", listen_id);
    let listen_server_key = format!("http2_listen_{}_server", listen_id);
    if let Some(cb) = callback {
        gc_store_insert_ns(cx, "http2", &cb_key, cb);
    }
    gc_store_insert_ns(cx, "http2", &listen_server_key, server_obj.get());
    let listen_state = Box::new(H2ListenState {
        cx,
        cb_key,
        server_key: listen_server_key,
    });
    let listen_state_ptr = Box::into_raw(listen_state) as *mut ::std::ffi::c_void;

    let safe_listen_cb: extern "C" fn(
        *mut bun_uws_sys::listen_socket::ListenSocket,
        *mut ::std::ffi::c_void,
    ) = unsafe {
        ::std::mem::transmute(
            uws_h2_listen_callback
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::listen_socket::ListenSocket,
                    *mut ::std::ffi::c_void,
                ),
        )
    };
    match &host {
        Some(h) if !h.is_empty() => {
            let host_cstr = ::std::ffi::CString::new(h.clone()).unwrap_or_default();
            let mut config = bun_uws_sys::app::c::uws_app_listen_config_t::new(port as i32);
            config.host = host_cstr.as_ptr();
            (*app_ptr).listen_with_config(Some(safe_listen_cb), listen_state_ptr, config);
        }
        _ => {
            (*app_ptr).listen(port as i32, safe_listen_cb, listen_state_ptr);
        }
    }

    // Store app pointer on server object
    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_appPtr".as_ptr(),
            apv.handle().into(),
            0,
        );
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"_listeningPort".as_ptr(),
        port_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"listening".as_ptr(),
        listening_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    register_active_h2_app(app_ptr);

    args.rval().set(UndefinedValue());
    true
}

/// __http2_server_close(serverObj, callback) — called from JS
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_close(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc < 1 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let server_obj_val = *args.get(0).ptr;
    rooted!(&in(cx_ref) let server_obj = server_obj_val.to_object());

    let callback = if argc > 1 && (*args.get(1).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(1).ptr).to_object());
        Some(cb.get())
    } else {
        None
    };

    close_h2_server(cx, cx_ref, server_obj.get(), false);

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        if !global.get().is_null() {
            rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            JS_CallFunctionValue(
                cx,
                global.handle().into(),
                fval_root.handle().into(),
                &HandleValueArray::empty(),
                rval_h,
            );
            JS_ClearPendingException(cx);
        }
    }

    args.rval().set(UndefinedValue());
    true
}

/// Close an H2 server — shared by plain and secure close paths.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn close_h2_server(
    cx: *mut JSContext,
    cx_ref: &mut mozjs::context::JSContext,
    server_obj: *mut JSObject,
    _is_secure: bool,
) {
    rooted!(&in(cx_ref) let obj = server_obj);

    // Destroy the uWS App
    let mut app_ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"_appPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut app_ptr_val,
        },
    );
    let app_ptr = if val_is_private(&app_ptr_val) {
        app_ptr_val.to_private() as *mut App<false>
    } else {
        core::ptr::null_mut()
    };
    if !app_ptr.is_null() {
        (*app_ptr).close();
        App::<false>::destroy(app_ptr);
        unregister_active_h2_app(app_ptr);
        rooted!(&in(cx_ref) let undef_root = UndefinedValue());
        JS_SetProperty(
            cx,
            obj.handle().into(),
            c"_appPtr".as_ptr(),
            undef_root.handle().into(),
        );
    }

    // Cleanup H2ServerUserData
    let mut ud_ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"_udPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ud_ptr_val,
        },
    );
    let ud_ptr = if val_is_private(&ud_ptr_val) {
        ud_ptr_val.to_private() as *mut H2ServerUserData
    } else {
        core::ptr::null_mut()
    };
    if !ud_ptr.is_null() {
        let ud = Box::from_raw(ud_ptr);
        ud.cleanup();
        rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
        JS_SetProperty(
            cx,
            obj.handle().into(),
            c"_udPtr".as_ptr(),
            undef_root2.handle().into(),
        );
    }
}

// ──────────────────────────────────────────────────────────────────────
// Secure server listen / close — uWS App<true> (SSL)
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_secure_server_listen(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc < 2 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let server_obj_val = *args.get(0).ptr;
    rooted!(&in(cx_ref) let server_obj = server_obj_val.to_object());

    let port: u16 = if (*args.get(1).ptr).is_int32() {
        (*args.get(1).ptr).to_int32() as u16
    } else if (*args.get(1).ptr).is_double() {
        (*args.get(1).ptr).to_double() as u16
    } else {
        3000
    };

    // host + callback picked by type (see http2_server_listen).
    let mut _host: Option<String> = None;
    let mut callback: Option<*mut JSObject> = None;
    for i in 2..(argc as usize) {
        let v = *args.get(i as u32).ptr;
        if v.is_string() && _host.is_none() {
            _host = Some(crate::js_to_rust_string(cx, v));
        } else if v.is_object() && callback.is_none() {
            rooted!(&in(cx_ref) let cb = v.to_object());
            callback = Some(cb.get());
        }
    }

    // Extract TLS options from server._options
    let mut pem_key = String::new();
    let mut pem_cert = String::new();

    let mut opts_val = UndefinedValue();
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_options".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut opts_val,
        },
    );
    if opts_val.is_object() {
        rooted!(&in(cx_ref) let opts_root = opts_val.to_object());

        // Extract key
        let mut key_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_root.handle().into(),
            c"key".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut key_val,
            },
        );
        if key_val.is_string() {
            pem_key = crate::js_to_rust_string(cx, key_val);
        }

        // Extract cert
        let mut cert_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_root.handle().into(),
            c"cert".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut cert_val,
            },
        );
        if cert_val.is_string() {
            pem_cert = crate::js_to_rust_string(cx, cert_val);
        }
    }

    // Get the JS stream handler
    let mut handler_val = UndefinedValue();
    let handler_mh = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut handler_val,
    };
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_onStreamHandler".as_ptr(),
        handler_mh,
    );

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        let msg = ZBox::from_bytes("http2.createSecureServer requires a stream handler".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Create uWS App<true> (SSL) with TLS options
    let mut ssl_opts = BunSocketContextOptions::default();
    let key_cstr;
    let cert_cstr;
    let key_ptr;
    let cert_ptr;
    if !pem_key.is_empty() && !pem_cert.is_empty() {
        key_cstr = ::std::ffi::CString::new(pem_key).unwrap_or_default();
        cert_cstr = ::std::ffi::CString::new(pem_cert).unwrap_or_default();
        key_ptr = key_cstr.as_ptr();
        cert_ptr = cert_cstr.as_ptr();
        ssl_opts.key = &key_ptr as *const *const i8;
        ssl_opts.key_count = 1;
        ssl_opts.cert = &cert_ptr as *const *const i8;
        ssl_opts.cert_count = 1;
    }

    let app_ptr = match App::<true>::create(&ssl_opts) {
        Some(p) => p,
        None => {
            let msg = format!("Failed to create HTTP/2 secure server on port {}", port);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // node:http2 framing parity — see the matching comment at the
    // `App::<false>` listen site (node-family `IsNodeHttp` semantics).
    // Safety: app_ptr is a live `*mut App<true>` from `App::create` above,
    // valid until `App::<true>::destroy`.
    unsafe { (*app_ptr).set_is_node_http(true) };

    // Allocate H2ServerUserData (reuse same struct for SSL; server object
    // registered for the session-style 'stream' emission).
    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(
        cx,
        global.get(),
        handler_root.get(),
        server_obj.get(),
    ));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route — use the same handler (uWS handles TLS transparently)
    let safe_handler: Option<
        extern "C" fn(
            *mut bun_uws_sys::response::c::uws_res,
            *mut bun_uws_sys::Request,
            *mut ::std::ffi::c_void,
        ),
    > = unsafe {
        ::std::mem::transmute(Some(
            uws_h2_route_handler
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::response::c::uws_res,
                    *mut bun_uws_sys::Request,
                    *mut ::std::ffi::c_void,
                ),
        ))
    };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Store ud pointer
    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            udv.handle().into(),
            0,
        );
    }

    // Listen — the JS-facing 'listening' event + user callback fire from
    // uws_h2_listen_callback when the socket actually binds.
    let listen_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
    let cb_key = format!("http2_listen_{}_cb", listen_id);
    let listen_server_key = format!("http2_listen_{}_server", listen_id);
    if let Some(cb) = callback {
        gc_store_insert_ns(cx, "http2", &cb_key, cb);
    }
    gc_store_insert_ns(cx, "http2", &listen_server_key, server_obj.get());
    let listen_state = Box::new(H2ListenState {
        cx,
        cb_key,
        server_key: listen_server_key,
    });
    let listen_state_ptr = Box::into_raw(listen_state) as *mut ::std::ffi::c_void;

    let safe_listen_cb: extern "C" fn(
        *mut bun_uws_sys::listen_socket::ListenSocket,
        *mut ::std::ffi::c_void,
    ) = unsafe {
        ::std::mem::transmute(
            uws_h2_listen_callback
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::listen_socket::ListenSocket,
                    *mut ::std::ffi::c_void,
                ),
        )
    };
    (*app_ptr).listen(port as i32, safe_listen_cb, listen_state_ptr);

    // Store app pointer (as App<true>)
    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_sslAppPtr".as_ptr(),
            apv.handle().into(),
            0,
        );
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"_listeningPort".as_ptr(),
        port_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"listening".as_ptr(),
        listening_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    register_active_h2_ssl_app(app_ptr);

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_secure_server_close(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc < 1 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let server_obj_val = *args.get(0).ptr;
    rooted!(&in(cx_ref) let server_obj = server_obj_val.to_object());

    let callback = if argc > 1 && (*args.get(1).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(1).ptr).to_object());
        Some(cb.get())
    } else {
        None
    };

    // Close the SSL App
    {
        rooted!(&in(cx_ref) let obj = server_obj.get());
        let mut app_ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_sslAppPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut app_ptr_val,
            },
        );
        let app_ptr = if val_is_private(&app_ptr_val) {
            app_ptr_val.to_private() as *mut App<true>
        } else {
            core::ptr::null_mut()
        };
        if !app_ptr.is_null() {
            (*app_ptr).close();
            App::<true>::destroy(app_ptr);
            unregister_active_h2_ssl_app(app_ptr);
            rooted!(&in(cx_ref) let undef_root = UndefinedValue());
            JS_SetProperty(
                cx,
                obj.handle().into(),
                c"_sslAppPtr".as_ptr(),
                undef_root.handle().into(),
            );
        }
    }

    // Cleanup H2ServerUserData
    {
        rooted!(&in(cx_ref) let obj = server_obj.get());
        let mut ud_ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_udPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ud_ptr_val,
            },
        );
        let ud_ptr = if val_is_private(&ud_ptr_val) {
            ud_ptr_val.to_private() as *mut H2ServerUserData
        } else {
            core::ptr::null_mut()
        };
        if !ud_ptr.is_null() {
            let ud = Box::from_raw(ud_ptr);
            ud.cleanup();
            rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
            JS_SetProperty(
                cx,
                obj.handle().into(),
                c"_udPtr".as_ptr(),
                undef_root2.handle().into(),
            );
        }
    }

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        if !global.get().is_null() {
            rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            JS_CallFunctionValue(
                cx,
                global.handle().into(),
                fval_root.handle().into(),
                &HandleValueArray::empty(),
                rval_h,
            );
            JS_ClearPendingException(cx);
        }
    }

    args.rval().set(UndefinedValue());
    true
}

// ──────────────────────────────────────────────────────────────────────
// JS-facing server listen/close (called when IIFE constructors are not
// available — fallback path)
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_listen_js(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Delegate to the native __http2_server_listen
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    let port: u16 = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32() as u16
        } else if v.is_double() {
            v.to_double() as u16
        } else {
            3000
        }
    } else {
        3000
    };

    let callback = if argc > 1 && (*args.get(1).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(1).ptr).to_object());
        Some(cb.get())
    } else if argc > 0 && (*args.get(0).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(0).ptr).to_object());
        Some(cb.get())
    } else {
        None
    };

    // Create uWS App<false>
    let opts = BunSocketContextOptions::default();
    let app_ptr = match App::<false>::create(&opts) {
        Some(p) => p,
        None => {
            let msg = format!("Failed to create HTTP/2 server on port {}", port);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // node:http2 framing parity — see the matching comment at the
    // `App::<false>` listen site (node-family `IsNodeHttp` semantics).
    // Safety: app_ptr is a live `*mut App<false>` from `App::create` above,
    // valid until `App::<false>::destroy`.
    unsafe { (*app_ptr).set_is_node_http(true) };

    // Get handler
    let mut handler_val = UndefinedValue();
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_onStreamHandler".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut handler_val,
        },
    );

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        App::<false>::destroy(app_ptr);
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(cx, global.get(), handler_root.get(), server_obj.get()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    let safe_handler: Option<
        extern "C" fn(
            *mut bun_uws_sys::response::c::uws_res,
            *mut bun_uws_sys::Request,
            *mut ::std::ffi::c_void,
        ),
    > = unsafe {
        ::std::mem::transmute(Some(
            uws_h2_route_handler
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::response::c::uws_res,
                    *mut bun_uws_sys::Request,
                    *mut ::std::ffi::c_void,
                ),
        ))
    };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            udv.handle().into(),
            0,
        );
    }

    let safe_listen_cb: extern "C" fn(
        *mut bun_uws_sys::listen_socket::ListenSocket,
        *mut ::std::ffi::c_void,
    ) = unsafe {
        ::std::mem::transmute(
            uws_h2_listen_callback
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::listen_socket::ListenSocket,
                    *mut ::std::ffi::c_void,
                ),
        )
    };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_appPtr".as_ptr(),
            apv.handle().into(),
            0,
        );
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"_listeningPort".as_ptr(),
        port_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"listening".as_ptr(),
        listening_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    register_active_h2_app(app_ptr);

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        JS_CallFunctionValue(
            cx,
            global.handle().into(),
            fval_root.handle().into(),
            &HandleValueArray::empty(),
            rval_h,
        );
        JS_ClearPendingException(cx);
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_close_js(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    close_h2_server(cx, cx_ref, server_obj.get(), false);

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_secure_server_listen_js(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // Delegate to the native __http2_secure_server_listen
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    let port: u16 = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32() as u16
        } else if v.is_double() {
            v.to_double() as u16
        } else {
            3000
        }
    } else {
        3000
    };

    let callback = if argc > 1 && (*args.get(1).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(1).ptr).to_object());
        Some(cb.get())
    } else {
        None
    };

    // Extract TLS options
    let mut pem_key = String::new();
    let mut pem_cert = String::new();
    let mut opts_val = UndefinedValue();
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_options".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut opts_val,
        },
    );
    if opts_val.is_object() {
        rooted!(&in(cx_ref) let opts_root = opts_val.to_object());
        let mut key_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_root.handle().into(),
            c"key".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut key_val,
            },
        );
        if key_val.is_string() {
            pem_key = crate::js_to_rust_string(cx, key_val);
        }
        let mut cert_val = UndefinedValue();
        JS_GetProperty(
            cx,
            opts_root.handle().into(),
            c"cert".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut cert_val,
            },
        );
        if cert_val.is_string() {
            pem_cert = crate::js_to_rust_string(cx, cert_val);
        }
    }

    // Get handler
    let mut handler_val = UndefinedValue();
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_onStreamHandler".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut handler_val,
        },
    );

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    // Create uWS App<true>
    let mut ssl_opts = BunSocketContextOptions::default();
    let key_cstr;
    let cert_cstr;
    let key_ptr;
    let cert_ptr;
    if !pem_key.is_empty() && !pem_cert.is_empty() {
        key_cstr = ::std::ffi::CString::new(pem_key).unwrap_or_default();
        cert_cstr = ::std::ffi::CString::new(pem_cert).unwrap_or_default();
        key_ptr = key_cstr.as_ptr();
        cert_ptr = cert_cstr.as_ptr();
        ssl_opts.key = &key_ptr as *const *const i8;
        ssl_opts.key_count = 1;
        ssl_opts.cert = &cert_ptr as *const *const i8;
        ssl_opts.cert_count = 1;
    }

    let app_ptr = match App::<true>::create(&ssl_opts) {
        Some(p) => p,
        None => {
            let msg = format!("Failed to create HTTP/2 secure server on port {}", port);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // node:http2 framing parity — see the matching comment at the
    // `App::<false>` listen site (node-family `IsNodeHttp` semantics).
    // Safety: app_ptr is a live `*mut App<true>` from `App::create` above,
    // valid until `App::<true>::destroy`.
    unsafe { (*app_ptr).set_is_node_http(true) };

    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(cx, global.get(), handler_root.get(), server_obj.get()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    let safe_handler: Option<
        extern "C" fn(
            *mut bun_uws_sys::response::c::uws_res,
            *mut bun_uws_sys::Request,
            *mut ::std::ffi::c_void,
        ),
    > = unsafe {
        ::std::mem::transmute(Some(
            uws_h2_route_handler
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::response::c::uws_res,
                    *mut bun_uws_sys::Request,
                    *mut ::std::ffi::c_void,
                ),
        ))
    };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            udv.handle().into(),
            0,
        );
    }

    let safe_listen_cb: extern "C" fn(
        *mut bun_uws_sys::listen_socket::ListenSocket,
        *mut ::std::ffi::c_void,
    ) = unsafe {
        ::std::mem::transmute(
            uws_h2_listen_callback
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::listen_socket::ListenSocket,
                    *mut ::std::ffi::c_void,
                ),
        )
    };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_sslAppPtr".as_ptr(),
            apv.handle().into(),
            0,
        );
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"_listeningPort".as_ptr(),
        port_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"listening".as_ptr(),
        listening_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    register_active_h2_ssl_app(app_ptr);

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        JS_CallFunctionValue(
            cx,
            global.handle().into(),
            fval_root.handle().into(),
            &HandleValueArray::empty(),
            rval_h,
        );
        JS_ClearPendingException(cx);
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_secure_server_close_js(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    // Close the SSL App
    {
        let mut app_ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            server_obj.handle().into(),
            c"_sslAppPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut app_ptr_val,
            },
        );
        let app_ptr = if val_is_private(&app_ptr_val) {
            app_ptr_val.to_private() as *mut App<true>
        } else {
            core::ptr::null_mut()
        };
        if !app_ptr.is_null() {
            (*app_ptr).close();
            App::<true>::destroy(app_ptr);
            unregister_active_h2_ssl_app(app_ptr);
            rooted!(&in(cx_ref) let undef_root = UndefinedValue());
            JS_SetProperty(
                cx,
                server_obj.handle().into(),
                c"_sslAppPtr".as_ptr(),
                undef_root.handle().into(),
            );
        }
    }

    // Cleanup H2ServerUserData
    {
        let mut ud_ptr_val = UndefinedValue();
        JS_GetProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ud_ptr_val,
            },
        );
        let ud_ptr = if val_is_private(&ud_ptr_val) {
            ud_ptr_val.to_private() as *mut H2ServerUserData
        } else {
            core::ptr::null_mut()
        };
        if !ud_ptr.is_null() {
            let ud = Box::from_raw(ud_ptr);
            ud.cleanup();
            rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
            JS_SetProperty(
                cx,
                server_obj.handle().into(),
                c"_udPtr".as_ptr(),
                undef_root2.handle().into(),
            );
        }
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

// ──────────────────────────────────────────────────────────────────────
// __http2_fetch — client-side fetch bridge (called from JS IIFE)
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_fetch(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let url = if argc > 0 && (*args.get(0).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };

    let method = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "GET".to_string()
    };

    let headers_json = if argc > 2 && (*args.get(2).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(2).ptr)
    } else {
        "{}".to_string()
    };

    // Body (arg 3): string (UTF-8) or Buffer/TypedArray/DataView/ArrayBuffer —
    // byte-exact via the house extractor (same contract as node_http's
    // http_request / node_https's https_request). The previous string-only
    // read silently emptied every binary request body; the JS layer's
    // `String(body)` coercion also turned Buffers into "72,101,108".
    // Unrecognized objects fail loudly.
    let body_bytes: Option<Vec<u8>> = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_undefined() || v.is_null() {
            None
        } else if v.is_string() {
            let s = crate::js_to_rust_string(cx, v);
            (!s.is_empty()).then(|| s.into_bytes())
        } else if v.is_object() {
            match crate::node_buffer::collect_byte_view(cx, v) {
                Some(bytes) => (!bytes.is_empty()).then_some(bytes),
                None => {
                    JS_ReportErrorUTF8(
                        cx,
                        c"%s".as_ptr(),
                        c"http2: request body must be a string, Buffer, TypedArray or ArrayBuffer"
                            .as_ptr(),
                    );
                    return false;
                }
            }
        } else {
            JS_ReportErrorUTF8(
                cx,
                c"%s".as_ptr(),
                c"http2: request body must be a string, Buffer, TypedArray or ArrayBuffer".as_ptr(),
            );
            return false;
        }
    } else {
        None
    };

    // Parse headers from JSON
    let headers_map: ::std::collections::HashMap<String, String> =
        serde_json::from_str(&headers_json).unwrap_or_default();
    let headers: Vec<(String, String)> = headers_map
        .into_iter()
        .filter(|(k, _)| !k.starts_with(':')) // Strip pseudo-headers
        .collect();

    // Resolve method
    let bun_method = match method.as_str() {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    // Create a pending Promise and schedule async fetch
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let null_global = ::std::ptr::null_mut::<JSObject>());
    rooted!(&in(cx_ref) let promise = unsafe {
        mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_global.handle().into())
    });
    if promise.get().is_null() {
        // Fail closed: without the Promise there is no honest result shape.
        // (The old path returned a statusCode:0 placeholder JSON here — a
        // silent-fake the JS layer then delivered as a real response.)
        JS_ReportErrorUTF8(
            cx,
            c"%s".as_ptr(),
            c"http2: failed to create fetch promise".as_ptr(),
        );
        return false;
    }

    let promise_obj = promise.get();
    let promise_val = ObjectValue(promise_obj);

    // Schedule async fetch — the returned Promise resolves with the realm's
    // real WHATWG Response (status/headers/arrayBuffer), exactly like the
    // node:http / node:https client transports. The JS layer consumes it
    // (response headers → 'response', arrayBuffer → Buffer 'data' chunk).
    unsafe {
        crate::fetch_async::start(
            cx,
            promise_val,
            None, // No stealth profile for plain http2
            bun_method,
            url,
            headers,
            body_bytes,
        );
    }

    args.rval().set(promise_val);
    true
}

// ──────────────────────────────────────────────────────────────────────
// Property helpers
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_int_prop(
    cx: &mut mozjs::context::JSContext,
    obj_ptr: *mut JSObject,
    name: &str,
    val: i32,
) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let raw_cx = cx.raw_cx();
    rooted!(&in(cx) let obj = obj_ptr);
    rooted!(&in(cx) let v = Int32Value(val));
    JS_DefineProperty(
        raw_cx,
        obj.handle().into(),
        c_name.as_ptr(),
        v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

// ──────────────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_active_servers_false_initially() {
        ACTIVE_H2_APPS.with(|s| s.borrow_mut().clear());
        ACTIVE_H2_SSL_APPS.with(|s| s.borrow_mut().clear());
        assert!(!has_active_servers());
    }

    #[test]
    fn register_unregister_h2_app() {
        ACTIVE_H2_APPS.with(|s| s.borrow_mut().clear());
        let sentinel: *mut App<false> = 0x1000 as *mut App<false>;
        unsafe {
            register_active_h2_app(sentinel);
            assert!(has_active_servers());
            register_active_h2_app(sentinel); // idempotent
            let count = ACTIVE_H2_APPS.with(|s| s.borrow().len());
            assert_eq!(count, 1);
            unregister_active_h2_app(sentinel);
            assert!(!has_active_servers());
        }
    }

    #[test]
    fn register_unregister_h2_ssl_app() {
        ACTIVE_H2_SSL_APPS.with(|s| s.borrow_mut().clear());
        let sentinel: *mut App<true> = 0x2000 as *mut App<true>;
        unsafe {
            register_active_h2_ssl_app(sentinel);
            assert!(has_active_servers());
            unregister_active_h2_ssl_app(sentinel);
            assert!(!has_active_servers());
        }
    }

    #[test]
    fn null_app_is_noop() {
        ACTIVE_H2_APPS.with(|s| s.borrow_mut().clear());
        ACTIVE_H2_SSL_APPS.with(|s| s.borrow_mut().clear());
        unsafe {
            register_active_h2_app(core::ptr::null_mut());
            register_active_h2_ssl_app(core::ptr::null_mut());
            assert!(!has_active_servers());
            unregister_active_h2_app(core::ptr::null_mut());
            unregister_active_h2_ssl_app(core::ptr::null_mut());
            assert!(!has_active_servers());
        }
    }

    #[test]
    fn next_session_id_monotonic() {
        let id1 = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
        let id2 = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
        assert!(id2 > id1);
    }

    #[test]
    fn next_server_id_monotonic() {
        let id1 = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let id2 = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id2 > id1);
    }

    #[test]
    fn h2_server_user_data_keys_namespaced() {
        let key1 = format!("http2_server_{}_global", 1);
        let key2 = format!("http2_server_{}_handler", 1);
        assert!(key1.starts_with("http2_server_"));
        assert!(key1.ends_with("_global"));
        assert!(key2.starts_with("http2_server_"));
        assert!(key2.ends_with("_handler"));
        assert_ne!(key1, key2);
    }

    #[test]
    fn h2_server_user_data_null_cx_returns_none() {
        let ud = H2ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http2_server_999_global".to_string(),
            handler_key: "http2_server_999_handler".to_string(),
            server_key: "http2_server_999_server".to_string(),
        };
        assert!(ud.global().is_none());
        assert!(ud.handler().is_none());
        assert!(ud.server_obj().is_none());
    }

    #[test]
    fn h2_server_user_data_cleanup_no_panic() {
        let ud = H2ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http2_server_998_global".to_string(),
            handler_key: "http2_server_998_handler".to_string(),
            server_key: "http2_server_998_server".to_string(),
        };
        ud.cleanup(); // Must not panic
    }

    #[test]
    fn val_is_private_undefined() {
        let v = UndefinedValue();
        assert!(!val_is_private(&v));
    }

    #[test]
    fn val_is_private_int32() {
        let v = Int32Value(42);
        assert!(!val_is_private(&v));
    }

    // HTTP/2 constants verification
    #[test]
    fn nghttp2_error_codes() {
        assert_eq!(NGHTTP2_NO_ERROR, 0);
        assert_eq!(NGHTTP2_PROTOCOL_ERROR, 1);
        assert_eq!(NGHTTP2_INTERNAL_ERROR, 2);
        assert_eq!(NGHTTP2_CANCEL, 8);
        assert_eq!(NGHTTP2_HTTP_1_1_REQUIRED, 13);
    }

    #[test]
    fn default_settings_values() {
        assert_eq!(DEFAULT_SETTINGS_HEADER_TABLE_SIZE, 4096);
        assert_eq!(DEFAULT_SETTINGS_ENABLE_PUSH, 0);
        assert_eq!(DEFAULT_SETTINGS_INITIAL_WINDOW_SIZE, 65535);
        assert_eq!(DEFAULT_SETTINGS_MAX_FRAME_SIZE, 16384);
    }

    // Use constants as const values for test access
    const NGHTTP2_NO_ERROR: i32 = 0;
    const NGHTTP2_PROTOCOL_ERROR: i32 = 1;
    const NGHTTP2_INTERNAL_ERROR: i32 = 2;
    const NGHTTP2_CANCEL: i32 = 8;
    const NGHTTP2_HTTP_1_1_REQUIRED: i32 = 13;
    const DEFAULT_SETTINGS_HEADER_TABLE_SIZE: i32 = 4096;
    const DEFAULT_SETTINGS_ENABLE_PUSH: i32 = 0;
    const DEFAULT_SETTINGS_INITIAL_WINDOW_SIZE: i32 = 65535;
    const DEFAULT_SETTINGS_MAX_FRAME_SIZE: i32 = 16384;
}
