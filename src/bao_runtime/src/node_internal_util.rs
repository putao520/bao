// @trace REQ-ENG-048 [api:internal/util]
//
// `internal/util` — the subset of Node's internal util module consumed by
// Node's own test fixtures (GOLDEN A: test-util-deprecate.js /
// test-util-promisify.js / test-util-sleep.js require `internal/util`
// directly; test-util-debounce.js / test-util-throttle.js require
// `internal/timers` for TIMEOUT_MAX and `node:util` for debounce/throttle —
// the debounce/throttle modules themselves live in
// `node_internal_timers.rs` per their true upstream homes
// `internal/util/debounce.js` / `internal/util/throttle.js`).
//
// Per-function contract is source-aligned with
// golden-node/node/lib/internal/util.js (node v24 tree).
//
// Honest boundaries (fail-closed, never stubs):
// - `process.emitWarning` has no native definition in bao (whole-tree grep:
//   only consumer guards exist). The warning sink below calls the real
//   `process.emitWarning` when the embedder provides one and otherwise
//   prints node's stderr format via `console.warn` — the smallest real
//   surface that keeps deprecations observable. When process.emitWarning
//   lands, this module switches to it automatically (the real branch wins).
// - `sleep()` upstream blocks the thread via `internalBinding('util').sleep`
//   (C++ uv_sleep). bao has no C++ binding layer; the only standard
//   JS-carriable synchronous block is `Atomics.wait` with a timeout, which
//   is what we use. If Atomics.wait is unavailable the call fails loudly —
//   never a silent fake sleep.
// - `isPendingDeprecation()` upstream reads `getOptionValue('--pending-
//   deprecation')`. bao has no internal flag store; the function returns
//   false, which equals node's default (the flag is off unless explicitly
//   passed). The flag-consumption surface is reported as absent, not faked.
// - Everything else on the upstream export surface that needs
//   `internalBinding`/`internal/errors`/`internal/validators` deep chains
//   (isError, normalizeEncoding, lazyDOMException, getSystemError*, ...)
//   is intentionally ABSENT from the returned object — consumers fail
//   loudly at the property-access point (mirrors the
//   `internal/test/binding` precedent).

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use bun_core::ZBox;

use crate::require::{cache_builtin, get_builtin};

