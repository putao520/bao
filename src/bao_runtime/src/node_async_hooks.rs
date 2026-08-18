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

// ── real hook registry (audit item 4) ──
// createHook callbacks actually fire. Minimal-but-correct per Node docs:
//   - init(asyncId, type, triggerAsyncId, resource) fires when a tracked
//     resource is constructed — wired to timers (setTimeout / setInterval /
//     setImmediate, type 'Timeout'/'Immediate') and Promise construction
//     (type 'PROMISE').
//   - destroy(asyncId) fires when the resource is destroyed — deterministic
//     for timers (after a one-shot callback runs, or on clearTimeout /
//     clearInterval). For PROMISE, Node itself ties destroy to GC (timing
//     explicitly not guaranteed by the docs); we do not fabricate it.
//   - promiseResolve fires when a promise created via the patched
//     constructor resolves.
// executionAsyncId/triggerAsyncId stay 0 (no engine-level execution context).
var _hooks = [];
var _nextAsyncId = 1;

function _emitInit(asyncId, type, triggerAsyncId, resource) {
  for (var i = 0; i < _hooks.length; i++) {
    if (typeof _hooks[i].init === 'function') {
      _hooks[i].init(asyncId, type, triggerAsyncId || 0, resource);
    }
  }
}

function _emitDestroy(asyncId) {
  for (var i = 0; i < _hooks.length; i++) {
    if (typeof _hooks[i].destroy === 'function') {
      _hooks[i].destroy(asyncId);
    }
  }
}

function _emitPromiseResolve(asyncId) {
  for (var i = 0; i < _hooks.length; i++) {
    if (typeof _hooks[i].promiseResolve === 'function') {
      _hooks[i].promiseResolve(asyncId);
    }
  }
}

