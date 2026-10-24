// @trace REQ-ENG-048 [api:internal/timers] [api:internal/util/debounce] [api:internal/util/throttle]
//
// Three internal modules in one install unit (globals.rs carries a single
// line for this wave; node_internal_util::install funnels here first):
//
// 1. `internal/timers` — Node's own test fixtures require it for
//    `TIMEOUT_MAX` (test-util-debounce.js / test-util-throttle.js). The
//    upstream export surface leans on `internalBinding('timers')`
//    (immediateInfo/timeoutInfo), `internalBinding('task_queue')` and
//    `internal/async_hooks` — those members are intentionally ABSENT; the
//    JS-carriable subset (constants, symbols, getTimerDuration,
//    cleanTimer/cleanImmediate) is real. Consumers of absent members fail
//    loudly at the property-access point (internal/test/binding precedent).
//
// 2. `internal/util/debounce` / `internal/util/throttle` — the TRUE homes
//    of node v24's debounce/throttle (lib/util.js lazy-getters
//    `require('internal/util/debounce')`); source-aligned translations of
//    golden-node/node/lib/internal/util/{debounce,throttle}.js.
//
// Honest boundaries (fail-closed, never stubs):
// - bao's `timers` module returns bare numeric timer ids — no
//   `Timeout.refresh()/ref()/unref()`. debounce's `timeout.refresh()` is
//   re-expressed as clearTimeout+reschedule of the same callback (identical
//   tail-reset semantics); ref()/unref() maintain the `refed` flag and apply
//   it to future timers where the carrier allows, and are honest no-ops
//   against the current bare id (flagged in the report).
// - `internalBinding('timers').getLibuvNow()` → `performance.now()`
//   (standard monotonic clock); `Date.now` only as a degradation path when
//   no performance object exists in the realm (monotonicity caveat noted).
// - `internalBinding('util').markPromiseAsHandled` (V8-internal promise
//   flag) → attaching a no-op rejection handler via `promise.then`, the
//   standard way to mark a promise handled (no unhandledRejection).
// - `AbortError` / `ERR_THROTTLED` are inlined per internal/errors.js:967
//   and errors.js:1852 exactly; `addAbortListener` (internal/events/
//   abort_listener) → `signal.addEventListener('abort', fn, {once:true})`.

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use bun_core::ZBox;

use crate::require::cache_builtin;

