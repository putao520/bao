// @trace REQ-ENG-006 [api:node:async_hooks]
//
// Node.js async_hooks module — simplified compatibility implementation.
//
// Since Bao uses SpiderMonkey (not JSC), we cannot use JSC's `$asyncContext`
// internal field. Instead, this implements a simplified version:
// - AsyncLocalStorage uses a simple context-stack-based tracking (global array)
// - AsyncResource.runInAsyncScope just calls the function directly
// - createHook returns a stub that never fires callbacks
// - executionAsyncId/triggerAsyncId return 0
//
// The module is registered via JS source evaluation (like Bun does with its
// async_hooks.ts), since the module has classes (AsyncLocalStorage,
// AsyncResource), closures, and complex state management that would be
// extremely verbose in Rust C API calls.

use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let source = r#"(function() {
var warnedExecutionAsyncId = false;
var warnedExecutionAsyncResource = false;
var warnedCreateHook = false;

// Simplified async context tracking (no JSC $asyncContext)
// Uses a global stack of context maps
var contextStack = [{}];

function getCurrentContext() {
  return contextStack[contextStack.length - 1];
}

function AsyncLocalStorage() {
  this._key = Symbol('AsyncLocalStorage');
  this._enabled = true;
}

AsyncLocalStorage.bind = function bind(fn) {
  if (typeof fn !== 'function') {
    throw new TypeError('"fn" argument must be a function');
  }
  var store = this.getStore();
  var self = this;
  return function() {
    return self.run(store, fn, this, arguments);
  };
};

AsyncLocalStorage.snapshot = function snapshot() {
  var self = this;
  var store = this.getStore();
  return function(fn) {
    return self.run(store, fn, this, arguments);
  };
};

AsyncLocalStorage.prototype.enterWith = function enterWith(store) {
  this._enabled = true;
  var ctx = getCurrentContext();
  var newCtx = {};
  for (var k in ctx) { if (Object.prototype.hasOwnProperty.call(ctx, k)) newCtx[k] = ctx[k]; }
  newCtx[this._key] = store;
  contextStack.push(newCtx);
};

AsyncLocalStorage.prototype.exit = function exit(callback) {
  var args = [];
  for (var i = 1; i < arguments.length; i++) args.push(arguments[i]);
  return this.run(undefined, callback, undefined, args);
};

AsyncLocalStorage.prototype.run = function run(store_value, callback, thisArg, args) {
  if (typeof callback !== 'function') {
    throw new TypeError('"callback" argument must be a function');
  }
  this._enabled = true;
  var ctx = getCurrentContext();
  var newCtx = {};
  for (var k in ctx) { if (Object.prototype.hasOwnProperty.call(ctx, k)) newCtx[k] = ctx[k]; }
  if (store_value === undefined) {
    delete newCtx[this._key];
  } else {
    newCtx[this._key] = store_value;
  }
  contextStack.push(newCtx);
  try {
    if (Array.isArray(args)) {
      return callback.apply(thisArg, args);
    }
    return callback.call(thisArg);
  } finally {
    contextStack.pop();
  }
};

AsyncLocalStorage.prototype.disable = function disable() {
  this._enabled = false;
};

AsyncLocalStorage.prototype.getStore = function getStore() {
  if (!this._enabled) return undefined;
  var ctx = getCurrentContext();
  if (ctx && Object.prototype.hasOwnProperty.call(ctx, this._key)) {
    return ctx[this._key];
  }
  return undefined;
};

AsyncLocalStorage.prototype._enable = function _enable() {};
AsyncLocalStorage.prototype._propagate = function _propagate() {};

function AsyncResource(type, opts) {
  if (typeof type !== 'string') {
    throw new TypeError('The "type" argument must be of type string');
  }
  this.type = type;
  this._triggerAsyncId = (opts && typeof opts === 'object' && typeof opts.triggerAsyncId === 'number')
    ? opts.triggerAsyncId
    : (typeof opts === 'number' ? opts : 0);
}

AsyncResource.prototype.emitBefore = function emitBefore() { return true; };
AsyncResource.prototype.emitAfter = function emitAfter() { return true; };
AsyncResource.prototype.asyncId = function asyncId() { return 0; };
AsyncResource.prototype.triggerAsyncId = function triggerAsyncId() { return this._triggerAsyncId; };
AsyncResource.prototype.emitDestroy = function emitDestroy() {};

AsyncResource.prototype.runInAsyncScope = function runInAsyncScope(fn, thisArg) {
  if (typeof fn !== 'function') {
    throw new TypeError('"fn" argument must be a function');
  }
  var args = [];
  for (var i = 2; i < arguments.length; i++) args.push(arguments[i]);
  return fn.apply(thisArg, args);
};

AsyncResource.prototype.bind = function bind(fn, thisArg) {
  if (typeof fn !== 'function') {
    throw new TypeError('"fn" argument must be a function');
  }
  var self = this;
  return function() {
    return self.runInAsyncScope(fn, thisArg, arguments);
  };
};

