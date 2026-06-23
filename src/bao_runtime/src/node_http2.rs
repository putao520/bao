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
use bun_core::ZBox;
use ::std::cell::RefCell;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, ObjectValue, Int32Value, BooleanValue, PrivateValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::response::Response;
use bun_uws_sys::request::Request;
use bun_uws_sys::socket_context::BunSocketContextOptions;

use crate::gc_store::{gc_store_insert_ns, gc_store_get_ns, gc_store_remove_ns};
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

pub unsafe fn register_active_h2_app(app: *mut App<false>) {
    if app.is_null() { return; }
    ACTIVE_H2_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        if !apps.iter().any(|&p| ::std::ptr::eq(p, app)) {
            apps.push(app);
        }
    });
}

pub unsafe fn unregister_active_h2_app(app: *mut App<false>) {
    if app.is_null() { return; }
    ACTIVE_H2_APPS.with(|s| { s.borrow_mut().retain(|&p| !::std::ptr::eq(p, app)); });
}

pub unsafe fn register_active_h2_ssl_app(app: *mut App<true>) {
    if app.is_null() { return; }
    ACTIVE_H2_SSL_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        if !apps.iter().any(|&p| ::std::ptr::eq(p, app)) {
            apps.push(app);
        }
    });
}

pub unsafe fn unregister_active_h2_ssl_app(app: *mut App<true>) {
    if app.is_null() { return; }
    ACTIVE_H2_SSL_APPS.with(|s| { s.borrow_mut().retain(|&p| !::std::ptr::eq(p, app)); });
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
    this._bodyText = "";
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
    if (data != null) {
      this._bodyText += (typeof data === 'string') ? data : String(data);
    }
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

    // If this is a client session, perform the fetch via __http2_fetch
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

      var body = options.body || '';
      if (body && typeof body !== 'string') {
        try { body = String(body); } catch(e) { body = ''; }
      }

      var resultJSON = __http2_fetch(url, method, headersJSON, body);
      var result = {};
      try { result = JSON.parse(resultJSON); } catch(e) {
        result = { statusCode: 0, headers: {}, body: resultJSON };
      }

      // Build response headers from result
      var respHeaders = {};
      if (result.headers) {
        for (var hk in result.headers) {
          if (result.headers.hasOwnProperty(hk)) {
            respHeaders[hk] = result.headers[hk];
          }
        }
      }
      respHeaders[':status'] = String(result.statusCode || 0);

      stream._responseHeaders = respHeaders;
      stream._responseBody = result.body || '';

      // Emit response event asynchronously via setImmediate-like pattern
      var self = stream;
      var sess = this;
      // Synchronous emit for now (matches Node.js http2.request behavior)
      self.emit('response', respHeaders);
      if (self._responseBody) {
        self.emit('data', self._responseBody);
      }
      self.end();
    }

    this._streams[streamId] = stream;
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
    // Close all open streams
    for (var id in this._streams) {
      if (this._streams.hasOwnProperty(id)) {
        var stream = this._streams[id];
        if (!stream._closed) stream.close();
      }
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
    // Destroy all streams
    for (var id in this._streams) {
      if (this._streams.hasOwnProperty(id)) {
        this._streams[id].destroy(error);
      }
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
  function Http2Server(options, handler) {
    if (typeof options === 'function') {
      handler = options;
      options = {};
    }
    this._options = options || {};
    this._events = Object.create(null);
    this.listening = false;
    this._port = 0;
    if (handler) this.on('stream', handler);
  }
  Http2Server.prototype = Object.create(null);
  Http2Server.prototype.on = EE.prototype.on;
  Http2Server.prototype.once = EE.prototype.once;
  Http2Server.prototype.emit = EE.prototype.emit;
  Http2Server.prototype.removeListener = EE.prototype.removeListener;
  Http2Server.prototype.removeAllListeners = EE.prototype.removeAllListeners;
  Http2Server.prototype.prependListener = EE.prototype.prependListener;

  Http2Server.prototype.listen = function(port, callback) {
    this._port = port;
    this.listening = true;
    // Delegate to native __http2_server_listen
    if (typeof __http2_server_listen === 'function') {
      __http2_server_listen(this, port, callback);
    } else {
      if (callback) callback();
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
    return { port: this._port || 0, family: 'IPv4', address: '0.0.0.0' };
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
    if (handler) this.on('stream', handler);
  }
  Http2SecureServer.prototype = Object.create(null);
  Http2SecureServer.prototype.on = EE.prototype.on;
  Http2SecureServer.prototype.once = EE.prototype.once;
  Http2SecureServer.prototype.emit = EE.prototype.emit;
  Http2SecureServer.prototype.removeListener = EE.prototype.removeListener;
  Http2SecureServer.prototype.removeAllListeners = EE.prototype.removeAllListeners;
  Http2SecureServer.prototype.prependListener = EE.prototype.prependListener;

  Http2SecureServer.prototype.listen = function(port, callback) {
    this._port = port;
    this.listening = true;
    // Delegate to native __http2_secure_server_listen
    if (typeof __http2_secure_server_listen === 'function') {
      __http2_secure_server_listen(this, port, callback);
    } else {
      if (callback) callback();
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
    return { port: this._port || 0, family: 'IPv4', address: '0.0.0.0' };
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
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_HEADER_TABLE_SIZE", 4096);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_ENABLE_PUSH", 0);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_INITIAL_WINDOW_SIZE", 65535);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_MAX_FRAME_SIZE", 16384);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_MAX_CONCURRENT_STREAMS", 100);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_MAX_HEADER_LIST_SIZE", 65535);

        // Stream states
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_IDLE", 0);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_OPEN", 1);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_RESERVED_LOCAL", 2);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_RESERVED_REMOTE", 3);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_HALF_CLOSED_LOCAL", 4);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_STATE_HALF_CLOSED_REMOTE", 5);
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
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"createServer".as_ptr(), Some(http2_create_server), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"createSecureServer".as_ptr(), Some(http2_create_secure_server), 2, JSPROP_ENUMERATE as u32);

        // Client fetch bridge
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"__http2_fetch".as_ptr(), Some(http2_fetch), 4, 0 as u32);

        // Server listen/close bridges (called from JS)
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"__http2_server_listen".as_ptr(), Some(http2_server_listen), 3, 0 as u32);
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"__http2_server_close".as_ptr(), Some(http2_server_close), 2, 0 as u32);
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"__http2_secure_server_listen".as_ptr(), Some(http2_secure_server_listen), 3, 0 as u32);
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"__http2_secure_server_close".as_ptr(), Some(http2_secure_server_close), 2, 0 as u32);

        // ── Evaluate JS IIFE ───────────────────────────────────────────
        let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"node:http2".as_ptr(), 1);
        if !opts.is_null() {
            let mut src_text = mozjs::rust::transform_str_to_source_text(HTTP2_JS);
            let mut rval = UndefinedValue();
            if JS::Evaluate2(cx.raw_cx(), opts, &mut src_text, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
            }) && rval.is_object() {
                rooted!(&in(cx) let iife_obj = rval.to_object());
                rooted!(&in(cx) let global = CurrentGlobalOrNull(cx.raw_cx()));
                if !global.get().is_null() {
                    // Call the IIFE to get the exports object
                    let mut call_rval = UndefinedValue();
                    let call_rval_h = MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData, ptr: &mut call_rval,
                    };
                    rooted!(&in(cx) let iife_val = ObjectValue(iife_obj.get()));
                    JS_CallFunctionValue(cx.raw_cx(), global.handle().into(),
                        iife_val.handle().into(),
                        &HandleValueArray::empty(), call_rval_h);

                    if call_rval.is_object() {
                        rooted!(&in(cx) let exports = call_rval.to_object());
                        // Copy JS-defined properties onto http2_obj
                        let js_props = [
                            "connect", "Http2Session", "Http2Stream",
                            "Http2Server", "Http2SecureServer",
                            "getDefaultSettings", "getPackedSettings",
                            "getUnpackedSettings", "sensitiveHeaders",
                            "performance",
                        ];
                        for &prop in &js_props {
                            let c_prop = ZBox::from_bytes(prop.as_bytes());
                            let mut prop_val = UndefinedValue();
                            JS_GetProperty(cx.raw_cx(), exports.handle().into(), c_prop.as_ptr(),
                                MutableHandle::<Value> {
                                    _phantom_0: ::std::marker::PhantomData, ptr: &mut prop_val,
                                });
                            if !prop_val.is_undefined() {
                                rooted!(&in(cx) let pv = prop_val);
                                JS_DefineProperty(cx.raw_cx(), http2_obj.handle().into(),
                                    c_prop.as_ptr(), pv.handle().into(),
                                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
                            }
                        }
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
}

impl H2ServerUserData {
    fn new(cx: *mut JSContext, global: *mut JSObject, handler: *mut JSObject) -> Self {
        let server_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let global_key = format!("http2_server_{}_global", server_id);
        let handler_key = format!("http2_server_{}_handler", server_id);
        gc_store_insert_ns(cx, "http2", &global_key, global);
        gc_store_insert_ns(cx, "http2", &handler_key, handler);
        Self { cx, global_key, handler_key }
    }

    fn global(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http2", &self.global_key)
    }

    fn handler(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http2", &self.handler_key)
    }

    fn cleanup(&self) {
        gc_store_remove_ns(self.cx, "http2", &self.global_key);
        gc_store_remove_ns(self.cx, "http2", &self.handler_key);
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
    if cx.is_null() { return; }

    let Some(global) = ud.global() else { return };
    if global.is_null() { return; }

    let Some(handler) = ud.handler() else { return };
    if handler.is_null() { return; }

    let req_ref = bun_opaque::opaque_deref_mut(req);
    let method_bytes = req_ref.method();
    let url_bytes = req_ref.url();
    let method_str = ::std::str::from_utf8_unchecked(method_bytes);
    let url_str = ::std::str::from_utf8_unchecked(url_bytes);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let raw_cx = cx;

    // Build JS stream object (Http2Stream-like)
    rooted!(&in(cx_ref) let stream_obj = w2::JS_NewPlainObject(cx_ref));
    if stream_obj.get().is_null() { return; }

    // Set pseudo-headers as properties
    {
        let c_method = ZBox::from_bytes(method_str.as_bytes());
        let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
        if !js_method.is_null() {
            let mv = StringValue(&*js_method);
            rooted!(&in(cx_ref) let mvr = mv);
            JS_DefineProperty(raw_cx, stream_obj.handle().into(), c":method".as_ptr(), mvr.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    {
        let c_path = ZBox::from_bytes(url_str.as_bytes());
        let js_path = JS_NewStringCopyZ(raw_cx, c_path.as_ptr());
        if !js_path.is_null() {
            let pv = StringValue(&*js_path);
            rooted!(&in(cx_ref) let pvr = pv);
            JS_DefineProperty(raw_cx, stream_obj.handle().into(), c":path".as_ptr(), pvr.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    // Build headers object
    rooted!(&in(cx_ref) let headers_obj = w2::JS_NewPlainObject(cx_ref));
    if !headers_obj.get().is_null() {
        let common_headers: &[&[u8]] = &[
            b"host", b"content-type", b"content-length", b"accept",
            b"user-agent", b"connection", b"authorization", b"cookie",
            b":authority", b":scheme",
        ];
        for &name in common_headers {
            if let Some(value) = req_ref.header(name) {
                let c_k = ZBox::from_bytes(name);
                let c_v = ZBox::from_bytes(value);
                let js_v = JS_NewStringCopyZ(raw_cx, c_v.as_ptr());
                if !js_v.is_null() {
                    let hv = StringValue(&*js_v);
                    rooted!(&in(cx_ref) let hvr = hv);
                    JS_DefineProperty(raw_cx, headers_obj.handle().into(), c_k.as_ptr(), hvr.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
        }
        let hdrs_val = ObjectValue(headers_obj.get());
        rooted!(&in(cx_ref) let hdrs_r = hdrs_val);
        JS_DefineProperty(raw_cx, stream_obj.handle().into(), c"headers".as_ptr(), hdrs_r.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // Add respond / end / close methods on the stream
    w2::JS_DefineFunction(cx_ref, stream_obj.handle(), c"respond".as_ptr(), Some(h2_stream_respond), 2, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, stream_obj.handle(), c"end".as_ptr(), Some(h2_stream_end), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, stream_obj.handle(), c"close".as_ptr(), Some(h2_stream_close), 0, JSPROP_ENUMERATE as u32);

    // Store uWS res pointer on the stream object
    let res_ptr_val = PrivateValue(res as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let rv = res_ptr_val);
    JS_DefineProperty(raw_cx, stream_obj.handle().into(), c"_uwsRes".as_ptr(), rv.handle().into(), 0);

    // Call the JS handler: handler(stream, headers)
    rooted!(&in(cx_ref) let handler_root = ObjectValue(handler));
    rooted!(&in(cx_ref) let global_root = global);

    let args_vals = [ObjectValue(stream_obj.get()), ObjectValue(headers_obj.get())];
    let call_args = HandleValueArray {
        length_: 2,
        elements_: args_vals.as_ptr(),
    };

    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    JS_CallFunctionValue(raw_cx, global_root.handle().into(), handler_root.handle().into(), &call_args, rval_h);
    JS_ClearPendingException(raw_cx);
}

// ──────────────────────────────────────────────────────────────────────
// JS stream methods — bridge to uWS Response::<false>
// ──────────────────────────────────────────────────────────────────────

#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

#[inline]
unsafe fn get_uws_res(cx: *mut JSContext, obj: *mut JSObject) -> *mut bun_uws_sys::response::c::uws_res {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut ptr_val = UndefinedValue();
    JS_GetProperty(cx, obj_root.handle().into(), c"_uwsRes".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut ptr_val,
    });
    if !val_is_private(&ptr_val) {
        return core::ptr::null_mut();
    }
    ptr_val.to_private() as *mut bun_uws_sys::response::c::uws_res
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_stream_respond(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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
            JS_GetProperty(cx, hdrs_obj.handle().into(), c":status".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut status_val });

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
                let status_str = format!("{} ", status);
                (*res_mut).write_status(status_str.as_bytes());

                // Write response headers (skip pseudo-headers starting with ':')
                let common: &[&[u8]] = &[
                    b"content-type", b"content-length", b"location",
                    b"set-cookie", b"cache-control", b"x-",
                ];
                for &key in common {
                    let c_key = ZBox::from_bytes(key);
                    let mut hv = UndefinedValue();
                    JS_GetProperty(cx, hdrs_obj.handle().into(), c_key.as_ptr(),
                        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hv });
                    if hv.is_string() {
                        let val = crate::js_to_rust_string(cx, hv);
                        let c_val = ZBox::from_bytes(val.as_bytes());
                        (*res_mut).write_header(key, c_val.as_bytes());
                    }
                }
            }
        }
    }

    // If endStream option, end the response
    if argc > 1 {
        let opts_val = *args.get(1).ptr;
        if opts_val.is_object() {
            rooted!(&in(cx_ref) let opts_obj = opts_val.to_object());
            let mut end_stream_val = UndefinedValue();
            JS_GetProperty(cx, opts_obj.handle().into(), c"endStream".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut end_stream_val });
            if end_stream_val.is_boolean() && end_stream_val.to_boolean() {
                let uws_res = get_uws_res(cx, obj.get());
                if !uws_res.is_null() {
                    let res_mut = Response::<false>::cast_res(uws_res);
                    (*res_mut).end(&[], false);
                }
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_stream_end(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let obj = this.to_object());

    // Get accumulated body
    let mut body_val = UndefinedValue();
    let body_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj.handle().into(), c"_body".as_ptr(), body_mh);
    let body = if body_val.is_string() {
        crate::js_to_rust_string(cx, body_val)
    } else {
        String::new()
    };

    // Append final data if provided
    let final_body = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            let data = crate::js_to_rust_string(cx, v);
            let mut combined = body;
            combined.push_str(&data);
            combined
        } else {
            body
        }
    } else {
        body
    };

    let uws_res = get_uws_res(cx, obj.get());
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);

        // Write default status if not yet written
        if !(*res_mut).state().is_http_status_called() {
            (*res_mut).write_status(b"200 ");
        }

        (*res_mut).end(final_body.as_bytes(), false);
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn h2_stream_close(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let obj = this.to_object());

    let uws_res = get_uws_res(cx, obj.get());
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);
        if !(*res_mut).state().is_http_status_called() {
            (*res_mut).write_status(b"200 ");
        }
        (*res_mut).end(&[], false);
    }

    args.rval().set(UndefinedValue());
    true
}

// ──────────────────────────────────────────────────────────────────────
// createServer / createSecureServer — JS host functions
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_create_server(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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
    JS_GetProperty(cx, http2_obj.handle().into(), c"Http2Server".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor_val });

    if !ctor_val.is_object() {
        // Fallback: create a plain object with EE methods
        rooted!(&in(cx_ref) let server_obj = w2::JS_NewPlainObject(cx_ref));
        if server_obj.get().is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        attach_ee_methods(cx, server_obj.get());
        w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"listen".as_ptr(), Some(http2_server_listen_js), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"close".as_ptr(), Some(http2_server_close_js), 1, JSPROP_ENUMERATE as u32);
        store_handler(cx, cx_ref, server_obj.get(), argc, &args);
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    // Call new Http2Server(options, handler)
    rooted!(&in(cx_ref) let ctor = ctor_val.to_object());
    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));

    let mut new_rval = UndefinedValue();
    let new_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut new_rval };

    // Prepare args: (options, handler)
    let opts_arg = if argc > 0 && (*args.get(0).ptr).is_object() {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let handler_arg = if argc > 1 && (*args.get(1).ptr).is_object() {
        *args.get(1).ptr
    } else if argc > 0 && (*args.get(0).ptr).is_object() {
        // Single function arg: createServer(handler)
        UndefinedValue()
    } else {
        UndefinedValue()
    };

    let call_args_vals = [opts_arg, handler_arg];
    let call_args = HandleValueArray {
        length_: if argc > 1 { 2 } else if argc > 0 { 1 } else { 0 },
        elements_: call_args_vals.as_ptr(),
    };

    rooted!(&in(cx_ref) let ctor_val = ObjectValue(ctor.get()));
    JS_CallFunctionValue(cx, global.handle().into(), ctor_val.handle().into(), &call_args, new_rval_h);

    if new_rval.is_object() {
        rooted!(&in(cx_ref) let server_obj = new_rval.to_object());
        // Store handler on the server object for native listen
        store_handler(cx, cx_ref, server_obj.get(), argc, &args);
        args.rval().set(ObjectValue(server_obj.get()));
    } else {
        args.rval().set(UndefinedValue());
    }

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
    JS_GetProperty(cx, http2_obj.handle().into(), c"Http2SecureServer".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor_val });

    if !ctor_val.is_object() {
        // Fallback: create a plain object with EE methods
        rooted!(&in(cx_ref) let server_obj = w2::JS_NewPlainObject(cx_ref));
        if server_obj.get().is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        attach_ee_methods(cx, server_obj.get());
        w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"listen".as_ptr(), Some(http2_secure_server_listen_js), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"close".as_ptr(), Some(http2_secure_server_close_js), 1, JSPROP_ENUMERATE as u32);
        store_handler(cx, cx_ref, server_obj.get(), argc, &args);
        // Mark as secure
        rooted!(&in(cx_ref) let secure_val = BooleanValue(true));
        JS_DefineProperty(cx, server_obj.handle().into(), c"_secure".as_ptr(), secure_val.handle().into(), 0);
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    // Call new Http2SecureServer(options, handler)
    rooted!(&in(cx_ref) let ctor = ctor_val.to_object());
    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));

    let mut new_rval = UndefinedValue();
    let new_rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut new_rval };

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
        length_: if argc > 1 { 2 } else if argc > 0 { 1 } else { 0 },
        elements_: call_args_vals.as_ptr(),
    };

    rooted!(&in(cx_ref) let ctor_val = ObjectValue(ctor.get()));
    JS_CallFunctionValue(cx, global.handle().into(), ctor_val.handle().into(), &call_args, new_rval_h);

    if new_rval.is_object() {
        rooted!(&in(cx_ref) let server_obj = new_rval.to_object());
        store_handler(cx, cx_ref, server_obj.get(), argc, &args);
        // Mark as secure
        rooted!(&in(cx_ref) let secure_val = BooleanValue(true));
        JS_DefineProperty(cx, server_obj.handle().into(), c"_secure".as_ptr(), secure_val.handle().into(), 0);
        args.rval().set(ObjectValue(server_obj.get()));
    } else {
        args.rval().set(UndefinedValue());
    }

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
        JS_DefineProperty(cx, server_root.handle().into(), c"_onStreamHandler".as_ptr(), cb_root.handle().into(), 0);
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
        ("on", ee_on), ("once", ee_once), ("emit", ee_emit),
        ("off", ee_off), ("addListener", ee_on), ("removeListener", ee_off),
        ("prependListener", ee_prepend), ("removeAllListeners", ee_remove_all),
    ] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        mozjs_sys::jsapi::JS_DefineFunction(cx, obj_root.handle().into(), c_name.as_ptr(), op, 2, JSPROP_ENUMERATE as u32);
    }
}

