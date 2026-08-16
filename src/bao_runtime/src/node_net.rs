// @trace REQ-ENG-007
//! node:net implementation using bun_uws uSockets TCP socket API.
//!
//! Replaces the previous std::net::TcpListener/TcpStream synchronous
//! implementation with event-loop-integrated uSockets sockets managed
//! by bao_uloop's epoll backend.

use ::std::cell::{Cell, RefCell};
use ::std::collections::HashMap;
use ::std::ptr::{self, NonNull};
use bun_core::ZBox;

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue,
    UndefinedValue,
};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::app::App;
use bun_uws_sys::socket_group::VTable;
use bun_uws_sys::{CloseCode, ListenSocket, Loop, SocketGroup, SocketKind, us_socket_t};

use crate::gc_store::{gc_store_get, gc_store_insert, gc_store_remove, gc_store_unique_key};
use crate::require::cache_builtin;

// Direct FFI declaration for inet_ntop (not exported by libc crate on all platforms).
unsafe extern "C" {
    fn inet_ntop(
        af: ::std::ffi::c_int,
        src: *const ::std::ffi::c_void,
        dst: *mut ::std::ffi::c_char,
        size: libc::socklen_t,
    ) -> *const ::std::ffi::c_char;
}

// ──────────────────── per-socket extension data ────────────────────

/// Extension data stored in each socket's `us_socket_ext` slot.
/// Tracks pending write buffer for backpressure handling.
#[repr(C)]
#[allow(dead_code)]
struct NetSocketExt {
    /// Non-zero if this socket is a client (connect) vs server-accepted.
    is_client: u8,
    /// Pending write data when socket write returns partial.
    pending_write: NetPendingWrite,
}

#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct NetPendingWrite {
    ptr: *mut u8,
    len: usize,
    cap: usize,
}

#[allow(dead_code)]
impl NetPendingWrite {
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn set_data(&mut self, data: &[u8]) {
        if data.is_empty() {
            self.clear();
            return;
        }
        let mut v = if self.cap > 0 && !self.ptr.is_null() {
            unsafe { Vec::from_raw_parts(self.ptr, self.len, self.cap) }
        } else {
            Vec::new()
        };
        v.clear();
        v.extend_from_slice(data);
        let mut md = ::std::mem::ManuallyDrop::new(v);
        self.ptr = md.as_mut_ptr();
        self.len = md.len();
        self.cap = md.capacity();
    }

    fn clear(&mut self) {
        if self.cap > 0 && !self.ptr.is_null() {
            unsafe {
                drop(Vec::from_raw_parts(self.ptr, 0, self.cap));
            }
        }
        self.ptr = ptr::null_mut();
        self.len = 0;
        self.cap = 0;
    }
}

impl Drop for NetPendingWrite {
    fn drop(&mut self) {
        self.clear();
    }
}

// ──────────────────── thread-local state ────────────────────

thread_local! {
    /// Server socket groups: listen_ptr (as usize) → SocketGroup.
    static NET_SERVER_GROUPS: RefCell<HashMap<usize, Box<SocketGroup>>> = RefCell::new(HashMap::new());

    /// Listen socket pointers: listen_ptr (as usize).
    static NET_LISTEN_SOCKETS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };

    /// Connected socket pointers: socket_ptr (as usize) → true.
    static NET_SOCKETS: RefCell<HashMap<usize, bool>> = RefCell::new(HashMap::new());

    /// Result of a pending connect, set by on_open/on_connect_error callbacks.
    static CONNECT_RESULT: Cell<Option<usize>> = const { Cell::new(None) };

    /// Whether a connect error occurred.
    static CONNECT_ERROR: Cell<bool> = const { Cell::new(false) };

    /// Per-socket incoming data buffers: socket_ptr (as usize) → Vec<u8>.
    static NET_INCOMING_DATA: RefCell<HashMap<usize, Vec<u8>>> = RefCell::new(HashMap::new());

    /// Per-socket listen port: socket_ptr (as usize) → port.
    /// Used to return the correct port in Server.address() when getsockname fails.
    static NET_LISTEN_PORTS: RefCell<HashMap<usize, u16>> = RefCell::new(HashMap::new());

    /// 'connection' dispatcher per listen socket: listen_ptr (as usize) →
    /// GcStore key of the JS callback registered by `__net_on_connection`.
    /// Consumed by `dispatch_accept` when the vtable on_open fires for an
    /// accepted (is_client == 0) socket.
    static NET_CONNECTION_CBS: RefCell<HashMap<usize, String>> = RefCell::new(HashMap::new());

    /// Reverse index: accept-group ptr (as usize) → listen_ptr. Accepted
    /// sockets reach the vtable with only their group pointer; this maps them
    /// back to the owning Server (whose 'connection' dispatcher fires).
    static NET_GROUP_LISTEN: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());

    /// Sockets that saw peer FIN (vtable on_end) but are not yet fully
    /// closed. Distinguishes `__net_poll_state` 2 (half-open, 'end' fired)
    /// from 1 (fully open).
    static NET_EOF_SOCKETS: RefCell<HashMap<usize, bool>> = RefCell::new(HashMap::new());

    /// JSContext pointer stored for use in C callbacks.
    static NET_CX: Cell<Option<*mut JSContext>> = const { Cell::new(None) };
}

pub struct NetCleanup;

impl Drop for NetCleanup {
    fn drop(&mut self) {
        // Drop liveness tokens FIRST so node_http::has_active_servers()
        // reflects the cleared state (a stale token would keep drain_and_check
        // ticking a loop whose groups are about to be freed). unregister is a
        // retain-based no-op for pointers never registered.
        NET_LISTEN_SOCKETS.with(|l| {
            for key in l.borrow().iter() {
                unsafe { crate::node_http::unregister_active_app(*key as *mut App<false>) };
            }
        });
        NET_SERVER_GROUPS.with(|g| g.borrow_mut().clear());
        NET_LISTEN_SOCKETS.with(|l| l.borrow_mut().clear());
        NET_SOCKETS.with(|s| s.borrow_mut().clear());
        NET_INCOMING_DATA.with(|d| d.borrow_mut().clear());
        NET_LISTEN_PORTS.with(|p| p.borrow_mut().clear());
        NET_CONNECTION_CBS.with(|c| c.borrow_mut().clear());
        NET_GROUP_LISTEN.with(|m| m.borrow_mut().clear());
        NET_EOF_SOCKETS.with(|e| e.borrow_mut().clear());
        NET_CX.with(|c| c.set(None));
    }
}

// ──────────────────── VTable callbacks ────────────────────

/// Socket opened (accept or connect completion). `is_client` distinguishes the
/// two (loop.c dispatches `us_dispatch_open(s, 0, ...)` for accepts).
unsafe extern "C" fn net_on_open(
    s: *mut us_socket_t,
    is_client: ::std::ffi::c_int,
    _ip: *mut u8,
    _ip_length: ::std::ffi::c_int,
) -> *mut us_socket_t {
    let key = s as usize;
    NET_SOCKETS.with(|m| m.borrow_mut().insert(key, true));
    if is_client != 0 {
        // Connect completion — feed net_connect's spin loop. Accepts MUST NOT
        // write CONNECT_RESULT: they fire on the same thread loop *during*
        // another socket's connect spin (the echo shape: the server's SYN-ACK
        // accept and the client's connect completion land in the same
        // epoll batch), and the unguarded write made net_connect return the
        // server-side accepted socket as the client socket.
        CONNECT_RESULT.with(|r| {
            if r.get().is_none() {
                r.set(Some(key));
            }
        });
    } else {
        // Server-side accept — bridge into JS ('connection' at accept time).
        unsafe { dispatch_accept(s) };
    }
    s
}

