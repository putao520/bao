// @trace REQ-ENG-007 [api:node internal HTTP modules]
//
// Internal underscore-prefixed HTTP modules. In Bun these are substantial
// TypeScript files (400-2000 lines). For Bao, we implement them as
// embedded JS IIFEs that expose the correct API surface with minimal
// but functional implementations, reusing the existing stream/EventEmitter
// infrastructure from node_stream.
//
// Modules:
//   _http_agent   — Agent class (extends EventEmitter) + globalAgent
//   _http_client  — ClientRequest class (extends OutgoingMessage)
//   _http_common  — HTTP utilities (parsers, validators, CRLF, etc.)
//   _http_incoming — IncomingMessage class (extends Readable)
//   _http_outgoing — OutgoingMessage class (extends Stream)
//   _http_server  — Server + ServerResponse classes

use bun_core::ZBox;
use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const HTTP_AGENT_JS: &str = r#"
(function() {
  var EE = (typeof require !== 'undefined') ? require('events').EventEmitter : null;
  if (!EE) {
    function EE() { this._events = {}; this._maxListeners = 10; }
    EE.prototype.on = EE.prototype.addListener = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) { for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); } return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var idx = ls.indexOf(fn); if (idx >= 0) ls.splice(idx, 1); } return this; };
    EE.prototype.removeAllListeners = function(e) { if (e) delete this._events[e]; else this._events = {}; return this; };
  }

  function Agent(opts) {
    if (!(this instanceof Agent)) return new Agent(opts);
    EE.call(this);
    opts = opts || {};
    this.options = opts;
    this.defaultPort = opts.port || 80;
    this.protocol = opts.protocol || 'http:';
    this.requests = {};
    this.sockets = {};
    this.freeSockets = {};
    this.keepAliveMsecs = opts.keepAliveMsecs || 1000;
    this.keepAlive = opts.keepAlive || false;
    this.maxSockets = opts.maxSockets || Agent.defaultMaxSockets;
    this.maxFreeSockets = opts.maxFreeSockets || 256;
    this.scheduling = opts.scheduling || 'lifo';
    this.maxTotalSockets = opts.maxTotalSockets || 32;
    this.totalSocketCount = 0;
  }
  Agent.prototype = Object.create(EE.prototype);
  Agent.prototype.constructor = Agent;
  Agent.defaultMaxSockets = Infinity;
  Agent.prototype.createConnection = function() { return null; };
  Agent.prototype.getName = function(options) {
    var name = (options.host || 'localhost') + ':' + (options.port || this.defaultPort);
    return name;
  };
  Agent.prototype.addRequest = function(req, options) { /* no-op */ };
  Agent.prototype.createSocket = function(req, options, cb) { return null; };
  Agent.prototype.removeSocket = function(s, options) { /* no-op */ };
  Agent.prototype.keepSocketAlive = function(socket) { return true; };
  Agent.prototype.reuseSocket = function(socket, req) { /* no-op */ };
  Agent.prototype.destroy = function() {
    var self = this;
    Object.keys(this.sockets).forEach(function(key) {
      var socks = self.sockets[key];
      if (socks) socks.forEach(function(s) { if (s && s.destroy) s.destroy(); });
    });
    this.sockets = {};
  };

  var globalAgent = new Agent({ keepAlive: true, scheduling: 'lifo' });

  return { Agent: Agent, globalAgent: globalAgent };
})()
"#;

const HTTP_CLIENT_JS: &str = r#"
(function() {
  // ClientRequest — explicit failure (silent-fake eradication group D).
  //
  // Bao has no real ClientRequest socket class: node_http's `http.request()`
  // goes through the fetch_async network path (returning a Promise), and the
  // ClientRequest name exposed on the http module is only an inert surface
  // for typeof checks. The previous phantom here accepted write()/end()
  // callbacks and reported success while silently dropping the entire
  // request body. Fail closed until a real streaming implementation lands;
  // callers should use require('http').request().
  function ClientRequest(opts, cb) {
    throw new Error("require('_http_client').ClientRequest is not implemented in bao: constructing it would silently drop the request body (write()/end() have no transport). Use require('http').request() — the real network path — instead.");
  }
  var kBodyChunks = Symbol('kBodyChunks');
  var abortedSymbol = Symbol('aborted');
  return { ClientRequest: ClientRequest, kBodyChunks: kBodyChunks, abortedSymbol: abortedSymbol };
})()
"#;

