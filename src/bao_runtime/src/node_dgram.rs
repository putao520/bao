// @trace REQ-ENG-006 [api:node:dgram]
// Node.js dgram (UDP) module with real UDP via std::net::UdpSocket.
// Two-layer architecture: native __dgram_* functions on global + JS IIFE wrapper.
// Uses non-blocking UdpSocket with JS setInterval polling for recv.
// uws_sys::udp deferred (requires bao_uloop POLL_TYPE_UDP dispatch).

use bun_core::ZBox;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, ObjectValue, Int32Value, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// UDP socket registry: fd -> UdpSocket
static UDP_REGISTRY: ::std::sync::OnceLock<::std::sync::Mutex<::std::collections::HashMap<i32, ::std::net::UdpSocket>>> =
    ::std::sync::OnceLock::new();

fn registry() -> &'static ::std::sync::Mutex<::std::collections::HashMap<i32, ::std::net::UdpSocket>> {
    UDP_REGISTRY.get_or_init(|| ::std::sync::Mutex::new(::std::collections::HashMap::new()))
}

// ── Native __dgram_bind ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_bind(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let port: u16 = if argc > 0 { (*args.get(0).ptr).to_int32() as u16 } else { 0 };
    let addr_str = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_string() { crate::js_to_rust_string(cx, v) } else { "0.0.0.0".to_string() }
    } else {
        "0.0.0.0".to_string()
    };

    let bind_addr = format!("{}:{}", addr_str, port);
    match ::std::net::UdpSocket::bind(&bind_addr) {
        Ok(sock) => {
            let _ = sock.set_nonblocking(true);
            let fd = sock.as_raw_fd();
            let local = sock.local_addr().unwrap_or_else(|_| "::std::net::SocketAddr::from(([0,0,0,0], 0))".parse().unwrap());
            registry().lock().unwrap().insert(fd, sock);
            let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(cx_ref) let ret = w2::JS_NewPlainObject(&mut cx_ref));
            unsafe {
                rooted!(&in(cx_ref) let fd_val = Int32Value(fd));
                w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"fd".as_ptr(), fd_val.handle().into(), JSPROP_ENUMERATE as u32);
                rooted!(&in(cx_ref) let js_port = Int32Value(local.port() as i32));
                w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"port".as_ptr(), js_port.handle().into(), JSPROP_ENUMERATE as u32);
            }
            *vp = ObjectValue(ret.get());
            true
        }
        Err(e) => {
            let msg = format!("dgram bind failed: {}", e);
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr() as *const i8);
            false
        }
    }
}

use ::std::os::unix::io::AsRawFd;

// ── Native __dgram_send_buf ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_send_buf(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 4 {
        JS_ReportErrorUTF8(cx, c"__dgram_send_buf requires (fd, dataArr, port, address)".as_ptr());
        return false;
    }
    let fd = (*args.get(0).ptr).to_int32();
    let data_val = *args.get(1).ptr;
    let port: u16 = (*args.get(2).ptr).to_int32() as u16;
    let addr_str = crate::js_to_rust_string(cx, *args.get(3).ptr);

    if !data_val.is_object() {
        JS_ReportErrorUTF8(cx, c"data must be an array".as_ptr());
        return false;
    }

    let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let data_obj = data_val.to_object());

    // Extract byte array from JS Array
    let mut arr_len: u32 = 0;
    if !w2::GetArrayLength(&mut cx_ref, data_obj.handle().into(), &mut arr_len) {
        JS_ReportErrorUTF8(cx, c"failed to get array length".as_ptr());
        return false;
    }
    let mut buf = Vec::with_capacity(arr_len as usize);
    for i in 0..arr_len {
        let mut elem = UndefinedValue();
        JS_GetElement(cx, data_obj.handle().into(), i, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut elem,
        });
        buf.push(elem.to_int32() as u8);
    }

    let reg = registry().lock().unwrap();
    match reg.get(&fd) {
        Some(sock) => {
            let target = format!("{}:{}", addr_str, port);
            match sock.send_to(&buf, &target) {
                Ok(n) => { *vp = Int32Value(n as i32); true }
                Err(e) => {
                    let msg = format!("send_to failed: {}", e);
                    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr() as *const i8);
                    false
                }
            }
        }
        None => {
            JS_ReportErrorUTF8(cx, c"socket fd not found in registry".as_ptr());
            false
        }
    }
}

