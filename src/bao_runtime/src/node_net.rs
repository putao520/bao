use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::net::{TcpListener, TcpStream};
use ::std::os::unix::io::AsRawFd;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

thread_local! {
    static NET_SERVERS: RefCell<Vec<*mut TcpListener>> = RefCell::new(Vec::new());
    static NET_SOCKETS: RefCell<Vec<*mut TcpStream>> = RefCell::new(Vec::new());
}

pub struct NetCleanup;

impl Drop for NetCleanup {
    fn drop(&mut self) {
        NET_SERVERS.with(|s| {
            for ptr in s.borrow_mut().drain(..) {
                unsafe { drop(Box::from_raw(ptr)); }
            }
        });
        NET_SOCKETS.with(|s| {
            for ptr in s.borrow_mut().drain(..) {
                unsafe { drop(Box::from_raw(ptr)); }
            }
        });
    }
}

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
    this._fd = -1;
  }
  Socket.prototype = Object.create(EE.prototype);
  Socket.prototype.constructor = Socket;
  Socket.prototype.connect = function(port, host, cb) {
    if (typeof host === "function") { cb = host; host = "127.0.0.1"; }
    if (!host) host = "127.0.0.1";
    this.connecting = true;
    if (typeof __net_connect === "function") {
      var fd = __net_connect(port, host);
      if (fd >= 0) {
        this._fd = fd;
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
    if (this.destroyed || this._fd < 0) return false;
    if (typeof __net_write === "function") {
      return __net_write(this._fd, data) >= 0;
    }
    return false;
  };
  Socket.prototype.end = function(data) {
    if (data) this.write(data);
    this.destroyed = true;
    if (typeof __net_close === "function") {
      __net_close(this._fd);
    }
    this._fd = -1;
    this.emit("end");
    this.emit("close");
    return this;
  };
  Socket.prototype.destroy = function() {
    if (this.destroyed) return this;
    this.destroyed = true;
    if (this._fd >= 0 && typeof __net_close === "function") {
      __net_close(this._fd);
    }
    this._fd = -1;
    this.emit("close");
    return this;
  };

  function Server(opts, connectionListener) {
    if (typeof opts === "function") { connectionListener = opts; opts = null; }
    EE.call(this);
    this.listening = false;
    this._fd = -1;
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
      var fd = __net_listen(port, host);
      if (fd >= 0) {
        this._fd = fd;
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
    if (this._fd >= 0 && typeof __net_close === "function") {
      __net_close(this._fd);
    }
    this._fd = -1;
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

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port = if argc > 0 { (*args.get(0).ptr).to_int32() as u16 } else { 0 };
    let addr = if argc > 1 && (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        "0.0.0.0".to_string()
    };

    match TcpListener::bind((addr.as_str(), port)) {
        Ok(listener) => {
            let fd = listener.as_raw_fd();
            NET_SERVERS.with(|s| s.borrow_mut().push(Box::into_raw(Box::new(listener))));
            args.rval().set(Int32Value(fd as i32));
        }
        Err(_) => {
            args.rval().set(Int32Value(-1));
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port = if argc > 0 { (*args.get(0).ptr).to_int32() as u16 } else { 0 };
    let addr = if argc > 1 && (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        "127.0.0.1".to_string()
    };

    match TcpStream::connect((addr.as_str(), port)) {
        Ok(stream) => {
            let fd = stream.as_raw_fd();
            NET_SOCKETS.with(|s| s.borrow_mut().push(Box::into_raw(Box::new(stream))));
            args.rval().set(Int32Value(fd as i32));
        }
        Err(_) => {
            args.rval().set(Int32Value(-1));
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

    let fd = (*args.get(0).ptr).to_int32();
    let data = if (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        String::new()
    };

    let written = NET_SOCKETS.with(|s| {
        for &ptr in s.borrow().iter() {
            let stream = &mut *ptr;
            if stream.as_raw_fd() == fd {
                return match ::std::io::Write::write(stream, data.as_bytes()) {
                    Ok(n) => n as i32,
                    Err(_) => -1,
                };
            }
        }
        -1
    });
    args.rval().set(Int32Value(written));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn net_close(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd = if argc > 0 { (*args.get(0).ptr).to_int32() } else { -1 };

    NET_SERVERS.with(|s| {
        s.borrow_mut().retain(|&ptr| {
            if (*ptr).as_raw_fd() == fd {
                drop(Box::from_raw(ptr));
                false
            } else {
                true
            }
        });
    });
    NET_SOCKETS.with(|s| {
        s.borrow_mut().retain(|&ptr| {
            if (*ptr).as_raw_fd() == fd {
                drop(Box::from_raw(ptr));
                false
            } else {
                true
            }
        });
    });
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

        let c_filename = CString::new("node:net").unwrap_or_default();
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
        let exports_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &exports_obj,
        };

        let mod_ptr2 = mod_obj.get();
        let mod_h2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr2 };

        for name in &["Socket", "Server", "createServer", "connect", "createConnection", "isIP", "isIPv4", "isIPv6"] {
            let cname = CString::new(*name).unwrap_or_default();
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

        cache_builtin("net", mod_obj.get());
    }
}