function createHook(hook) {
  if (hook && typeof hook === 'object') {
    var validKeys = ['init', 'before', 'after', 'destroy', 'promiseResolve'];
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
      if (!enabled) {
        enabled = true;
        _hooks.push(hook);
      }
      return this;
    },
    disable: function() {
      if (enabled) {
        enabled = false;
        var idx = _hooks.indexOf(hook);
        if (idx !== -1) _hooks.splice(idx, 1);
      }
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

// ── emit wiring: timers ──
// Wraps the timer constructors on BOTH surfaces that exist by the time this
// module installs: the global functions (timers::install_timer_globals runs
// in install_web_apis, before install_node_apis) and the `timers` builtin
// module object (node_timers_module::install runs just before this install).
// One-shot timers (setTimeout / setImmediate) emit destroy after the wrapped
// callback runs; interval destroy fires on clearInterval. clearTimeout /
// clearInterval emit destroy for still-live ids. Double emission is guarded
// by the handle → asyncId map.
var _timerAsyncIds = (typeof Map === 'function') ? new Map() : null;

function _timerDestroyed(handle, asyncId) {
  if (_timerAsyncIds) {
    if (_timerAsyncIds.get(handle) !== asyncId) return false;
    _timerAsyncIds.delete(handle);
  }
  return true;
}

function _wrapTimerConstructor(original, type, oneShot) {
  if (typeof original !== 'function') return original;
  // Never double-wrap: aliasing surfaces (window.setTimeout etc.) that
  // already carry our wrapper keep identity with the wrapped global.
  if (original.__baoAsyncHookWrapped) return original;
  var wrapper = function(fn) {
    if (typeof fn !== 'function') return original.apply(this, arguments);
    var args = Array.prototype.slice.call(arguments);
    var asyncId = _nextAsyncId++;
    _emitInit(asyncId, type, 0, { type: type, oneShot: oneShot });
    var wrapped = function() {
      var r = fn.apply(this, arguments);
      if (oneShot && handle !== undefined && _timerDestroyed(handle, asyncId)) {
        _emitDestroy(asyncId);
      }
      return r;
    };
    var handle = original.apply(this, [wrapped].concat(args.slice(1)));
    if (handle !== undefined && handle !== null && _timerAsyncIds) {
      _timerAsyncIds.set(handle, asyncId);
    }
    return handle;
  };
  try {
    Object.defineProperty(wrapper, '__baoAsyncHookWrapped', {
      value: true, writable: false, enumerable: false, configurable: false
    });
  } catch (e) { /* non-configurable re-run — leave as is */ }
  return wrapper;
}

function _wrapTimerClear(original) {
  if (typeof original !== 'function') return original;
  if (original.__baoAsyncHookWrapped) return original;
  var wrapper = function(handle) {
    if (_timerAsyncIds && _timerAsyncIds.has(handle)) {
      var asyncId = _timerAsyncIds.get(handle);
      if (_timerDestroyed(handle, asyncId)) _emitDestroy(asyncId);
    }
    return original.apply(this, arguments);
  };
  try {
    Object.defineProperty(wrapper, '__baoAsyncHookWrapped', {
      value: true, writable: false, enumerable: false, configurable: false
    });
  } catch (e) { /* non-configurable re-run — leave as is */ }
  return wrapper;
}

(function _wireTimerHost(host) {
  if (!host) return;
  host.setTimeout = _wrapTimerConstructor(host.setTimeout, 'Timeout', true);
  host.setInterval = _wrapTimerConstructor(host.setInterval, 'Timeout', false);
  host.setImmediate = _wrapTimerConstructor(host.setImmediate, 'Immediate', true);
  host.clearTimeout = _wrapTimerClear(host.clearTimeout);
  host.clearInterval = _wrapTimerClear(host.clearInterval);
  host.clearImmediate = _wrapTimerClear(host.clearImmediate);
})(_g);

// NOTE: no `window` wiring — in CLI mode `window` is a throw-gate Proxy
// (web_api browser-gate), not a timer alias; in browser mode page globals
// are a separate realm installed before this module. The tagged
// double-wrap guard above keeps alias surfaces that share the SAME function
// object identity-stable.

(function _wireTimersModule() {
  if (typeof _g.require !== 'function') return;
  var timersMod = null;
  try { timersMod = _g.require('timers'); } catch (e) { return; }
  if (!timersMod || timersMod.setTimeout === _g.setTimeout) return;
  timersMod.setTimeout = _wrapTimerConstructor(timersMod.setTimeout, 'Timeout', true);
  timersMod.setInterval = _wrapTimerConstructor(timersMod.setInterval, 'Timeout', false);
  timersMod.setImmediate = _wrapTimerConstructor(timersMod.setImmediate, 'Immediate', true);
  timersMod.clearTimeout = _wrapTimerClear(timersMod.clearTimeout);
  timersMod.clearInterval = _wrapTimerClear(timersMod.clearInterval);
  timersMod.clearImmediate = _wrapTimerClear(timersMod.clearImmediate);
})();

// ── emit wiring: Promise ──
// Replaces globalThis.Promise with an IDENTITY-PRESERVING constructor: a
// plain function whose .prototype IS the original Promise.prototype and
// whose statics resolve through a prototype-chain link onto the original
// constructor. A `class Promise extends P` subclass broke the reverse
// direction — engine-created promises (async functions, await, intrinsics)
// kept the ORIGINAL prototype, so `p instanceof Promise` and
// `Object.getPrototypeOf(p) === Promise.prototype` were both false against
// the patched global (async-function realm split symptom). With the shared
// prototype, identity holds in BOTH directions: patched constructions
// return real original instances, and engine-created promises match the
// patched global too.
// NO reaction is attached inside the constructor — attaching one would mark
// every rejection "handled" and suppress the unhandledRejection router.
// promiseResolve is observed by wrapping `then` on the SHARED prototype
// (reaction attached only when the user attaches one; untracked promises
// pass through untouched).
var _OrigPromise = _g.Promise;
if (typeof _OrigPromise === 'function') {
  // WeakMap: entries die with the promise (no GC pinning).
  var _prIds = (typeof WeakMap === 'function') ? new WeakMap() : null;
  var _PatchedPromise = (function(P) {
    function Promise(executor) {
      var asyncId = _nextAsyncId++;
      _emitInit(asyncId, 'PROMISE', 0, P);
      var inst = new P(executor);
      if (_prIds) _prIds.set(inst, asyncId);
      return inst;
    }
    // Identity contract: same prototype object ⇒ `p instanceof Promise`
    // and prototype-chain comparisons hold for BOTH patched constructions
    // and engine-created (async/await intrinsic) promises.
    Promise.prototype = P.prototype;
    Promise.prototype.constructor = Promise;
    // Statics (resolve/reject/all/allSettled/any/race, Symbol.species)
    // resolve through the constructor prototype chain; species lookups
    // land on %Promise.prototype% via our shared prototype, so every
    // derived construction is a real original instance.
    Object.setPrototypeOf(Promise, P);
    return Promise;
  })(_OrigPromise);
  // promiseResolve: piggyback on the user's own fulfilled reactions.
  // Wrapped on the shared prototype so engine-created promises (which the
  // old subclass-arm never saw) also report promiseResolve once tracked.
  var _origThen = _OrigPromise.prototype.then;
  _OrigPromise.prototype.then = function(onFulfilled, onRejected) {
    var asyncId = _prIds ? _prIds.get(this) : undefined;
    var wrappedFulfill = (typeof onFulfilled === 'function' && asyncId !== undefined)
      ? function(v) { _emitPromiseResolve(asyncId); return onFulfilled(v); }
      : onFulfilled;
    return _origThen.call(this, wrappedFulfill, onRejected);
  };
  _g.Promise = _PatchedPromise;
}

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
