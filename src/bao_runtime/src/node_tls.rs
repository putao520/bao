use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const TLS_JS: &str = r#"
(function() {
  function TLSSocket(socket, options) {
    this._socket = socket;
    this.authorized = false;
    this.encrypted = true;
  }
  TLSSocket.prototype.write = function(data, cb) {
    if (this._socket) return this._socket.write(data, cb);
    if (cb) cb();
  };
  TLSSocket.prototype.end = function(data, cb) {
    if (this._socket) return this._socket.end(data, cb);
    if (cb) cb();
  };
  TLSSocket.prototype.destroy = function() {
    if (this._socket) this._socket.destroy();
  };
  TLSSocket.prototype.on = function(ev, fn) {
    if (this._socket) this._socket.on(ev, fn);
    return this;
  };
  TLSSocket.prototype.once = function(ev, fn) {
    if (this._socket) this._socket.once(ev, fn);
    return this;
  };
  TLSSocket.prototype.emit = function() {
    if (this._socket) this._socket.emit.apply(this._socket, arguments);
    return true;
  };
  TLSSocket.prototype.removeListener = function(ev, fn) {
    if (this._socket) this._socket.removeListener(ev, fn);
    return this;
  };
  TLSSocket.prototype.getProtocol = function() { return "TLSv1.3"; };
  TLSSocket.prototype.getCipher = function() {
    return { name: "TLS_AES_256_GCM_SHA384", version: "TLSv1/SSLv3" };
  };
  TLSSocket.prototype.getPeerCertificate = function() {
    return { subject: null, issuer: null, valid_from: "", valid_to: "", fingerprint: "" };
  };

  function SecureContext() {}
  SecureContext.prototype.setKey = function() {};
  SecureContext.prototype.setCert = function() {};
  SecureContext.prototype.addCACert = function() {};
  SecureContext.prototype.setCA = function() {};

  return {
    TLSSocket: TLSSocket,
    SecureContext: SecureContext,
    createSecureContext: function() { return new SecureContext(); },
    connect: function(options, cb) {
      var net = require('net');
      var socket = net.connect(options);
      var tlsSocket = new TLSSocket(socket, options);
      if (cb) cb(tlsSocket);
      return tlsSocket;
    },
    createServer: function(options, connListener) {
      var net = require('net');
      return net.createServer(function(socket) {
        var tlsSocket = new TLSSocket(socket, options);
        tlsSocket.authorized = true;
        if (connListener) connListener(tlsSocket);
      });
    },
    getCiphers: function() {
      return ["TLS_AES_256_GCM_SHA384","TLS_CHACHA20_POLY1305_SHA256",
              "TLS_AES_128_GCM_SHA256","ECDHE-RSA-AES256-GCM-SHA384"];
    },
    DEFAULT_CIPHERS: "TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256",
    DEFAULT_MIN_VERSION: "TLSv1.2",
    DEFAULT_MAX_VERSION: "TLSv1.3"
  };
})()
"#;

pub fn install_tls(cx: &mut mozjs::context::JSContext) {
    unsafe {
        let raw = cx.raw_cx();
        let c_filename = CString::new("<node:tls>").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }
        let mut src = mozjs::rust::transform_str_to_source_text(TLS_JS);
        let mut js_val = UndefinedValue();
        let js_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut js_val,
        };
        if JS::Evaluate2(raw, opts, &mut src, js_h) && js_val.is_object() {
            cache_builtin(cx, "tls", js_val.to_object());
        }
        libc::free(opts as *mut _);
    }
}
