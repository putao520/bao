// @trace REQ-ENG-006 [api:node:dgram]
//
// Node.js dgram (UDP) module. Bao does not have native UDP support
// (no Bun.udpSocket() binding), so this module provides a structural
// surface that matches the Node.js dgram API shape:
//   - createSocket(type, listener) — returns a Socket instance
//   - Socket class — extends EventEmitter with bind/connect/send/close/
//     address/ref/unref/setBroadcast/setTTL/setMulticastTTL/
//     setMulticastLoopback/setMulticastInterface/addMembership/
//     dropMembership methods
//   - bind/connect/send throw NOT_SUPPORTED (no native UDP)
//   - close emits 'close' event
//   - Other methods are no-ops or throw NOT_SUPPORTED
//
// Registered via JS source evaluation matching Bun's approach
// (dgram.ts uses classes, closures, and complex state).

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let source = r#"(function() {
// Look up EventEmitter from the builtin cache
var EventEmitter;
if (typeof require === 'function') {
  try { EventEmitter = require('events').EventEmitter; } catch(e) {}
}
if (!EventEmitter) {
  // Minimal EventEmitter fallback
  EventEmitter = function EventEmitter() {
    this._events = {};
  };
  EventEmitter.prototype.on = function(event, listener) {
    if (!this._events[event]) this._events[event] = [];
    this._events[event].push(listener);
    return this;
  };
  EventEmitter.prototype.once = function(event, listener) {
    var self = this;
    function onceWrapper() {
      listener.apply(this, arguments);
      self.removeListener(event, onceWrapper);
    }
    this.on(event, onceWrapper);
    return this;
  };
  EventEmitter.prototype.emit = function(event) {
    var args = [];
    for (var i = 1; i < arguments.length; i++) args.push(arguments[i]);
    var listeners = this._events[event];
    if (listeners) {
      for (var j = 0; j < listeners.length; j++) {
        listeners[j].apply(this, args);
      }
    }
    return !!(listeners && listeners.length);
  };
  EventEmitter.prototype.removeListener = function(event, listener) {
    var listeners = this._events[event];
    if (listeners) {
      this._events[event] = listeners.filter(function(l) { return l !== listener; });
    }
    return this;
  };
  EventEmitter.prototype.removeAllListeners = function(event) {
    if (event) { delete this._events[event]; }
    else { this._events = {}; }
    return this;
  };
}

var BIND_STATE_UNBOUND = 0;
var BIND_STATE_BINDING = 1;
var BIND_STATE_BOUND = 2;

function notImplemented(method) {
  throw new Error('dgram.' + method + ' not implemented in Bao (no native UDP support)');
}

function Socket(type, listener) {
  if (!(this instanceof Socket)) return new Socket(type, listener);
  EventEmitter.call(this);

  if (typeof type === 'object') {
    this.type = type.type || 'udp4';
  } else {
    this.type = type || 'udp4';
  }

  this._bindState = BIND_STATE_UNBOUND;
  this._connectState = 0;

  if (typeof listener === 'function') {
    this.on('message', listener);
  }
}

// Inherit from EventEmitter
Socket.prototype = Object.create(EventEmitter.prototype);
Socket.prototype.constructor = Socket;

Socket.prototype.bind = function bind(port_, address_, callback) {
  if (this._bindState !== BIND_STATE_UNBOUND) {
    throw new Error('Socket is already bound');
  }
  this._bindState = BIND_STATE_BINDING;
  var self = this;
  // Bao does not have native UDP — emit error
  if (typeof callback === 'function') {
    callback(new Error('dgram.bind is not implemented in Bao'));
  }
  process.nextTick(function() {
    self.emit('error', new Error('dgram.bind is not implemented in Bao (no native UDP support)'));
  });
  return this;
};

Socket.prototype.connect = function connect(port, address, callback) {
  notImplemented('connect');
};

Socket.prototype.disconnect = function disconnect() {
  notImplemented('disconnect');
};

Socket.prototype.send = function send(buffer, offset, length, port, address, callback) {
  if (typeof callback === 'function') {
    callback(new Error('dgram.send is not implemented in Bao'));
  }
  notImplemented('send');
};

Socket.prototype.close = function close(callback) {
  var self = this;
  this._bindState = BIND_STATE_UNBOUND;
  if (typeof callback === 'function') {
    callback();
  }
  process.nextTick(function() {
    self.emit('close');
  });
  return this;
};

Socket.prototype.address = function address() {
  return { address: '0.0.0.0', family: this.type === 'udp6' ? 'IPv6' : 'IPv4', port: 0 };
};

Socket.prototype.remoteAddress = function remoteAddress() {
  return undefined;
};

Socket.prototype.setBroadcast = function setBroadcast(arg) {
  notImplemented('setBroadcast');
};

Socket.prototype.setTTL = function setTTL(ttl) {
  notImplemented('setTTL');
};

Socket.prototype.setMulticastTTL = function setMulticastTTL(ttl) {
  notImplemented('setMulticastTTL');
};

Socket.prototype.setMulticastLoopback = function setMulticastLoopback(arg) {
  notImplemented('setMulticastLoopback');
};

Socket.prototype.setMulticastInterface = function setMulticastInterface(interfaceAddress) {
  notImplemented('setMulticastInterface');
};

Socket.prototype.addMembership = function addMembership(multicastAddress, interfaceAddress) {
  notImplemented('addMembership');
};

Socket.prototype.dropMembership = function dropMembership(multicastAddress, interfaceAddress) {
  notImplemented('dropMembership');
};

Socket.prototype.addSourceSpecificMembership = function addSourceSpecificMembership(source, group, iface) {
  notImplemented('addSourceSpecificMembership');
};

Socket.prototype.dropSourceSpecificMembership = function dropSourceSpecificMembership(source, group, iface) {
  notImplemented('dropSourceSpecificMembership');
};

Socket.prototype.ref = function ref() { return this; };
Socket.prototype.unref = function unref() { return this; };

Socket.prototype.setRecvBufferSize = function setRecvBufferSize(size) {};
Socket.prototype.setSendBufferSize = function setSendBufferSize(size) {};
Socket.prototype.getRecvBufferSize = function getRecvBufferSize() { return 1 << 19; };
Socket.prototype.getSendBufferSize = function getSendBufferSize() { return 1 << 19; };
Socket.prototype.getSendQueueSize = function getSendQueueSize() { return 0; };
Socket.prototype.getSendQueueCount = function getSendQueueCount() { return 0; };

function createSocket(type, listener) {
  return new Socket(type, listener);
}

return {
  createSocket: createSocket,
  Socket: Socket
};
})()"#;

    unsafe {
    let raw_cx = cx.raw_cx();
    let mut source_text = mozjs::rust::transform_str_to_source_text(source);
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let opts = mozjs::glue::NewCompileOptions(raw_cx, c"<node:dgram>".as_ptr(), 1);
    if !opts.is_null() {
        let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
        libc::free(opts as *mut _);
        if ok && rval.is_object() {
            let obj = rval.to_object();
            cache_builtin(cx, "dgram", obj);
            return;
        }
    }
    } // end unsafe
    // Fallback: register empty object so require() doesn't throw
    rooted!(&in(cx) let fallback = unsafe { w2::JS_NewPlainObject(cx) });
    if !fallback.get().is_null() {
        cache_builtin(cx, "dgram", fallback.get());
    }
}