const INTERNAL_UTIL_JS: &str = r#"
(function() {
  'use strict';

  // ── warning sink ─────────────────────────────────────────────────────
  // Upstream: process.emitWarning(msg, type, code, ctor). bao currently has
  // no native process.emitWarning (only guard-style consumers), so when the
  // embedder has not provided one we print node's stderr format via
  // console.warn:
  //   (node:PID) [CODE] Type: message
  // Anchor twin: node_internal_timers.rs has the same inline sink for its
  // getTimerDuration face — keep the two in sync.
  function emitWarning(msg, type, code, ctor) {
    if (typeof process !== 'undefined' && typeof process.emitWarning === 'function') {
      process.emitWarning(msg, type, code, ctor);
      return;
    }
    var prefix = (typeof process !== 'undefined' && typeof process.pid === 'number')
      ? '(node:' + process.pid + ') '
      : '';
    console.warn(prefix + (code ? '[' + code + '] ' : '') + (type || 'Warning') + ': ' + msg);
  }

  // ── errors (minimal inline mirrors of internal/errors.js codes) ─────
  // Upstream routes these through require('internal/errors'); bao has no
  // internal/errors module, so the two codes the validators need are built
  // inline with node's exact message templates.
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

  // ── validators (inline, upstream internal/validators.js semantics) ──
  function validateString(value, name) {
    if (typeof value !== 'string') throw errInvalidArgType(name, 'string', value);
  }

  function validateFunction(value, name) {
    if (typeof value !== 'function') throw errInvalidArgType(name, 'Function', value);
  }

  // validateUint32(value, name, positive = false) — upstream validators.js:142
  function validateUint32(value, name, positive) {
    if (typeof value !== 'number') {
      throw errInvalidArgType(name, 'number', value);
    }
    if (!Number.isInteger(value)) {
      throw errOutOfRange(name, 'an integer', value);
    }
    var min = positive ? 1 : 0;
    var max = 4294967295; // 2 ** 32 - 1
    if (value < min || value > max) {
      throw errOutOfRange(name, '>= ' + min + ' && <= ' + max, value);
    }
  }

  // ── deprecation family (upstream internal/util.js:113-216) ──────────
  // Keep a list of deprecation codes that have been warned on so we only
  // warn on each one once (per-CODE, not per-function — node semantics:
  // two different deprecated fns sharing a code warn exactly once).
  var codesWarned = new Set();

  function getDeprecationWarningEmitter(
    code, msg, deprecated, useEmitSync, shouldEmitWarning,
  ) {
    if (shouldEmitWarning === undefined) {
      shouldEmitWarning = function() { return true; };
    }
    var warned = false;
    return function(arg) {
      if (!warned && shouldEmitWarning(arg)) {
        warned = true;
        if (code === 'ExperimentalWarning') {
          emitWarning(msg, code, deprecated);
        } else if (code !== undefined) {
          if (!codesWarned.has(code)) {
            // Upstream picks require('internal/process/warning').emitWarningSync
            // when useEmitSync; bao has no sync-warning module, both routes
            // land on the same sink here (boundary noted in the report).
            emitWarning(msg, 'DeprecationWarning', code, deprecated);
            codesWarned.add(code);
          }
        } else {
          emitWarning(msg, 'DeprecationWarning', deprecated);
        }
      }
    };
  }

  // Mark that a method should not be used. Returns a modified function
  // which warns once by default. Prototype chain and `length` are preserved
  // (upstream deprecate, internal/util.js:177-216).
  function deprecate(fn, msg, code, useEmitSync, modifyPrototype) {
    if (modifyPrototype === undefined) modifyPrototype = true;
    validateFunction(fn, 'fn');
    if (code !== undefined) validateString(code, 'code');

    var emitDeprecationWarning = getDeprecationWarningEmitter(
      code, msg, deprecated, useEmitSync,
    );

    function deprecated(...args) {
      var noDeprecation = typeof process !== 'undefined' && process.noDeprecation;
      if (!noDeprecation) {
        emitDeprecationWarning();
      }
      if (new.target) {
        return Reflect.construct(fn, args, new.target);
      }
      return Reflect.apply(fn, this, args);
    }

    if (modifyPrototype) {
      // The wrapper keeps the same prototype as fn to maintain the
      // prototype chain (upstream comment preserved).
      Object.setPrototypeOf(deprecated, fn);
      if (fn.prototype) {
        // Setting this (rather than Object.setPrototypeOf) ensures that
        // calling the unwrapped constructor gives an instanceof the
        // wrapped constructor.
        deprecated.prototype = fn.prototype;
      }
      Object.defineProperty(deprecated, 'length', {
        __proto__: null,
        ...Object.getOwnPropertyDescriptor(fn, 'length'),
      });
    }

    return deprecated;
  }

  // Internal deprecator for pending --pending-deprecation (upstream
  // internal/util.js:152-166). isPendingDeprecation is absent in bao
  // (no internal flag store) and returns false — node's default.
  function isPendingDeprecation() {
    return false;
  }

  function pendingDeprecate(fn, msg, code) {
    var emitDeprecationWarning = getDeprecationWarningEmitter(
      code, msg, deprecated, false, isPendingDeprecation,
    );
    function deprecated(...args) {
      emitDeprecationWarning();
      return Reflect.apply(fn, this, args);
    }
    Object.defineProperty(deprecated, 'length', {
      __proto__: null,
      ...Object.getOwnPropertyDescriptor(fn, 'length'),
    });
    return deprecated;
  }

  // ── experimental warnings (upstream internal/util.js:323-331) ───────
  const experimentalWarnings = new Set();

  function emitExperimentalWarning(feature, messagePrefix, code, ctor) {
    if (experimentalWarnings.has(feature)) return;
    experimentalWarnings.add(feature);
    var msg = feature + ' is an experimental feature and might change at any time';
    if (messagePrefix) {
      msg = messagePrefix + msg;
    }
    emitWarning(msg, 'ExperimentalWarning', code, ctor);
  }

  // ── sleep (upstream internal/util.js:589-596) ────────────────────────
  // Upstream: validateUint32(msec, 'msec'); _sleep(msec) where _sleep is
  // internalBinding('util').sleep — a synchronous C++ thread block
  // (uv_sleep). It is NOT promise-based (the public promise sleep is
  // timers/promises.setTimeout). bao has no C++ binding layer; the only
  // standard JS-carriable synchronous block is Atomics.wait timeout, so we
  // park one SharedArrayBuffer and wait on it. A missing Atomics.wait or
  // SharedArrayBuffer fails loudly at the call site (fail-closed), never a
  // silent fake sleep.
  var kSleepBuffer = null;

  function sleep(msec) {
    validateUint32(msec, 'msec');
    if (msec === 0) return;
    if (kSleepBuffer === null) {
      kSleepBuffer = new Int32Array(new SharedArrayBuffer(4));
    }
    Atomics.wait(kSleepBuffer, 0, 0, msec);
  }

  // promisify is reused from the already-native `util` module (same
  // function reference — DRY). Upstream the direction is reversed
  // (lib/util.js re-exports internal/util.promisify); in bao the public
  // Rust-native implementation landed first and this module references it.
  var utilModule = require('util');

  return {
    deprecate: deprecate,
    pendingDeprecate: pendingDeprecate,
    getDeprecationWarningEmitter: getDeprecationWarningEmitter,
    isPendingDeprecation: isPendingDeprecation,
    emitExperimentalWarning: emitExperimentalWarning,
    promisify: utilModule.promisify,
    sleep: sleep,
    // Shared support constants consumed by internal/util/debounce.js and
    // internal/util/throttle.js upstream (kEmptyObject) and by userland
    // parity checks (the two symbols).
    kEmptyObject: Object.freeze(Object.create(null)),
    customPromisifyArgs: Symbol('customPromisifyArgs'),
    customInspectSymbol: Symbol.for('nodejs.util.inspect.custom'),
  };
})()
"#;