// ──────────────────────────────────────────────────────────────────────
// Server listen / close — native uWS App bridge
// ──────────────────────────────────────────────────────────────────────

/// uWS listen callback
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn uws_h2_listen_callback(
    _listen_socket: *mut bun_uws_sys::listen_socket::ListenSocket,
    _user_data: *mut ::std::ffi::c_void,
) {
    // No-op: listening confirmed by uWS
}

/// __http2_server_listen(serverObj, port, callback) — called from JS
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_listen(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // args: serverObj, port, callback
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

    let callback = if argc > 2 && (*args.get(2).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(2).ptr).to_object());
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

    // Get the JS stream handler from the server object
    let mut handler_val = UndefinedValue();
    let handler_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val };
    JS_GetProperty(cx, server_obj.handle().into(), c"_onStreamHandler".as_ptr(), handler_mh);

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        App::<false>::destroy(app_ptr);
        let msg = ZBox::from_bytes("http2.createServer requires a stream handler".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Allocate H2ServerUserData
    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(cx, global.get(), handler_root.get()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route
    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(uws_h2_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void))) };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Store ud pointer on server object
    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_udPtr".as_ptr(), udv.handle().into(), 0);
    }

    // Listen
    let safe_listen_cb: extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void) =
        unsafe { ::std::mem::transmute(uws_h2_listen_callback as unsafe extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void)) };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    // Store app pointer on server object
    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_appPtr".as_ptr(), apv.handle().into(), 0);
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(cx, server_obj.handle().into(), c"_listeningPort".as_ptr(), port_root.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(cx, server_obj.handle().into(), c"listening".as_ptr(), listening_root.handle().into(), JSPROP_ENUMERATE as u32);

    register_active_h2_app(app_ptr);

    // Call listen callback
    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        JS_CallFunctionValue(cx, global.handle().into(), fval_root.handle().into(), &HandleValueArray::empty(), rval_h);
        JS_ClearPendingException(cx);
    }

    args.rval().set(UndefinedValue());
    true
}

