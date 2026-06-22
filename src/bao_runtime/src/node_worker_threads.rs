// @trace REQ-ENG-006 [api:node:worker_threads]
//
// Node.js `worker_threads` builtin module — JS source evaluation.
//
// Bao is a single-process runtime; Worker construction throws NOT_SUPPORTED.
// MessageChannel / MessagePort / BroadcastChannel delegate to globalThis
// when available (servo provides them), otherwise stub.
// isMainThread = true, threadId = 0, workerData = null, parentPort = null
// are the correct values for the main thread in a single-process runtime.

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use ::std::ptr::NonNull;
use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let source = r#"(function() {
var environmentData = new Map();

// SHARE_ENV symbol
var SHARE_ENV = Symbol('nodejs.worker_threads.SHARE_ENV');

// Worker class stub — throws on construction
function Worker(filename, options) {
  throw new Error('Worker is not supported in Bao (single-process runtime)');
}

// Add EventEmitter-like methods to Worker prototype
Worker.prototype.ref = function() {};
Worker.prototype.unref = function() {};
Worker.prototype.terminate = function() { return Promise.resolve(0); };
Worker.prototype.postMessage = function() {};
Worker.prototype.threadId = 0;

// MessageChannel: use globalThis.MessageChannel if available
var MessageChannel = (typeof globalThis.MessageChannel === 'function')
  ? globalThis.MessageChannel
  : function MessageChannel() { this.port1 = {}; this.port2 = {}; };

// MessagePort: use globalThis.MessagePort if available, otherwise stub
var MessagePort = (typeof globalThis.MessagePort === 'function')
  ? globalThis.MessagePort
  : function MessagePort() {};

// BroadcastChannel: use globalThis.BroadcastChannel if available, otherwise stub
var BroadcastChannel = (typeof globalThis.BroadcastChannel === 'function')
  ? globalThis.BroadcastChannel
  : function BroadcastChannel(name) { this.name = name; };
BroadcastChannel.prototype.postMessage = function() {};
BroadcastChannel.prototype.close = function() {};

// Inject EventEmitter-style methods on MessagePort prototype if available
if (typeof globalThis.MessagePort === 'function') {
  var MPProto = globalThis.MessagePort.prototype;
  if (MPProto && !MPProto.on) {
    MPProto.on = function(event, listener) {
      this.addEventListener(event, listener);
      return this;
    };
    MPProto.off = function(event, listener) {
      this.removeEventListener(event, listener);
      return this;
    };
    MPProto.once = function(event, listener) {
      this.addEventListener(event, listener, { once: true });
      return this;
    };
    MPProto.emit = function(event) {
      var args = [];
      for (var i = 1; i < arguments.length; i++) args.push(arguments[i]);
      var ev;
      if (event === 'message') {
        ev = new MessageEvent('message', { data: args[0] });
      } else if (event === 'messageerror') {
        ev = new ErrorEvent('messageerror', { message: args[0] });
      } else {
        ev = new Event(event);
      }
      this.dispatchEvent(ev);
      return this;
    };
    MPProto.prependListener = MPProto.on;
    MPProto.prependOnceListener = MPProto.once;
  }
}

function getEnvironmentData(key) {
  return environmentData.get(key);
}

function setEnvironmentData(key, value) {
  if (value === undefined) {
    environmentData.delete(key);
  } else {
    environmentData.set(key, value);
  }
}

function getHeapSnapshot() {
  return {};
}

function markAsUntransferable() {
  throw new Error('markAsUntransferable is not implemented in Bao');
}

function moveMessagePortToContext() {
  throw new Error('moveMessagePortToContext is not implemented in Bao');
}

function receiveMessageOnPort(port) {
  return undefined;
}

return {
  Worker: Worker,
  MessageChannel: MessageChannel,
  MessagePort: MessagePort,
  BroadcastChannel: BroadcastChannel,
  isMainThread: true,
  threadId: 0,
  workerData: null,
  parentPort: null,
  resourceLimits: {},
  SHARE_ENV: SHARE_ENV,
  getEnvironmentData: getEnvironmentData,
  setEnvironmentData: setEnvironmentData,
  getHeapSnapshot: getHeapSnapshot,
  markAsUntransferable: markAsUntransferable,
  moveMessagePortToContext: moveMessagePortToContext,
  receiveMessageOnPort: receiveMessageOnPort
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
    let opts = mozjs::glue::NewCompileOptions(raw_cx, c"<node:worker_threads>".as_ptr(), 1);
    if !opts.is_null() {
        let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
        libc::free(opts as *mut _);
        if ok && rval.is_object() {
            let obj = rval.to_object();
            cache_builtin(cx, "worker_threads", obj);
        }
    }
    }
}
