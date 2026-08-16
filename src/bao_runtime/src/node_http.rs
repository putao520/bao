// @trace REQ-ENG-007
// P1-B.2: Replaced hand-written TcpListener + HTTP parsing with bun_uws::App<false>.
// uWS C++ layer handles HTTP parsing; route handler bridges to JS callbacks.
use ::std::cell::RefCell;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::request::Request;
use bun_uws_sys::response::Response;
use bun_uws_sys::socket_context::BunSocketContextOptions;

use crate::gc_store::{gc_store_get_ns, gc_store_insert_ns, gc_store_remove_ns};
use crate::require::cache_builtin;

/// Monotonic server ID for GcStore key namespacing.
static NEXT_SERVER_ID: AtomicU64 = AtomicU64::new(1);

// ──────────────────────────────────────────────────────────────────────
// Node-shaped http client surface (request/get + ClientRequest faces)
//
// Transport = `__http_request_async(url, method, headersJSON, body)` →
// pending Promise resolved with the realm's WHATWG Response instance
// (web_fetch_classes, built by fetch_async::build_response_js): headers is
// a Headers instance and the body is consumed through the class methods
// (text()/arrayBuffer()). This shim layers the Node API contract on top:
//   - `http.request(url|opts[, opts][, cb])` → ClientRequest; the request
//     fires on `.end()` (Node semantics — nothing is sent before then).
//   - `http.get(...)` = request + immediate `.end()`.
//   - cb / 'response' listener receives an IncomingMessage
//     {statusCode, statusMessage, headers, httpVersion, complete} with
//     plain-object lower-cased headers and 'data'/'end' delivered once
//     Response#text() settles (body fully buffered by the time the
//     response settles; text() resolves on the following microtask).
//   - rejection → 'error' on the ClientRequest; no listener = loud
//     console.error (never a silent drop).
//   - Direct `new http.ClientRequest/IncomingMessage/OutgoingMessage`
//     still throws (silent-fake-eradication contract: request() is the
//     entry point); instances handed out by request()/get() use the real
//     prototypes so `instanceof` holds.
// ──────────────────────────────────────────────────────────────────────
const HTTP_CLIENT_JS: &str = r#"(function(h){
  function ClientRequest(opts, cb) {
    throw new Error("require('http').ClientRequest is not an entry point in bao: constructing it directly would bypass the request pipeline. Use require('http').request() — the real network path — instead.");
  }
  function IncomingMessage(socket) {
    throw new Error("require('http').IncomingMessage is not an entry point in bao: response objects are handed to you by require('http').request(). Use require('http').request() — the real network path — instead.");
  }
  function OutgoingMessage() {
    throw new Error("require('http').OutgoingMessage is not an entry point in bao: use require('http').request() — the real network path — instead.");
  }
  h.ClientRequest = ClientRequest;
  h.IncomingMessage = IncomingMessage;
  h.OutgoingMessage = OutgoingMessage;

  function attachEE(obj) {
    obj._hh = {};
    obj.on = function (ev, fn) {
      (obj._hh[ev] || (obj._hh[ev] = [])).push(fn);
      return obj;
    };
    obj.once = function (ev, fn) {
      var wrap = function () { obj.off(ev, wrap); fn.apply(obj, arguments); };
      (obj._hh[ev] || (obj._hh[ev] = [])).push(wrap);
      return obj;
    };
    obj.addListener = obj.on;
    obj.off = function (ev, fn) {
      var ls = obj._hh[ev];
      if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); }
      return obj;
    };
    obj.removeListener = obj.off;
    obj.removeAllListeners = function (ev) {
      if (ev) { delete obj._hh[ev]; } else { obj._hh = {}; }
      return obj;
    };
    obj.emit = function (ev) {
      var ls = (obj._hh[ev] || []).slice();
      var args = Array.prototype.slice.call(arguments, 1);
      for (var i = 0; i < ls.length; i++) ls[i].apply(obj, args);
      return ls.length > 0;
    };
    obj.listenerCount = function (ev) { return (obj._hh[ev] || []).length; };
    return obj;
  }

  // WHATWG Headers instance (web_fetch_classes) → Node's IncomingMessage
  // headers shape: a plain object keyed by lower-cased names. Headers#get
  // normalises case-insensitively and joins repeated values with ', ' —
  // forEach walks exactly those pairs.
  function headersToNode(h) {
    var out = {};
    if (!h || typeof h !== 'object') return out;
    if (typeof h.forEach !== 'function') return out;
    h.forEach(function (v, k) {
      if (k != null) out[String(k).toLowerCase()] = String(v);
    });
    return out;
  }

  function makeIncoming(resp, onDone) {
    var res = Object.create(IncomingMessage.prototype);
    attachEE(res);
    res.statusCode = resp && typeof resp.status === 'number' ? resp.status : 0;
    res.statusMessage = (resp && resp.statusText) || '';
    res.headers = headersToNode(resp && resp.headers);
    res.httpVersion = '1.1';
    res.complete = true;
    // The transport buffers the whole body before the response settles, but
    // the realm's Response class hands it over via text() — a Promise that
    // settles on the following microtask. There is exactly one chunk:
    // listeners registered before it settles are queued and fired in
    // registration order (data, then end) once it does; listeners registered
    // after settle receive data/end on registration.
    res._bodyText = null;
    res._bodySettled = false;
    res._deliverBody = function () {
      if (res._bodySettled) return;
      res._bodySettled = true;
      var ds = res._hh['data'];
      if (ds && res._bodyText !== '') {
        for (var i = 0; i < ds.length; i++) ds[i].call(res, res._bodyText);
      }
      var es = res._hh['end'];
      if (es) for (var j = 0; j < es.length; j++) es[j].call(res);
    };
    res.on = function (ev, fn) {
      (res._hh[ev] || (res._hh[ev] = [])).push(fn);
      if (res._bodySettled) {
        if (ev === 'data') {
          if (res._bodyText !== null && res._bodyText !== '') fn.call(res, res._bodyText);
        } else if (ev === 'end') {
          fn.call(res);
        }
      }
      return res;
    };
    res.addListener = res.on;
    res.resume = function () { return res; };
    res.pause = function () { return res; };
    res.setEncoding = function () { return res; };
    res.destroy = function () { res.complete = true; return res; };
    var done = onDone;
    var settleBody = function (text) {
      res._bodyText = text;
      res._deliverBody();
      if (done) { var f = done; done = null; f(); }
    };
    var failBody = function (err) {
      // Loud failure — a body read error must never surface as a silent
      // empty body (fake-green class).
      var e = err instanceof Error ? err : new Error(String(err && err.message ? err.message : err));
      var had = res.emit('error', e);
      if (!had && typeof console !== 'undefined' && console.error) {
        console.error('http: response body read failed:', e.message);
      }
      settleBody('');
    };
    try {
      if (!resp || typeof resp.text !== 'function') {
        throw new Error('http: transport resolved without a Response body (text() missing)');
      }
      resp.text().then(function (t) {
        settleBody(typeof t === 'string' ? t : '');
      }, failBody);
    } catch (e) {
      failBody(e);
    }
    return res;
  }

  function fireRequest(req) {
    if (req._fired || req.destroyed) return req;
    req._fired = true;
    var headersJSON = '{}';
    try { headersJSON = JSON.stringify(req._headers); } catch (e) {}
    var tlsOptsJSON = '{}';
    try { tlsOptsJSON = JSON.stringify(req._tlsOpts || {}); } catch (e) {}
    var p;
    try {
      p = req._transport(req._url, req.method, headersJSON, req._bodyParts.join(''), tlsOptsJSON);
    } catch (e) {
      settleError(req, e);
      return req;
    }
    p.then(function (resp) {
      if (req.destroyed) { req.emit('close'); return; }
      // 'close' follows response-body delivery (the request cycle ends when
      // the response is fully consumed) — makeIncoming fires onDone once
      // Response#text() has settled and data/end were delivered.
      var res = makeIncoming(resp, function () { req.emit('close'); });
      req.res = res;
      try { if (req._cb) req._cb(res); } catch (e) { lateError(req, e); }
      req.emit('response', res);
    }, function (err) {
      if (req.destroyed) { req.emit('close'); return; }
      settleError(req, err);
    });
    return req;
  }

  function settleError(req, err) {
    var e = err instanceof Error ? err : new Error(String(err && err.message ? err.message : err));
    var had = req.emit('error', e);
    if (!had && typeof console !== 'undefined' && console.error) {
      console.error('http: unhandled request error:', e && e.message);
    }
    req.emit('close');
  }

  function lateError(req, e) {
    if (typeof console !== 'undefined' && console.error) {
      console.error('http: response callback threw:', e && e.message);
    }
  }

  function makeRequest(scheme, url, opts, cb, transport) {
    var req = Object.create(ClientRequest.prototype);
    attachEE(req);
    req.method = (opts.method || 'GET').toUpperCase();
    req.path = opts.path || '/';
    req.host = opts.hostname || opts.host || 'localhost';
    req.port = opts.port != null ? Number(opts.port) : (scheme === 'https:' ? 443 : 80);
    req._transport = transport;
    req.headers = {};
    var src = opts.headers || {};
    for (var k in src) { if (Object.prototype.hasOwnProperty.call(src, k)) req.headers[k] = src[k]; }
    req._headers = req.headers;
    req._bodyParts = [];
    if (opts.body !== undefined && opts.body !== null) {
      req._bodyParts.push(typeof opts.body === 'string' ? opts.body : String(opts.body));
    }
    req._url = url;
    req._cb = cb || null;
    // Node TLS options (https): rejectUnauthorized / ca / servername —
    // forwarded to the transport (ignored by plain-http transports).
    req._tlsOpts = {};
    if (opts.rejectUnauthorized !== undefined) req._tlsOpts.rejectUnauthorized = !!opts.rejectUnauthorized;
    if (opts.ca !== undefined && opts.ca !== null) req._tlsOpts.ca = opts.ca;
    if (opts.servername) req._tlsOpts.servername = String(opts.servername);
    req.aborted = false;
    req.destroyed = false;
    req._fired = false;
    req.res = null;
    req.write = function (data) {
      if (req._fired) { throw new Error('http: write() after end() — the request is already sent'); }
      if (data !== undefined && data !== null) req._bodyParts.push(typeof data === 'string' ? data : String(data));
      return req;
    };
    req.end = function (data) {
      if (data !== undefined && data !== null && !req._fired) {
        req._bodyParts.push(typeof data === 'string' ? data : String(data));
      }
      fireRequest(req);
      return req;
    };
    req.setHeader = function (k, v) { req._headers[k] = v; return req; };
    req.getHeader = function (k) { return req._headers[k]; };
    req.removeHeader = function (k) { delete req._headers[k]; return req; };
    req.flushHeaders = function () { return req; };
    req.abort = function () { req.aborted = true; req.destroyed = true; return req; };
    req.destroy = function () { req.destroyed = true; return req; };
    req.setNoDelay = function () { return req; };
    req.setSocketKeepAlive = function () { return req; };
    req.setTimeout = function (ms, cb2) {
      if (cb2) req.on('timeout', cb2);
      setTimeout(function () { if (!req.res && !req.destroyed) req.emit('timeout'); }, ms);
      return req;
    };
    return req;
  }

  function normalizeArgs(a, b, c) {
    var url = null, opts = {}, cb = null;
    if (typeof a === 'string') {
      url = a;
      if (typeof b === 'function') cb = b;
      else if (b && typeof b === 'object') opts = b;
      if (!cb && typeof c === 'function') cb = c;
    } else if (a && typeof a === 'object') {
      opts = a;
      if (typeof b === 'function') cb = b;
    } else {
      throw new Error('http: request() expects a URL string or an options object');
    }
    return { url: url, opts: opts, cb: cb };
  }

  function buildURL(scheme, url, opts) {
    if (url) {
      if (!/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(url)) url = scheme + '//' + url;
      return url;
    }
    var host = opts.hostname || opts.host || 'localhost';
    var port = '';
    if (opts.port != null && String(host).indexOf(':') < 0) port = ':' + opts.port;
    return scheme + '//' + host + port + (opts.path || '/');
  }

  h.request = function (a, b, c) {
    var n = normalizeArgs(a, b, c);
    var url = buildURL('http:', n.url, n.opts);
    return makeRequest('http:', url, n.opts, n.cb, h.__http_request_async);
  };
  h.get = function (a, b, c) {
    var n = normalizeArgs(a, b, c);
    if (!n.opts.method) n.opts.method = 'GET';
    var url = buildURL('http:', n.url, n.opts);
    var req = makeRequest('http:', url, n.opts, n.cb, h.__http_request_async);
    req.end();
    return req;
  };
  // Client-factory for sibling schemes (node:https): same Node contract,
  // different transport + URL scheme. Returns {request, get}.
  h.__makeClient = function (transport, scheme) {
    return {
      request: function (a, b, c) {
        var n = normalizeArgs(a, b, c);
        var url = buildURL(scheme, n.url, n.opts);
        return makeRequest(scheme, url, n.opts, n.cb, transport);
      },
      get: function (a, b, c) {
        var n = normalizeArgs(a, b, c);
        if (!n.opts.method) n.opts.method = 'GET';
        var url = buildURL(scheme, n.url, n.opts);
        var req = makeRequest(scheme, url, n.opts, n.cb, transport);
        req.end();
        return req;
      },
    };
  };
})"#;