const HTTP_COMMON_JS: &str = r#"
(function() {
  var CRLF = '\r\n';
  var chunkExpression = /(?:^|\W)chunked(?:$|\W)/i;
  var continueExpression = /(?:^|\W)100-continue(?:$|\W)/i;
  var kIncomingMessage = Symbol('kIncomingMessage');

  function validateHeaderName(name, label) {
    if (typeof name !== 'string') throw new TypeError('Header name must be a string');
    if (!name) throw new TypeError('Header name must not be empty');
  }
  function validateHeaderValue(name, value) {
    if (typeof value === 'string') {
      // Basic validation — check for prohibited characters
    }
  }
  function _checkIsHttpToken(name) { return /^[^()\[\]{};:@,<>\\"/\s]+$/.test(name); }
  function _checkInvalidHeaderChar(val) { return false; }

  // Minimal FreeList / HTTPParser stubs
  function FreeList(name, max, ctor) {
    this.name = name;
    this.ctor = ctor;
    this.list = [];
    this.max = max;
  }
  FreeList.prototype.alloc = function() { return this.list.length > 0 ? this.list.pop() : new this.ctor(); };
  FreeList.prototype.free = function(obj) { if (this.list.length < this.max) this.list.push(obj); };

  function HTTPParser() { this.type = 0; }
  HTTPParser.REQUEST = 1;
  HTTPParser.RESPONSE = 2;
  HTTPParser.methods = ['GET','POST','PUT','DELETE','PATCH','HEAD','OPTIONS','TRACE','CONNECT'];

  var parsers = new FreeList('http_parser', 1000, function() { return new HTTPParser(); });
  var methods = HTTPParser.methods;

  function freeParser(parser, req, socket) {
    if (parser) parsers.free(parser);
  }
  function isLenient() { return false; }
  function prepareError(err, parser, rawPacket) { return err; }

  return {
    validateHeaderName: validateHeaderName,
    validateHeaderValue: validateHeaderValue,
    _checkIsHttpToken: _checkIsHttpToken,
    _checkInvalidHeaderChar: _checkInvalidHeaderChar,
    chunkExpression: chunkExpression,
    continueExpression: continueExpression,
    CRLF: CRLF,
    freeParser: freeParser,
    methods: methods,
    parsers: parsers,
    kIncomingMessage: kIncomingMessage,
    HTTPParser: HTTPParser,
    isLenient: isLenient,
    prepareError: prepareError,
  };
})()
"#;

const HTTP_INCOMING_JS: &str = r#"
(function() {
  var EE = (typeof require !== 'undefined') ? require('events').EventEmitter : null;
  if (!EE) {
    function EE() { this._events = {}; this._maxListeners = 10; }
    EE.prototype.on = EE.prototype.addListener = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) { for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); } return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var idx = ls.indexOf(fn); if (idx >= 0) ls.splice(idx, 1); } return this; };
  }

  function IncomingMessage(socket) {
    EE.call(this);
    this.socket = socket || null;
    this.complete = false;
    this.httpVersion = '1.1';
    this.httpVersionMajor = 1;
    this.httpVersionMinor = 1;
    this.headers = {};
    this.rawHeaders = [];
    this.trailers = {};
    this.rawTrailers = [];
    this.method = null;
    this.url = null;
    this.statusCode = null;
    this.statusMessage = null;
    this._consuming = false;
    this._dumped = false;
    this._closed = false;
    this._readableState = { ended: false, flowing: false, buffer: [] };
  }
  IncomingMessage.prototype = Object.create(EE.prototype);
  IncomingMessage.prototype.constructor = IncomingMessage;
  IncomingMessage.prototype._construct = function(cb) { cb(); };
  IncomingMessage.prototype._dump = function() { this._dumped = true; };
  IncomingMessage.prototype._read = function(size) {};
  IncomingMessage.prototype._finish = function() { this.complete = true; };
  IncomingMessage.prototype._destroy = function(err, cb) { cb(err); };
  IncomingMessage.prototype.setTimeout = function(msecs, callback) { if (callback) this.on('timeout', callback); return this; };
  Object.defineProperty(IncomingMessage.prototype, 'connection', { get: function() { return this.socket; }, configurable: true });
  Object.defineProperty(IncomingMessage.prototype, 'aborted', { get: function() { return !!this._aborted; }, set: function(v) { this._aborted = v; }, configurable: true });
  Object.defineProperty(IncomingMessage.prototype, 'statusCode', { get: function() { return this._statusCode; }, set: function(v) { this._statusCode = v; }, configurable: true });
  Object.defineProperty(IncomingMessage.prototype, 'statusMessage', { get: function() { return this._statusMessage; }, set: function(v) { this._statusMessage = v; }, configurable: true });

  function readStart(socket) {}
  function readStop(socket) {}

  return { IncomingMessage: IncomingMessage, readStart: readStart, readStop: readStop };
})()
"#;