/// Bridge a usockets accept (vtable on_open, is_client == 0) into JS: build a
/// net.Socket via the NET_JS IIFE's global factory (`__net_make_socket`, so
/// the prototype chain and the __net_read poll machinery stay owned by the
/// IIFE) and dispatch the owning Server's 'connection' handler with it.
///
/// Runs on the JS thread inside a loop tick — possibly re-entrantly while a
/// `__net_connect` spin is on the native stack (same-thread re-entrancy is
/// the same shape bun_listen's vtable callbacks already use). The handler is
/// resolved from GcStore inside the context's persistent realm, mirroring
/// `invoke_js_callback` in bun_listen.rs.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dispatch_accept(s: *mut us_socket_t) {
    let Some(cx) = NET_CX.with(|c| c.get()) else {
        return;
    };
    if cx.is_null() {
        return;
    }

    // Which Server owns this accept? Accepted sockets carry only their group.
    let group_ptr = (*s).group() as *mut SocketGroup as usize;
    let listen_key = NET_GROUP_LISTEN.with(|m| m.borrow().get(&group_ptr).copied());
    let Some(listen_key) = listen_key else { return };
    let cb_key = NET_CONNECTION_CBS.with(|m| m.borrow().get(&listen_key).cloned());
    let Some(cb_key) = cb_key else { return };

    // Enter the context's persistent realm — GcStore resolves the callback as
    // a property on this realm's global.
    let Some(global) = bao_engine::context::thread_realm_global() else {
        return;
    };
    if global.is_null() {
        return;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let global_root = global);
    let mut realm = AutoRealm::new_from_handle(cx_ref, global_root.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let Some(handler) = gc_store_get(cx, &cb_key) else {
        return;
    };
    if handler.is_null() {
        return;
    }

    // Build the JS socket: __net_make_socket(ptr) → new Socket with _ptr set.
    rooted!(&in(realm_cx) let ptr_arg = DoubleValue(s as usize as f64));
    let factory_args = HandleValueArray {
        length_: 1,
        elements_: &*ptr_arg.handle(),
    };
    let mut sock_val = UndefinedValue();
    let sock_ok = JS_CallFunctionName(
        realm_cx.raw_cx(),
        global_root.handle().into(),
        c"__net_make_socket".as_ptr(),
        &factory_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut sock_val,
        },
    );
    if !sock_ok || !sock_val.is_object() {
        JS_ClearPendingException(realm_cx.raw_cx());
        return;
    }
    rooted!(&in(realm_cx) let sock_obj = sock_val.to_object());
    let sock_h = sock_obj.handle().into();

    // Remote address/port — the accepted socket knows its peer.
    let mut ip_buf = [0u8; 64];
    if let Ok(ip) = (*s).remote_address(&mut ip_buf) {
        let c_ip = ZBox::from_bytes(ip);
        let ip_js = JS_NewStringCopyZ(realm_cx.raw_cx(), c_ip.as_ptr());
        if !ip_js.is_null() {
            rooted!(&in(realm_cx) let ip_v = StringValue(&*ip_js));
            JS_DefineProperty(
                realm_cx.raw_cx(),
                sock_h,
                c"remoteAddress".as_ptr(),
                ip_v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(realm_cx) let rp_v = Int32Value((*s).remote_port()));
    JS_DefineProperty(
        realm_cx.raw_cx(),
        sock_h,
        c"remotePort".as_ptr(),
        rp_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // connection handler(socket)
    rooted!(&in(realm_cx) let handler_val = ObjectValue(handler));
    rooted!(&in(realm_cx) let sock_elem = ObjectValue(sock_obj.get()));
    let call_args = HandleValueArray {
        length_: 1,
        elements_: &*sock_elem.handle(),
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        realm_cx.raw_cx(),
        global_root.handle().into(),
        handler_val.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(realm_cx.raw_cx());
    }
}

/// Socket received data — store in per-socket buffer for JS to read via __net_read.
unsafe extern "C" fn net_on_data(
    s: *mut us_socket_t,
    data: *mut u8,
    length: ::std::ffi::c_int,
) -> *mut us_socket_t {
    let key = s as usize;
    if length > 0 && !data.is_null() {
        let slice = ::std::slice::from_raw_parts(data, length as usize);
        NET_INCOMING_DATA.with(|m| {
            let mut map = m.borrow_mut();
            let buf = map.entry(key).or_insert_with(Vec::new);
            buf.extend_from_slice(slice);
        });
    }
    s
}

/// Socket became writable.
unsafe extern "C" fn net_on_writable(s: *mut us_socket_t) -> *mut us_socket_t {
    // Could trigger JS "drain" event for backpressure.
    s
}

/// Socket closed.
unsafe extern "C" fn net_on_close(
    s: *mut us_socket_t,
    _code: ::std::ffi::c_int,
    _reason: *mut ::std::ffi::c_void,
) -> *mut us_socket_t {
    let key = s as usize;
    NET_SOCKETS.with(|m| m.borrow_mut().remove(&key));
    NET_INCOMING_DATA.with(|d| d.borrow_mut().remove(&key));
    NET_EOF_SOCKETS.with(|e| e.borrow_mut().remove(&key));
    s
}

/// Socket timed out.
unsafe extern "C" fn net_on_timeout(s: *mut us_socket_t) -> *mut us_socket_t {
    s
}

/// Socket long-timeout.
unsafe extern "C" fn net_on_long_timeout(s: *mut us_socket_t) -> *mut us_socket_t {
    s
}

/// Socket received FIN/EOF — mark half-closed so `__net_poll_state` reports 2
/// (the JS poll chain delivers 'end' once, Node semantics).
unsafe extern "C" fn net_on_end(s: *mut us_socket_t) -> *mut us_socket_t {
    let key = s as usize;
    NET_EOF_SOCKETS.with(|e| e.borrow_mut().insert(key, true));
    s
}

/// Connect error on established socket.
unsafe extern "C" fn net_on_connect_error(
    s: *mut us_socket_t,
    _code: ::std::ffi::c_int,
) -> *mut us_socket_t {
    CONNECT_ERROR.with(|e| e.set(true));
    CONNECT_RESULT.with(|r| r.set(Some(0))); // sentinel: error
    // The IP-literal fast path hands net_connect a SEMI_SOCKET with no
    // ConnectingSocket, so uSockets expects THIS handler to close (same C
    // contract as HTTPContext::on_connect_error — "close is called by the
    // caller"). Without the close the never-opened socket stays registered
    // in epoll as level-triggered EPOLLERR and every subsequent loop tick
    // re-dispatches this callback forever. close() on a SEMI socket raw-closes
    // the fd without firing on_close (owner already notified via this event).
    unsafe {
        (*s).close(CloseCode::failure);
    }
    s
}

/// Connecting socket error.
unsafe extern "C" fn net_on_connecting_error(
    _c: *mut bun_uws_sys::ConnectingSocket,
    _code: ::std::ffi::c_int,
) -> *mut bun_uws_sys::ConnectingSocket {
    CONNECT_ERROR.with(|e| e.set(true));
    CONNECT_RESULT.with(|r| r.set(Some(0)));
    _c
}

/// SSL handshake completion — no-op for plain TCP.
unsafe extern "C" fn net_on_handshake(
    _s: *mut us_socket_t,
    _success: ::std::ffi::c_int,
    _err: bun_uws_sys::us_bun_verify_error_t,
    _custom_data: *mut ::std::ffi::c_void,
) {
    // No-op for plain TCP.
}

/// Static VTable for all net TCP sockets.
static NET_VTABLE: VTable = VTable {
    on_open: Some(net_on_open),
    on_data: Some(net_on_data),
    on_fd: None,
    on_writable: Some(net_on_writable),
    on_close: Some(net_on_close),
    on_timeout: Some(net_on_timeout),
    on_long_timeout: Some(net_on_long_timeout),
    on_end: Some(net_on_end),
    on_connect_error: Some(net_on_connect_error),
    on_connecting_error: Some(net_on_connecting_error),
    on_handshake: Some(net_on_handshake),
};

// ──────────────────── JS helper functions ────────────────────

const NET_JS: &str = r#"
(function() {
  var EE = null;
  try { EE = require("events").EventEmitter; } catch(e) {
    EE = function EE() { this._events = {}; };
    EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); } return this; };
  }

  function Socket(opts) {
    EE.call(this);
    this.destroyed = false;
    this.connecting = false;
    this._ptr = 0;
    this._polling = false;
    this._sawEnd = false;
  }
  Socket.prototype = Object.create(EE.prototype);
  Socket.prototype.constructor = Socket;
  Socket.prototype.connect = function(port, host, cb) {
    if (typeof host === "function") { cb = host; host = "127.0.0.1"; }
    if (!host) host = "127.0.0.1";
    this.connecting = true;
    if (typeof __net_connect === "function") {
      var ptr = __net_connect(port, host);
      if (ptr > 0) {
        this._ptr = ptr;
        this.connecting = false;
        // Node semantics: the 'connect' event and the connect callback fire
        // on a LATER tick — never synchronously inside net.connect(). The
        // synchronous form broke `var c = net.connect(p, h, function () {
        // c.on(...) })` and `c.on('connect')` registered after the call: the
        // callback ran while `c` was still undefined. Scheduling BEFORE
        // _startPoll keeps 'connect' ahead of any 'data' (both are
        // setTimeout(0); same-deadline timers fire in registration order).
        var self = this;
        setTimeout(function () {
          if (self.destroyed || self._ptr === 0) return;
          self.emit("connect");
          if (cb) cb();
        }, 0);
        this._startPoll();
      } else {
        // 'error' equally deferred: the listener is registered after
        // net.connect() returns in the common `var c = net.connect(...);
        // c.on('error', ...)` shape.
        var self = this;
        setTimeout(function () {
          if (self.destroyed) return;
          self.emit("error", new Error("connect ECONNREFUSED " + host + ":" + port));
        }, 0);
      }
    }
    return this;
  };
  Socket.prototype.write = function(data) {
    if (this.destroyed || this._ptr === 0) return false;
    if (typeof __net_write === "function") {
      return __net_write(this._ptr, data) >= 0;
    }
    return false;
  };
  Socket.prototype.end = function(data) {
    // Node semantics: end() is idempotent — a second end() (including the
    // canonical `sock.on('end', () => sock.end())` half-close echo shape) is
    // a no-op. Without the guard the re-entrant end() re-emitted 'end'
    // synchronously, recursing until SpiderMonkey's "too much recursion"
    // throw aborted the poll tick mid-delivery (flaky peer-FIN test, log
    // flooded with hundreds of 'end' events per single FIN).
    if (this.destroyed) return this;
    if (data) this.write(data);
    this.destroyed = true;
    this._stopPoll();
    if (typeof __net_close === "function") {
      __net_close(this._ptr);
    }
    this._ptr = 0;
    this.emit("end");
    this.emit("close");
    return this;
  };
  Socket.prototype.on = function(event, listener) {
    EE.prototype.on.call(this, event, listener);
    if (event === "data" && this._ptr > 0) {
      this._startPoll();
    }
    return this;
  };
  Socket.prototype.destroy = function() {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._stopPoll();
    if (this._ptr > 0 && typeof __net_close === "function") {
      __net_close(this._ptr);
    }
    this._ptr = 0;
    this.emit("close");
    return this;
  };
  // Poll __net_read for buffered incoming data and emit 'data' events
  Socket.prototype._startPoll = function() {
    if (this._polling || this._ptr === 0) return;
    this._polling = true;
    // DEFERRED first tick — same class as the CP shim fix: a synchronous
    // first tick drained buffered data before listeners registered later in
    // the same block (on('connect') cb writing, then on('data')) could see it.
    setTimeout(this._pollTick.bind(this), 0);
  };
  Socket.prototype._stopPoll = function() {
    this._polling = false;
  };
  Socket.prototype._pollTick = function() {
    if (!this._polling || this.destroyed || this._ptr === 0) return;
    // Socket lifecycle from the native side: 1 open, 2 peer-FIN seen,
    // 3 fully closed. Without this the poll chain spun forever after the
    // peer (or the server's close_all) closed the socket, holding the event
    // loop open and never delivering 'end'/'close'. usockets commonly closes
    // the socket right after dispatching on_end (no half-open window), so the
    // poll may first observe state 3 — deliver 'end' before 'close' there
    // too (Node ordering: end precedes close).
    if (typeof __net_poll_state === "function") {
      var st = __net_poll_state(this._ptr);
      if (st === 3) {
        this._stopPoll();
        this._ptr = 0;
        this.destroyed = true;
        if (!this._sawEnd) {
          this._sawEnd = true;
          this.emit("end");
        }
        this.emit("close");
        return;
      }
      if (st === 2 && !this._sawEnd) {
        this._sawEnd = true;
        this.emit("end");
      }
    }
    if (typeof __net_read === "function") {
      var buf = __net_read(this._ptr);
      // __net_read returns an ArrayBuffer (transfer-owned) — length lives on
      // .byteLength; the old `.length` check was always undefined and 'data'
      // never fired.
      // BCE-20260816-NET-DATABUFFER — Node delivers 'data' chunks as Buffer,
      // not ArrayBuffer (audit: net chunk arrived as ArrayBuffer so
      // Buffer.isBuffer(chunk) === false and .toString(enc) was missing).
      // Buffer.view over the transferred ArrayBuffer (zero-copy).
      if (buf && buf.byteLength > 0) {
        this.emit("data", Buffer.from(buf));
      }
    }
    // Schedule next poll via setTimeout(0) to yield to other events
    if (this._polling && !this.destroyed && this._ptr !== 0) {
      setTimeout(this._pollTick.bind(this), 0);
    }
  };

  function Server(opts, connectionListener) {
    if (typeof opts === "function") { connectionListener = opts; opts = null; }
    EE.call(this);
    this.listening = false;
    this._ptr = 0;
    this._port = 0;
    if (connectionListener) this.on("connection", connectionListener);
  }
  Server.prototype = Object.create(EE.prototype);
  Server.prototype.constructor = Server;
  Server.prototype.listen = function() {
    var port = 0, host = "0.0.0.0", cb;
    for (var i = 0; i < arguments.length; i++) {
      var arg = arguments[i];
      if (typeof arg === "function") cb = arg;
      else if (typeof arg === "number") port = arg;
      else if (typeof arg === "string") host = arg;
    }
    if (typeof __net_listen === "function") {
      var ptr = __net_listen(port, host);
      if (ptr > 0) {
        this._ptr = ptr;
        this._port = port;
        this.listening = true;
        // Register the accept dispatcher BEFORE any callback runs: the native
        // side (dispatch_accept, vtable on_open with is_client == 0) drops the
        // accept silently when NET_CONNECTION_CBS has no entry for the listen
        // socket. A 'listening' callback that immediately net.connect()s to
        // itself spins the loop inline (net_connect waits for the real TCP
        // open), so the inbound accept can dispatch inside that spin — before
        // this function returns. Registering after `cb()` (the old order)
        // lost that first connection forever (echo server never saw it).
        if (typeof __net_on_connection === "function") {
          var self = this;
          __net_on_connection(ptr, function(sock) {
            self.emit("connection", sock);
          });
        }
        this.emit("listening");
        if (cb) cb();
      } else {
        this.emit("error", new Error("listen EADDRINUSE"));
      }
    }
    return this;
  };
  Server.prototype.close = function(cb) {
    this.listening = false;
    if (this._ptr > 0 && typeof __net_close === "function") {
      __net_close(this._ptr);
    }
    this._ptr = 0;
    this.emit("close");
    if (cb) cb();
    return this;
  };
  Server.prototype.address = function() {
    if (this._ptr > 0 && typeof __net_address === "function") {
      var addr = __net_address(this._ptr);
      if (addr) return addr;
    }
    // Fallback: return the port passed to listen() if getsockname failed
    if (this._port > 0) {
      return { port: this._port, family: "IPv4", address: "0.0.0.0" };
    }
    return { port: 0, family: "IPv4", address: "0.0.0.0" };
  };

  function isIP(input) {
    if (!input || typeof input !== "string") return 0;
    // Check IPv4
    var parts = input.split(".");
    if (parts.length === 4) {
      for (var i = 0; i < 4; i++) {
        var n = parseInt(parts[i], 10);
        if (isNaN(n) || n < 0 || n > 255 || parts[i] !== String(n)) return 0;
      }
      return 4;
    }
    // Check IPv6 — use native __net_isIPv6 if available for robust detection
    if (typeof __net_isIPv6 === "function") {
      if (__net_isIPv6(input)) return 6;
    } else if (input.indexOf(":") !== -1) {
      return isIPv6String(input) ? 6 : 0;
    }
    return 0;
  }

  function isIPv6String(input) {
    // Basic IPv6 validation: must contain ':', valid hextets and '::' compression
    if (input.indexOf(":") === -1) return false;
    // Reject embedded IPv4 unless it's the last two parts (::ffff:1.2.3.4)
    var doubleColon = input.indexOf("::");
    if (doubleColon !== input.lastIndexOf("::")) return false; // only one :: allowed
    var segments = input.split(":");
    // Handle trailing IPv4 mapped address (e.g. ::ffff:192.168.1.1)
    var lastSeg = segments[segments.length - 1];
    if (lastSeg && lastSeg.indexOf(".") !== -1) {
      var v4parts = lastSeg.split(".");
      if (v4parts.length !== 4) return false;
      for (var j = 0; j < 4; j++) {
        var n = parseInt(v4parts[j], 10);
        if (isNaN(n) || n < 0 || n > 255 || v4parts[j] !== String(n)) return false;
      }
      segments = segments.slice(0, -1);
    }
    if (segments.length > 8) return false;
    var hasDoubleColon = input.indexOf("::") !== -1;
    if (!hasDoubleColon && segments.length !== 8) return false;
    if (hasDoubleColon && segments.length >= 8) return false;
    for (var i = 0; i < segments.length; i++) {
      var seg = segments[i];
      if (seg === "" && (i === 0 || i === segments.length - 1)) continue; // leading/trailing empty from ::
      if (seg === "") continue; // empty from :: expansion
      if (!/^[0-9a-fA-F]{1,4}$/.test(seg)) return false;
    }
    return true;
  }

  // Accept-bridge factory: the native side (dispatch_accept) calls this to
  // build the JS net.Socket for a usockets-accepted socket — the prototype
  // chain and the __net_read poll machinery stay owned by this IIFE. Written
  // TO the global (not probed FROM it), so the free-variable probe class
  // fixed in bbe20a81 does not apply.
  try {
    globalThis.__net_make_socket = function(ptr) {
      var s = new Socket();
      s._ptr = ptr;
      s.connecting = false;
      return s;
    };
  } catch (e) { /* globalThis unavailable — accept bridge disabled */ }

  return {
    Socket: Socket,
    Server: Server,
    createServer: function(opts, cb) { return new Server(opts, cb); },
    connect: function(port, host, cb) { var s = new Socket(); return s.connect(port, host, cb); },
    createConnection: function(port, host, cb) { var s = new Socket(); return s.connect(port, host, cb); },
    isIP: isIP,
    isIPv4: function(input) { return isIP(input) === 4; },
    isIPv6: function(input) { return isIP(input) === 6; },
  };
})();
"#;