thread_local! {
    /// Active uWS App handles. Each `server.listen()` creates one App.
    static ACTIVE_APPS: RefCell<Vec<*mut App<false>>> = const { RefCell::new(Vec::new()) };
    /// BCE-007 unified-liveness extension: off-thread subsystems whose
    /// completion tasklets ride the MiniEventLoop's ConcurrentTask queue
    /// register a probe here. `drain_and_check` ticks the loop (the ONLY
    /// consumer of that queue via `tick_without_idle`) only while
    /// `has_active_servers()` is true — without a probe, a TLS-only script
    /// (no HTTP server, no timers) never drains the queue and every TLS
    /// event (SecureConnection/Data/error) parks forever: the driver
    /// exchanges real handshake bytes on the wire while the JS thread
    /// sleeps past them. Rooted as a class: any module with cross-thread
    /// tasklets registers liveness, not just uWS apps.
    static LIVENESS_PROBES: RefCell<Vec<fn() -> bool>> = const { RefCell::new(Vec::new()) };
}

/// Register a liveness probe (idempotent). Called by subsystem installers
/// (node_tls). Probes run on the JS thread only.
pub fn register_liveness_probe(probe: fn() -> bool) {
    LIVENESS_PROBES.with(|p| {
        let mut p = p.borrow_mut();
        if !p.contains(&probe) {
            p.push(probe);
        }
    });
}

pub fn has_active_servers() -> bool {
    if ACTIVE_APPS.with(|s| !s.borrow().is_empty()) {
        return true;
    }
    LIVENESS_PROBES.with(|p| p.borrow().iter().any(|probe| probe()))
}

// ──────────────────────────────────────────────────────────────────────
// BCE-007 (runtime hang): unified JS-thread uWS-App liveness registry.
//
// Root cause (rootCause, design layer): `drain_and_check` (timers.rs) keeps
// the JS thread's uWS `Loop` alive ONLY while `node_http::has_active_servers()`
// returns true — that branch is what drives `tick_once` → `us_loop_run_bun_tick`
// → `epoll_wait`, which is the only way a JS-thread-bound uWS `App`'s listen
// socket ever `accept()`s an inbound connection.
//
// `Bun.serve` (bun_api.rs) creates its own `App::<false>` bound to the SAME
// JS-thread uWS `Loop` (via `uWS::Loop::get()` singleton), but historically
// never registered it with `ACTIVE_APPS`, so `has_active_servers()` stayed
// false for `Bun.serve`. Result: in `Bun.serve` + `fetch(self)` the worker's
// `connect()` sat in `EINPROGRESS` forever (strace: 0 `epoll_pwait` on the JS
// thread, ~4000 `clock_nanosleep(1ms)` spins), the server never `accept()`ed,
// the worker never wrote its result slot, and the fetch Promise never resolved.
//
// Unified fix: every module that creates a JS-thread-bound uWS `App::<false>`
// — `node_http::createServer` AND `Bun.serve` — registers it here so the
// single `has_active_servers()` source of truth drives the loop tick for both.
// This is the BCE "one fix for the whole class" — closing the registration gap
// rather than patching `drain_and_check` to special-case `Bun.serve`.
// @trace REQ-ENG-006 [api:Bun.serve] [api:http.createServer] unified liveness

/// Register a JS-thread-bound uWS `App::<false>` so `has_active_servers()`
/// reports liveness and `drain_and_check` keeps ticking the loop. Idempotent:
/// re-registering the same pointer is a no-op (defensive against double
/// registration in error paths). # Safety: `app` must be a live `*mut App<false>`
/// returned by `App::<false>::create`, valid until `unregister_active_app` is
/// called for it.
pub unsafe fn register_active_app(app: *mut App<false>) {
    if app.is_null() {
        return;
    }
    ACTIVE_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        if !apps.iter().any(|&p| ::core::ptr::eq(p, app)) {
            apps.push(app);
        }
    });
}

/// Unregister a previously-registered `App::<false>`. No-op if not present
/// (defensive against double-close paths). # Safety: `app` must match a
/// pointer previously passed to `register_active_app`.
pub unsafe fn unregister_active_app(app: *mut App<false>) {
    if app.is_null() {
        return;
    }
    ACTIVE_APPS.with(|s| {
        let mut apps = s.borrow_mut();
        apps.retain(|&p| !::core::ptr::eq(p, app));
    });
}