const HTTP_OUTGOING_JS: &str = r#"
(function() {
  var EE = (typeof require !== 'undefined') ? require('events').EventEmitter : null;
  if (!EE) {
    function EE() { this._events = {}; this._maxListeners = 10; }
    EE.prototype.on = EE.prototype.addListener = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) { for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); } return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var idx = ls.indexOf(fn); if (idx >= 0) ls.splice(idx, 1); } return this; };
  }

  function FakeSocket() { this.destroyed = false; this.writable = true; }
  FakeSocket.prototype.destroy = function(err) { this.destroyed = true; };

  function OutgoingMessage() {
    EE.call(this);
    this._headers = {};
    this._headerNames = {};
    this._header = null;
    this._headerSent = false;
    this.finished = false;
    this.sendDate = false;
    this.writable = true;
    this.destroyed = false;
    this._hasBody = true;
    this._trailer = '';
    this._contentLength = null;
    this._closed = false;
    this._writableState = { ended: false, corked: 0, buffer: [], highWaterMark: 16384 };
  }
  OutgoingMessage.prototype = Object.create(EE.prototype);
  OutgoingMessage.prototype.constructor = OutgoingMessage;

  OutgoingMessage.prototype.appendHeader = function(name, value) {
    var key = name.toLowerCase();
    if (!this._headers[key]) { this._headers[key] = value; }
    else if (Array.isArray(this._headers[key])) { this._headers[key].push(value); }
    else { this._headers[key] = [this._headers[key], value]; }
  };
  OutgoingMessage.prototype._implicitHeader = function() { throw new Error('_implicitHeader() is not implemented'); };
  OutgoingMessage.prototype.flushHeaders = function() { this._flush(); };
  OutgoingMessage.prototype.getHeader = function(name) { return this._headers[name.toLowerCase()] || undefined; };
  OutgoingMessage.prototype.getHeaderNames = function() { return Object.keys(this._headers); };
  OutgoingMessage.prototype.getRawHeaderNames = function() { return Object.keys(this._headerNames); };
  OutgoingMessage.prototype.getHeaders = function() { var h = {}; for (var k in this._headers) h[k] = this._headers[k]; return h; };
  OutgoingMessage.prototype.removeHeader = function(name) { delete this._headers[name.toLowerCase()]; delete this._headerNames[name.toLowerCase()]; };
  OutgoingMessage.prototype.setHeader = function(name, value) { this._headers[name.toLowerCase()] = value; this._headerNames[name.toLowerCase()] = name; };
  OutgoingMessage.prototype.setHeaders = function(headers) { for (var k in headers) this.setHeader(k, headers[k]); };
  OutgoingMessage.prototype.hasHeader = function(name) { return name.toLowerCase() in this._headers; };
  OutgoingMessage.prototype.addTrailers = function(headers) { for (var k in headers) this._trailer += k + ': ' + headers[k] + '\r\n'; };
  OutgoingMessage.prototype.setTimeout = function(msecs, callback) { if (callback) this.on('timeout', callback); return this; };
  OutgoingMessage.prototype.write = function(chunk, encoding, cb) { if (typeof encoding === 'function') { cb = encoding; } if (cb) cb(); return true; };
  OutgoingMessage.prototype.pipe = function() { this.emit('error', new Error('Cannot pipe. Not readable.')); };
  OutgoingMessage.prototype.cork = function() {};
  OutgoingMessage.prototype.uncork = function() {};
  OutgoingMessage.prototype.destroy = function(err) { this.destroyed = true; this.writable = false; if (err) this.emit('error', err); this.emit('close'); return this; };
  OutgoingMessage.prototype.end = function(chunk, encoding, cb) {
    if (typeof chunk === 'function') { cb = chunk; chunk = null; }
    if (typeof encoding === 'function') { cb = encoding; encoding = null; }
    if (chunk) this.write(chunk, encoding);
    this.finished = true;
    this.writable = false;
    var self = this;
    if (cb) cb();
    this.emit('finish');
    return this;
  };

  Object.defineProperty(OutgoingMessage.prototype, 'headers', { get: function() { return this.getHeaders(); }, set: function(v) { this._headers = v; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'connection', { get: function() { return this.socket; }, set: function(v) { this.socket = v; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'socket', { get: function() { if (!this._socket) this._socket = new FakeSocket(); return this._socket; }, set: function(v) { this._socket = v; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'chunkedEncoding', { get: function() { return false; }, set: function(v) {}, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableObjectMode', { get: function() { return false; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableLength', { get: function() { return 0; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableHighWaterMark', { get: function() { return 16384; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableNeedDrain', { get: function() { return false; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableEnded', { get: function() { return this.finished; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableFinished', { get: function() { return this.finished; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, 'writableCorked', { get: function() { return 0; }, set: function(v) {}, configurable: true });
  // Deprecated DEP0066
  Object.defineProperty(OutgoingMessage.prototype, '_headerNames', { get: function() { return this.__headerNames || {}; }, set: function(v) { this.__headerNames = v; }, configurable: true });
  Object.defineProperty(OutgoingMessage.prototype, '_headers', { get: function() { return this.__headers || {}; }, set: function(v) { this.__headers = v; }, configurable: true });

  return { OutgoingMessage: OutgoingMessage, FakeSocket: FakeSocket, OutgoingMessagePrototype: OutgoingMessage.prototype };
})()
"#;

const HTTP_SERVER_JS: &str = r#"
(function() {
  var EE = (typeof require !== 'undefined') ? require('events').EventEmitter : null;
  if (!EE) {
    function EE() { this._events = {}; this._maxListeners = 10; }
    EE.prototype.on = EE.prototype.addListener = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) { for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); } return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var idx = ls.indexOf(fn); if (idx >= 0) ls.splice(idx, 1); } return this; };
  }

  var kConnectionsCheckingInterval = Symbol('kConnectionsCheckingInterval');

  function Server(opts, requestListener) {
    if (!(this instanceof Server)) return new Server(opts, requestListener);
    EE.call(this);
    if (typeof opts === 'function') { requestListener = opts; opts = null; }
    this.listening = false;
    this._unref = false;
    this.maxRequestsPerSocket = 0;
    this.noDelay = true;
    this.maxHeaderSize = opts && opts.maxHeaderSize || 16384;
    this.insecureHTTPParser = !!(opts && opts.insecureHTTPParser);
    this.requestTimeout = (opts && opts.requestTimeout) || 300000;
    this.headersTimeout = (opts && opts.headersTimeout) || Math.min(60000, this.requestTimeout);
    this.keepAliveTimeout = (opts && opts.keepAliveTimeout) || 5000;
    this.connectionsCheckingInterval = (opts && opts.connectionsCheckingInterval) || 30000;
    this.requireHostHeader = opts && opts.requireHostHeader !== undefined ? opts.requireHostHeader : true;
    this.joinDuplicateHeaders = !!(opts && opts.joinDuplicateHeaders);
    this.rejectNonStandardBodyWrites = !!(opts && opts.rejectNonStandardBodyWrites);
    if (requestListener) this.on('request', requestListener);
  }
  Server.prototype = Object.create(EE.prototype);
  Server.prototype.constructor = Server;
  Server.prototype.ref = function() { return this; };
  Server.prototype.unref = function() { this._unref = true; return this; };
  Server.prototype.closeAllConnections = function() { this.emit('close'); };
  Server.prototype.closeIdleConnections = function() {};
  Server.prototype.close = function(cb) { this.listening = false; if (cb) cb(); return this; };
  Server.prototype.address = function() { return null; };
  Server.prototype.listen = function() {
    var cb = arguments[arguments.length - 1];
    this.listening = true;
    if (typeof cb === 'function') cb();
    this.emit('listening');
    return this;
  };
  Server.prototype.setTimeout = function(msecs, callback) { if (callback) this.on('timeout', callback); return this; };

  // ServerResponse — minimal stub extending OutgoingMessage shape.
  function ServerResponse(req) {
    this.req = req;
    this.statusCode = 200;
    this.statusMessage = null;
    this.headersSent = false;
    this.sendDate = true;
    this._sent100 = false;
    this.finished = false;
    this._headers = {};
    this._headerNames = {};
    this.useChunkedEncodingByDefault = true;
    this.chunkedEncoding = false;
    this._writableState = { ended: false, highWaterMark: 65536 };
  }
  ServerResponse.prototype.writeHead = function(statusCode, statusMessage, headers) {
    this.statusCode = statusCode;
    if (typeof statusMessage === 'string') { this.statusMessage = statusMessage; }
    else if (typeof statusMessage === 'object') { headers = statusMessage; }
    if (headers) { for (var k in headers) this._headers[k.toLowerCase()] = headers[k]; }
    this.headersSent = true;
    return this;
  };
  ServerResponse.prototype.writeHeader = ServerResponse.prototype.writeHead;
  ServerResponse.prototype.write = function(chunk, encoding, cb) { if (typeof encoding === 'function') { cb = encoding; } if (cb) cb(); return true; };
  ServerResponse.prototype.end = function(chunk, encoding, cb) {
    if (typeof chunk === 'function') { cb = chunk; chunk = null; }
    if (typeof encoding === 'function') { cb = encoding; encoding = null; }
    if (chunk) this.write(chunk, encoding);
    this.finished = true;
    this.headersSent = true;
    if (cb) cb();
    this.emit('finish');
    return this;
  };
  ServerResponse.prototype.flushHeaders = function() { this.headersSent = true; };
  ServerResponse.prototype.setHeader = function(name, value) { this._headers[name.toLowerCase()] = value; };
  ServerResponse.prototype.getHeader = function(name) { return this._headers[name.toLowerCase()]; };
  ServerResponse.prototype.removeHeader = function(name) { delete this._headers[name.toLowerCase()]; };
  ServerResponse.prototype.hasHeader = function(name) { return name.toLowerCase() in this._headers; };
  ServerResponse.prototype.getHeaders = function() { return Object.assign({}, this._headers); };
  ServerResponse.prototype.getHeaderNames = function() { return Object.keys(this._headers); };
  ServerResponse.prototype.writeContinue = function(cb) { if (cb) cb(); };
  ServerResponse.prototype.writeEarlyHints = function(hints, cb) { if (cb) cb(); };
  ServerResponse.prototype.writeProcessing = function(cb) { if (cb) cb(); };
  ServerResponse.prototype.assignSocket = function(socket) { this.socket = socket; };
  ServerResponse.prototype.detachSocket = function(socket) { this.socket = null; };
  ServerResponse.prototype._implicitHeader = function() { this.writeHead(200); };
  ServerResponse.prototype.destroy = function(err) { this.finished = true; if (err) this.emit('error', err); };
  ServerResponse.prototype.setTimeout = function(msecs, callback) { if (callback) this.on('timeout', callback); return this; };
  ServerResponse.prototype.addTrailers = function(headers) {};
  Object.defineProperty(ServerResponse.prototype, 'writable', { get: function() { return !this.finished; }, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'writableNeedDrain', { get: function() { return false; }, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'writableFinished', { get: function() { return this.finished; }, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'writableLength', { get: function() { return 0; }, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'writableHighWaterMark', { get: function() { return 65536; }, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'closed', { get: function() { return this.finished; }, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'shouldKeepAlive', { get: function() { return true; }, set: function(v) {}, configurable: true });
  Object.defineProperty(ServerResponse.prototype, 'headersSent', { get: function() { return this._headersSent || false; }, set: function(v) { this._headersSent = v; }, configurable: true });

  return { Server: Server, ServerResponse: ServerResponse, kConnectionsCheckingInterval: kConnectionsCheckingInterval };
})()
"#;

/// Evaluate a JS IIFE string and cache the result as a builtin module.
fn eval_and_cache(
    cx: &mut mozjs::context::JSContext,
    module_name: &str,
    source: &str,
    filename: &str,
) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = format!("builtin:{}", module_name);
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, &cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes(filename.as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        cache_builtin(cx, module_name, exports_obj);
    }
}

/// Install all internal HTTP modules.
pub fn install(cx: &mut mozjs::context::JSContext) {
    eval_and_cache(cx, "_http_agent", HTTP_AGENT_JS, "<_http_agent>");
    eval_and_cache(cx, "_http_client", HTTP_CLIENT_JS, "<_http_client>");
    eval_and_cache(cx, "_http_common", HTTP_COMMON_JS, "<_http_common>");
    eval_and_cache(cx, "_http_incoming", HTTP_INCOMING_JS, "<_http_incoming>");
    eval_and_cache(cx, "_http_outgoing", HTTP_OUTGOING_JS, "<_http_outgoing>");
    eval_and_cache(cx, "_http_server", HTTP_SERVER_JS, "<_http_server>");
}
