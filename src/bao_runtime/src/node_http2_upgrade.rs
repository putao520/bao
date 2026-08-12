// @trace REQ-ENG-006 [api:node:_http2_upgrade]
// HTTP/2 upgrade handler — creates a TLS-like socket wrapper for H2 upgrade
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// JS IIFE implementing _http2_upgrade module.
/// Exports: createHttp2UpgradeHandler, upgradeRawSocketToH2
const HTTP2_UPGRADE_JS: &str = r#"
(function() {
  // Minimal EventEmitter for internal use
  function EE() { this._events = Object.create(null); }
  EE.prototype.on = function(ev, fn) {
    (this._events[ev] || (this._events[ev] = [])).push(fn);
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
    var idx = list.indexOf(fn);
    if (idx >= 0) list.splice(idx, 1);
    return this;
  };

  // DuplexStream — minimal stream that wraps a raw socket for H2 upgrade
  function DuplexStream(rawSocket) {
    EE.call(this);
    this._rawSocket = rawSocket;
    this._reading = false;
    this._ended = false;
    this._destroyed = false;
    this._paused = false;
    this._buffer = [];

    // Copy address info from raw socket
    if (rawSocket && rawSocket.remoteAddress !== undefined) {
      this.remoteAddress = rawSocket.remoteAddress;
      this.remotePort = rawSocket.remotePort;
      this.localAddress = rawSocket.localAddress;
      this.localPort = rawSocket.localPort;
    }
  }
  DuplexStream.prototype = Object.create(EE.prototype);
  DuplexStream.prototype.constructor = DuplexStream;

  DuplexStream.prototype.push = function(chunk) {
    if (this._destroyed) return false;
    if (chunk === null) {
      this._ended = true;
      this.emit('end');
      return false;
    }
    if (this._paused) {
      this._buffer.push(chunk);
      return false;
    }
    this.emit('data', chunk);
    return true;
  };

  DuplexStream.prototype.read = function() {
    if (this._buffer.length > 0) return this._buffer.shift();
    return null;
  };

  DuplexStream.prototype.write = function(data) {
    if (this._destroyed) return false;
    if (this._rawSocket && typeof this._rawSocket.write === 'function') {
      return this._rawSocket.write(data);
    }
    this.emit('data', data);
    return true;
  };

  DuplexStream.prototype.end = function() {
    if (this._destroyed) return this;
    this._ended = true;
    if (this._rawSocket && typeof this._rawSocket.end === 'function') {
      this._rawSocket.end();
    }
    this.emit('finish');
    this.emit('end');
    return this;
  };

  DuplexStream.prototype.destroy = function(err) {
    if (this._destroyed) return this;
    this._destroyed = true;
    if (this._rawSocket && typeof this._rawSocket.destroy === 'function') {
      this._rawSocket.destroy();
    }
    if (err) this.emit('error', err);
    this.emit('close');
    return this;
  };

  DuplexStream.prototype.pipe = function(dest) {
    this.on('data', function(chunk) { dest.write(chunk); });
    this.on('end', function() { dest.end(); });
    this.on('error', function(err) { dest.emit('error', err); });
    return dest;
  };

  DuplexStream.prototype.pause = function() {
    this._paused = true;
    return this;
  };

  DuplexStream.prototype.resume = function() {
    this._paused = false;
    while (this._buffer.length > 0 && !this._paused) {
      this.emit('data', this._buffer.shift());
    }
    return this;
  };

  // createHttp2UpgradeHandler(options)
  // Returns a function that can be used as an HTTP upgrade handler for H2.
  // The returned function receives (request, socket, head) and creates a
  // TLS-like socket wrapper that the HTTP/2 session can use.
  function createHttp2UpgradeHandler(options) {
    options = options || {};
    var allowHTTP1 = options.allowHTTP1 || false;

    return function handleUpgrade(request, rawSocket, head) {
      // Create a TLS-like socket wrapper around the raw socket.
      // CRITICAL: store native handle in _ctx.nativeHandle NOT _handle
      // to prevent H2FrameParser from bypassing our wrapper.
      var tlsSocket = new DuplexStream(rawSocket);

      // Store the raw socket's native handle in _ctx.nativeHandle
      // (NOT _handle — this is the key difference from a regular TLS socket)
      if (rawSocket && rawSocket._ctx && rawSocket._ctx.nativeHandle) {
        tlsSocket._ctx = { nativeHandle: rawSocket._ctx.nativeHandle };
      } else if (rawSocket && rawSocket._handle) {
        tlsSocket._ctx = { nativeHandle: rawSocket._handle };
      } else {
        tlsSocket._ctx = {};
      }

      // TLS-like properties
      tlsSocket.encrypted = true;
      tlsSocket.authorized = false;
      tlsSocket.alpnProtocol = 'h2';
      tlsSocket.protocol = 'h2';
      tlsSocket.getProtocol = function() { return 'h2'; };
      tlsSocket.getSession = function() { return null; };
      tlsSocket.isSessionReused = function() { return false; };
      tlsSocket.getPeerCertificate = function() { return {}; };
      tlsSocket.getCipher = function() {
        return {
          name: 'TLS_AES_256_GCM_SHA384',
          standardName: 'TLS_AES_256_GCM_SHA384',
          version: 'TLSv1.3'
        };
      };

      // Forward raw socket events to the TLS wrapper
      if (rawSocket) {
        rawSocket.on('data', function(chunk) {
          if (!tlsSocket._destroyed) tlsSocket.emit('data', chunk);
        });
        rawSocket.on('end', function() {
          if (!tlsSocket._destroyed) tlsSocket.emit('end');
        });
        rawSocket.on('error', function(err) {
          if (!tlsSocket._destroyed) tlsSocket.emit('error', err);
        });
        rawSocket.on('close', function() {
          if (!tlsSocket._destroyed) tlsSocket.destroy();
        });
        rawSocket.on('timeout', function() {
          tlsSocket.emit('timeout');
        });
        rawSocket.on('drain', function() {
          tlsSocket.emit('drain');
        });
      }

      return tlsSocket;
    };
  }

  // upgradeRawSocketToH2(connectionListener, server, rawSocket)
  // Convenience function: wraps rawSocket in a TLS-like socket and calls
  // the connectionListener with it, enabling HTTP/2 on a raw TCP connection.
  function upgradeRawSocketToH2(connectionListener, server, rawSocket) {
    var handler = createHttp2UpgradeHandler({});
    var tlsSocket = handler(null, rawSocket, null);
    if (typeof connectionListener === 'function') {
      connectionListener(tlsSocket);
    }
    return tlsSocket;
  }

  return {
    createHttp2UpgradeHandler: createHttp2UpgradeHandler,
    upgradeRawSocketToH2: upgradeRawSocketToH2
  };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let cx_raw = unsafe { cx.raw_cx() };
    unsafe {
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c"_http2_upgrade".as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(HTTP2_UPGRADE_JS);
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

        rooted!(&in(cx) let module_obj = w2::JS_NewPlainObject(cx));
        if module_obj.get().is_null() {
            return;
        }

        let exports = ["createHttp2UpgradeHandler", "upgradeRawSocketToH2"];
        for name in &exports {
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
                    module_obj.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "_http2_upgrade", module_obj.get());
    }
}