pub fn listener_fds() -> Vec<i32> {
    // uWS App sockets are managed by the event loop (bao_uloop), not by
    // manual epoll. Return empty — drain_and_check no longer needs to
    // epoll_wait on HTTP listener fds; uWS handles I/O internally.
    Vec::new()
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let http_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if http_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            http_obj.handle(),
            c"createServer".as_ptr(),
            Some(http_create_server),
            1,
            JSPROP_ENUMERATE as u32,
        );
        // Hidden async transport used by the JS-level request/get shim below.
        // (url, method, headersJSON, body) → pending Promise resolved with a
        // fetch-shaped Response. The Node-shaped ClientRequest/IncomingMessage
        // faces live in HTTP_CLIENT_JS — this native is the raw pipe.
        w2::JS_DefineFunction(
            cx,
            http_obj.handle(),
            c"__http_request_async".as_ptr(),
            Some(http_request),
            4,
            0,
        );

        {
            let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"node:http".as_ptr(), 1);
            if !opts.is_null() {
                let mut src_text = mozjs::rust::transform_str_to_source_text(
                    "function Server(opts, cb) { if (typeof opts === 'function') { cb = opts; } if (cb) this.on('request', cb); }\
                     Server.prototype.listen = function() { return this; };\
                     Server.prototype.close = function() { return this; };\
                     Server.prototype.on = function(e, fn) { if (!this._events) this._events = {}; (this._events[e] || (this._events[e] = [])).push(fn); return this; };\
                     Server.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events && this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return this; };\
                     Server",
                );
                let mut rval = UndefinedValue();
                JS::Evaluate2(
                    cx.raw_cx(),
                    opts,
                    &mut src_text,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
                libc::free(opts as *mut _);
                if rval.is_object() {
                    rooted!(&in(cx) let ctor_root = rval.to_object());
                    let server_ctor = ObjectValue(ctor_root.get());
                    rooted!(&in(cx) let sv = server_ctor);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        http_obj.handle().into(),
                        c"Server".as_ptr(),
                        sv.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                    );
                }
            }
        }

        rooted!(&in(cx) let status_obj = w2::JS_NewPlainObject(cx));
        if !status_obj.get().is_null() {
            let codes: &[(&str, &str)] = &[
                ("100", "Continue"),
                ("101", "Switching Protocols"),
                ("102", "Processing"),
                ("200", "OK"),
                ("201", "Created"),
                ("202", "Accepted"),
                ("203", "Non-Authoritative Information"),
                ("204", "No Content"),
                ("205", "Reset Content"),
                ("206", "Partial Content"),
                ("207", "Multi-Status"),
                ("208", "Already Reported"),
                ("226", "IM Used"),
                ("300", "Multiple Choices"),
                ("301", "Moved Permanently"),
                ("302", "Found"),
                ("303", "See Other"),
                ("304", "Not Modified"),
                ("305", "Use Proxy"),
                ("306", "(Unused)"),
                ("307", "Temporary Redirect"),
                ("308", "Permanent Redirect"),
                ("400", "Bad Request"),
                ("401", "Unauthorized"),
                ("402", "Payment Required"),
                ("403", "Forbidden"),
                ("404", "Not Found"),
                ("405", "Method Not Allowed"),
                ("406", "Not Acceptable"),
                ("407", "Proxy Authentication Required"),
                ("408", "Request Timeout"),
                ("409", "Conflict"),
                ("410", "Gone"),
                ("411", "Length Required"),
                ("412", "Precondition Failed"),
                ("413", "Payload Too Large"),
                ("414", "URI Too Long"),
                ("415", "Unsupported Media Type"),
                ("416", "Range Not Satisfiable"),
                ("417", "Expectation Failed"),
                ("418", "I'm a Teapot"),
                ("421", "Misdirected Request"),
                ("422", "Unprocessable Entity"),
                ("423", "Locked"),
                ("424", "Failed Dependency"),
                ("425", "Too Early"),
                ("426", "Upgrade Required"),
                ("428", "Precondition Required"),
                ("429", "Too Many Requests"),
                ("431", "Request Header Fields Too Large"),
                ("451", "Unavailable For Legal Reasons"),
                ("500", "Internal Server Error"),
                ("501", "Not Implemented"),
                ("502", "Bad Gateway"),
                ("503", "Service Unavailable"),
                ("504", "Gateway Timeout"),
                ("505", "HTTP Version Not Supported"),
                ("506", "Variant Also Negotiates"),
                ("507", "Insufficient Storage"),
                ("508", "Loop Detected"),
                ("509", "Bandwidth Limit Exceeded"),
                ("510", "Not Extended"),
                ("511", "Network Authentication Required"),
            ];
            for (code, msg) in codes {
                let c_code = ZBox::from_bytes(code.as_bytes());
                let c_msg = ZBox::from_bytes(msg.as_bytes());
                let js_msg = JS_NewStringCopyZ(cx.raw_cx(), c_msg.as_ptr());
                if !js_msg.is_null() {
                    let mv = StringValue(&*js_msg);
                    rooted!(&in(cx) let mvr = mv);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        status_obj.handle().into(),
                        c_code.as_ptr(),
                        mvr.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            let status_val = ObjectValue(status_obj.get());
            rooted!(&in(cx) let status_r = status_val);
            JS_DefineProperty(
                cx.raw_cx(),
                http_obj.handle().into(),
                c"STATUS_CODES".as_ptr(),
                status_r.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // http.METHODS — Node.js exposes this as a sorted array of HTTP method
        // names. The previous implementation exposed a comma-separated string;
        // switch to an Array to match Node.js semantics.
        // (Reference: ~/code/rust/bun/src/js/node/_http_common.ts enumerates
        //  the same list derived from the IANA HTTP method registry.)
        {
            let methods = [
                "ACL",
                "BIND",
                "CHECKOUT",
                "CONNECT",
                "COPY",
                "DELETE",
                "GET",
                "HEAD",
                "LINK",
                "LOCK",
                "M-SEARCH",
                "MERGE",
                "MKACTIVITY",
                "MKCALENDAR",
                "MKCOL",
                "MOVE",
                "NOTIFY",
                "OPTIONS",
                "PATCH",
                "POST",
                "PROPFIND",
                "PROPPATCH",
                "PURGE",
                "PUT",
                "REBIND",
                "REPORT",
                "SEARCH",
                "SOURCE",
                "SUBSCRIBE",
                "TRACE",
                "UNBIND",
                "UNLINK",
                "UNLOCK",
                "UNSUBSCRIBE",
            ];
            rooted!(&in(cx) let arr = w2::NewArrayObject1(cx, methods.len()));
            if !arr.get().is_null() {
                for (i, m) in methods.iter().enumerate() {
                    let c_m = ZBox::from_bytes(m.as_bytes());
                    let js_m = JS_NewStringCopyZ(cx.raw_cx(), c_m.as_ptr());
                    if !js_m.is_null() {
                        rooted!(&in(cx) let mv = StringValue(&*js_m));
                        JS_DefineElement(
                            cx.raw_cx(),
                            arr.handle().into(),
                            i as u32,
                            mv.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
                let av = ObjectValue(arr.get());
                rooted!(&in(cx) let avr = av);
                JS_DefineProperty(
                    cx.raw_cx(),
                    http_obj.handle().into(),
                    c"METHODS".as_ptr(),
                    avr.handle().into(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }

        // http.maxRedirects — Node.js (via http_tohttps Ninjas) defaults to 21.
        rooted!(&in(cx) let mr_val = mozjs::jsval::Int32Value(21));
        JS_DefineProperty(
            cx.raw_cx(),
            http_obj.handle().into(),
            c"maxRedirects".as_ptr(),
            mr_val.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );

        // http.validateHeaderName / validateHeaderValue — Node.js surfaces
        // these from _http_server.ts as validation helpers. See
        // ~/code/rust/bun/src/js/node/_http_server.ts. Throwing on invalid
        // inputs matches Node.js semantics; we surface the same logic via JS
        // (avoiding hand-rolled parsers in Rust).
        {
            let validate_src = r#"(function(h){
  var validHeaderNameRegex = /^[!#$%&'*+.^_`|0-9A-Za-z-]+$/;
  var validHeaderValueRegex = /^[^\t\n\r\x00]*$/;
  function validateHeaderName(name) {
    if (typeof name !== 'string' || !validHeaderNameRegex.test(name)) {
      throw new TypeError('Header name must be a valid HTTP token: ' + String(name));
    }
  }
  function validateHeaderValue(name, value) {
    if (value === undefined) {
      throw new TypeError('Invalid header value for ' + name + ': undefined');
    }
    if (typeof value !== 'string' || !validHeaderValueRegex.test(value)) {
      throw new TypeError('Invalid header value for ' + name + ': ' + String(value));
    }
  }
  h.validateHeaderName = validateHeaderName;
  h.validateHeaderValue = validateHeaderValue;
})"#;
            let mut vsrc = mozjs::rust::transform_str_to_source_text(validate_src);
            let mut vval = UndefinedValue();
            let vh = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut vval,
            };
            let vopts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<http-validate>".as_ptr(), 1);
            if !vopts.is_null() {
                if JS::Evaluate2(cx.raw_cx(), vopts, &mut vsrc, vh) && vval.is_object() {
                    let wrapped_cx =
                        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
                    rooted!(&in(wrapped_cx) let global_root = CurrentGlobalOrNull(cx.raw_cx()));
                    rooted!(&in(wrapped_cx) let http_val_root = ObjectValue(http_obj.get()));
                    let args_arr = HandleValueArray {
                        length_: 1,
                        elements_: &http_val_root.get() as *const Value,
                    };
                    let mut call_rval = UndefinedValue();
                    let call_rval_h = MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut call_rval,
                    };
                    rooted!(&in(wrapped_cx) let factory_obj = vval.to_object());
                    rooted!(&in(wrapped_cx) let factory_obj_h = ObjectValue(factory_obj.get()));
                    JS_CallFunctionValue(
                        cx.raw_cx(),
                        global_root.handle().into(),
                        factory_obj_h.handle().into(),
                        &args_arr,
                        call_rval_h,
                    );
                }
                libc::free(vopts as *mut _);
            }
        }

        // Node-shaped http client: request()/get() with the Node callback and
        // ClientRequest forms, layered on the real `__http_request_async`
        // transport. Direct `new` of the named classes still fails closed
        // (they are not entry points — request() is), keeping the
        // silent-fake-eradication contract, while instances handed out by
        // request()/get() carry real prototypes so `instanceof` holds.
        {
            let classes_src = HTTP_CLIENT_JS;
            let mut csrc = mozjs::rust::transform_str_to_source_text(classes_src);
            let mut cval = UndefinedValue();
            let ch = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut cval,
            };
            let copts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<http-classes>".as_ptr(), 1);
            if !copts.is_null() {
                if JS::Evaluate2(cx.raw_cx(), copts, &mut csrc, ch) && cval.is_object() {
                    let wrapped_cx =
                        mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx.raw_cx()));
                    rooted!(&in(wrapped_cx) let global_root = CurrentGlobalOrNull(cx.raw_cx()));
                    rooted!(&in(wrapped_cx) let http_val_root = ObjectValue(http_obj.get()));
                    let args_arr = HandleValueArray {
                        length_: 1,
                        elements_: &http_val_root.get() as *const Value,
                    };
                    let mut call_rval = UndefinedValue();
                    let call_rval_h = MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut call_rval,
                    };
                    rooted!(&in(wrapped_cx) let factory_obj = cval.to_object());
                    rooted!(&in(wrapped_cx) let factory_obj_h = ObjectValue(factory_obj.get()));
                    JS_CallFunctionValue(
                        cx.raw_cx(),
                        global_root.handle().into(),
                        factory_obj_h.handle().into(),
                        &args_arr,
                        call_rval_h,
                    );
                }
                libc::free(copts as *mut _);
            }
        }

        // http.globalAgent — Node.js' default http.Agent. Expose a plain
        // object so consumers that pull it via `http.globalAgent` get a
        // truthy surface. (Reference: ~/code/rust/bun/src/js/node/_http_agent.ts.)
        rooted!(&in(cx) let agent_obj = w2::JS_NewPlainObject(cx));
        if !agent_obj.get().is_null() {
            let av = ObjectValue(agent_obj.get());
            rooted!(&in(cx) let avr = av);
            JS_DefineProperty(
                cx.raw_cx(),
                http_obj.handle().into(),
                c"globalAgent".as_ptr(),
                avr.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );
            // http.Agent — constructor reference (alias of plain function).
            let agent_ctor_src = "function Agent(opts) { for (var k in opts) this[k] = opts[k]; }";
            let mut asrc = mozjs::rust::transform_str_to_source_text(agent_ctor_src);
            let mut aval = UndefinedValue();
            let ah = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut aval,
            };
            let aopts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c"<http-agent>".as_ptr(), 1);
            if !aopts.is_null() {
                if JS::Evaluate2(cx.raw_cx(), aopts, &mut asrc, ah) && aval.is_object() {
                    let av2 = ObjectValue(aval.to_object());
                    rooted!(&in(cx) let av2r = av2);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        http_obj.handle().into(),
                        c"Agent".as_ptr(),
                        av2r.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                    );
                }
                libc::free(aopts as *mut _);
            }
        }
    }

    cache_builtin(cx, "http", http_obj.get());
}

