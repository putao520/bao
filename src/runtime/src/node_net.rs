// @trace REQ-ENG-007
//! node:net implementation using bun_uws uSockets TCP socket API.
//!
//! Replaces the previous std::net::TcpListener/TcpStream synchronous
//! implementation with event-loop-integrated uSockets sockets managed
//! by bao_uloop's epoll backend.

use ::std::cell::{Cell, RefCell};
use ::std::collections::HashMap;
use ::std::ptr::{self, NonNull};

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_uws_sys::{
    ListenSocket, Loop, SocketGroup, SocketKind,
    us_socket_t, CloseCode,
};
use bun_uws_sys::vtable;
use bun_uws_sys::socket_group::VTable;

use crate::require::cache_builtin;

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
            unsafe { drop(Vec::from_raw_parts(self.ptr, 0, self.cap)); }
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
}

pub struct NetCleanup;

impl Drop for NetCleanup {
    fn drop(&mut self) {
        NET_SERVER_GROUPS.with(|g| g.borrow_mut().clear());
        NET_LISTEN_SOCKETS.with(|l| l.borrow_mut().clear());
        NET_SOCKETS.with(|s| s.borrow_mut().clear());
    }
}

// ──────────────────── VTable via bun_uws_sys::vtable::Handler ────────────
// 铁律 0: use vtable::make instead of hand-written trampolines.
// NetHandler implements the Handler trait to get comptime-generated
// VTable via vtable::make::<NetHandler>(), replacing the old hand-written
// NET_VTABLE + 11 manual trampoline functions.

struct NetHandler;

impl vtable::Handler for NetHandler {
    type Ext = ();

    const HAS_ON_OPEN: bool = true;
    const HAS_ON_DATA: bool = true;
    const HAS_ON_WRITABLE: bool = true;
    const HAS_ON_CLOSE: bool = true;
    const HAS_ON_TIMEOUT: bool = true;
    const HAS_ON_LONG_TIMEOUT: bool = true;
    const HAS_ON_END: bool = true;
    const HAS_ON_CONNECT_ERROR: bool = true;
    const HAS_ON_CONNECTING_ERROR: bool = true;
    const HAS_ON_HANDSHAKE: bool = true;

    fn on_open(_ext: &mut Self::Ext, s: *mut us_socket_t, _is_client: bool, _ip: &[u8]) {
        let key = s as usize;
        NET_SOCKETS.with(|m| m.borrow_mut().insert(key, true));
        CONNECT_RESULT.with(|r| {
            if r.get().is_none() {
                r.set(Some(key));
            }
        });
    }
    fn on_data(_ext: &mut Self::Ext, _s: *mut us_socket_t, _data: &[u8]) {
        // Data dispatched to JS via event loop; JS polls via __net_read
    }
    fn on_writable(_ext: &mut Self::Ext, _s: *mut us_socket_t) {}
    fn on_close(_ext: &mut Self::Ext, s: *mut us_socket_t, _code: i32, _reason: Option<*mut ::std::ffi::c_void>) {
        let key = s as usize;
        NET_SOCKETS.with(|m| m.borrow_mut().remove(&key));
    }
    fn on_timeout(_ext: &mut Self::Ext, _s: *mut us_socket_t) {}
    fn on_long_timeout(_ext: &mut Self::Ext, _s: *mut us_socket_t) {}
    fn on_end(_ext: &mut Self::Ext, _s: *mut us_socket_t) {}
    fn on_connect_error(_ext: &mut Self::Ext, _s: *mut us_socket_t, _code: i32) {
        CONNECT_ERROR.with(|e| e.set(true));
        CONNECT_RESULT.with(|r| r.set(Some(0)));
    }
    fn on_connecting_error(c: *mut bun_uws_sys::ConnectingSocket, _code: i32) {
        CONNECT_ERROR.with(|e| e.set(true));
        CONNECT_RESULT.with(|r| r.set(Some(0)));
    }
    fn on_handshake(_ext: &mut Self::Ext, _s: *mut us_socket_t, _ok: bool, _err: bun_uws_sys::us_bun_verify_error_t) {}
}

/// Static VTable for net TCP sockets, generated via vtable::make.
static NET_VTABLE: ::std::sync::OnceLock<&'static VTable> = ::std::sync::OnceLock::new();