// ── Native __dgram_recv ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_recv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        *vp = UndefinedValue();
        return true;
    }
    let fd = (*args.get(0).ptr).to_int32();
    let buf_size: usize = if argc > 1 { (*args.get(1).ptr).to_int32() as usize } else { 65536 };

    let reg = registry().lock().unwrap();
    match reg.get(&fd) {
        Some(sock) => {
            let mut buf = vec![0u8; buf_size];
            match sock.recv_from(&mut buf) {
                Ok((len, addr)) => {
                    drop(reg);
                    let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                    rooted!(&in(cx_ref) let ret = w2::JS_NewPlainObject(&mut cx_ref));
                    // data as JS array
                    rooted!(&in(cx_ref) let data_arr = w2::NewArrayObject1(&mut cx_ref, len));
                    for i in 0..len {
                        rooted!(&in(cx_ref) let byte_val = Int32Value(buf[i] as i32));
                        JS_DefineElement(cx, data_arr.handle().into(), i as u32, byte_val.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                    unsafe {
                        rooted!(&in(cx_ref) let data_val = ObjectValue(data_arr.get()));
                        w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"data".as_ptr(), data_val.handle().into(), JSPROP_ENUMERATE as u32);
                        let ip = addr.ip().to_string();
                        let c_ip = ZBox::from_bytes(ip.as_bytes());
                        let js_ip = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                        if !js_ip.is_null() {
                            rooted!(&in(cx_ref) let ip_val = StringValue(&*js_ip));
                            w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"address".as_ptr(), ip_val.handle().into(), JSPROP_ENUMERATE as u32);
                        }
                        let family = if addr.is_ipv6() { "IPv6" } else { "IPv4" };
                        let c_fam = ZBox::from_bytes(family.as_bytes());
                        let js_fam = JS_NewStringCopyZ(cx, c_fam.as_ptr());
                        if !js_fam.is_null() {
                            rooted!(&in(cx_ref) let fam_val = StringValue(&*js_fam));
                            w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"family".as_ptr(), fam_val.handle().into(), JSPROP_ENUMERATE as u32);
                        }
                        rooted!(&in(cx_ref) let port_val = Int32Value(addr.port() as i32));
                        w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"port".as_ptr(), port_val.handle().into(), JSPROP_ENUMERATE as u32);
                    }
                    *vp = ObjectValue(ret.get());
                    true
                }
                Err(ref e) if e.kind() == ::std::io::ErrorKind::WouldBlock => {
                    // No data available yet
                    *vp = UndefinedValue();
                    true
                }
                Err(_) => {
                    *vp = UndefinedValue();
                    true
                }
            }
        }
        None => {
            *vp = UndefinedValue();
            true
        }
    }
}

// ── Native __dgram_connect ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 { JS_ReportErrorUTF8(cx, c"__dgram_connect requires (fd, port, address)".as_ptr()); return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let port: u16 = (*args.get(1).ptr).to_int32() as u16;
    let addr_str = crate::js_to_rust_string(cx, *args.get(2).ptr);
    let target = format!("{}:{}", addr_str, port);
    let reg = registry().lock().unwrap();
    match reg.get(&fd) {
        Some(sock) => match sock.connect(&target) {
            Ok(()) => { *vp = Int32Value(1); true }
            Err(e) => { let msg = format!("connect failed: {}", e); JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr() as *const i8); false }
        },
        None => { JS_ReportErrorUTF8(cx, c"socket fd not found".as_ptr()); false }
    }
}

// ── Native __dgram_disconnect ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_disconnect(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let reg = registry().lock().unwrap();
    if let Some(sock) = reg.get(&fd) {
        // disconnect by connecting to 0.0.0.0:0
        let _ = sock.connect("0.0.0.0:0");
    }
    *vp = Int32Value(1);
    true
}

// ── Native __dgram_close ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_close(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    registry().lock().unwrap().remove(&fd);
    *vp = Int32Value(1);
    true
}

// ── Native __dgram_set_broadcast ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_set_broadcast(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let on = (*args.get(1).ptr).to_int32() != 0;
    let reg = registry().lock().unwrap();
    if let Some(sock) = reg.get(&fd) { let _ = sock.set_broadcast(on); }
    *vp = Int32Value(1);
    true
}