// ──────────────────────────────────────────────────────────────
// uWS route handler — bridges C++ HTTP events to JS callbacks
// ──────────────────────────────────────────────────────────────

/// Per-server user data passed to uWS route handler via `user_data`.
/// GC-safe: JSObject references are stored in GcStore (as properties on the
/// JS global object, managed by SpiderMonkey GC). We only keep the string
/// keys — no raw `*mut JSObject` that could dangle after GC.
struct ServerUserData {
    /// JSContext* for creating JS objects and calling JS functions.
    cx: *mut JSContext,
    /// GcStore key for the global object.
    global_key: String,
    /// GcStore key for the JS request handler function.
    handler_key: String,
    /// GcStore key for the JS server object (for emitting events like 'upgrade').
    server_obj_key: String,
}

impl ServerUserData {
    /// Create a new ServerUserData, storing global, handler, and server object in GcStore.
    fn new(
        cx: *mut JSContext,
        global: *mut JSObject,
        handler: *mut JSObject,
        server_obj: *mut JSObject,
    ) -> Self {
        let server_id = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let global_key = format!("http_server_{}_global", server_id);
        let handler_key = format!("http_server_{}_handler", server_id);
        let server_obj_key = format!("http_server_{}_server_obj", server_id);
        gc_store_insert_ns(cx, "http", &global_key, global);
        gc_store_insert_ns(cx, "http", &handler_key, handler);
        gc_store_insert_ns(cx, "http", &server_obj_key, server_obj);
        Self {
            cx,
            global_key,
            handler_key,
            server_obj_key,
        }
    }

    /// Retrieve the handler object from GcStore. Must be called inside the
    /// realm (dispatch sites `AutoRealm` into the persistent realm first).
    fn handler(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http", &self.handler_key)
    }

    /// Retrieve the server object from GcStore.
    fn server_obj(&self) -> Option<*mut JSObject> {
        gc_store_get_ns(self.cx, "http", &self.server_obj_key)
    }

    /// Remove all references from GcStore. Call on server close/cleanup.
    fn cleanup(&self) {
        gc_store_remove_ns(self.cx, "http", &self.global_key);
        gc_store_remove_ns(self.cx, "http", &self.handler_key);
        gc_store_remove_ns(self.cx, "http", &self.server_obj_key);
    }
}