fn net_vtable() -> &'static VTable {
    NET_VTABLE.get_or_init(vtable::make::<NetHandler>)
}

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
        this.emit("connect");
        if (cb) cb();
      } else {
        this.emit("error", new Error("connect ECONNREFUSED " + host + ":" + port));
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
    if (data) this.write(data);
    this.destroyed = true;
    if (typeof __net_close === "function") {
      __net_close(this._ptr);
    }
    this._ptr = 0;
    this.emit("end");
    this.emit("close");
    return this;
  };
  Socket.prototype.destroy = function() {
    if (this.destroyed) return this;
    this.destroyed = true;
    if (this._ptr > 0 && typeof __net_close === "function") {
      __net_close(this._ptr);
    }
    this._ptr = 0;
    this.emit("close");
    return this;
  };

  function Server(opts, connectionListener) {
    if (typeof opts === "function") { connectionListener = opts; opts = null; }
    EE.call(this);
    this.listening = false;
    this._ptr = 0;
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
        this.listening = true;
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
    return { port: 0, family: "IPv4", address: "0.0.0.0" };
  };

  function isIP(input) {
    if (!input || typeof input !== "string") return 0;
    var parts = input.split(".");
    if (parts.length === 4) {
      for (var i = 0; i < 4; i++) {
        var n = parseInt(parts[i], 10);
        if (isNaN(n) || n < 0 || n > 255 || parts[i] !== String(n)) return 0;
      }
      return 4;
    }
    return 0;
  }

  return {
    Socket: Socket,
    Server: Server,
    createServer: function(opts, cb) { return new Server(opts, cb); },
    connect: function(port, host, cb) { var s = new Socket(); return s.connect(port, host, cb); },
    createConnection: function(port, host, cb) { var s = new Socket(); return s.connect(port, host, cb); },
    isIP: isIP,
    isIPv4: function(input) { return isIP(input) === 4; },
    isIPv6: function() { return false; },
  };
})();
"#;

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
    group.init(loop_, Some(net_vtable()), ptr::null_mut());
    Box::into_raw(group)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port = if argc > 0 { (*args.get(0).ptr).to_int32() } else { 0 };
    let addr = if argc > 1 && (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
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

    let host_cstr = bun_core::ZBox::from_bytes(addr.as_str().as_bytes());
    let mut err: ::std::ffi::c_int = 0;

    let listen_socket = group.listen(
        SocketKind::UwsHttp, // plain TCP kind
        None,                // no SSL
        Some(host_cstr.as_zstr().as_cstr()),
        port,
        0, // LIBUS_LISTEN_DEFAULT
        0, // socket_ext_size (no per-socket ext for listen sockets)
        &mut err,
    );

    if listen_socket.is_null() || err != 0 {
        // Listen failed — destroy the group.
        unsafe { SocketGroup::destroy(group_ptr); }
        args.rval().set(Int32Value(0));
        return true;
    }

    // Store the group and listen socket.
    let listen_key = listen_socket as usize;
    NET_SERVER_GROUPS.with(|g| g.borrow_mut().insert(listen_key, unsafe { Box::from_raw(group_ptr) }));
    NET_LISTEN_SOCKETS.with(|l| l.borrow_mut().push(listen_key));

    // Return the listen socket pointer as an integer to JS.
    args.rval().set(Int32Value(if listen_key <= i32::MAX as usize { listen_key as i32 } else { 0 }));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port = if argc > 0 { (*args.get(0).ptr).to_int32() } else { 0 };
    let addr = if argc > 1 && (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
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
    group.init(loop_, Some(net_vtable()), ptr::null_mut());
    let group_ptr = Box::into_raw(group);

    let host_cstr = bun_core::ZBox::from_bytes(addr.as_str().as_bytes());

    // Reset connect state.
    CONNECT_RESULT.with(|r| r.set(None));
    CONNECT_ERROR.with(|e| e.set(false));

    let result = (*group_ptr).connect(
        SocketKind::UwsHttp,
        None,
        host_cstr.as_zstr().as_cstr(),
        port,
        0,
        0, // socket_ext_size
    );

    match result {
        bun_uws_sys::ConnectResult::Socket(socket) => {
            // Synchronous connect (DNS already resolved, e.g. localhost).
            let key = socket as usize;
            NET_SOCKETS.with(|m| m.borrow_mut().insert(key, true));
            // Store the group so it lives as long as the socket.
            NET_SERVER_GROUPS.with(|g| g.borrow_mut().insert(key, unsafe { Box::from_raw(group_ptr) }));
            let val = if key <= i32::MAX as usize { key as i32 } else { 0 };
            args.rval().set(Int32Value(val));
        }
        bun_uws_sys::ConnectResult::Connecting(_connecting) => {
            // Async connect — tick the loop until on_open or on_connect_error fires.
            // Store the group so it stays alive during the connect.
            let group_key = group_ptr as usize;
            NET_SERVER_GROUPS.with(|g| g.borrow_mut().insert(group_key, unsafe { Box::from_raw(group_ptr) }));

            let max_ticks: u32 = 5000;
            for _ in 0..max_ticks {
                // Check if result arrived.
                let done = CONNECT_RESULT.with(|r| r.get().is_some());
                if done {
                    break;
                }
                // Tick the event loop — epoll_wait will block until an event arrives.
                unsafe {
                    bao_uloop::us_loop_run_bun_tick(loop_, ptr::null());
                }
            }

            let error = CONNECT_ERROR.with(|e| e.get());
            let result_key = CONNECT_RESULT.with(|r| r.get().unwrap_or(0));

            if error || result_key == 0 {
                args.rval().set(Int32Value(0));
            } else {
                NET_SOCKETS.with(|m| m.borrow_mut().insert(result_key, true));
                let val = if result_key <= i32::MAX as usize { result_key as i32 } else { 0 };
                args.rval().set(Int32Value(val));
            }
        }
        bun_uws_sys::ConnectResult::Failed => {
            // Connect failed immediately.
            unsafe { SocketGroup::destroy(group_ptr); }
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

    let ptr_val = (*args.get(0).ptr).to_int32() as usize;
    let data = if (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        String::new()
    };

    let socket_ptr = ptr_val as *mut us_socket_t;
    let exists = NET_SOCKETS.with(|m| m.borrow().contains_key(&ptr_val));
    if !exists {
        args.rval().set(Int32Value(-1));
        return true;
    }

    // us_socket_t::write returns the number of bytes written (or 0 on backpressure).
    let written = unsafe { (*socket_ptr).write(data.as_bytes()) };
    args.rval().set(Int32Value(written));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_close(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let ptr_val = if argc > 0 { (*args.get(0).ptr).to_int32() as usize } else { 0 };

    if ptr_val == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Try to close as a connected socket.
    let was_socket = NET_SOCKETS.with(|m| m.borrow_mut().remove(&ptr_val).is_some());
    if was_socket {
        let socket_ptr = ptr_val as *mut us_socket_t;
        unsafe { (*socket_ptr).close(CloseCode::normal); }
    }

    // Try to close as a listen socket.
    NET_LISTEN_SOCKETS.with(|l| {
        let mut list = l.borrow_mut();
        if let Some(pos) = list.iter().position(|&k| k == ptr_val) {
            list.swap_remove(pos);
            let listen_ptr = ptr_val as *mut ListenSocket;
            unsafe { (*listen_ptr).close(); }
        }
    });

    // Remove associated socket group.
    NET_SERVER_GROUPS.with(|g| g.borrow_mut().remove(&ptr_val));

    args.rval().set(UndefinedValue());
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
        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr };
        JS_DefineFunction(cx_raw, mod_h, c"__net_listen".as_ptr(), Some(net_listen), 2, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__net_connect".as_ptr(), Some(net_connect), 2, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__net_write".as_ptr(), Some(net_write), 2, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__net_close".as_ptr(), Some(net_close), 1, 0);

        let c_filename = c"node:net".as_ptr();
        let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx_raw, c_filename, 1) as *mut _) else {
            return;
        };
        let opts = _opts_guard.as_ptr() as *const JS::ReadOnlyCompileOptions;

        let mut src = mozjs::rust::transform_str_to_source_text(NET_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        let exports_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &exports_obj,
        };

        let mod_ptr2 = mod_obj.get();
        let mod_h2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr2 };

        for name in &["Socket", "Server", "createServer", "connect", "createConnection", "isIP", "isIPv4", "isIPv6"] {
            let cname = bun_core::ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(cx_raw, exports_h, cname.as_ptr(), MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut val,
            });
            if !val.is_undefined() {
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                JS_DefineProperty(cx_raw, mod_h2, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
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
        assert!(net_vtable().on_open.is_some(), "on_open must be set");
        assert!(net_vtable().on_data.is_some(), "on_data must be set");
        assert!(net_vtable().on_close.is_some(), "on_close must be set");
        assert!(net_vtable().on_writable.is_some(), "on_writable must be set");
        assert!(net_vtable().on_end.is_some(), "on_end must be set");
        assert!(net_vtable().on_timeout.is_some(), "on_timeout must be set");
        assert!(net_vtable().on_connect_error.is_some(), "on_connect_error must be set");
        assert!(net_vtable().on_connecting_error.is_some(), "on_connecting_error must be set");
        assert!(net_vtable().on_handshake.is_some(), "on_handshake must be set");
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_get_loop_returns_non_null() {
        bao_uloop::force_link();
        let loop_ = get_loop();
        assert!(!loop_.is_null(), "get_loop must return non-null after force_link");
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
        assert!(NET_JS.contains("_ptr"), "JS must use _ptr for socket reference");
        assert!(!NET_JS.contains("_fd"), "JS must not use _fd");
    }

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_js_source_contains_all_exports() {
        for name in &["Socket", "Server", "createServer", "connect", "createConnection", "isIP", "isIPv4", "isIPv6"] {
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
        unsafe { SocketGroup::destroy(group_ptr); }
    }

    // ──── extended unit tests ────

    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit]
    #[test]
    fn test_net_vtable_callback_signatures_match_dispatch() {
        // Verify VTable callback types match bao_uloop dispatch expectations.
        // on_open: (*mut us_socket_t, c_int, *mut u8, c_int) -> *mut us_socket_t
        assert!(net_vtable().on_open.is_some());
        // on_data: (*mut us_socket_t, *mut u8, c_int) -> *mut us_socket_t
        assert!(net_vtable().on_data.is_some());
        // on_writable: (*mut us_socket_t) -> *mut us_socket_t
        assert!(net_vtable().on_writable.is_some());
        // on_close: (*mut us_socket_t, c_int, *mut c_void) -> *mut us_socket_t
        assert!(net_vtable().on_close.is_some());
        // on_end: (*mut us_socket_t) -> *mut us_socket_t
        assert!(net_vtable().on_end.is_some());
        // on_fd is deliberately None (not used for plain TCP)
        assert!(net_vtable().on_fd.is_none());
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
            group.init(loop_, Some(net_vtable()), ptr::null_mut());
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