// ── Native __dgram_set_ttl ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_set_ttl(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let ttl = (*args.get(1).ptr).to_int32();
    let reg = registry().lock().unwrap();
    if let Some(sock) = reg.get(&fd) { let _ = sock.set_ttl(ttl as u32); }
    *vp = Int32Value(1);
    true
}

// ── Native __dgram_set_multicast_ttl ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_set_multicast_ttl(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let ttl = (*args.get(1).ptr).to_int32();
    let reg = registry().lock().unwrap();
    if let Some(sock) = reg.get(&fd) { let _ = sock.set_multicast_ttl_v4(ttl as u32); }
    *vp = Int32Value(1);
    true
}

// ── Native __dgram_set_multicast_loopback ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_set_multicast_loopback(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let on = (*args.get(1).ptr).to_int32() != 0;
    let reg = registry().lock().unwrap();
    if let Some(sock) = reg.get(&fd) { let _ = sock.set_multicast_loop_v4(on); }
    *vp = Int32Value(1);
    true
}

// ── Native __dgram_add_membership ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_add_membership(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let maddr = crate::js_to_rust_string(cx, *args.get(1).ptr);
    let iface = if argc > 2 { crate::js_to_rust_string(cx, *args.get(2).ptr) } else { "0.0.0.0".to_string() };
    let reg = registry().lock().unwrap();
    match reg.get(&fd) {
        Some(sock) => {
            match maddr.parse::<::std::net::Ipv4Addr>() {
                Ok(ip) if ip.is_multicast() => {
                    match iface.parse::<::std::net::Ipv4Addr>() {
                        Ok(iface_ip) => { let _ = sock.join_multicast_v4(&ip, &iface_ip); *vp = Int32Value(1); true }
                        Err(_) => { JS_ReportErrorUTF8(cx, c"invalid interface address".as_ptr()); false }
                    }
                }
                _ => { JS_ReportErrorUTF8(cx, c"invalid multicast address".as_ptr()); false }
            }
        }
        None => { JS_ReportErrorUTF8(cx, c"socket fd not found".as_ptr()); false }
    }
}

// ── Native __dgram_drop_membership ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_drop_membership(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let maddr = crate::js_to_rust_string(cx, *args.get(1).ptr);
    let iface = if argc > 2 { crate::js_to_rust_string(cx, *args.get(2).ptr) } else { "0.0.0.0".to_string() };
    let reg = registry().lock().unwrap();
    match reg.get(&fd) {
        Some(sock) => {
            match maddr.parse::<::std::net::Ipv4Addr>() {
                Ok(ip) if ip.is_multicast() => {
                    match iface.parse::<::std::net::Ipv4Addr>() {
                        Ok(iface_ip) => { let _ = sock.leave_multicast_v4(&ip, &iface_ip); *vp = Int32Value(1); true }
                        Err(_) => { JS_ReportErrorUTF8(cx, c"invalid interface address".as_ptr()); false }
                    }
                }
                _ => { JS_ReportErrorUTF8(cx, c"invalid multicast address".as_ptr()); false }
            }
        }
        None => { JS_ReportErrorUTF8(cx, c"socket fd not found".as_ptr()); false }
    }
}

// ── Native __dgram_address ──
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dgram_address(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 { return false; }
    let fd = (*args.get(0).ptr).to_int32();
    let reg = registry().lock().unwrap();
    match reg.get(&fd) {
        Some(sock) => match sock.local_addr() {
            Ok(addr) => {
                drop(reg);
                let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
                rooted!(&in(cx_ref) let ret = w2::JS_NewPlainObject(&mut cx_ref));
                let ip = addr.ip().to_string();
                let c_ip = ZBox::from_bytes(ip.as_bytes());
                let js_ip = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                if !js_ip.is_null() {
                    rooted!(&in(cx_ref) let ip_val = StringValue(&*js_ip));
                    unsafe { w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"address".as_ptr(), ip_val.handle().into(), JSPROP_ENUMERATE as u32); }
                }
                let family = if addr.is_ipv6() { "IPv6" } else { "IPv4" };
                let c_fam = ZBox::from_bytes(family.as_bytes());
                let js_fam = JS_NewStringCopyZ(cx, c_fam.as_ptr());
                if !js_fam.is_null() {
                    rooted!(&in(cx_ref) let fam_val = StringValue(&*js_fam));
                    unsafe { w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"family".as_ptr(), fam_val.handle().into(), JSPROP_ENUMERATE as u32); }
                }
                rooted!(&in(cx_ref) let port_val = Int32Value(addr.port() as i32));
                unsafe { w2::JS_DefineProperty(&mut cx_ref, ret.handle().into(), c"port".as_ptr(), port_val.handle().into(), JSPROP_ENUMERATE as u32); }
                *vp = ObjectValue(ret.get());
                true
            }
            Err(_) => { *vp = UndefinedValue(); true }
        },
        None => { *vp = UndefinedValue(); true }
    }
}