// ──────────────────── JS↔Rust pointer helpers ────────────────────

/// Extract a socket pointer from a JSVal as a full `usize`.
///
/// JS stores pointers as `Number` (f64), which can losslessly represent
/// integers up to 2^53 — more than enough for 64-bit pointers (always < 2^48).
/// Previously `to_int32()` was used, which truncated the high 32 bits on
/// 64-bit systems, causing HashMap lookups to fail silently.
#[inline]
fn jsval_to_ptr(val: &JSVal) -> usize {
    if val.is_double() {
        val.to_double() as usize
    } else if val.is_int32() {
        val.to_int32() as usize
    } else {
        0
    }
}

/// Convert a `usize` pointer to a JS `DoubleValue` for return to JS.
///
/// Uses f64 which can losslessly represent integers up to 2^53.
#[inline]
fn ptr_to_jsval(ptr: usize) -> JSVal {
    DoubleValue(ptr as f64)
}

// ──────────────────── host_fn implementations ────────────────────

/// Get the uSockets event loop, ensuring bao_uloop is initialized.
fn get_loop() -> *mut Loop {
    bao_uloop::force_link();
    bao_uloop::uws_get_loop()
}

/// Create or get the per-thread TCP socket group for server listen.
fn ensure_server_group(loop_: *mut Loop) -> *mut SocketGroup {
    // Allocate a new SocketGroup for each server (matching Bun's pattern
    // where each server has its own socket group).
    let mut group = Box::new(SocketGroup::default());
    group.init(loop_, Some(&NET_VTABLE), ptr::null_mut());
    Box::into_raw(group)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    let addr = if argc > 1 && (*args.get(1).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        "0.0.0.0".to_string()
    };

    let loop_ = get_loop();
    if loop_.is_null() {
        args.rval().set(Int32Value(0));
        return true;
    }

    let group_ptr = ensure_server_group(loop_);
    let group: &mut SocketGroup = unsafe { &mut *group_ptr };

    let host_cstr = ZBox::from_bytes(addr.as_bytes());
    let mut err: ::std::ffi::c_int = 0;

    let listen_socket = group.listen(
        SocketKind::UwsHttp, // plain TCP kind
        None,                // no SSL
        Some((*host_cstr).as_cstr()),
        port,
        0, // LIBUS_LISTEN_DEFAULT
        0, // socket_ext_size (no per-socket ext for listen sockets)
        &mut err,
    );

    // The return value is the authoritative success signal (NULL ⟺
    // failure, the uWS contract). The C layer's *error is ALSO set on the
    // success path (us_internal_bind_and_listen writes LIBUS_ERR after a
    // successful listen() — a usockets divergence from upstream), so it
    // must not be part of the success test: `err != 0` on a live listen
    // socket used to drive this branch into destroying a group that still
    // had the socket linked, tripping the head_listen_sockets assert in
    // us_socket_group_deinit (SIGABRT on every net.Server.listen).
    let _ = err;
    if listen_socket.is_null() {
        // Listen failed — destroy the group.
        unsafe {
            SocketGroup::destroy(group_ptr);
        }
        args.rval().set(Int32Value(0));
        return true;
    }

    // Store the group and listen socket.
    let listen_key = listen_socket as usize;
    NET_SERVER_GROUPS.with(|g| {
        g.borrow_mut()
            .insert(listen_key, unsafe { Box::from_raw(group_ptr) })
    });
    NET_LISTEN_SOCKETS.with(|l| l.borrow_mut().push(listen_key));

    // Reverse index for the accept bridge: an accepted socket reaches the
    // vtable with only its group pointer — map it back to this listen socket
    // (whose 'connection' dispatcher fires).
    NET_GROUP_LISTEN.with(|m| m.borrow_mut().insert(group_ptr as usize, listen_key));

    // JS-idle tick surface (BCE-007 registration gap, node:net variant):
    // drain_and_check only drives the uWS loop while
    // node_http::has_active_servers() is true — that branch is the only thing
    // that ever `accept()`s on this listen socket. Register it in the unified
    // liveness registry so an idle script (no pending timers, no poll chains)
    // still accepts inbound connections. Liveness token only: the registry
    // does ptr-eq bookkeeping and never dereferences (same representation-
    // preserving alias node_http2 uses for its `App<true>` tokens).
    unsafe { crate::node_http::register_active_app(listen_socket as *mut App<false>) };

    // Store the requested port so address() can return it if getsockname fails.
    // If port 0 was requested, the OS assigned a port — getsockname in net_address
    // will retrieve the actual port, but we store the requested port as fallback.
    NET_LISTEN_PORTS.with(|p| p.borrow_mut().insert(listen_key, port as u16));

    // Return the listen socket pointer as a JS Number (f64) to avoid i32 truncation on 64-bit.
    args.rval().set(ptr_to_jsval(listen_key));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port = if argc > 0 {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    let addr = if argc > 1 && (*args.get(1).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        "127.0.0.1".to_string()
    };

    let loop_ = get_loop();
    if loop_.is_null() {
        args.rval().set(Int32Value(0));
        return true;
    }

    // Create a per-connect socket group.
    let mut group = Box::new(SocketGroup::default());
    group.init(loop_, Some(&NET_VTABLE), ptr::null_mut());
    let group_ptr = Box::into_raw(group);

    let host_cstr = ZBox::from_bytes(addr.as_bytes());

    // Reset connect state.
    CONNECT_RESULT.with(|r| r.set(None));
    CONNECT_ERROR.with(|e| e.set(false));

    let result = (*group_ptr).connect(
        SocketKind::UwsHttp,
        None,
        (*host_cstr).as_cstr(),
        port,
        0,
        0, // socket_ext_size
    );

    match result {
        bun_uws_sys::ConnectResult::Socket(socket) => {
            // IP-literal fast path (try_parse_ip in us_socket_group_connect):
            // the address is resolved, so uSockets hands back the SEMI_SOCKET
            // immediately — but connect(2) is still IN PROGRESS on the
            // non-blocking fd. Treating this branch as "already connected"
            // emitted 'connect' before the TCP handshake completed (and before
            // `var c = net.connect(...)` finished assigning). The socket is
            // driven by the same loop ticks as the Connecting branch: wait for
            // net_on_open (CONNECT_RESULT = socket key) or
            // net_on_connect_error (CONNECT_RESULT = 0 sentinel), exactly like
            // the Connecting arm below.
            let key = socket as usize;
            // Store the group so it lives as long as the socket.
            NET_SERVER_GROUPS.with(|g| {
                g.borrow_mut()
                    .insert(key, unsafe { Box::from_raw(group_ptr) })
            });

            let max_ticks: u32 = 5000;
            for _ in 0..max_ticks {
                let done = CONNECT_RESULT.with(|r| r.get().is_some());
                if done {
                    break;
                }
                unsafe {
                    bao_uloop::bao_loop_tick(loop_, ptr::null());
                }
            }

            let error = CONNECT_ERROR.with(|e| e.get());
            let result_key = CONNECT_RESULT.with(|r| r.get().unwrap_or(0));

            if error || result_key == 0 {
                // Connect failed — net_on_connect_error already closed the
                // socket, so the per-connect group is empty. Reclaim it now
                // (the Failed arm's destroy shape): JS got 0 and will never
                // call __net_close for this key, so parking the Box in
                // NET_SERVER_GROUPS would leak one group per refused connect.
                if let Some(group_box) = NET_SERVER_GROUPS.with(|g| g.borrow_mut().remove(&key)) {
                    let raw = Box::into_raw(group_box);
                    unsafe {
                        SocketGroup::destroy(raw);
                        drop(Box::from_raw(raw));
                    }
                }
                args.rval().set(Int32Value(0));
            } else {
                NET_SOCKETS.with(|m| m.borrow_mut().insert(result_key, true));
                args.rval().set(ptr_to_jsval(result_key));
            }
        }
        bun_uws_sys::ConnectResult::Connecting(_connecting) => {
            // Async connect — tick the loop until on_open or on_connect_error fires.
            // Store the group so it stays alive during the connect.
            let group_key = group_ptr as usize;
            NET_SERVER_GROUPS.with(|g| {
                g.borrow_mut()
                    .insert(group_key, unsafe { Box::from_raw(group_ptr) })
            });

            let max_ticks: u32 = 5000;
            for _ in 0..max_ticks {
                // Check if result arrived.
                let done = CONNECT_RESULT.with(|r| r.get().is_some());
                if done {
                    break;
                }
                // Tick the event loop — epoll_wait will block until an event arrives.
                unsafe {
                    bao_uloop::bao_loop_tick(loop_, ptr::null());
                }
            }

            let error = CONNECT_ERROR.with(|e| e.get());
            let result_key = CONNECT_RESULT.with(|r| r.get().unwrap_or(0));

            if error || result_key == 0 {
                args.rval().set(Int32Value(0));
            } else {
                NET_SOCKETS.with(|m| m.borrow_mut().insert(result_key, true));
                args.rval().set(ptr_to_jsval(result_key));
            }
        }
        bun_uws_sys::ConnectResult::Failed => {
            // Connect failed immediately.
            unsafe {
                SocketGroup::destroy(group_ptr);
            }
            args.rval().set(Int32Value(0));
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(Int32Value(-1));
        return true;
    }

    let ptr_val = jsval_to_ptr(&(*args.get(0).ptr));
    // Node accepts string | Buffer | Uint8Array | ArrayBuffer. The previous
    // string-only branch silently wrote an EMPTY payload for every non-string
    // argument (the silent no-op class) — echo servers writing the received
    // ArrayBuffer back transmitted nothing.
    let data: Vec<u8> = if (*args.get(1).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
            .into_bytes()
    } else {
        match crate::node_buffer::collect_byte_view(cx, *args.get(1).ptr) {
            Some(b) => b,
            None => {
                args.rval().set(Int32Value(-1));
                return true;
            }
        }
    };

    let socket_ptr = ptr_val as *mut us_socket_t;
    let exists = NET_SOCKETS.with(|m| m.borrow().contains_key(&ptr_val));
    if !exists {
        args.rval().set(Int32Value(-1));
        return true;
    }

    // us_socket_t::write returns the number of bytes written (or 0 on backpressure).
    let written = unsafe { (*socket_ptr).write(&data) };
    args.rval().set(Int32Value(written));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_close(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ptr_val = if argc > 0 {
        jsval_to_ptr(&(*args.get(0).ptr))
    } else {
        0
    };

    if ptr_val == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Try to close as a connected socket.
    let was_socket = NET_SOCKETS.with(|m| m.borrow_mut().remove(&ptr_val).is_some());
    if was_socket {
        let socket_ptr = ptr_val as *mut us_socket_t;
        unsafe {
            (*socket_ptr).close(CloseCode::normal);
        }
    }

    // Try to close as a listen socket.
    let is_listen = NET_LISTEN_SOCKETS.with(|l| {
        let mut list = l.borrow_mut();
        match list.iter().position(|&k| k == ptr_val) {
            Some(pos) => {
                list.swap_remove(pos);
                true
            }
            None => false,
        }
    });

    if is_listen {
        // Server teardown. close_all first (listeners, then accepted sockets —
        // their net_on_close fires synchronously and cleans NET_SOCKETS /
        // NET_EOF_SOCKETS), then explicit destroy. Dropping the group Box
        // alone leaves a linked group in the loop's list whenever accepted
        // sockets existed (use-after-free on the next tick), and destroy
        // asserts the empty-lists contract (the same head_listen_sockets
        // assert that SIGABRT'd net_listen before the bbe20a81 root-cause).
        if let Some(group_box) = NET_SERVER_GROUPS.with(|g| g.borrow_mut().remove(&ptr_val)) {
            let raw = Box::into_raw(group_box);
            let group_addr = raw as usize;
            unsafe {
                (*raw).close_all();
                SocketGroup::destroy(raw);
                drop(Box::from_raw(raw));
            }
            NET_GROUP_LISTEN.with(|m| m.borrow_mut().remove(&group_addr));
        } else {
            // No group tracked (defensive) — at least close the listener fd.
            let listen_ptr = ptr_val as *mut ListenSocket;
            unsafe {
                (*listen_ptr).close();
            }
        }
        NET_LISTEN_PORTS.with(|p| p.borrow_mut().remove(&ptr_val));
        // Drop the connection dispatcher (GcStore entry + map slot) and the
        // JS-idle liveness token registered at listen time.
        NET_CONNECTION_CBS.with(|c| {
            if let Some(key) = c.borrow_mut().remove(&ptr_val) {
                gc_store_remove(cx, &key);
            }
        });
        unsafe { crate::node_http::unregister_active_app(ptr_val as *mut App<false>) };
    } else {
        // Not a listen socket — a per-connect group may still be tracked.
        NET_SERVER_GROUPS.with(|g| g.borrow_mut().remove(&ptr_val));
    }

    args.rval().set(UndefinedValue());
    true
}

/// Get the bound address and port of a listen socket.
/// Returns a JS object { port, family, address } or null on failure.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_address(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ptr_val = if argc > 0 {
        jsval_to_ptr(&(*args.get(0).ptr))
    } else {
        0
    };

    if ptr_val == 0 {
        args.rval().set(ObjectValue(::std::ptr::null_mut()));
        return true;
    }

    let listen_ptr = ptr_val as *mut ListenSocket;
    // Try get_local_port first (works even if getsockname fails)
    let port = unsafe { (*listen_ptr).get_local_port() };

    // Get local address via libc::getsockname as fallback
    let mut addr: libc::sockaddr_storage = unsafe { ::std::mem::zeroed() };
    let mut addr_len: libc::socklen_t =
        ::std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let fd = unsafe { (*listen_ptr).fd() };

    let (address_str, family_str, resolved_port) = if unsafe {
        libc::getsockname(
            fd.native(),
            &mut addr as *mut _ as *mut libc::sockaddr,
            &mut addr_len,
        )
    } == 0
    {
        let actual_port = if addr.ss_family as i32 == libc::AF_INET6 {
            let addr_in6 = &addr as *const _ as *const libc::sockaddr_in6;
            unsafe { u16::from_be((*addr_in6).sin6_port) as i32 }
        } else {
            let addr_in = &addr as *const _ as *const libc::sockaddr_in;
            unsafe { u16::from_be((*addr_in).sin_port) as i32 }
        };
        if addr.ss_family as i32 == libc::AF_INET6 {
            // IPv6
            let addr_in6 = &addr as *const _ as *const libc::sockaddr_in6;
            let mut buf = [0u8; 64];
            let ok = unsafe {
                inet_ntop(
                    libc::AF_INET6,
                    &(*addr_in6).sin6_addr as *const _ as *const ::std::ffi::c_void,
                    buf.as_mut_ptr() as *mut ::std::ffi::c_char,
                    buf.len() as libc::socklen_t,
                )
            };
            let addr_str = if ok.is_null() {
                "::".to_string()
            } else {
                unsafe { ::std::ffi::CStr::from_ptr(ok) }
                    .to_string_lossy()
                    .into_owned()
            };
            (addr_str, "IPv6", actual_port)
        } else {
            // IPv4
            let addr_in = &addr as *const _ as *const libc::sockaddr_in;
            let mut buf = [0u8; 32];
            let ok = unsafe {
                inet_ntop(
                    libc::AF_INET,
                    &(*addr_in).sin_addr as *const _ as *const ::std::ffi::c_void,
                    buf.as_mut_ptr() as *mut ::std::ffi::c_char,
                    buf.len() as libc::socklen_t,
                )
            };
            let addr_str = if ok.is_null() {
                "0.0.0.0".to_string()
            } else {
                unsafe { ::std::ffi::CStr::from_ptr(ok) }
                    .to_string_lossy()
                    .into_owned()
            };
            (addr_str, "IPv4", actual_port)
        }
    } else {
        // getsockname failed — use port from get_local_port() or stored port
        let fallback_port = if port > 0 {
            port
        } else {
            NET_LISTEN_PORTS.with(|p| p.borrow().get(&ptr_val).copied().unwrap_or(0) as i32)
        };
        ("0.0.0.0".to_string(), "IPv4", fallback_port)
    };

    // Build JS return object { port, family, address }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let result_obj = w2::JS_NewPlainObject(cx_ref));
    if result_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let result_h = result_obj.handle().into();

    // port
    rooted!(&in(cx_ref) let pv = Int32Value(resolved_port));
    JS_DefineProperty(
        cx,
        result_h,
        c"port".as_ptr(),
        pv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // family
    let c_family = ZBox::from_bytes(family_str.as_bytes());
    let family_js = JS_NewStringCopyZ(cx, c_family.as_ptr());
    if !family_js.is_null() {
        rooted!(&in(cx_ref) let fv = StringValue(&*family_js));
        JS_DefineProperty(
            cx,
            result_h,
            c"family".as_ptr(),
            fv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // address
    let c_addr = ZBox::from_bytes(address_str.as_bytes());
    let addr_js = JS_NewStringCopyZ(cx, c_addr.as_ptr());
    if !addr_js.is_null() {
        rooted!(&in(cx_ref) let av = StringValue(&*addr_js));
        JS_DefineProperty(
            cx,
            result_h,
            c"address".as_ptr(),
            av.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    args.rval().set(ObjectValue(result_obj.get()));
    true
}

/// Read buffered incoming data from a socket.
/// __net_read(socket_ptr) -> Uint8Array or null
/// Drains the per-socket buffer and returns all accumulated data.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ptr_val = if argc > 0 {
        jsval_to_ptr(&(*args.get(0).ptr))
    } else {
        0
    };

    if ptr_val == 0 {
        args.rval().set(NullValue());
        return true;
    }

    let data = NET_INCOMING_DATA.with(|m| {
        let mut map = m.borrow_mut();
        match map.remove(&ptr_val) {
            Some(v) => v,
            None => Vec::new(),
        }
    });

    if data.is_empty() {
        // Return null to indicate no data available
        args.rval().set(NullValue());
        return true;
    }

    // Create a JS ArrayBuffer and return it wrapped as a Uint8Array-like object
    // Use JS_NewArrayBufferWithContents to transfer ownership of the data
    let len = data.len();
    let buf_ptr = data.as_ptr();
    // We need to copy data into a new allocation because Vec will be freed
    let alloc = ::std::alloc::alloc(
        ::std::alloc::Layout::from_size_align(len, 1)
            .unwrap_or_else(|_| ::std::alloc::Layout::from_size_align(1, 1).unwrap()),
    );
    if alloc.is_null() {
        args.rval().set(NullValue());
        return true;
    }
    unsafe {
        ::std::ptr::copy_nonoverlapping(buf_ptr, alloc, len);
    }
    // Vec is freed here (data goes out of scope), alloc is our copy

    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;

    let array_buffer =
        w2::NewArrayBufferWithContents(cx_ref, len, alloc as *mut ::std::os::raw::c_void);
    if array_buffer.is_null() {
        // NewArrayBufferWithContents failed — free our allocation
        ::std::alloc::dealloc(
            alloc,
            ::std::alloc::Layout::from_size_align(len, 1)
                .unwrap_or_else(|_| ::std::alloc::Layout::from_size_align(1, 1).unwrap()),
        );
        args.rval().set(NullValue());
        return true;
    }

    rooted!(&in(cx_ref) let ab = array_buffer);
    args.rval().set(ObjectValue(ab.get()));
    true
}

/// Check if a string is an IPv6 address.
/// __net_isIPv6(input_string) -> boolean
/// Simple detection: IPv6 addresses contain ':' characters.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_is_ipv6(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let input = unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(0).ptr).to_string()));
    let result = input.contains(':');
    args.rval().set(BooleanValue(result));
    true
}

/// __net_on_connection(listen_ptr, callback) — Server.listen registers its
/// 'connection' dispatcher here. dispatch_accept (vtable accept path) resolves
/// the callback from GcStore and calls it with the accepted JS Socket.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_on_connection(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let listen_ptr = jsval_to_ptr(&(*args.get(0).ptr));
    let cb_val = *args.get(1).ptr;
    if listen_ptr == 0 || !cb_val.is_object() {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let cb_obj = cb_val.to_object());
    if !unsafe { JS_ObjectIsFunction(cb_obj.get()) } {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let key = gc_store_unique_key(&format!("net_connection_{}", listen_ptr));
    gc_store_insert(cx, &key, cb_obj.get());
    NET_CONNECTION_CBS.with(|c| c.borrow_mut().insert(listen_ptr, key));

    args.rval().set(BooleanValue(true));
    true
}

/// __net_poll_state(socket_ptr) → 1 open | 2 peer-FIN seen ('end' pending) |
/// 3 fully closed. The JS Socket poll chain consumes this to deliver
/// 'end'/'close' and stop scheduling when the native socket is gone.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_poll_state(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ptr_val = if argc > 0 {
        jsval_to_ptr(&(*args.get(0).ptr))
    } else {
        0
    };
    if ptr_val == 0 {
        args.rval().set(Int32Value(3));
        return true;
    }

    let open = NET_SOCKETS.with(|m| m.borrow().contains_key(&ptr_val));
    if !open {
        args.rval().set(Int32Value(3));
        return true;
    }
    let eof = NET_EOF_SOCKETS.with(|e| e.borrow().contains_key(&ptr_val));
    args.rval().set(Int32Value(if eof { 2 } else { 1 }));
    true
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();

        // Register native helper functions on module object for JS code to call
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_listen".as_ptr(),
            Some(net_listen),
            2,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_connect".as_ptr(),
            Some(net_connect),
            2,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_write".as_ptr(),
            Some(net_write),
            2,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_close".as_ptr(),
            Some(net_close),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_address".as_ptr(),
            Some(net_address),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_read".as_ptr(),
            Some(net_read),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_isIPv6".as_ptr(),
            Some(net_is_ipv6),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_on_connection".as_ptr(),
            Some(net_on_connection),
            2,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__net_poll_state".as_ptr(),
            Some(net_poll_state),
            1,
            0,
        );

        // The NET_JS IIFE resolves these host bridges as FREE variables —
        // the `typeof __net_connect === "function"` probes inside the IIFE
        // look at the GLOBAL, never at this module object. Defining them
        // only on mod_obj left every probe false: Socket.connect /
        // Server.listen / write / read / address all silently no-op'd.
        // Mirror them onto the global (non-enumerable, configurable) so the
        // IIFE sees them (same class as the http2 fix, commit 854677b0).
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let bridges: &[(&str, JSNative, u32)] = &[
                ("__net_listen", Some(net_listen), 2),
                ("__net_connect", Some(net_connect), 2),
                ("__net_write", Some(net_write), 2),
                ("__net_close", Some(net_close), 1),
                ("__net_address", Some(net_address), 1),
                ("__net_read", Some(net_read), 1),
                ("__net_isIPv6", Some(net_is_ipv6), 1),
                ("__net_on_connection", Some(net_on_connection), 2),
                ("__net_poll_state", Some(net_poll_state), 1),
            ];
            for &(name, native, nargs) in bridges {
                let c_name = ZBox::from_bytes(name);
                JS_DefineFunction(
                    cx_raw,
                    global_root.handle().into(),
                    c_name.as_ptr(),
                    native,
                    nargs,
                    0,
                );
            }
        }

        // Store the JSContext for use in C callbacks.
        NET_CX.with(|c| c.set(Some(cx_raw)));

        let c_filename = ZBox::from_bytes("node:net".as_bytes());
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(NET_JS);
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
        rooted!(&in(cx) let exports_rooted = exports_obj);

        for name in &[
            "Socket",
            "Server",
            "createServer",
            "connect",
            "createConnection",
            "isIP",
            "isIPv4",
            "isIPv6",
        ] {
            let cname = ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(
                cx_raw,
                exports_rooted.handle().into(),
                cname.as_ptr(),
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
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "net", mod_obj.get());
    }
}