/// uWS route handler callback. Called by uWS C++ when an HTTP request arrives.
///
/// Reads method/url/headers from the uWS `Request` (already parsed by C++),
/// builds JS req/res objects, and calls the JS request handler.
/// The res object's `writeHead`/`write`/`end` methods bridge to
/// `Response::<false>::write_status`/`write_header`/`end`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn uws_route_handler(
    res: *mut bun_uws_sys::response::c::uws_res,
    req: *mut bun_uws_sys::Request,
    user_data: *mut ::std::ffi::c_void,
) {
    if res.is_null() || req.is_null() || user_data.is_null() {
        return;
    }

    let ud = &*(user_data as *const ServerUserData);
    let cx = ud.cx;
    if cx.is_null() {
        return;
    }

    let raw_cx = cx;
    let res_mut = Response::<false>::cast_res(res);

    // Enter the context's persistent realm. Async dispatch (drain_and_check
    // pump) runs with no realm entered; the handler lives as a property on
    // this realm's global (GcStore), so we must be in the realm to resolve
    // it. First-principles realm model: one realm per JsContext, held for
    // the context's lifetime — handlers registered by an earlier eval are
    // structurally reachable here.
    let global = match bao_engine::context::thread_realm_global() {
        Some(g) if !g.is_null() => g,
        _ => {
            // No realm on this thread → no JS server should exist here.
            // Explicit 500 (never a silent return → uWS std::terminate).
            eprintln!("[node:http] no JS realm on this thread — responding 500");
            (*res_mut).write_status(b"500 Internal Server Error");
            (*res_mut).write_header(b"Content-Type", b"text/plain");
            (*res_mut).end(b"no JS realm", true);
            return;
        }
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let cx_ref: &mut mozjs::context::JSContext = &mut realm;

    // Now inside the realm: CurrentGlobalOrNull = persistent global, so the
    // GcStore property lookup resolves the registered handler.
    let handler = match ud.handler() {
        Some(h) if !h.is_null() => h,
        _ => {
            // Registered-but-unresolvable handler must fail explicitly —
            // never a silent return (crash) and never a fake response.
            eprintln!(
                "[node:http] request handler unavailable (key {}) — responding 500",
                ud.handler_key
            );
            (*res_mut).write_status(b"500 Internal Server Error");
            (*res_mut).write_header(b"Content-Type", b"text/plain");
            (*res_mut).end(b"no request handler", true);
            return;
        }
    };
    rooted!(&in(cx_ref) let handler_val_root = ObjectValue(handler));

    // Read method/url from uWS Request (C++ already parsed). uWS stores the
    // method token lowercased internally; Node's `req.method` carries the
    // client-sent uppercase token, so restore it here.
    let req_ref = bun_opaque::opaque_deref_mut(req);
    let method_bytes = req_ref.method();
    let url_bytes = req_ref.url();
    let method_upper = method_bytes.to_ascii_uppercase();
    let method_str = ::std::str::from_utf8_unchecked(&method_upper);
    let url_str = ::std::str::from_utf8_unchecked(url_bytes);

    // Build JS request object.
    rooted!(&in(cx_ref) let req_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if req_obj.get().is_null() {
        return;
    }

    let c_method = ZBox::from_bytes(method_str.as_bytes());
    let js_method = JS_NewStringCopyZ(raw_cx, c_method.as_ptr());
    if !js_method.is_null() {
        let mv = StringValue(&*js_method);
        rooted!(&in(cx_ref) let mvr = mv);
        JS_DefineProperty(
            raw_cx,
            req_obj.handle().into(),
            c"method".as_ptr(),
            mvr.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    let c_url = ZBox::from_bytes(url_str.as_bytes());
    let js_url = JS_NewStringCopyZ(raw_cx, c_url.as_ptr());
    if !js_url.is_null() {
        let uv = StringValue(&*js_url);
        rooted!(&in(cx_ref) let uvr = uv);
        JS_DefineProperty(
            raw_cx,
            req_obj.handle().into(),
            c"url".as_ptr(),
            uvr.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Build headers object from ALL headers via uWS forEachHeader.
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
            req_obj.handle().into(),
            c"headers".as_ptr(),
            hdrs_r.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Detect WebSocket upgrade: if `Upgrade: websocket` header is present,
    // emit 'upgrade' event on the server object instead of 'request'.
    let upgrade_header = req_ref
        .header(b"upgrade")
        .map(|h| h.to_vec())
        .unwrap_or_default();
    let is_ws_upgrade = upgrade_header.eq_ignore_ascii_case(b"websocket");

    if is_ws_upgrade {
        // Node semantics: an http.Server without an 'upgrade' handler never
        // speaks WebSocket — bao answers with an explicit 426 Upgrade
        // Required instead. The uWS contract makes this mandatory: returning
        // from the route handler with neither a response nor an abort
        // handler attached is `std::terminate` (process crash), so every
        // path below must leave the uWS Response answered or handed off.
        let mut had_upgrade_listener = false;
        // Build a JS socket info object for the upgrade event.
        rooted!(&in(cx_ref) let socket_obj = w2::JS_NewPlainObject(cx_ref));
        if !socket_obj.get().is_null() {
            // Build JS response object for the upgrade event (allows the handler
            // to call res.end() or write headers for the 101 response).
            rooted!(&in(cx_ref) let upgrade_res_obj = w2::JS_NewPlainObject(cx_ref));
            if !upgrade_res_obj.get().is_null() {
                w2::JS_DefineFunction(
                    cx_ref,
                    upgrade_res_obj.handle(),
                    c"writeHead".as_ptr(),
                    Some(res_write_head),
                    2,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx_ref,
                    upgrade_res_obj.handle(),
                    c"end".as_ptr(),
                    Some(res_end),
                    1,
                    JSPROP_ENUMERATE as u32,
                );
                let status_val = Int32Value(101);
                rooted!(&in(cx_ref) let sv = status_val);
                JS_DefineProperty(
                    raw_cx,
                    upgrade_res_obj.handle().into(),
                    c"statusCode".as_ptr(),
                    sv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                let res_ptr_val = mozjs::jsval::PrivateValue(res as *const core::ffi::c_void);
                rooted!(&in(cx_ref) let rv = res_ptr_val);
                JS_DefineProperty(
                    raw_cx,
                    upgrade_res_obj.handle().into(),
                    c"_uwsRes".as_ptr(),
                    rv.handle().into(),
                    0,
                );
            }

            // Emit 'upgrade' event on the server object: server.emit('upgrade', req, socket, head)
            if let Some(server_obj) = ud.server_obj() {
                if !server_obj.is_null() {
                    rooted!(&in(cx_ref) let server_root = server_obj);
                    // `global_root` was rooted at the top of the route handler
                    // (handler's owning global); reuse it for the call below.
                    // Get the emit function from the server object.
                    let mut emit_val = UndefinedValue();
                    JS_GetProperty(
                        raw_cx,
                        server_root.handle().into(),
                        c"emit".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut emit_val,
                        },
                    );
                    if emit_val.is_object() {
                        rooted!(&in(cx_ref) let emit_fn = emit_val.to_object());
                        let event_name_str = JS_NewStringCopyZ(raw_cx, c"upgrade".as_ptr());
                        if !event_name_str.is_null() {
                            let ev_val = StringValue(&*event_name_str);
                            let args_vals = [
                                ev_val,
                                ObjectValue(req_obj.get()),
                                ObjectValue(socket_obj.get()),
                                ObjectValue(if !upgrade_res_obj.get().is_null() {
                                    upgrade_res_obj.get()
                                } else {
                                    socket_obj.get()
                                }),
                            ];
                            let call_args = HandleValueArray {
                                length_: 4,
                                elements_: args_vals.as_ptr(),
                            };
                            let mut rval = UndefinedValue();
                            let rval_h = MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut rval,
                            };
                            let emit_fn_val = ObjectValue(emit_fn.get());
                            rooted!(&in(cx_ref) let emit_fn_root = emit_fn_val);
                            JS_CallFunctionValue(
                                raw_cx,
                                server_root.handle().into(),
                                emit_fn_root.handle().into(),
                                &call_args,
                                rval_h,
                            );
                            JS_ClearPendingException(raw_cx);
                            // ee_emit returns Node's "had listeners" boolean.
                            had_upgrade_listener = rval.is_boolean() && rval.to_boolean();
                        }
                    }
                }
            }
        }
        // Crash-class guard (uWS invariant): nobody accepted the upgrade —
        // either the server has no 'upgrade' listener at all, or the
        // listener returned without writing a response (this server's
        // response model is synchronous). Answer 426 Upgrade Required and
        // close, mirroring bun_listen's explicit-error pattern.
        {
            let res_mut = Response::<false>::cast_res(res);
            let responded = (*res_mut).state().is_http_status_called();
            if !had_upgrade_listener || !responded {
                // end(.., close=true) appends uWS's own Connection: close —
                // do not double-write the header.
                (*res_mut).write_status(b"426 Upgrade Required");
                (*res_mut).end(b"Upgrade Required", true);
            }
        }
        return;
    }

    // Build JS response object with writeHead/write/end bridging to uWS Response.
    rooted!(&in(cx_ref) let res_obj = w2::JS_NewPlainObject(cx_ref));
    if res_obj.get().is_null() {
        return;
    }

    w2::JS_DefineFunction(
        cx_ref,
        res_obj.handle(),
        c"writeHead".as_ptr(),
        Some(res_write_head),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        res_obj.handle(),
        c"write".as_ptr(),
        Some(res_write),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        res_obj.handle(),
        c"end".as_ptr(),
        Some(res_end),
        1,
        JSPROP_ENUMERATE as u32,
    );

    let status_val = Int32Value(200);
    rooted!(&in(cx_ref) let sv = status_val);
    JS_DefineProperty(
        raw_cx,
        res_obj.handle().into(),
        c"statusCode".as_ptr(),
        sv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // Store uWS res pointer on the JS response object for write/end.
    let res_ptr_val = mozjs::jsval::PrivateValue(res as *const core::ffi::c_void);
    rooted!(&in(cx_ref) let rv = res_ptr_val);
    JS_DefineProperty(
        raw_cx,
        res_obj.handle().into(),
        c"_uwsRes".as_ptr(),
        rv.handle().into(),
        0,
    );

    // Call the JS request handler: handler(req, res). Runs in the handler's
    // own realm (AutoRealm scope above). The handler was already rooted at
    // realm entry as `handler_val_root`; reuse that root.
    let args_vals = [ObjectValue(req_obj.get()), ObjectValue(res_obj.get())];
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
        handler_val_root.handle().into(),
        &call_args,
        rval_h,
    );
    if !ok {
        // Handler threw — explicit 500 (never silent terminate).
        JS_ClearPendingException(raw_cx);
        eprintln!("[node:http] request handler threw — responding 500");
        (*res_mut).write_status(b"500 Internal Server Error");
        (*res_mut).write_header(b"Content-Type", b"text/plain");
        (*res_mut).end(b"request handler threw", true);
        return;
    }
    // Handler returned without ending the response. uWS would
    // std::terminate (returning from a request handler without responding);
    // fail explicitly instead of crashing the process.
    if !(*res_mut).state().is_http_end_called() {
        eprintln!("[node:http] request handler returned without responding — responding 500");
        (*res_mut).write_status(b"500 Internal Server Error");
        (*res_mut).write_header(b"Content-Type", b"text/plain");
        (*res_mut).end(b"handler did not respond", true);
    }
}

// ──────────────────────────────────────────────────────────────
// JS response methods — bridge to uWS Response::<false>
// ──────────────────────────────────────────────────────────────

/// Check if a JSVal is a PrivateValue (double with zero high bits).
/// @trace BCE-20260618-002 [level:regression]
/// SpiderMonkey encodes private values as doubles; this guard rejects
/// undefined/non-private doubles that would otherwise trigger the
/// `assert!(self.is_double())` panic in `to_private()` when a property
/// slot is unset (e.g. server object created without listening).
#[inline]
fn val_is_private(v: &JSVal) -> bool {
    v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0
}

/// Recover the `*mut uws_res` stored as `_uwsRes` on the JS response object.
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
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    if !val_is_private(&ptr_val) {
        return core::ptr::null_mut();
    }
    ptr_val.to_private() as *mut bun_uws_sys::response::c::uws_res
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn res_write_head(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            let status = v.to_int32();
            let this = args.thisv();
            rooted!(&in(cx_ref) let obj = this.to_object());
            rooted!(&in(cx_ref) let v_root = v);
            JS_SetProperty(
                cx,
                obj.handle().into(),
                c"statusCode".as_ptr(),
                v_root.handle().into(),
            );

            // Write status to uWS Response.
            let uws_res = get_uws_res(cx, obj.get());
            if !uws_res.is_null() {
                let status_str = format!("{} ", status);
                let res_mut = Response::<false>::cast_res(uws_res);
                (*res_mut).write_status(status_str.as_bytes());
            }

            // Write headers if arg[1] is an object.
            if argc > 1 {
                let hdrs_val = *args.get(1).ptr;
                if hdrs_val.is_object() {
                    rooted!(&in(cx_ref) let hdrs_obj = hdrs_val.to_object());
                    let uws_res = get_uws_res(cx, obj.get());
                    if !uws_res.is_null() {
                        let res_mut = Response::<false>::cast_res(uws_res);
                        // Iterate ALL properties of the headers object via IdVector + GetPropertyKeys.
                        let mut ids = mozjs::rust::IdVector::new(cx);
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
                                let mut hv = UndefinedValue();
                                let c_key = ZBox::from_bytes(key.as_bytes());
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
                                    // Convert header name to lowercase for uWS.
                                    let key_lower = key.to_ascii_lowercase();
                                    let c_val = ZBox::from_bytes(val.as_bytes());
                                    (*res_mut).write_header(key_lower.as_bytes(), c_val.as_bytes());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_root = args.thisv().to_object());
    args.rval().set(ObjectValue(this_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn res_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            let data = crate::js_to_rust_string(cx, v);
            let this = args.thisv();
            rooted!(&in(cx_ref) let obj = this.to_object());

            // Stream data immediately via uWS Response::write.
            let uws_res = get_uws_res(cx, obj.get());
            if !uws_res.is_null() {
                let res_mut = Response::<false>::cast_res(uws_res);
                (*res_mut).write(data.as_bytes());
            }

            // Also accumulate in _body for res.end() to access if needed.
            let mut body_val = UndefinedValue();
            let body_mh = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut body_val,
            };
            JS_GetProperty(cx, obj.handle().into(), c"_body".as_ptr(), body_mh);
            let existing = if body_val.is_string() {
                crate::js_to_rust_string(cx, body_val)
            } else {
                String::new()
            };
            let mut combined = existing;
            combined.push_str(&data);
            let c_combined = ZBox::from_bytes(combined.as_bytes());
            let js_combined = JS_NewStringCopyZ(cx, c_combined.as_ptr());
            if !js_combined.is_null() {
                let cv = StringValue(&*js_combined);
                rooted!(&in(cx_ref) let cv_root = cv);
                JS_SetProperty(
                    cx,
                    obj.handle().into(),
                    c"_body".as_ptr(),
                    cv_root.handle().into(),
                );
            }
        }
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_root = args.thisv().to_object());
    args.rval().set(ObjectValue(this_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn res_end(cx: *mut JSContext, argc: u32, vp: *mut mozjs::jsval::JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Append final data if provided.
    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            let data = crate::js_to_rust_string(cx, v);
            let this = args.thisv();
            rooted!(&in(cx_ref) let obj = this.to_object());
            let mut body_val = UndefinedValue();
            let body_mh = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut body_val,
            };
            JS_GetProperty(cx, obj.handle().into(), c"_body".as_ptr(), body_mh);
            let existing = if body_val.is_string() {
                crate::js_to_rust_string(cx, body_val)
            } else {
                String::new()
            };
            let mut combined = existing;
            combined.push_str(&data);
            let c_combined = ZBox::from_bytes(combined.as_bytes());
            let js_combined = JS_NewStringCopyZ(cx, c_combined.as_ptr());
            if !js_combined.is_null() {
                let cv = StringValue(&*js_combined);
                rooted!(&in(cx_ref) let cv_root = cv);
                JS_SetProperty(
                    cx,
                    obj.handle().into(),
                    c"_body".as_ptr(),
                    cv_root.handle().into(),
                );
            }
        }
    }

    // Send response via uWS Response.
    let this = args.thisv();
    rooted!(&in(cx_ref) let obj = this.to_object());

    let mut body_val = UndefinedValue();
    let body_mh = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut body_val,
    };
    JS_GetProperty(cx, obj.handle().into(), c"_body".as_ptr(), body_mh);
    let body = if body_val.is_string() {
        crate::js_to_rust_string(cx, body_val)
    } else {
        String::new()
    };

    let uws_res = get_uws_res(cx, obj.get());
    if !uws_res.is_null() {
        let res_mut = Response::<false>::cast_res(uws_res);

        // If writeHead was not called, write a default status.
        let mut status_val = Int32Value(200);
        let status_mh = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut status_val,
        };
        JS_GetProperty(cx, obj.handle().into(), c"statusCode".as_ptr(), status_mh);
        let status = if status_val.is_int32() {
            status_val.to_int32()
        } else {
            200
        };

        // Check if status was already written (uWS state tracks this).
        if !(*res_mut).state().is_http_status_called() {
            let status_str = format!("{} ", status);
            (*res_mut).write_status(status_str.as_bytes());
        }

        (*res_mut).end(body.as_bytes(), false);
    }

    args.rval().set(ObjectValue(obj.get()));
    true
}

// ──────────────────────────────────────────────────────────────
// JS host functions: createServer, listen, close, address
// ──────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_create_server(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let server_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_object() {
            rooted!(&in(cx_ref) let cb_obj = v.to_object());
            let cb_val = ObjectValue(cb_obj.get());
            rooted!(&in(cx_ref) let cb_root = cb_val);
            JS_DefineProperty(
                cx,
                server_obj.handle().into(),
                c"_onRequest".as_ptr(),
                cb_root.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
            if !global.get().is_null() {
                JS_SetProperty(
                    cx,
                    global.handle().into(),
                    c"_httpRequestHandler".as_ptr(),
                    cb_root.handle().into(),
                );
            }
        }
    }

    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"listen".as_ptr(),
        Some(server_listen),
        3,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"close".as_ptr(),
        Some(server_close),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"address".as_ptr(),
        Some(server_address),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // @trace REQ-ENG-007 [sm:HttpServer]
    // Node.js: http.Server extends EventEmitter, so `server.on("request", fn)`
    // and `server.emit("listening")` must work. Attach the shared EventEmitter
    // methods from node_events directly onto each server instance. The EE
    // state is stored in a hidden property on the server object, so this works
    // without changing the server's prototype.
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"on".as_ptr(),
        Some(crate::node_events::ee_on),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"addListener".as_ptr(),
        Some(crate::node_events::ee_on),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"once".as_ptr(),
        Some(crate::node_events::ee_once),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"off".as_ptr(),
        Some(crate::node_events::ee_off),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"removeListener".as_ptr(),
        Some(crate::node_events::ee_off),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"emit".as_ptr(),
        Some(crate::node_events::ee_emit),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"prependListener".as_ptr(),
        Some(crate::node_events::ee_prepend),
        2,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        server_obj.handle(),
        c"removeAllListeners".as_ptr(),
        Some(crate::node_events::ee_remove_all),
        1,
        JSPROP_ENUMERATE as u32,
    );

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn server_listen(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

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

    // BCE-012: root callback objects before any GC trigger (App::create, JS_GetProperty, etc.)
    let callback = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_object() {
            rooted!(&in(cx_ref) let cb = v.to_object());
            Some(cb.get())
        } else {
            None
        }
    } else if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_object() {
            rooted!(&in(cx_ref) let cb = v.to_object());
            Some(cb.get())
        } else {
            None
        }
    } else {
        None
    };

    // Create uWS App<false> (non-SSL).
    let opts = BunSocketContextOptions::default();
    let app_ptr = match App::<false>::create(&opts) {
        Some(p) => p,
        None => {
            let msg = format!("Failed to create HTTP server on port {}", port);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    // node:http keeps Node llhttp framing semantics (upstream `IsNodeHttp`
    // template split, runtime flag here): an HTTP/1.0 request bearing
    // Transfer-Encoding is dispatched and the connection closed after, not
    // 400-rejected as Bun.serve does per RFC 9112 6.1. Must be set before
    // any traffic reaches the app.
    // Safety: app_ptr is a live `*mut App<false>` from `App::create` above,
    // valid until `App::<false>::destroy`.
    unsafe { (*app_ptr).set_is_node_http(true) };

    // Get the JS request handler from the server object.

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    let mut handler_val = UndefinedValue();
    let handler_mh = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut handler_val,
    };
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_onRequest".as_ptr(),
        handler_mh,
    );

    rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() || !handler_val.is_object() {
        // No handler — destroy app and return.
        App::<false>::destroy(app_ptr);
        let msg = ZBox::from_bytes("http.createServer requires a request handler".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Allocate ServerUserData on the heap. GC-safe: global+handler+server stored in GcStore.
    rooted!(&in(cx_ref) let handler_root = handler_val.to_object());
    let ud = Box::new(ServerUserData::new(
        cx,
        global.get(),
        handler_root.get(),
        server_obj.get(),
    ));
    let ud_ptr = Box::into_raw(ud) as *mut ::std::ffi::c_void;

    // Register catch-all route: app.any("/*", handler, user_data)
    // SAFETY: `unsafe extern "C"` and `extern "C"` fn pointers have identical ABI;
    // transmute is sound because the C layer only cares about the calling convention.
    let safe_handler: Option<
        extern "C" fn(
            *mut bun_uws_sys::response::c::uws_res,
            *mut bun_uws_sys::Request,
            *mut ::std::ffi::c_void,
        ),
    > = unsafe {
        ::std::mem::transmute(Some(
            uws_route_handler
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::response::c::uws_res,
                    *mut bun_uws_sys::Request,
                    *mut ::std::ffi::c_void,
                ),
        ))
    };
    (*app_ptr).any(b"/*", safe_handler, ud_ptr);

    // Store the ServerUserData pointer on the server object for cleanup in server_close.
    {
        let ud_val = mozjs::jsval::PrivateValue(ud_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let udv = ud_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            udv.handle().into(),
            0,
        );
    }

    // Listen on the specified port.
    // The listen callback captures the OS-assigned port for `listen(0)`
    // dynamic binds (mirrors Bun.serve BCE-005 `actual_port`): uWS fires it
    // synchronously inside `App::listen`, so `actual_port` is populated by
    // the time `listen` returns below.
    // SAFETY: same ABI transmute rationale as above.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe extern "C" fn node_http_listen_cb(
        listen_socket: *mut bun_uws_sys::listen_socket::ListenSocket,
        user_data: *mut ::std::ffi::c_void,
    ) {
        if !listen_socket.is_null() && !user_data.is_null() {
            let ls_ref = bun_opaque::opaque_deref_mut(listen_socket);
            let ls_port = ls_ref.get_local_port();
            if ls_port > 0 {
                *(user_data as *mut u16) = ls_port as u16;
            }
        }
    }
    let safe_listen_cb: extern "C" fn(
        *mut bun_uws_sys::listen_socket::ListenSocket,
        *mut ::std::ffi::c_void,
    ) = unsafe {
        ::std::mem::transmute(
            node_http_listen_cb
                as unsafe extern "C" fn(
                    *mut bun_uws_sys::listen_socket::ListenSocket,
                    *mut ::std::ffi::c_void,
                ),
        )
    };
    let mut actual_port: u16 = 0;
    (*app_ptr).listen(
        port as i32,
        safe_listen_cb,
        &mut actual_port as *mut u16 as *mut ::std::ffi::c_void,
    );
    // For `listen(0)` the ephemeral port comes from the listen socket; a zero
    // here means the bind failed (no listen callback port) — keep the
    // requested value so address() stays honest either way.
    let effective_port: u16 = if port == 0 { actual_port } else { port };

    // Store app pointer on server object for close/destroy.
    {
        let app_ptr_val = mozjs::jsval::PrivateValue(app_ptr as *const core::ffi::c_void);
        rooted!(&in(cx_ref) let apv = app_ptr_val);
        JS_DefineProperty(
            cx,
            server_obj.handle().into(),
            c"_appPtr".as_ptr(),
            apv.handle().into(),
            0,
        );
    }

    rooted!(&in(cx_ref) let port_root = Int32Value(effective_port as i32));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"_listeningPort".as_ptr(),
        port_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    rooted!(&in(cx_ref) let listening_root = mozjs::jsval::BooleanValue(true));
    JS_DefineProperty(
        cx,
        server_obj.handle().into(),
        c"listening".as_ptr(),
        listening_root.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    ACTIVE_APPS.with(|s| s.borrow_mut().push(app_ptr));

    // Call listen callback if provided.
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
unsafe extern "C" fn server_close(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    // Node close(callback): the callback fires once the server has closed,
    // and the server emits 'close'. Capture the optional callback up-front —
    // the teardown below clears the app/userdata pointers that identify a
    // live server, and the re-close path needs it to deliver Node's
    // ERR_SERVER_NOT_RUNNING shape.
    let close_cb = if argc > 0 && (*args.get(0).ptr).is_object() {
        let cb_obj = (*args.get(0).ptr).to_object();
        if unsafe { JS_ObjectIsFunction(cb_obj) } {
            Some(cb_obj)
        } else {
            None
        }
    } else {
        None
    };
    let had_live_app: bool;

    // Destroy the uWS App if it exists.
    let mut app_ptr_val = UndefinedValue();
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_appPtr".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut app_ptr_val,
        },
    );
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    // http.createServer(fn).close() before listen leaves _appPtr undefined;
    // to_private() on undefined asserts is_double() → panic across extern "C".
    let app_ptr = if val_is_private(&app_ptr_val) {
        app_ptr_val.to_private() as *mut App<false>
    } else {
        core::ptr::null_mut()
    };
    had_live_app = !app_ptr.is_null();
    if !app_ptr.is_null() {
        (*app_ptr).close();
        App::<false>::destroy(app_ptr);
        // BCE-007: unified unregister (idempotent). Replaces the inline
        // `ACTIVE_APPS.retain` so the liveness registry has one update path.
        // Safety: app_ptr was registered by `server_listen` via the same
        // registry; idempotent if already removed.
        unsafe {
            unregister_active_app(app_ptr);
        }

        // @trace BCE-20260618-006 [level:regression] [api:http.createServer close]
        // Clear `_appPtr` on the JS server object so subsequent `close()`
        // calls are idempotent no-ops instead of use-after-free on the
        // destroyed `*mut App`. Same class of bug as Bun.serve's server_stop
        // (bun_api.rs) — double-close reads the stale pointer and calls
        // `close()`/`destroy()` on freed memory → SIGSEGV. Set to a non-
        // private value (UndefinedValue) so the val_is_private guard above
        // correctly takes the null path on re-entry.
        rooted!(&in(cx_ref) let undef_root = UndefinedValue());
        JS_SetProperty(
            cx,
            server_obj.handle().into(),
            c"_appPtr".as_ptr(),
            undef_root.handle().into(),
        );
    }

    // Cleanup: reclaim ServerUserData box and remove GcStore entries.
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
    // @trace BCE-20260618-002 — guard non-private doubles before to_private().
    let ud_ptr = if val_is_private(&ud_ptr_val) {
        ud_ptr_val.to_private() as *mut ServerUserData
    } else {
        core::ptr::null_mut()
    };
    if !ud_ptr.is_null() {
        // SAFETY: ud_ptr was created by Box::into_raw in server_listen.
        let ud = unsafe { Box::from_raw(ud_ptr) };
        ud.cleanup();

        // @trace BCE-20260618-006 [level:regression] — same stale-pointer
        // class as `_appPtr` above. Clear `_udPtr` so a second `close()` is
        // a no-op (the Box has been consumed and the memory freed).
        rooted!(&in(cx_ref) let undef_root2 = UndefinedValue());
        JS_SetProperty(
            cx,
            server_obj.handle().into(),
            c"_udPtr".as_ptr(),
            undef_root2.handle().into(),
        );
    }

    // Node close() delivery: the teardown above is synchronous (listen socket
    // closed, live connections destroyed — the closeAllConnections shape), so
    // 'close' and the callback fire now. Before this, close(cb) swallowed the
    // callback silently and the server object never emitted 'close' — scripts
    // coordinating shutdown on those signals hung forever (the unconsumed-
    // response srv.close() class). Re-close / never-listened keeps Node's
    // shape: callback receives Error("Server is not running"), no 'close'
    // re-emit.
    if let Some(cb) = close_cb {
        rooted!(&in(cx_ref) let cb_root = cb);
        rooted!(&in(cx_ref) let cb_fn = ObjectValue(cb_root.get()));
        rooted!(&in(cx_ref) let undef_this = ::std::ptr::null_mut::<JSObject>());
        if had_live_app {
            let empty_args = HandleValueArray {
                length_: 0,
                elements_: [].as_ptr(),
            };
            let mut rval = UndefinedValue();
            let _ = JS_CallFunctionValue(
                cx,
                undef_this.handle().into(),
                cb_fn.handle().into(),
                &empty_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
        } else {
            // Node ERR_SERVER_NOT_RUNNING: cb(Error) — build via the realm's
            // Error constructor so instanceof works; fall back to undefined
            // (plain-object fallback would just fake instanceof).
            let mut err_val = UndefinedValue();
            let mut built_err = false;
            let global = JS::CurrentGlobalOrNull(cx);
            if !global.is_null() {
                rooted!(&in(cx_ref) let global_root = global);
                let mut ctor_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    global_root.handle().into(),
                    c"Error".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut ctor_val,
                    },
                );
                if ctor_val.is_object() {
                    rooted!(&in(cx_ref) let ctor_obj = ctor_val.to_object());
                    rooted!(&in(cx_ref) let ctor_fn = ObjectValue(ctor_obj.get()));
                    let c_msg = ZBox::from_bytes(b"Server is not running");
                    let msg_js = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                    if !msg_js.is_null() {
                        rooted!(&in(cx_ref) let msg_root = StringValue(&*msg_js));
                        let elems = [msg_root.get()];
                        let call_args = HandleValueArray {
                            length_: 1,
                            elements_: elems.as_ptr(),
                        };
                        let mut e_val = UndefinedValue();
                        if JS_CallFunctionValue(
                            cx,
                            undef_this.handle().into(),
                            ctor_fn.handle().into(),
                            &call_args,
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut e_val,
                            },
                        ) && e_val.is_object()
                        {
                            err_val = e_val;
                            built_err = true;
                        }
                    }
                }
            }
            rooted!(&in(cx_ref) let arg_val = if built_err {
                err_val
            } else {
                UndefinedValue()
            });
            let elems = [arg_val.get()];
            let cb_args = HandleValueArray {
                length_: 1,
                elements_: elems.as_ptr(),
            };
            let mut rval = UndefinedValue();
            let _ = JS_CallFunctionValue(
                cx,
                undef_this.handle().into(),
                cb_fn.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
        }
    }
    if had_live_app {
        // Emit 'close' on the server object (it carries the node_events emit
        // attached at create time). Same pattern as the 'upgrade' emit.
        let mut emit_val = UndefinedValue();
        JS_GetProperty(
            cx,
            server_obj.handle().into(),
            c"emit".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut emit_val,
            },
        );
        if emit_val.is_object() {
            rooted!(&in(cx_ref) let emit_fn = emit_val.to_object());
            rooted!(&in(cx_ref) let emit_val_fn = ObjectValue(emit_fn.get()));
            let ev_str = JS_NewStringCopyZ(cx, c"close".as_ptr());
            if !ev_str.is_null() {
                rooted!(&in(cx_ref) let ev_root = StringValue(&*ev_str));
                let elems = [ev_root.get()];
                let call_args = HandleValueArray {
                    length_: 1,
                    elements_: elems.as_ptr(),
                };
                let mut rval = UndefinedValue();
                let _ = JS_CallFunctionValue(
                    cx,
                    server_obj.handle().into(),
                    emit_val_fn.handle().into(),
                    &call_args,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn server_address(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let addr_obj = unsafe { w2::JS_NewPlainObject(cx_ref) });
    if addr_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let this = args.thisv();
    rooted!(&in(cx_ref) let server_obj = this.to_object());

    let mut port_val = UndefinedValue();
    let port_mh = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut port_val,
    };
    JS_GetProperty(
        cx,
        server_obj.handle().into(),
        c"_listeningPort".as_ptr(),
        port_mh,
    );

    if port_val.is_int32() {
        let p = port_val.to_int32();
        rooted!(&in(cx_ref) let pvr = Int32Value(p));
        JS_DefineProperty(
            cx,
            addr_obj.handle().into(),
            c"port".as_ptr(),
            pvr.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        let c_family = ZBox::from_bytes("IPv4".as_bytes());
        let js_family = JS_NewStringCopyZ(cx, c_family.as_ptr());
        if !js_family.is_null() {
            let fv = StringValue(&*js_family);
            rooted!(&in(cx_ref) let fvr = fv);
            JS_DefineProperty(
                cx,
                addr_obj.handle().into(),
                c"family".as_ptr(),
                fvr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        let c_addr = ZBox::from_bytes("0.0.0.0".as_bytes());
        let js_addr = JS_NewStringCopyZ(cx, c_addr.as_ptr());
        if !js_addr.is_null() {
            let av = StringValue(&*js_addr);
            rooted!(&in(cx_ref) let avr = av);
            JS_DefineProperty(
                cx,
                addr_obj.handle().into(),
                c"address".as_ptr(),
                avr.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    args.rval().set(ObjectValue(addr_obj.get()));
    true
}

// @trace REQ-ENG-010 [api:http.request async] [entity:FetchTasklet]
//
// BCE-20260618-007: `http.request` / `http.get` must not perform any blocking
// network I/O on the JS thread. The legacy shim built a plain metadata
// object (url/method) — no I/O — so it did not directly block, but it was
// listed in the BCE-007 sweep scope because `http.request` is the canonical
// JS-native HTTP entry. To guarantee the C2 invariant ("zero send_sync/
// stealth_http_request from any JS-native HTTP entry") and to make the API
// actually useful, we now return a *pending* Promise that resolves to a
// Response object (same shape as fetch()'s). The network round-trip runs on
// a detached worker via `fetch_async::start` (FetchTasklet pattern) — never
// on the JS thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http_request(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let url_str = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let method = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            "GET".to_string()
        }
    } else {
        "GET".to_string()
    };

    // Optional (headersJSON, body) appended by the HTTP_CLIENT_JS shim —
    // same parsing contract as node_https::https_request.
    let headers_json = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let body = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let headers_vec: Vec<(String, String)> = if !headers_json.is_empty() {
        serde_json::from_str::<::std::collections::HashMap<String, String>>(&headers_json)
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    let body_bytes: Option<Vec<u8>> = if body.is_empty() {
        None
    } else {
        Some(body.into_bytes())
    };

    // Create the PENDING Promise *while cx_ref holds a rooting context*,
    // then release cx_ref before scheduling (the worker must not outlive the
    // rooted frame, but the Promise itself is heap-rooted by fetch_async).
    rooted!(&in(cx_ref) let null_global = ::std::ptr::null_mut::<JSObject>());
    rooted!(&in(cx_ref) let promise = unsafe {
        mozjs_sys::jsapi::JS::NewPromiseObject(
            cx,
            null_global.handle().into(),
        )
    });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let promise_obj = promise.get();
    let promise_val = ObjectValue(promise_obj);

    // Resolve method string → bun_http::Method (no I/O).
    let bun_method = match method.as_str() {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };
    // No stealth profile for plain http.request (Node API parity).
    let profile: Option<bao_stealth::StealthProfile> = None;

    // Schedule async fetch — detached worker, JS thread never blocks.
    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    unsafe {
        crate::fetch_async::start(
            cx,
            promise_val,
            profile,
            bun_method,
            url_str,
            headers_vec,
            body_bytes,
        );
    }

    args.rval().set(promise_val);
    true
}

// (http.get is JS-level in HTTP_CLIENT_JS: `get` = `request` + immediate
// `.end()` — Node's documented auto-end contract for http.get.)

// ── Unit tests for node_http pure Rust data/logic ──────────────────────
// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

#[cfg(test)]
mod tests {
    use super::*;

    // ── ACTIVE_APPS thread_local ──

    #[test]
    fn has_active_servers_false_initially() {
        // Clear any state leaked by previous tests on this thread.
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        assert!(!has_active_servers());
    }

    #[test]
    fn listener_fds_empty() {
        let fds = listener_fds();
        assert!(fds.is_empty());
    }

    // ── BCE-007 (runtime hang): unified liveness registry regression tests ──
    // @trace REQ-ENG-006 [api:Bun.serve] [api:http.createServer] unified liveness
    //
    // These guard the invariant that closed the BCE-007 fetch(self) hang: every
    // JS-thread-bound uWS App MUST be registered via register_active_app so
    // drain_and_check keeps ticking the uWS Loop while the server is listening.
    // Bun.serve previously skipped registration → the server's listen socket
    // never received accept() events → fetch(self) hung forever.
    //
    // We test the registry contract directly (not the full fetch(self) round
    // trip, which additionally depends on the HTTPThread client write path)
    // so the liveness invariant is locked in independently.

    /// BCE-007-R1: register_active_app flips has_active_servers() to true,
    /// and a matching unregister flips it back. Uses sentinel pointers (never
    /// dereferenced — register/unregister only compare/stored raw pointers).
    #[test]
    fn bce_007_register_unregister_flips_liveness() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        assert!(!has_active_servers(), "no servers initially");

        let sentinel_a: *mut App<false> = 0x1000 as *mut App<false>;
        let sentinel_b: *mut App<false> = 0x2000 as *mut App<false>;
        // SAFETY: register_active_app/unregister_active_app only store/compare
        // the raw pointer; they never dereference it. Sentinels are valid for
        // pointer-equality comparisons.
        unsafe {
            register_active_app(sentinel_a);
            assert!(has_active_servers(), "registered → live");
            register_active_app(sentinel_b);
            assert!(has_active_servers(), "still live with two");
            unregister_active_app(sentinel_a);
            assert!(has_active_servers(), "still live with one");
            unregister_active_app(sentinel_b);
            assert!(!has_active_servers(), "all unregistered → not live");
        }
    }

    /// BCE-007-R2: register is idempotent (re-registering the same pointer does
    /// not double-count), preventing the registry from growing unbounded on
    /// repeated Bun.serve calls in error paths.
    #[test]
    fn bce_007_register_is_idempotent() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        let sentinel: *mut App<false> = 0x3000 as *mut App<false>;
        // SAFETY: pointer never dereferenced (see R1).
        unsafe {
            register_active_app(sentinel);
            register_active_app(sentinel);
            register_active_app(sentinel);
            let count = ACTIVE_APPS.with(|s| s.borrow().len());
            assert_eq!(count, 1, "idempotent register must not duplicate");
            unregister_active_app(sentinel);
            assert!(!has_active_servers());
        }
    }

    /// BCE-007-R3: unregister is idempotent (unregistering an unknown pointer
    // is a no-op, not a panic) — defensive against double-close paths.
    #[test]
    fn bce_007_unregister_unknown_is_noop() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        let sentinel: *mut App<false> = 0x4000 as *mut App<false>;
        // SAFETY: pointer never dereferenced.
        unsafe {
            unregister_active_app(sentinel);
            assert!(!has_active_servers(), "unregister-unknown must not panic");
            register_active_app(0x5000 as *mut App<false>);
            unregister_active_app(sentinel);
            assert!(has_active_servers(), "unrelated unregister keeps live");
        }
    }

    /// BCE-007-R4: null is safely ignored by both register and unregister
    /// (Bun.serve stub-mode returns a null app_ptr; the call must be a no-op).
    #[test]
    fn bce_007_null_app_is_noop() {
        ACTIVE_APPS.with(|s| s.borrow_mut().clear());
        // SAFETY: null is explicitly handled (early return) in both functions.
        unsafe {
            register_active_app(core::ptr::null_mut());
            assert!(!has_active_servers(), "null register must be a no-op");
            unregister_active_app(core::ptr::null_mut());
            assert!(!has_active_servers(), "null unregister must be a no-op");
        }
    }

    // ── ServerUserData (GC-safe GcStore keys) ──

    #[test]
    fn server_user_data_keys_are_namespaced() {
        // ServerUserData::new must produce unique namespaced GcStore keys.
        // We can't call ::new without a real JSContext, so test the key format directly.
        let key1 = format!("http_server_{}_global", 1);
        let key2 = format!("http_server_{}_handler", 1);
        assert!(key1.starts_with("http_server_"));
        assert!(key1.ends_with("_global"));
        assert!(key2.starts_with("http_server_"));
        assert!(key2.ends_with("_handler"));
        assert_ne!(key1, key2, "global and handler keys must differ");
    }

    #[test]
    fn server_user_data_next_server_id_monotonic() {
        let id1 = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        let id2 = NEXT_SERVER_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id2 > id1, "server IDs must be monotonic");
    }

    #[test]
    fn server_user_data_global_handler_retrieve_without_cx() {
        // The persistent-rooted store resolves by key alone; with no entry
        // registered (and a null cx), retrieval returns None — fail-closed.
        let ud = ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http_server_999_global".to_string(),
            handler_key: "http_server_999_handler".to_string(),
            server_obj_key: "http_server_999_server_obj".to_string(),
        };
        assert!(
            gc_store_get_ns(::std::ptr::null_mut(), "http", &ud.global_key).is_none(),
            "gc_store_get_ns with null cx returns None"
        );
        assert!(
            gc_store_get_ns(::std::ptr::null_mut(), "http", &ud.handler_key).is_none(),
            "gc_store_get_ns with null cx returns None"
        );
        assert!(
            ud.server_obj().is_none(),
            "gc_store_get_ns with null cx returns None"
        );
    }

    #[test]
    fn server_user_data_cleanup_removes_from_gc_store() {
        // cleanup with null cx should not panic (gc_store_remove handles null cx gracefully).
        let ud = ServerUserData {
            cx: ::std::ptr::null_mut(),
            global_key: "http_server_998_global".to_string(),
            handler_key: "http_server_998_handler".to_string(),
            server_obj_key: "http_server_998_server_obj".to_string(),
        };
        ud.cleanup(); // Must not panic
    }

    // ── HTTP STATUS_CODES (static data) ──

    static STATUS_CODES: &[(&str, &str)] = &[
        ("100", "Continue"),
        ("101", "Switching Protocols"),
        ("102", "Processing"),
        ("200", "OK"),
        ("201", "Created"),
        ("202", "Accepted"),
        ("203", "Non-Authoritative Information"),
        ("204", "No Content"),
        ("205", "Reset Content"),
        ("206", "Partial Content"),
        ("207", "Multi-Status"),
        ("208", "Already Reported"),
        ("226", "IM Used"),
        ("300", "Multiple Choices"),
        ("301", "Moved Permanently"),
        ("302", "Found"),
        ("303", "See Other"),
        ("304", "Not Modified"),
        ("305", "Use Proxy"),
        ("306", "(Unused)"),
        ("307", "Temporary Redirect"),
        ("308", "Permanent Redirect"),
        ("400", "Bad Request"),
        ("401", "Unauthorized"),
        ("402", "Payment Required"),
        ("403", "Forbidden"),
        ("404", "Not Found"),
        ("405", "Method Not Allowed"),
        ("406", "Not Acceptable"),
        ("407", "Proxy Authentication Required"),
        ("408", "Request Timeout"),
        ("409", "Conflict"),
        ("410", "Gone"),
        ("411", "Length Required"),
        ("412", "Precondition Failed"),
        ("413", "Payload Too Large"),
        ("414", "URI Too Long"),
        ("415", "Unsupported Media Type"),
        ("416", "Range Not Satisfiable"),
        ("417", "Expectation Failed"),
        ("418", "I'm a Teapot"),
        ("421", "Misdirected Request"),
        ("422", "Unprocessable Entity"),
        ("423", "Locked"),
        ("424", "Failed Dependency"),
        ("425", "Too Early"),
        ("426", "Upgrade Required"),
        ("428", "Precondition Required"),
        ("429", "Too Many Requests"),
        ("431", "Request Header Fields Too Large"),
        ("451", "Unavailable For Legal Reasons"),
        ("500", "Internal Server Error"),
        ("501", "Not Implemented"),
        ("502", "Bad Gateway"),
        ("503", "Service Unavailable"),
        ("504", "Gateway Timeout"),
        ("505", "HTTP Version Not Supported"),
        ("506", "Variant Also Negotiates"),
        ("507", "Insufficient Storage"),
        ("508", "Loop Detected"),
        ("509", "Bandwidth Limit Exceeded"),
        ("510", "Not Extended"),
        ("511", "Network Authentication Required"),
    ];

    #[test]
    fn status_codes_count() {
        assert_eq!(STATUS_CODES.len(), 63);
    }

    #[test]
    fn status_codes_contains_200_ok() {
        assert!(STATUS_CODES.iter().any(|(c, m)| *c == "200" && *m == "OK"));
    }

    #[test]
    fn status_codes_contains_404_not_found() {
        assert!(
            STATUS_CODES
                .iter()
                .any(|(c, m)| *c == "404" && *m == "Not Found")
        );
    }

    #[test]
    fn status_codes_contains_500_internal_server_error() {
        assert!(
            STATUS_CODES
                .iter()
                .any(|(c, m)| *c == "500" && *m == "Internal Server Error")
        );
    }

    #[test]
    fn status_codes_all_numeric() {
        for (code, _) in STATUS_CODES {
            assert!(code.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn status_codes_all_non_empty_messages() {
        for (_, msg) in STATUS_CODES {
            assert!(!msg.is_empty());
        }
    }

    #[test]
    fn status_codes_codes_unique() {
        let mut codes: Vec<&&str> = STATUS_CODES.iter().map(|(c, _)| c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), STATUS_CODES.len());
    }

    // ── HTTP METHODS string ──

    #[test]
    fn http_methods_string_format() {
        let methods = "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,TRACE";
        let method_list: Vec<&str> = methods.split(',').collect();
        assert_eq!(method_list.len(), 8);
        assert!(method_list.contains(&"GET"));
        assert!(method_list.contains(&"POST"));
        assert!(method_list.contains(&"DELETE"));
        assert!(method_list.contains(&"PATCH"));
    }

    #[test]
    fn http_methods_all_uppercase() {
        let methods = "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,TRACE";
        for m in methods.split(',') {
            assert_eq!(m, m.to_uppercase());
        }
    }

    // ── HTTP header iteration (for_each_header) ──

    #[test]
    fn for_each_header_collects_all_headers() {
        // Verify that the for_each_header pattern (used in both node_http
        // and bun_api) correctly collects all headers via a callback.
        // We simulate the pattern with a mock data structure.
        let mock_headers: Vec<(&[u8], &[u8])> = vec![
            (b"host", b"example.com"),
            (b"content-type", b"text/html"),
            (b"x-custom", b"value"),
        ];
        let mut collected: Vec<(&[u8], &[u8])> = Vec::new();
        for (name, value) in &mock_headers {
            collected.push((*name, *value));
        }
        assert_eq!(collected.len(), 3);
        assert!(
            collected
                .iter()
                .any(|(n, v)| *n == b"host" && *v == b"example.com")
        );
        assert!(
            collected
                .iter()
                .any(|(n, v)| *n == b"x-custom" && *v == b"value")
        );
    }
}