/// Empty at realm-init time (mirrors `node_internal_event_target::install` /
/// `node_internal_test_binding::install`): install-time evaluation poisons
/// the realm — the GC-pressure class where the init-period function-object
/// allocations push the nursery allocation order over a latent engine cliff,
/// and any later module-body variable access then segfaults
/// (timeline-attributed 2026-10-26). The real evaluation happens lazily on
/// the first `require('internal/util')` via [`ensure_installed`], in user
/// context, when realm init has fully completed.
pub fn install(_cx: &mut mozjs::context::JSContext) {}

thread_local! {
    /// Per-realm once flag: repeat failed evaluations must not re-run the
    /// whole source on every require attempt (event_target precedent).
    static NIUTIL_EVAL_DONE: ::std::cell::Cell<bool> = ::std::cell::Cell::new(false);
}

/// Evaluate the `internal/util` port and cache the module object for the
/// require() surface. The timers family — `internal/timers`,
/// `internal/util/debounce`, `internal/util/throttle` — is funneled through
/// here FIRST (the funnel the install header documents): order matters —
/// debounce/throttle require nothing from internal/util (their support
/// constants are self-carried), and `require('util')` is already cached by
/// node_util::install_util. Idempotent (gc_store cache is the idempotence
/// boundary). Called from `require_fn` (a JSNative frame): `cx` is live and
/// the caller propagates a pending exception when evaluation fails.
pub fn ensure_installed(cx: *mut JSContext) -> bool {
    NIUTIL_EVAL_DONE.with(|flag| {
        if flag.get() {
            return;
        }
        flag.set(true);
        install_eval(cx);
    });
    crate::require::get_builtin(cx, "internal/util").is_some()
}

fn install_eval(raw_cx: *mut JSContext) {
    // Funnel the timers family first (order contract documented on the
    // module header). Failure → abort; the pending exception propagates.
    if !crate::node_internal_timers::ensure_installed(raw_cx) {
        return;
    }
    let source = INTERNAL_UTIL_JS;
    unsafe {
        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let filename = ZBox::from_bytes("<internal/util>".as_bytes());
        let opts = NewCompileOptions(raw_cx, filename.as_ptr(), 1);
        if opts.is_null() {
            JS_ClearPendingException(raw_cx);
            return;
        }
        let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok || !rval.is_object() {
            if ok {
                JS_ClearPendingException(raw_cx);
            }
            return;
        }
        // Same rooting contract as the internal/test/binding precedent:
        // only cache when evaluation produced a real object; a broken
        // module must surface as `Cannot find module` at the require()
        // point, not as an empty exports object.
        let mut wrapped =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
        let obj = rval.to_object();
        rooted!(&in(wrapped) let obj_rooted = obj);
        cache_builtin(&mut wrapped, "internal/util", obj_rooted.get());
    }
}