AsyncResource.bind = function bind(fn, type, thisArg) {
  if (typeof fn !== 'function') {
    throw new TypeError('"fn" argument must be a function');
  }
  var res = new AsyncResource(type || 'bound');
  return res.bind(fn, thisArg);
};

function createHook(hook) {
  if (hook && typeof hook === 'object') {
    var validKeys = ['init', 'before', 'after', 'destroy', 'promiseResolve'];
    for (var k in hook) {
      if (Object.prototype.hasOwnProperty.call(hook, k) && validKeys.indexOf(k) === -1) {
        // Unknown key — ignore
      }
    }
    for (var i = 0; i < validKeys.length; i++) {
      var key = validKeys[i];
      if (hook[key] !== undefined && typeof hook[key] !== 'function') {
        throw new TypeError('hook.' + key + ' must be a function');
      }
    }
  }
  var enabled = false;
  return {
    enable: function() {
      if (!enabled && !warnedCreateHook) {
        warnedCreateHook = true;
        if (typeof process !== 'undefined' && process.emitWarning) {
          process.emitWarning('async_hooks.createHook is not fully implemented in Bao; hooks will not be called');
        }
      }
      enabled = true;
      return this;
    },
    disable: function() {
      enabled = false;
      return this;
    }
  };
}

function executionAsyncId() {
  if (!warnedExecutionAsyncId) {
    warnedExecutionAsyncId = true;
    if (typeof process !== 'undefined' && process.emitWarning) {
      process.emitWarning('async_hooks.executionAsyncId is not implemented; always returns 0');
    }
  }
  return 0;
}

function triggerAsyncId() {
  return 0;
}

function executionAsyncResource() {
  if (!warnedExecutionAsyncResource) {
    warnedExecutionAsyncResource = true;
    if (typeof process !== 'undefined' && process.emitWarning) {
      process.emitWarning('async_hooks.executionAsyncResource is not implemented');
    }
  }
  if (typeof process !== 'undefined' && process.stdin) return process.stdin;
  return {};
}

// asyncWrapProviders — provider name → numeric ID mapping
var asyncWrapProviders = {
  NONE: 0, BINDWRAP: 1, BUILTIN: 2, CALLBACK: 3, DNSCHANNEL: 4,
  FSREQCALLBACK: 5, GETADDRINFOREQWRAP: 6, GETNAMEINFOREQWRAP: 7,
  HTTPINCOMINGMESSAGE: 8, HTTPCLIENTREQUEST: 9, JSSTREAM: 10,
  PIPECONNECTWRAP: 11, PIPEWRAP: 12, PROCESSWRAP: 13,
  QUERYWRAP: 14, SHUTDOWNWRAP: 15, SIGNALWRAP: 16,
  STATWATCHER: 17, TCPCONNECTWRAP: 18, TCPWRAP: 19,
  TIMERWRAP: 20, TLSWRAP: 21, TTYWRAP: 22, UDPSENDWRAP: 23,
  UDPWRAP: 24, SIGINTWATCHDOG: 25, WORKERHEAPSNAPSHOT: 26,
  FSREQCALLBACKIMPORT: 27, FSEVENTWRAP: 28, DIRHANDLE: 29,
  FILEHANDLE: 30, FILEHANDLECLOSEREQ: 31, HEAPSNAPSHOTREQUEST: 32,
  STREAMPIPE: 33, CONNECTIONWRAP: 34, ZLIB: 35,
  CHECKPRIMEREQUEST: 36, PBKDF2REQUEST: 37, KEYPAIRGENREQUEST: 38,
  KEYGENREQUEST: 39, KEYEXPORTREQUEST: 40, CIPHERREQUEST: 41,
  DIGESTREQUEST: 42, SIGNREQUEST: 43, VERIFYREQUEST: 44,
  SCRYPTREQUEST: 45, HKDFREQUEST: 46, RANDOMBYTESREQUEST: 47,
  TLSWRAPREQUEUE: 48, MICROTASK: 49, PROMISE: 50,
  TTYWRAPFD: 51, HTTP2SESSION: 52, HTTP2STREAM: 53,
  HTTP2PING: 54, HTTP2SETTINGS: 55, HTTP2STREAMSESSION: 56,
  INSPECTORJSBINDING: 57
};

return {
  AsyncLocalStorage: AsyncLocalStorage,
  AsyncResource: AsyncResource,
  createHook: createHook,
  executionAsyncId: executionAsyncId,
  triggerAsyncId: triggerAsyncId,
  executionAsyncResource: executionAsyncResource,
  asyncWrapProviders: asyncWrapProviders
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
        let opts = mozjs::glue::NewCompileOptions(raw_cx, c"<node:async_hooks>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            libc::free(opts as *mut _);
            if ok && rval.is_object() {
                let obj = rval.to_object();
                cache_builtin(cx, "async_hooks", obj);
            }
        }
    }
}