// ──────────────────── unit tests ────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_vtable_is_complete() {
        // Verify all critical vtable slots are populated.
        assert!(NET_VTABLE.on_open.is_some(), "on_open must be set");
        assert!(NET_VTABLE.on_data.is_some(), "on_data must be set");
        assert!(NET_VTABLE.on_close.is_some(), "on_close must be set");
        assert!(NET_VTABLE.on_writable.is_some(), "on_writable must be set");
        assert!(NET_VTABLE.on_end.is_some(), "on_end must be set");
        assert!(NET_VTABLE.on_timeout.is_some(), "on_timeout must be set");
        assert!(
            NET_VTABLE.on_connect_error.is_some(),
            "on_connect_error must be set"
        );
        assert!(
            NET_VTABLE.on_connecting_error.is_some(),
            "on_connecting_error must be set"
        );
        assert!(
            NET_VTABLE.on_handshake.is_some(),
            "on_handshake must be set"
        );
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_get_loop_returns_non_null() {
        bao_uloop::force_link();
        let loop_ = get_loop();
        assert!(
            !loop_.is_null(),
            "get_loop must return non-null after force_link"
        );
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_empty() {
        let pw = NetPendingWrite::default();
        assert!(pw.is_empty());
        assert_eq!(pw.len, 0);
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_set_and_clear() {
        let mut pw = NetPendingWrite::default();
        pw.set_data(b"hello");
        assert!(!pw.is_empty());
        assert_eq!(pw.len, 5);
        pw.clear();
        assert!(pw.is_empty());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_set_empty_data() {
        let mut pw = NetPendingWrite::default();
        pw.set_data(b"first");
        pw.set_data(b"");
        assert!(pw.is_empty());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_overwrite() {
        let mut pw = NetPendingWrite::default();
        pw.set_data(b"hello");
        pw.set_data(b"world!");
        assert_eq!(pw.len, 6);
        pw.clear();
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_cleanup_does_not_panic() {
        let _cleanup = NetCleanup;
        // Drop should not panic even with empty state.
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_socket_kind_tcp() {
        // Verify we use a valid SocketKind for plain TCP.
        let kind = SocketKind::UwsHttp;
        assert_ne!(kind, SocketKind::Invalid);
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_close_code_normal() {
        // Verify CloseCode::normal is 0 (matches C enum).
        assert_eq!(CloseCode::normal as i32, 0);
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_source_contains_ptr_not_fd() {
        // Verify JS source uses _ptr instead of _fd.
        assert!(
            NET_JS.contains("_ptr"),
            "JS must use _ptr for socket reference"
        );
        assert!(!NET_JS.contains("_fd"), "JS must not use _fd");
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_source_contains_all_exports() {
        for name in &[
            "Socket",
            "Server",
            "createServer",
            "connect",
            "createConnection",
            "isIP",
            "isIPv4",
            "isIPv6",
        ] {
            assert!(NET_JS.contains(name), "JS must export {}", name);
        }
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_socket_ext_layout() {
        // Verify NetSocketExt is repr(C) and has expected size.
        assert!(::std::mem::size_of::<NetSocketExt>() > 0);
        assert!(::std::mem::size_of::<NetSocketExt>() >= ::std::mem::size_of::<u8>());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_ensure_server_group_creates_valid_group() {
        bao_uloop::force_link();
        let loop_ = get_loop();
        assert!(!loop_.is_null());
        let group_ptr = ensure_server_group(loop_);
        assert!(!group_ptr.is_null());
        // Clean up — destroy the group.
        unsafe {
            SocketGroup::destroy(group_ptr);
        }
    }

    // ──── extended unit tests ────

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_vtable_callback_signatures_match_dispatch() {
        // Verify VTable callback types match bao_uloop dispatch expectations.
        // on_open: (*mut us_socket_t, c_int, *mut u8, c_int) -> *mut us_socket_t
        assert!(NET_VTABLE.on_open.is_some());
        // on_data: (*mut us_socket_t, *mut u8, c_int) -> *mut us_socket_t
        assert!(NET_VTABLE.on_data.is_some());
        // on_writable: (*mut us_socket_t) -> *mut us_socket_t
        assert!(NET_VTABLE.on_writable.is_some());
        // on_close: (*mut us_socket_t, c_int, *mut c_void) -> *mut us_socket_t
        assert!(NET_VTABLE.on_close.is_some());
        // on_end: (*mut us_socket_t) -> *mut us_socket_t
        assert!(NET_VTABLE.on_end.is_some());
        // on_fd is deliberately None (not used for plain TCP)
        assert!(NET_VTABLE.on_fd.is_none());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_large_data() {
        let mut pw = NetPendingWrite::default();
        let large: Vec<u8> = vec![0xAB; 1024 * 64]; // 64 KiB
        pw.set_data(&large);
        assert_eq!(pw.len, large.len());
        assert!(!pw.is_empty());
        pw.clear();
        assert!(pw.is_empty());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_reuse_buffer() {
        let mut pw = NetPendingWrite::default();
        pw.set_data(b"first_write");
        assert_eq!(pw.len, 11);
        // Reusing buffer should not leak — set_data clears then extends
        pw.set_data(b"second");
        assert_eq!(pw.len, 6);
        pw.clear();
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_cleanup_clears_all_thread_local_state() {
        bao_uloop::force_link();
        let loop_ = get_loop();
        assert!(!loop_.is_null());

        // Manually populate thread-local state to verify cleanup
        NET_SERVER_GROUPS.with(|g| {
            let mut group = Box::new(SocketGroup::default());
            group.init(loop_, Some(&NET_VTABLE), ptr::null_mut());
            g.borrow_mut().insert(9999, group);
        });
        NET_LISTEN_SOCKETS.with(|l| l.borrow_mut().push(9999));
        NET_SOCKETS.with(|s| s.borrow_mut().insert(9998, true));

        NET_SERVER_GROUPS.with(|g| assert!(!g.borrow().is_empty()));
        NET_SOCKETS.with(|s| assert!(!s.borrow().is_empty()));

        // NetCleanup drop should clear all thread-local state
        let cleanup = NetCleanup;
        drop(cleanup);

        NET_SERVER_GROUPS.with(|g| assert!(g.borrow().is_empty()));
        NET_LISTEN_SOCKETS.with(|l| assert!(l.borrow().is_empty()));
        NET_SOCKETS.with(|s| assert!(s.borrow().is_empty()));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_connect_result_initial_state() {
        CONNECT_RESULT.with(|r| assert!(r.get().is_none(), "initial CONNECT_RESULT is None"));
        CONNECT_ERROR.with(|e| assert!(!e.get(), "initial CONNECT_ERROR is false"));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_connect_result_set_and_reset() {
        CONNECT_RESULT.with(|r| r.set(Some(42)));
        assert_eq!(CONNECT_RESULT.with(|r| r.get()), Some(42));
        CONNECT_RESULT.with(|r| r.set(None));
        assert!(CONNECT_RESULT.with(|r| r.get()).is_none());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_connect_error_set_and_reset() {
        CONNECT_ERROR.with(|e| e.set(true));
        assert!(CONNECT_ERROR.with(|e| e.get()));
        CONNECT_ERROR.with(|e| e.set(false));
        assert!(!CONNECT_ERROR.with(|e| e.get()));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_socket_methods_exist() {
        // Verify JS Socket class has expected method names
        assert!(NET_JS.contains("Socket.prototype.connect"));
        assert!(NET_JS.contains("Socket.prototype.write"));
        assert!(NET_JS.contains("Socket.prototype.end"));
        assert!(NET_JS.contains("Socket.prototype.destroy"));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_server_methods_exist() {
        // Verify JS Server class has expected method names
        assert!(NET_JS.contains("Server.prototype.listen"));
        assert!(NET_JS.contains("Server.prototype.close"));
        assert!(NET_JS.contains("Server.prototype.address"));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_net_native_functions() {
        // Verify JS code references native helper functions
        assert!(NET_JS.contains("__net_listen"));
        assert!(NET_JS.contains("__net_connect"));
        assert!(NET_JS.contains("__net_write"));
        assert!(NET_JS.contains("__net_close"));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_isip_validation_logic() {
        // Verify isIP JS logic checks IPv4 format
        assert!(NET_JS.contains("split(\".\")"));
        assert!(NET_JS.contains("parts.length === 4"));
        assert!(NET_JS.contains("parseInt"));
        assert!(NET_JS.contains("0 <= n && n <= 255") || NET_JS.contains("n < 0 || n > 255"));
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_socket_ext_default_is_zero() {
        let ext = NetSocketExt {
            is_client: 0,
            pending_write: NetPendingWrite::default(),
        };
        assert_eq!(ext.is_client, 0);
        assert!(ext.pending_write.is_empty());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_socket_ext_client_flag() {
        let ext = NetSocketExt {
            is_client: 1,
            pending_write: NetPendingWrite::default(),
        };
        assert_eq!(ext.is_client, 1);
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_thread_local_hashmap_operations() {
        // Test basic HashMap operations on thread-local NET_SOCKETS
        NET_SOCKETS.with(|m| {
            let mut map = m.borrow_mut();
            map.insert(100, true);
            map.insert(200, true);
            assert_eq!(map.len(), 2);
            assert!(map.contains_key(&100));
            assert!(map.contains_key(&200));
            assert!(!map.contains_key(&300));
            map.remove(&100);
            assert_eq!(map.len(), 1);
        });
        // Clean up
        NET_SOCKETS.with(|m| m.borrow_mut().clear());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_thread_local_listen_socket_vec_operations() {
        NET_LISTEN_SOCKETS.with(|l| {
            let mut list = l.borrow_mut();
            list.push(500);
            list.push(600);
            assert_eq!(list.len(), 2);
            assert!(list.contains(&500));
            assert!(list.contains(&600));
            // swap_remove matches net_close logic
            let pos = list.iter().position(|&k| k == 500).unwrap();
            list.swap_remove(pos);
            assert_eq!(list.len(), 1);
        });
        NET_LISTEN_SOCKETS.with(|l| l.borrow_mut().clear());
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_pending_write_drop_does_not_double_free() {
        // Create and drop multiple times — should not panic or double-free
        let mut pw = NetPendingWrite::default();
        pw.set_data(b"test_data");
        pw.clear();
        pw.set_data(b"more_data");
        // Drop should handle already-cleared state
        drop(pw);
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_multiple_server_groups_in_thread_local() {
        bao_uloop::force_link();
        let loop_ = get_loop();
        assert!(!loop_.is_null());

        let g1 = ensure_server_group(loop_);
        let g2 = ensure_server_group(loop_);
        assert!(!g1.is_null());
        assert!(!g2.is_null());
        assert_ne!(g1, g2, "each server should get a unique group");

        // Store both groups
        NET_SERVER_GROUPS.with(|g| {
            let mut map = g.borrow_mut();
            map.insert(g1 as usize, unsafe { Box::from_raw(g1) });
            map.insert(g2 as usize, unsafe { Box::from_raw(g2) });
            assert_eq!(map.len(), 2);
        });

        // Clean up via NetCleanup
        let cleanup = NetCleanup;
        drop(cleanup);
        NET_SERVER_GROUPS.with(|g| assert!(g.borrow().is_empty()));
    }
}