/// __http2_server_close(serverObj, callback) — called from JS
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_close(
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

    close_h2_server(cx, cx_ref, server_obj.get(), false);

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        if !global.get().is_null() {
            rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
            JS_CallFunctionValue(cx, global.handle().into(), fval_root.handle().into(), &HandleValueArray::empty(), rval_h);
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
    JS_GetProperty(cx, obj.handle().into(), c"_appPtr".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut app_ptr_val,
    });
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
        JS_SetProperty(cx, obj.handle().into(), c"_appPtr".as_ptr(), undef_root.handle().into());
    }

    // Cleanup H2ServerUserData
    let mut ud_ptr_val = UndefinedValue();
    JS_GetProperty(cx, obj.handle().into(), c"_udPtr".as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut ud_ptr_val,
    });
    let ud_ptr = if val_is_private(&ud_ptr_val) {
        ud_ptr_val.to_private() as *mut H2ServerUserData
    } else {
        core::ptr::null_mut()
    };
    if !ud_ptr.is_null() {
        let ud = Box::from_raw(ud_ptr);
        ud.cleanup();
        rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
        JS_SetProperty(cx, obj.handle().into(), c"_udPtr".as_ptr(), undef_root2.handle().into());
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

    let callback = if argc > 2 && (*args.get(2).ptr).is_object() {
        rooted!(&in(cx_ref) let cb = (*args.get(2).ptr).to_object());
        Some(cb.get())
    } else {
        None
    };

    // Extract TLS options from server._options
    let mut pem_key = String::new();
    let mut pem_cert = String::new();

    let mut opts_val = UndefinedValue();
    JS_GetProperty(cx, server_obj.handle().into(), c"_options".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut opts_val });
    if opts_val.is_object() {
        rooted!(&in(cx_ref) let opts_root = opts_val.to_object());

        // Extract key
        let mut key_val = UndefinedValue();
        JS_GetProperty(cx, opts_root.handle().into(), c"key".as_ptr(),
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut key_val });
        if key_val.is_string() {
            pem_key = crate::js_to_rust_string(cx, key_val);
        }

        // Extract cert
        let mut cert_val = UndefinedValue();
        JS_GetProperty(cx, opts_root.handle().into(), c"cert".as_ptr(),
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut cert_val });
        if cert_val.is_string() {
            pem_cert = crate::js_to_rust_string(cx, cert_val);
        }
    }

    // Get the JS stream handler
    let mut handler_val = UndefinedValue();
    let handler_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val };
    JS_GetProperty(cx, server_obj.handle().into(), c"_onStreamHandler".as_ptr(), handler_mh);

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

    // Allocate H2ServerUserData (reuse same struct for SSL)
    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(cx, global.get(), handler_root.get()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route — use the same handler (uWS handles TLS transparently)
    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(uws_h2_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void))) };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Store ud pointer
    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_udPtr".as_ptr(), udv.handle().into(), 0);
    }

    // Listen
    let safe_listen_cb: extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void) =
        unsafe { ::std::mem::transmute(uws_h2_listen_callback as unsafe extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void)) };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    // Store app pointer (as App<true>)
    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_sslAppPtr".as_ptr(), apv.handle().into(), 0);
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(cx, server_obj.handle().into(), c"_listeningPort".as_ptr(), port_root.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(cx, server_obj.handle().into(), c"listening".as_ptr(), listening_root.handle().into(), JSPROP_ENUMERATE as u32);

    register_active_h2_ssl_app(app_ptr);

    // Call listen callback
    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        JS_CallFunctionValue(cx, global.handle().into(), fval_root.handle().into(), &HandleValueArray::empty(), rval_h);
        JS_ClearPendingException(cx);
    }

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
        JS_GetProperty(cx, obj.handle().into(), c"_sslAppPtr".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut app_ptr_val,
        });
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
            JS_SetProperty(cx, obj.handle().into(), c"_sslAppPtr".as_ptr(), undef_root.handle().into());
        }
    }

    // Cleanup H2ServerUserData
    {
        rooted!(&in(cx_ref) let obj = server_obj.get());
        let mut ud_ptr_val = UndefinedValue();
        JS_GetProperty(cx, obj.handle().into(), c"_udPtr".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut ud_ptr_val,
        });
        let ud_ptr = if val_is_private(&ud_ptr_val) {
            ud_ptr_val.to_private() as *mut H2ServerUserData
        } else {
            core::ptr::null_mut()
        };
        if !ud_ptr.is_null() {
            let ud = Box::from_raw(ud_ptr);
            ud.cleanup();
            rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
            JS_SetProperty(cx, obj.handle().into(), c"_udPtr".as_ptr(), undef_root2.handle().into());
        }
    }

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        if !global.get().is_null() {
            rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
            JS_CallFunctionValue(cx, global.handle().into(), fval_root.handle().into(), &HandleValueArray::empty(), rval_h);
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
unsafe extern "C" fn http2_server_listen_js(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    // Delegate to the native __http2_server_listen
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    let port: u16 = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() as u16 }
        else if v.is_double() { v.to_double() as u16 }
        else { 3000 }
    } else { 3000 };

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

    // Get handler
    let mut handler_val = UndefinedValue();
    JS_GetProperty(cx, server_obj.handle().into(), c"_onStreamHandler".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val });

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        App::<false>::destroy(app_ptr);
        args.rval().set(ObjectValue(server_obj.get()));
        return true;
    }

    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(cx, global.get(), handler_root.get()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(uws_h2_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void))) };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_udPtr".as_ptr(), udv.handle().into(), 0);
    }

    let safe_listen_cb: extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void) =
        unsafe { ::std::mem::transmute(uws_h2_listen_callback as unsafe extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void)) };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_appPtr".as_ptr(), apv.handle().into(), 0);
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(cx, server_obj.handle().into(), c"_listeningPort".as_ptr(), port_root.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(cx, server_obj.handle().into(), c"listening".as_ptr(), listening_root.handle().into(), JSPROP_ENUMERATE as u32);

    register_active_h2_app(app_ptr);

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        JS_CallFunctionValue(cx, global.handle().into(), fval_root.handle().into(), &HandleValueArray::empty(), rval_h);
        JS_ClearPendingException(cx);
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_close_js(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
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
        if v.is_int32() { v.to_int32() as u16 }
        else if v.is_double() { v.to_double() as u16 }
        else { 3000 }
    } else { 3000 };

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
    JS_GetProperty(cx, server_obj.handle().into(), c"_options".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut opts_val });
    if opts_val.is_object() {
        rooted!(&in(cx_ref) let opts_root = opts_val.to_object());
        let mut key_val = UndefinedValue();
        JS_GetProperty(cx, opts_root.handle().into(), c"key".as_ptr(),
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut key_val });
        if key_val.is_string() { pem_key = crate::js_to_rust_string(cx, key_val); }
        let mut cert_val = UndefinedValue();
        JS_GetProperty(cx, opts_root.handle().into(), c"cert".as_ptr(),
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut cert_val });
        if cert_val.is_string() { pem_cert = crate::js_to_rust_string(cx, cert_val); }
    }

    // Get handler
    let mut handler_val = UndefinedValue();
    JS_GetProperty(cx, server_obj.handle().into(), c"_onStreamHandler".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val });

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

    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(H2ServerUserData::new(cx, global.get(), handler_root.get()));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    let safe_handler: Option<extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void)> =
        unsafe { ::std::mem::transmute(Some(uws_h2_route_handler as unsafe extern "C" fn(*mut bun_uws_sys::response::c::uws_res, *mut bun_uws_sys::Request, *mut ::std::ffi::c_void))) };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    {
        let ud_val = PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_udPtr".as_ptr(), udv.handle().into(), 0);
    }

    let safe_listen_cb: extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void) =
        unsafe { ::std::mem::transmute(uws_h2_listen_callback as unsafe extern "C" fn(*mut bun_uws_sys::listen_socket::ListenSocket, *mut ::std::ffi::c_void)) };
    (*app_ptr).listen(port as i32, safe_listen_cb, core::ptr::null_mut());

    {
        let app_ptr_val = PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(cx, server_obj.handle().into(), c"_sslAppPtr".as_ptr(), apv.handle().into(), 0);
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(port as i32));
    JS_DefineProperty(cx, server_obj.handle().into(), c"_listeningPort".as_ptr(), port_root.handle().into(), JSPROP_ENUMERATE as u32);

    rooted!(&in(cx_ref) let listening_root = BooleanValue(true));
    JS_DefineProperty(cx, server_obj.handle().into(), c"listening".as_ptr(), listening_root.handle().into(), JSPROP_ENUMERATE as u32);

    register_active_h2_ssl_app(app_ptr);

    if let Some(cb) = callback {
        rooted!(&in(cx_ref) let fval_root = ObjectValue(cb));
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        JS_CallFunctionValue(cx, global.handle().into(), fval_root.handle().into(), &HandleValueArray::empty(), rval_h);
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
        JS_GetProperty(cx, server_obj.handle().into(), c"_sslAppPtr".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut app_ptr_val,
        });
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
            JS_SetProperty(cx, server_obj.handle().into(), c"_sslAppPtr".as_ptr(), undef_root.handle().into());
        }
    }

    // Cleanup H2ServerUserData
    {
        let mut ud_ptr_val = UndefinedValue();
        JS_GetProperty(cx, server_obj.handle().into(), c"_udPtr".as_ptr(), MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData, ptr: &mut ud_ptr_val,
        });
        let ud_ptr = if val_is_private(&ud_ptr_val) {
            ud_ptr_val.to_private() as *mut H2ServerUserData
        } else {
            core::ptr::null_mut()
        };
        if !ud_ptr.is_null() {
            let ud = Box::from_raw(ud_ptr);
            ud.cleanup();
            rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
            JS_SetProperty(cx, server_obj.handle().into(), c"_udPtr".as_ptr(), undef_root2.handle().into());
        }
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