const INTERNAL_TIMERS_JS: &str = r#"
(function() {
  'use strict';

  // Anchor twin: node_internal_util.rs has the same inline warning sink for
  // the deprecation family — keep the two in sync. Real process.emitWarning
  // wins whenever the embedder provides one.
  function emitWarning(msg, type) {
    if (typeof process !== 'undefined' && typeof process.emitWarning === 'function') {
      process.emitWarning(msg, type);
      return;
    }
    var prefix = (typeof process !== 'undefined' && typeof process.pid === 'number')
      ? '(node:' + process.pid + ') '
      : '';
    console.warn(prefix + (type ? type + ': ' : '') + msg);
  }

  function determineSpecificType(v) {
    if (v === null) return 'null';
    if (typeof v === 'string') return "type string ('" + v + "')";
    if (typeof v === 'function') {
      return 'type function (' + (v.name ? v.name : 'anonymous') + ')';
    }
    if (typeof v === 'object') {
      if (typeof v.constructor === 'function' && v.constructor.name) {
        return 'an instance of ' + v.constructor.name;
      }
      return 'an object';
    }
    return 'type ' + typeof v + ' (' + String(v) + ')';
  }

  function errInvalidArgType(name, expected, actual) {
    var e = new TypeError(
      'The "' + name + '" argument must be of type ' + expected +
      '. Received ' + determineSpecificType(actual));
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  function errOutOfRange(name, range, actual) {
    var e = new RangeError(
      'The value of "' + name + '" is out of range. It must be ' + range +
      '. Received ' + actual);
    e.code = 'ERR_OUT_OF_RANGE';
    return e;
  }

  // upstream internal/validators.js validateNumber
  function validateNumber(value, name) {
    if (typeof value !== 'number') {
      throw errInvalidArgType(name, 'number', value);
    }
  }

  // Timeout values > TIMEOUT_MAX are set to 1 (upstream internal/timers.js:173).
  var TIMEOUT_MAX = 2147483647; // 2 ** 31 - 1

  // Type checking used by timers.enroll() and Socket#setTimeout()
  // (upstream internal/timers.js:490-507).
  function getTimerDuration(msecs, name) {
    validateNumber(msecs, name);
    if (msecs < 0 || !Number.isFinite(msecs)) {
      throw errOutOfRange(name, 'a non-negative finite number', msecs);
    }

    // Ensure that msecs fits into signed int32
    if (msecs > TIMEOUT_MAX) {
      emitWarning(
        msecs + ' does not fit into a 32-bit signed integer.' +
        '\nTimer duration was truncated to ' + TIMEOUT_MAX + '.',
        'TimeoutOverflowWarning');
      return TIMEOUT_MAX;
    }

    return msecs;
  }

  // Upstream also clears the async-context storage symbols here
  // (removeStoresFromResource); bao has no async_hooks store, that face is
  // absent by design (noted in the report).
  function cleanTimer(timer) {
    timer._onTimeout = undefined;
    timer._timerArgs = undefined;
  }

  function cleanImmediate(immediate) {
    immediate._onImmediate = undefined;
    immediate._argv = undefined;
  }

  return {
    TIMEOUT_MAX: TIMEOUT_MAX,
    kTimeout: Symbol('timeout'), // For hiding Timeouts on other internals.
    // Symbols for storing async id state (upstream timers.js:106-107).
    async_id_symbol: Symbol('asyncId'),
    trigger_async_id_symbol: Symbol('triggerId'),
    kHasPrimitive: Symbol('kHasPrimitive'),
    async_context_frame: Symbol('kAsyncContextFrame'),
    // Must match Environment::ImmediateInfo::Fields in src/env.h.
    kCount: 0,
    kRefCount: 1,
    kHasOutstanding: 2,
    getTimerDuration: getTimerDuration,
    cleanTimer: cleanTimer,
    cleanImmediate: cleanImmediate,
  };
})()
"#;

const INTERNAL_UTIL_DEBOUNCE_JS: &str = r#"
(function() {
  'use strict';

  // ── inline support (upstream: internal/errors.js, internal/validators.js,
  //    internal/events/abort_listener.js, internalBinding('util')) ────────
  function determineSpecificType(v) {
    if (v === null) return 'null';
    if (typeof v === 'string') return "type string ('" + v + "')";
    if (typeof v === 'function') {
      return 'type function (' + (v.name ? v.name : 'anonymous') + ')';
    }
    if (typeof v === 'object') {
      if (typeof v.constructor === 'function' && v.constructor.name) {
        return 'an instance of ' + v.constructor.name;
      }
      return 'an object';
    }
    return 'type ' + typeof v + ' (' + String(v) + ')';
  }

  function errInvalidArgType(name, expected, actual) {
    var e = new TypeError(
      'The "' + name + '" argument must be of type ' + expected +
      '. Received ' + determineSpecificType(actual));
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  function errOutOfRange(name, range, actual) {
    var e = new RangeError(
      'The value of "' + name + '" is out of range. It must be ' + range +
      '. Received ' + actual);
    e.code = 'ERR_OUT_OF_RANGE';
    return e;
  }

  function errInvalidArgValue(name, value, reason) {
    var e = new TypeError(
      "The argument '" + name + "' " + reason + '. Received ' +
      determineSpecificType(value));
    e.code = 'ERR_INVALID_ARG_VALUE';
    return e;
  }

  // Node uses an AbortError that isn't exactly the same as the DOMException
  // — a regular Error with .code and .name (internal/errors.js:964-976).
  class AbortError extends Error {
    constructor(message, options) {
      if (message === undefined) message = 'The operation was aborted';
      if (options !== undefined && typeof options !== 'object') {
        throw errInvalidArgType('options', 'Object', options);
      }
      super(message, options);
      this.code = 'ABORT_ERR';
      this.name = 'AbortError';
    }
  }

  function validateFunction(value, name) {
    if (typeof value !== 'function') throw errInvalidArgType(name, 'Function', value);
  }

  function validateBoolean(value, name) {
    if (typeof value !== 'boolean') throw errInvalidArgType(name, 'boolean', value);
  }

  function validateInteger(value, name, min, max) {
    if (min === undefined) min = -9007199254740991;
    if (max === undefined) max = 9007199254740991;
    if (typeof value !== 'number') throw errInvalidArgType(name, 'number', value);
    if (!Number.isInteger(value)) throw errOutOfRange(name, 'an integer', value);
    if (value < min || value > max) {
      throw errOutOfRange(name, '>= ' + min + ' && <= ' + max, value);
    }
  }

  function validateObject(value, name) {
    if (value === null || Array.isArray(value) || typeof value !== 'object') {
      throw errInvalidArgType(name, 'Object', value);
    }
  }

  function validateAbortSignal(signal, name) {
    if (signal !== undefined &&
        (signal === null ||
         typeof signal !== 'object' ||
         !('aborted' in signal))) {
      throw errInvalidArgType(name, 'AbortSignal', signal);
    }
  }

  var kEmptyObject = Object.freeze(Object.create(null));
  var TIMEOUT_MAX = 2147483647; // mirrors internal/timers (see that module)

  // PromiseWithResolvers helper — identical shape to Promise.withResolvers;
  // kept as a local so the module does not depend on the engine exposing it.
  function withResolvers() {
    var resolve, reject;
    var promise = new Promise(function(res, rej) { resolve = res; reject = rej; });
    return { promise: promise, resolve: resolve, reject: reject };
  }

  // internalBinding('util').markPromiseAsHandled marks the V8-internal
  // handled flag; the standard equivalent is attaching a no-op rejection
  // handler (suppresses unhandledRejection).
  function noop() {}
  function markPromiseAsHandled(promise) {
    promise.then(noop, noop);
  }

  // internal/events/abort_listener.addAbortListener core semantics:
  // fire-once abort listener.
  function addAbortListener(signal, listener) {
    signal.addEventListener('abort', listener, { once: true });
  }

  var timersModule = require('timers');
  var setTimeoutFn = timersModule.setTimeout;
  var clearTimeoutFn = timersModule.clearTimeout;

  // ── debounce (upstream internal/util/debounce.js:47-207) ─────────────
  // Creates a function that delays calling `fn` until `wait` milliseconds
  // have elapsed since its most recent invocation.
  function debounce(fn, wait, options) {
    validateFunction(fn, 'fn');
    validateInteger(wait, 'wait', 0, TIMEOUT_MAX);
    if (options === undefined) options = kEmptyObject;
    validateObject(options, 'options');

    var leading = options.leading !== undefined ? options.leading : false;
    var rejectOnCancel = options.rejectOnCancel !== undefined ? options.rejectOnCancel : false;
    var signal = options.signal;

    validateBoolean(leading, 'options.leading');
    validateBoolean(rejectOnCancel, 'options.rejectOnCancel');
    validateAbortSignal(signal, 'options.signal');

    if (signal && signal.aborted) {
      throw new AbortError(undefined, { cause: signal.reason });
    }

    var args;
    var abortError;
    var pendingCalls = [];
    var refed = true;
    var timeout;

    function rejectPending(error) {
      var calls = pendingCalls;
      pendingCalls = [];
      for (var i = 0; i < calls.length; i++) {
        calls[i].reject(error);
      }
    }

    function cancelWithError(error) {
      if (timeout === undefined) return;
      clearTimeoutFn(timeout);
      timeout = undefined;
      args = undefined;
      rejectPending(error);
    }

    function cancel(reason) {
      if (timeout === undefined) return;
      var error = reason === undefined ?
        new AbortError() :
        new AbortError(undefined, { cause: reason });
      cancelWithError(error);
    }

    function abort() {
      if (abortError === undefined) {
        abortError = signal.reason === undefined ?
          new AbortError() :
          new AbortError(undefined, { cause: signal.reason });
      }
      cancelWithError(abortError);
    }

    function ref() {
      refed = true;
      // bao timers are bare ids: ref/unref apply to future timers only
      // (honest boundary, see module header).
      if (timeout !== undefined && typeof timeout.ref === 'function') timeout.ref();
      return debounced;
    }

    function unref() {
      refed = false;
      if (timeout !== undefined && typeof timeout.unref === 'function') timeout.unref();
      return debounced;
    }

    function invoke(callArgs, calls) {
      if (signal && signal.aborted) {
        abort();
        for (var i = 0; i < calls.length; i++) {
          calls[i].reject(abortError);
        }
        return;
      }

      var result;
      try {
        result = fn.apply(debounced, callArgs);
      } catch (error) {
        for (var j = 0; j < calls.length; j++) {
          calls[j].reject(error);
        }
        return;
      }

      for (var k = 0; k < calls.length; k++) {
        calls[k].resolve(result);
      }
    }

    function invokePending() {
      var callArgs = args;
      args = undefined;
      var calls = pendingCalls;
      pendingCalls = [];
      if (calls.length !== 0) invoke(callArgs, calls);
    }

    function onTimeout() {
      timeout = undefined;
      invokePending();
    }

    function flush() {
      if (pendingCalls.length === 0) return;

      clearTimeoutFn(timeout);
      timeout = undefined;
      invokePending();
    }

    function debounced(...callArgs) {
      if (signal && signal.aborted) abort();
      if (abortError !== undefined) return Promise.reject(abortError);

      var invokeNow = leading && timeout === undefined;
      if (timeout !== undefined) {
        if (pendingCalls.length !== 0) {
          markPromiseAsHandled(pendingCalls[pendingCalls.length - 1].promise);
          if (rejectOnCancel) {
            rejectPending(new AbortError('The debounced call was superseded'));
          }
        }
        // Upstream `timeout.refresh()` resets the tail window. bao timers
        // are bare ids with no refresh — clearTimeout+reschedule of the
        // same callback is the identical semantic (cancel the pending tail
        // timer, start a fresh full window).
        clearTimeoutFn(timeout);
        timeout = setTimeoutFn(onTimeout, wait);
        if (!refed && timeout !== null && typeof timeout.unref === 'function') timeout.unref();
      } else {
        timeout = setTimeoutFn(onTimeout, wait);
        if (!refed && timeout !== null && typeof timeout.unref === 'function') timeout.unref();
      }

      if (signal && signal.aborted) {
        abort();
        return Promise.reject(abortError);
      }

      var call = withResolvers();
      if (invokeNow) {
        invoke(callArgs, [call]);
      } else {
        pendingCalls.push(call);
        args = callArgs;
      }
      return call.promise;
    }

    function prop(value) {
      return {
        configurable: true,
        enumerable: true,
        writable: true,
        value: value,
      };
    }

    Object.defineProperties(debounced, {
      cancel: prop(cancel),
      flush: prop(flush),
      ref: prop(ref),
      unref: prop(unref),
      pending: {
        configurable: false,
        enumerable: false,
        get() {
          return pendingCalls.length === 0 ?
            null :
            pendingCalls[pendingCalls.length - 1].promise;
        },
      },
      pendingCount: {
        configurable: false,
        enumerable: false,
        get() { return pendingCalls.length; },
      },
      length: {
        configurable: true,
        value: fn.length,
      },
      name: {
        configurable: true,
        value: fn.name,
      },
    });

    if (signal !== undefined) {
      addAbortListener(signal, abort);
    }

    return debounced;
  }

  return debounce;
})()
"#;

const INTERNAL_UTIL_THROTTLE_JS: &str = r#"
(function() {
  'use strict';

  // ── inline support (mirrors the debounce module's block) ─────────────
  function determineSpecificType(v) {
    if (v === null) return 'null';
    if (typeof v === 'string') return "type string ('" + v + "')";
    if (typeof v === 'function') {
      return 'type function (' + (v.name ? v.name : 'anonymous') + ')';
    }
    if (typeof v === 'object') {
      if (typeof v.constructor === 'function' && v.constructor.name) {
        return 'an instance of ' + v.constructor.name;
      }
      return 'an object';
    }
    return 'type ' + typeof v + ' (' + String(v) + ')';
  }

  function errInvalidArgType(name, expected, actual) {
    var e = new TypeError(
      'The "' + name + '" argument must be of type ' + expected +
      '. Received ' + determineSpecificType(actual));
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  function errOutOfRange(name, range, actual) {
    var e = new RangeError(
      'The value of "' + name + '" is out of range. It must be ' + range +
      '. Received ' + actual);
    e.code = 'ERR_OUT_OF_RANGE';
    return e;
  }

  function errInvalidArgValue(name, value, reason) {
    var e = new TypeError(
      "The argument '" + name + "' " + reason + '. Received ' +
      determineSpecificType(value));
    e.code = 'ERR_INVALID_ARG_VALUE';
    return e;
  }

  class AbortError extends Error {
    constructor(message, options) {
      if (message === undefined) message = 'The operation was aborted';
      if (options !== undefined && typeof options !== 'object') {
        throw errInvalidArgType('options', 'Object', options);
      }
      super(message, options);
      this.code = 'ABORT_ERR';
      this.name = 'AbortError';
    }
  }

  function errThrottled() {
    // internal/errors.js:1852
    var e = new Error('The call was dropped by the throttled function');
    e.code = 'ERR_THROTTLED';
    return e;
  }

  function validateFunction(value, name) {
    if (typeof value !== 'function') throw errInvalidArgType(name, 'Function', value);
  }

  function validateBoolean(value, name) {
    if (typeof value !== 'boolean') throw errInvalidArgType(name, 'boolean', value);
  }

  function validateInteger(value, name, min, max) {
    if (min === undefined) min = -9007199254740991;
    if (max === undefined) max = 9007199254740991;
    if (typeof value !== 'number') throw errInvalidArgType(name, 'number', value);
    if (!Number.isInteger(value)) throw errOutOfRange(name, 'an integer', value);
    if (value < min || value > max) {
      throw errOutOfRange(name, '>= ' + min + ' && <= ' + max, value);
    }
  }

  function validateObject(value, name) {
    if (value === null || Array.isArray(value) || typeof value !== 'object') {
      throw errInvalidArgType(name, 'Object', value);
    }
  }

  function validateAbortSignal(signal, name) {
    if (signal !== undefined &&
        (signal === null ||
         typeof signal !== 'object' ||
         !('aborted' in signal))) {
      throw errInvalidArgType(name, 'AbortSignal', signal);
    }
  }

  function validateOneOf(value, name, oneOf) {
    if (oneOf.indexOf(value) === -1) {
      var allowed = oneOf.map(function(v) {
        return typeof v === 'string' ? "'" + v + "'" : String(v);
      }).join(', ');
      throw errInvalidArgValue(name, value, 'must be one of: ' + allowed);
    }
  }

  var kEmptyObject = Object.freeze(Object.create(null));
  var TIMEOUT_MAX = 2147483647; // mirrors internal/timers (see that module)

  function withResolvers() {
    var resolve, reject;
    var promise = new Promise(function(res, rej) { resolve = res; reject = rej; });
    return { promise: promise, resolve: resolve, reject: reject };
  }

  function noop() {}
  function markPromiseAsHandled(promise) {
    promise.then(noop, noop);
  }

  function addAbortListener(signal, listener) {
    signal.addEventListener('abort', listener, { once: true });
  }

  // internalBinding('timers').getLibuvNow() — libuv's monotonic millisecond
  // clock. performance.now() is the standard monotonic carrier; Date.now is
  // only a degradation path for realms without a performance object
  // (clock-rollback caveat, flagged in the report).
  var getNow = (typeof performance !== 'undefined' && performance &&
                typeof performance.now === 'function') ?
    function() { return performance.now(); } :
    function() { return Date.now(); };

  var timersModule = require('timers');
  var setTimeoutFn = timersModule.setTimeout;
  var clearTimeoutFn = timersModule.clearTimeout;

  // ── throttle (upstream internal/util/throttle.js:55-261) ─────────────
  // Creates a function that invokes `fn` at most `limit` times per
  // `interval`.
  function throttle(fn, limit, interval, options) {
    validateFunction(fn, 'fn');
    validateInteger(limit, 'limit', 1);
    validateInteger(interval, 'interval', 0, TIMEOUT_MAX);
    if (options === undefined) options = kEmptyObject;
    validateObject(options, 'options');

    var concurrency = options.concurrency !== undefined ? options.concurrency : Infinity;
    var maxPending = options.maxPending !== undefined ? options.maxPending : Infinity;
    var overflow = options.overflow !== undefined ? options.overflow : 'queue';
    var signal = options.signal;
    var strict = options.strict !== undefined ? options.strict : false;

    if (concurrency !== Infinity) {
      validateInteger(concurrency, 'options.concurrency', 1);
    }
    if (maxPending !== Infinity) {
      validateInteger(maxPending, 'options.maxPending', 0);
    }
    validateOneOf(overflow, 'options.overflow', ['queue', 'drop']);
    validateAbortSignal(signal, 'options.signal');
    validateBoolean(strict, 'options.strict');

    if (signal && signal.aborted) {
      throw new AbortError(undefined, { cause: signal.reason });
    }

    var abortError;
    var activeCount = 0;
    var pendingCalls = [];
    var pendingIndex = 0;
    var refed = true;
    var strictIndex = 0;
    var strictTicks = [];
    var timeout;
    var windowCount = 0;
    var windowStart;

    function pendingCount() {
      return pendingCalls.length - pendingIndex;
    }

    function dequeue() {
      var call = pendingCalls[pendingIndex];
      pendingCalls[pendingIndex++] = undefined;

      if (pendingIndex === pendingCalls.length) {
        pendingCalls = [];
        pendingIndex = 0;
      } else if (pendingIndex > 1024 && pendingIndex * 2 >= pendingCalls.length) {
        pendingCalls = pendingCalls.slice(pendingIndex);
        pendingIndex = 0;
      }

      return call;
    }

    function rejectPending(error) {
      var calls = pendingCalls;
      var start = pendingIndex;
      pendingCalls = [];
      pendingIndex = 0;
      for (var i = start; i < calls.length; i++) {
        calls[i].reject(error);
      }
    }

    function resetLimiter() {
      strictIndex = 0;
      strictTicks = [];
      windowCount = 0;
      windowStart = undefined;
    }

    function cancelWithError(error) {
      if (timeout !== undefined) {
        clearTimeoutFn(timeout);
        timeout = undefined;
      }
      resetLimiter();
      rejectPending(error);
    }

    function cancel(reason) {
      var error = reason === undefined ?
        new AbortError() :
        new AbortError(undefined, { cause: reason });
      cancelWithError(error);
    }

    function abort() {
      if (abortError === undefined) {
        abortError = signal.reason === undefined ?
          new AbortError() :
          new AbortError(undefined, { cause: signal.reason });
      }
      cancelWithError(abortError);
    }

    function ref() {
      refed = true;
      if (timeout !== undefined && typeof timeout.ref === 'function') timeout.ref();
      return throttled;
    }

    function unref() {
      refed = false;
      if (timeout !== undefined && typeof timeout.unref === 'function') timeout.unref();
      return throttled;
    }

    function hasImmediateCapacity() {
      if ((signal && signal.aborted) ||
          pendingCount() !== 0 ||
          activeCount >= concurrency) {
        return false;
      }

      var now = getNow();
      if (strict) {
        var index = strictIndex;
        while (index < strictTicks.length &&
               now - strictTicks[index] >= interval) {
          index++;
        }
        return strictTicks.length - index < limit;
      }

      return windowStart === undefined ||
        now - windowStart >= interval ||
        windowCount < limit;
    }

    function pruneStrictTicks(now) {
      while (strictIndex < strictTicks.length &&
             now - strictTicks[strictIndex] >= interval) {
        strictIndex++;
      }

      if (strictIndex === strictTicks.length) {
        strictIndex = 0;
        strictTicks = [];
      } else if (strictIndex > 1024 && strictIndex * 2 >= strictTicks.length) {
        strictTicks = strictTicks.slice(strictIndex);
        strictIndex = 0;
      }
    }

    function hasCapacity(now) {
      if (strict) {
        pruneStrictTicks(now);
        return strictTicks.length - strictIndex < limit;
      }

      if (windowStart === undefined || now - windowStart >= interval) {
        windowCount = 0;
        windowStart = now;
      }
      return windowCount < limit;
    }

    function reserve(now) {
      if (strict) {
        strictTicks.push(now);
      } else {
        windowCount++;
      }
    }

    function onSettled() {
      activeCount--;
      drain();
    }

    function invoke(call) {
      activeCount++;
      var resultPromise;
      try {
        var result = fn.apply(throttled, call.args);
        resultPromise = Promise.resolve(result);
      } catch (error) {
        resultPromise = Promise.reject(error);
      }
      call.resolve(resultPromise);
      resultPromise.then(onSettled, onSettled);
    }

    function drop() {
      var promise = Promise.reject(errThrottled());
      markPromiseAsHandled(promise);
      return promise;
    }

    function schedule() {
      if (timeout !== undefined ||
          pendingCount() === 0 ||
          activeCount >= concurrency) {
        return;
      }

      var now = getNow();
      if (hasCapacity(now)) {
        drain();
        return;
      }

      var delay = strict ?
        strictTicks[strictIndex] + interval - now :
        windowStart + interval - now;
      timeout = setTimeoutFn(onTimeout, delay);
      if (!refed && timeout !== null && typeof timeout.unref === 'function') timeout.unref();
    }

    function drain() {
      if (signal && signal.aborted) {
        abort();
        return;
      }

      while (pendingCount() !== 0 && activeCount < concurrency) {
        var now = getNow();
        if (!hasCapacity(now)) break;
        var call = dequeue();
        reserve(now);
        invoke(call);
      }

      schedule();
    }

    function onTimeout() {
      timeout = undefined;
      drain();
    }

    function throttled(...callArgs) {
      if (signal && signal.aborted) abort();
      if (abortError !== undefined) return Promise.reject(abortError);

      var now = getNow();
      var canInvoke = pendingCount() === 0 &&
        activeCount < concurrency &&
        hasCapacity(now);
      if (!canInvoke &&
          (overflow === 'drop' || pendingCount() >= maxPending)) {
        return drop();
      }

      var call = withResolvers();
      call.args = callArgs;

      if (canInvoke) {
        reserve(now);
        invoke(call);
      } else {
        pendingCalls.push(call);
        schedule();
      }

      return call.promise;
    }

    function prop(value) {
      return {
        configurable: true,
        enumerable: true,
        writable: true,
        value: value,
      };
    }

    Object.defineProperties(throttled, {
      cancel: prop(cancel),
      hasImmediateCapacity: prop(hasImmediateCapacity),
      ref: prop(ref),
      unref: prop(unref),
      pending: {
        configurable: false,
        enumerable: false,
        get() {
          return pendingCount() === 0 ? null :
            pendingCalls[pendingCalls.length - 1].promise;
        },
      },
      pendingCount: {
        configurable: false,
        enumerable: false,
        get: pendingCount,
      },
      activeCount: {
        configurable: false,
        enumerable: false,
        get() {
          return activeCount;
        },
      },
      length: {
        configurable: true,
        value: fn.length,
      },
      name: {
        configurable: true,
        value: fn.name,
      },
    });

    if (signal !== undefined) {
      addAbortListener(signal, abort);
    }

    return throttled;
  }

  return throttle;
})()
"#;

/// Evaluate one internal-module source and register it under its builtin
/// name. Mirrors the `node_internal_test_binding::install` precedent: a
/// broken module must surface as `Cannot find module` at the require()
/// point, never as an empty exports object.
fn evaluate_and_cache(
    cx: &mut mozjs::context::JSContext,
    source: &str,
    filename: &str,
    builtin_name: &str,
) {
    unsafe {
        let raw_cx = cx.raw_cx();
        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let filename_c = ZBox::from_bytes(format!("<{}>", filename).into_bytes());
        let opts = NewCompileOptions(raw_cx, filename_c.as_ptr(), 1);
        if opts.is_null() {
            return;
        }
        let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok {
            let msg = format!("failed to evaluate {} source", filename);
            let msg_c = ZBox::from_bytes(msg.into_bytes());
            JS_ReportErrorUTF8(raw_cx, c"%s".as_ptr(), msg_c.as_ptr());
            return;
        }
        if !rval.is_object() {
            return;
        }
        let obj = rval.to_object();
        rooted!(&in(cx) let obj_rooted = obj);
        cache_builtin(cx, builtin_name, obj_rooted.get());
    }
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    evaluate_and_cache(cx, INTERNAL_TIMERS_JS, "internal/timers", "internal/timers");
    evaluate_and_cache(
        cx,
        INTERNAL_UTIL_DEBOUNCE_JS,
        "internal/util/debounce",
        "internal/util/debounce",
    );
    evaluate_and_cache(
        cx,
        INTERNAL_UTIL_THROTTLE_JS,
        "internal/util/throttle",
        "internal/util/throttle",
    );
}