const DGRAM_JS: &str = r#"
(function() {
  var EventEmitter;
  if (typeof require === 'function') {
    try { EventEmitter = require('events').EventEmitter; } catch(e) {}
  }
  if (!EventEmitter) {
    EventEmitter = function EE() { this._events = {}; };
    EventEmitter.prototype.on = function(ev, fn) { (this._events[ev] || (this._events[ev] = [])).push(fn); return this; };
    EventEmitter.prototype.emit = function(ev) {
      var a = Array.prototype.slice.call(arguments, 1);
      var l = this._events[ev]; if (l) { for (var i = 0; i < l.length; i++) l[i].apply(this, a); } return !!l;
    };
    EventEmitter.prototype.removeListener = function(ev, fn) {
      var l = this._events[ev]; if (l) { var i = l.indexOf(fn); if (i >= 0) l.splice(i, 1); } return this;
    };
    EventEmitter.prototype.once = function(ev, fn) {
      var self = this; function w() { fn.apply(this, arguments); self.removeListener(ev, w); }
      this.on(ev, w); return this;
    };
  }

  function Socket(type, listener) {
    if (!(this instanceof Socket)) return new Socket(type, listener);
    EventEmitter.call(this);
    this.type = (typeof type === 'object') ? (type.type || 'udp4') : (type || 'udp4');
    this._fd = -1;
    this._connected = false;
    this._recvTimer = null;
    this._destroyed = false;
    if (typeof listener === 'function') this.on('message', listener);
  }
  Socket.prototype = Object.create(EventEmitter.prototype);
  Socket.prototype.constructor = Socket;

  Socket.prototype.bind = function(port, addr, cb) {
    if (this._fd >= 0) { var e = new Error('Already bound'); if (cb) cb(e); this.emit('error', e); return this; }
    var res = __dgram_bind(port || 0, addr || '0.0.0.0');
    if (res && typeof res === 'object') {
      this._fd = res.fd;
      var self = this;
      this._recvTimer = setInterval(function() {
        if (self._destroyed || self._fd < 0) return;
        var msg = __dgram_recv(self._fd, 65536);
        if (msg && typeof msg === 'object' && msg.data) {
          var buf = Buffer.from(msg.data);
          self.emit('message', buf, { address: msg.address, family: msg.family, port: msg.port });
        }
      }, 1);
      if (cb) cb(null);
      this.emit('listening');
    } else {
      var e = new Error('bind failed');
      if (cb) cb(e);
      this.emit('error', e);
    }
    return this;
  };

  Socket.prototype.connect = function(port, addr, cb) {
    if (this._fd < 0) { var e = new Error('Not bound'); if (cb) cb(e); this.emit('error', e); return this; }
    __dgram_connect(this._fd, port, addr || '127.0.0.1');
    this._connected = true;
    if (cb) cb(null);
    this.emit('connect');
    return this;
  };

  Socket.prototype.disconnect = function() {
    if (this._fd < 0) return;
    __dgram_disconnect(this._fd);
    this._connected = false;
  };

  Socket.prototype.send = function(buf, off, len, port, addr, cb) {
    if (this._fd < 0) { var e = new Error('Not bound'); if (cb) cb(e); return this; }
    var arr;
    if (Buffer.isBuffer(buf)) {
      arr = []; for (var i = off || 0; i < (len || buf.length); i++) arr.push(buf[i]);
    } else if (Array.isArray(buf)) {
      arr = buf;
    } else {
      arr = []; for (var j = 0; j < buf.length; j++) arr.push(buf.charCodeAt(j));
    }
    try {
      __dgram_send_buf(this._fd, arr, port, addr);
      if (cb) cb(null);
    } catch(e) { if (cb) cb(e); }
    return this;
  };

  Socket.prototype.close = function(cb) {
    if (this._destroyed) return this;
    this._destroyed = true;
    if (this._recvTimer) { clearInterval(this._recvTimer); this._recvTimer = null; }
    if (this._fd >= 0) { __dgram_close(this._fd); this._fd = -1; }
    if (cb) cb();
    this.emit('close');
    return this;
  };

  Socket.prototype.address = function() {
    if (this._fd < 0) return {};
    return __dgram_address(this._fd) || {};
  };

  Socket.prototype.setBroadcast = function(on) { if (this._fd >= 0) __dgram_set_broadcast(this._fd, on ? 1 : 0); return this; };
  Socket.prototype.setTTL = function(ttl) { if (this._fd >= 0) __dgram_set_ttl(this._fd, ttl); return this; };
  Socket.prototype.setMulticastTTL = function(ttl) { if (this._fd >= 0) __dgram_set_multicast_ttl(this._fd, ttl); return this; };
  Socket.prototype.setMulticastLoopback = function(on) { if (this._fd >= 0) __dgram_set_multicast_loopback(this._fd, on ? 1 : 0); return this; };
  Socket.prototype.addMembership = function(maddr, iface) { if (this._fd >= 0) __dgram_add_membership(this._fd, maddr, iface || '0.0.0.0'); };
  Socket.prototype.dropMembership = function(maddr, iface) { if (this._fd >= 0) __dgram_drop_membership(this._fd, maddr, iface || '0.0.0.0'); };
  Socket.prototype.ref = function() { return this; };
  Socket.prototype.unref = function() { return this; };
  Socket.prototype.setRecvBufferSize = function(s) {};
  Socket.prototype.setSendBufferSize = function(s) {};
  Socket.prototype.getRecvBufferSize = function() { return 1 << 19; };
  Socket.prototype.getSendBufferSize = function() { return 1 << 19; };
  Socket.prototype.getSendQueueSize = function() { return 0; };
  Socket.prototype.getSendQueueCount = function() { return 0; };
  Socket.prototype.remoteAddress = function() { return undefined; };

  function createSocket(type, listener) { return new Socket(type, listener); }

  return { createSocket: createSocket, Socket: Socket };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let cx_raw = unsafe { cx.raw_cx() };
    unsafe {
        // Register native __dgram_* helpers on global object
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let native_fns: &[(&str, u32, JSNative)] = &[
                ("__dgram_bind", 2, Some(dgram_bind)),
                ("__dgram_send_buf", 4, Some(dgram_send_buf)),
                ("__dgram_recv", 2, Some(dgram_recv)),
                ("__dgram_connect", 3, Some(dgram_connect)),
                ("__dgram_disconnect", 1, Some(dgram_disconnect)),
                ("__dgram_close", 1, Some(dgram_close)),
                ("__dgram_set_broadcast", 2, Some(dgram_set_broadcast)),
                ("__dgram_set_ttl", 2, Some(dgram_set_ttl)),
                ("__dgram_set_multicast_ttl", 2, Some(dgram_set_multicast_ttl)),
                ("__dgram_set_multicast_loopback", 2, Some(dgram_set_multicast_loopback)),
                ("__dgram_add_membership", 3, Some(dgram_add_membership)),
                ("__dgram_drop_membership", 3, Some(dgram_drop_membership)),
                ("__dgram_address", 1, Some(dgram_address)),
            ];
            for (name, nargs, fn_ptr) in native_fns {
                let c_name = ZBox::from_bytes(name.as_bytes());
                JS_DefineFunction(cx_raw, global_root.handle().into(), c_name.as_ptr(), *fn_ptr, *nargs, 0);
            }
        }

        // Evaluate IIFE
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c"<node:dgram>".as_ptr(), 1);
        if opts.is_null() { return; }
        let mut src = mozjs::rust::transform_str_to_source_text(DGRAM_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);
        if ok && rval.is_object() {
            cache_builtin(cx, "dgram", rval.to_object());
            return;
        }
    }
    // Fallback
    rooted!(&in(cx) let fallback = unsafe { w2::JS_NewPlainObject(cx) });
    if !fallback.get().is_null() {
        cache_builtin(cx, "dgram", fallback.get());
    }
}