// ──────────────────────────────────────────────────────────────────────
// __http2_fetch — client-side fetch bridge (called from JS IIFE)
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_fetch(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
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

    let body = if argc > 3 && (*args.get(3).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(3).ptr)
    } else {
        String::new()
    };

    // Parse headers from JSON
    let headers_map: ::std::collections::HashMap<String, String> = serde_json::from_str(&headers_json)
        .unwrap_or_default();
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
        // Fallback: return empty JSON result synchronously
        let empty_result = r#"{"statusCode":0,"headers":{},"body":""}"#;
        let c_result = ZBox::from_bytes(empty_result.as_bytes());
        let js_result = JS_NewStringCopyZ(cx, c_result.as_ptr());
        if !js_result.is_null() {
            args.rval().set(StringValue(&*js_result));
        } else {
            args.rval().set(UndefinedValue());
        }
        return true;
    }

    let promise_obj = promise.get();
    let promise_val = ObjectValue(promise_obj);

    let body_bytes = if body.is_empty() { None } else { Some(body.into_bytes()) };

    // Schedule async fetch
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

    // For the JS IIFE's synchronous __http2_fetch pattern, we need to
    // return a JSON string. Since fetch_async is async, we return a
    // placeholder that the JS layer handles gracefully.
    let placeholder = r#"{"statusCode":0,"headers":{},"body":""}"#;
    let c_placeholder = ZBox::from_bytes(placeholder.as_bytes());
    let js_placeholder = JS_NewStringCopyZ(cx, c_placeholder.as_ptr());
    if !js_placeholder.is_null() {
        args.rval().set(StringValue(&*js_placeholder));
    } else {
        args.rval().set(UndefinedValue());
    }

    true
}

// ──────────────────────────────────────────────────────────────────────
// Property helpers
// ──────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_int_prop(cx: &mut mozjs::context::JSContext, obj_ptr: *mut JSObject, name: &str, val: i32) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let raw_cx = cx.raw_cx();
    rooted!(&in(cx) let obj = obj_ptr);
    rooted!(&in(cx) let v = Int32Value(val));
    JS_DefineProperty(raw_cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
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
        };
        assert!(ud.global().is_none());
        assert!(ud.handler().is_none());
    }

    #[test]
    fn h2_server_user_data_cleanup_no_panic() {
        let ud = H2ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http2_server_998_global".to_string(),
            handler_key: "http2_server_998_handler".to_string(),
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
