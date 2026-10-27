// @trace REQ-ENG-004 [api:events/internal/event_target]
//
// `events` + `internal/event_target` — JS-native port of node v24
// lib/events.js and lib/internal/event_target.js (GOLDEN A events domain).
//
// Why JS, not Rust: the node test corpus asserts JS-visible shapes the
// previous Rust EventEmitter cannot express without re-encoding every
// detail — `_events` is a null-prototype object whose value is the listener
// function itself for a single listener and an array for several, the
// `warned` flag lives ON that function/array, `listeners()` returns fresh
// arrays of identical function references, `once` wraps in a `g.listener`
// carrier, and the static surface (setMaxListeners/getMaxListeners/on/once/
// getEventListeners/listenerCount/addAbortListener/EventEmitterAsyncResource)
// shares mutable module state (`defaultMaxListeners`) with the prototype
// methods. The port is the node source adapted to bao's realm; every
// deliberate deviation is listed in the JS header and marked inline.
//
// Consumers / wiring:
// - `require('events')` returns the ported EventEmitter constructor itself
//   (node shape: module value === .EventEmitter === the function). This
//   overwrites the earlier Rust-native `builtin:events` cache from
//   node_events::install, which stays installed for the native-class ee_*
//   method family (node_http / node_worker_threads / node_net wire those
//   methods onto their own objects with hidden-state storage — zero
//   prototype-chain coupling, verified: EMITTER_PROTO_KEY has no external
//   readers).
// - `require('internal/event_target')` is cached here; the specifier is
//   added to require.rs `known_builtins` (registration list).
// - Global faces: node's EventTarget/Event/CustomEvent ARE the internal
//   module's classes; bao's WHATWG shim (globals.rs, guarded
//   `typeof === 'undefined'` installs) is replaced here at runtime — the
//   shim keeps zero external consumers (grep: `_bao_get_listeners` /
//   `_bao_listeners` / `_bao_dispatching` are globals.rs-internal).
// - AbortSignal integration: node's contract is "AbortSignal IS an
//   EventTarget with kMaxEventTargetListeners = 0" (oracle:
//   getMaxListeners(ac.signal) === 0; events.once/on/setMaxListeners all
//   operate on signals; events tests introspect `ac.signal[kEvents]`). bao's
//   shim AbortSignal (globals.rs) stores plain listeners in `this._listeners`
//   and is not an EventTarget. The integration below brands the shim class,
//   routes its add/remove/dispatch through the ported EventTarget methods,
//   wraps the constructor to install the node slots (kEvents map,
//   kMaxEventTargetListeners = 0), re-parents AbortController's abort()
//   through real dispatch, preserves the shim's `timeout`/`reason` /
//   DOMException contract, and adds the `AbortSignal.abort(reason)` static
//   (node 18+). web_fetch's `new _g.AbortController().signal` face keeps
//   working (signal/abort/aborted/reason/listener delivery all preserved).
// - AbortSignal BOOTSTRAP STATICS (GOLDEN A test-fs-watch-abort-signal):
//   node's contract is that the full AbortSignal static face
//   (abort/timeout/any) exists at bootstrap, before any require. `install`
//   therefore defines NATIVE trampolines for `AbortSignal.abort`/`.any` on
//   the realm ctor DURING realm init — a native function-object definition
//   is the proven-safe init shape (the events ctor's `once`/`on`
//   trampolines in node_events::install do exactly this); JS module-body
//   evaluation at init stays forbidden (GC-pressure poison class). The
//   trampolines' FIRST user-context call forces `ensure_installed` (the
//   exact `events_static_once` lazy-install discipline) and re-dispatches
//   to the port's integrated static; `timeout` already exists from the
//   globals.rs shim at init and is left untouched, and a pre-existing
//   function face always wins (the ensureProcessEmitWarning contract).
//   Post-integration the port REPLACES the global ctor and its own
//   abort/timeout/any statics supersede the trampolines — the events_once
//   path is byte-identical to before this change.
// - `process.emitWarning` does not exist in bao (whole-tree grep); node's
//   warnMaxListenersExceeded / EventTarget kNewListener / the
//   AddEventListenerArgumentTypeWarning path all go through it and the
//   events tests hook `process.on('warning', ...)`. It is defined here with
//   node's semantics (deferred 'warning' emit via process.nextTick) when
//   absent; a native definition would win automatically.

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use ::std::ptr::NonNull;

use bun_core::ZBox;

use crate::require::{cache_builtin, get_builtin};

const EVENT_TARGET_ONLY_JS: &str = r#"(function() {
  'use strict';

  // ════════════════════════════════════════════════════════════════════
  // internal/event_target — JS-native port of node v24
  // lib/internal/event_target.js (+ the shared internal shims it needs).
  // SPLIT BUILD (GC-pressure containment): the full events.js port is NOT
  // evaluated here — `require('events')` stays the Rust-native constructor
  // (E10 shape) and this module binds to it via the builtin cache. See the
  // module header for the crash-localization history that forced the split.
  // ════════════════════════════════════════════════════════════════════

  const kEmptyObject = Object.freeze({ __proto__: null });

  // node internal/util.js spliceOne (used instead of splice on hot paths).
  function spliceOne(list, index) {
    for (let i = index, k = index + 1, n = list.length; k < n; i += 1, k += 1) {
      list[i] = list[k];
    }
    list.pop();
  }

  // node internal/util.js kEnumerableProperty — a GENERIC descriptor
  // (enumerable only, no value/writable/get/set): applying it flips the
  // enumerable flag and preserves the existing value, writable and
  // configurable attributes. Verified against node's internal/util.js:698.
  const kEnumerableProperty = {
    __proto__: null,
    enumerable: true,
  };

  // node internal/errors.js determineSpecificType — the Received suffix of
  // ERR_INVALID_ARG_TYPE. Verified templates:
  //   null            -> 'null'
  //   'foo'           -> "type string ('foo')"
  //   function fn(){} -> 'type function (fn)' / 'type function (anonymous)'
  //   {}              -> 'an instance of Object'
  //   1n / true       -> 'type bigint (1)' / 'type boolean (true)'
  function determineSpecificType(v) {
    if (v === null) {
      return 'null';
    }
    const type = typeof v;
    if (type === 'string') {
      return `type string ('${v}')`;
    }
    if (type === 'function') {
      return v.name ? `type function (${v.name})` : 'type function (anonymous)';
    }
    if (type === 'object') {
      if (typeof v.constructor === 'function' && v.constructor.name) {
        return `an instance of ${v.constructor.name}`;
      }
      return 'an object';
    }
    return `type ${type} (${String(v)})`;
  }

  // ERR_INVALID_ARG_TYPE with the of-type phrasing (expected is a type word;
  // node renders the 'Function' spelling as lowercase 'function' — oracle:
  // The "listener" argument must be of type function. Received null).
  // node's createInvalidArgType switches to the "property" noun when the
  // name contains a dot (oracle: The "options.captureRejections" property
  // must be of type boolean. Received type number (1)).
  function errInvalidArgType(name, expected, actual) {
    const word = expected === 'Function' ? 'function' : expected;
    const noun = name.includes('.') ? 'property' : 'argument';
    const e = new TypeError(
      `The "${name}" ${noun} must be of type ${word}. ` +
      `Received ${determineSpecificType(actual)}`);
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  // ERR_INVALID_ARG_TYPE with the instance phrasing (expected is a class
  // name or an array of class names — oracle:
  // The "emitter" argument must be an instance of EventEmitter or
  // EventTarget. Received type string ('INVALID')).
  function errInvalidArgTypeInstance(name, expected, actual) {
    const joined = Array.isArray(expected) ? expected.join(' or ') : expected;
    const noun = name.includes('.') ? 'property' : 'argument';
    const e = new TypeError(
      `The "${name}" ${noun} must be an instance of ${joined}. ` +
      `Received ${determineSpecificType(actual)}`);
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  function errOutOfRange(name, range, value) {
    const e = new RangeError(
      `The value of "${name}" is out of range. It must be ${range}. ` +
      `Received ${value}`);
    e.code = 'ERR_OUT_OF_RANGE';
    return e;
  }

  function errInvalidThis(type) {
    const e = new TypeError(`Value of "this" must be of type ${type}`);
    e.code = 'ERR_INVALID_THIS';
    return e;
  }

  function errMissingArgs(...args) {
    let msg;
    if (args.length === 1) {
      msg = `The "${args[0]}" argument must be specified`;
    } else {
      const head = args.slice(0, -1).map((a) => `"${a}"`).join(', ');
      msg = `The ${head} and "${args[args.length - 1]}" arguments must be specified`;
    }
    const e = new TypeError(msg);
    e.code = 'ERR_MISSING_ARGS';
    return e;
  }

  // Oracle: The event "foo" is already being dispatched (ERR_EVENT_RECURSION).
  function errEventRecursion(type) {
    const e = new Error(`The event "${type}" is already being dispatched`);
    e.code = 'ERR_EVENT_RECURSION';
    return e;
  }

  function errUnhandledError(er) {
    const e = new Error('Unhandled error.');
    e.code = 'ERR_UNHANDLED_ERROR';
    e.context = er;
    return e;
  }

  // node internal/errors.js AbortError — a plain Error carrying
  // name='AbortError' + code='ABORT_ERR' (oracle: AbortError | The operation
  // was aborted | ABORT_ERR; NOT a DOMException — DOMException.code is a
  // read-only getter and assignment would throw).
  function abortError(cause) {
    const err = new Error('The operation was aborted');
    err.name = 'AbortError';
    err.code = 'ABORT_ERR';
    if (cause !== undefined) {
      err.cause = cause;
    }
    return err;
  }

  // ── validators (node internal/validators.js slices) ──────────────────

  function validateString(value, name) {
    if (typeof value !== 'string') {
      throw errInvalidArgType(name, 'string', value);
    }
  }

  function validateFunction(value, name) {
    if (typeof value !== 'function') {
      throw errInvalidArgType(name, 'Function', value);
    }
  }

  function validateBoolean(value, name) {
    if (typeof value !== 'boolean') {
      throw errInvalidArgType(name, 'boolean', value);
    }
  }

  function validateNumber(value, name, min = undefined, max = undefined) {
    if (typeof value !== 'number') {
      throw errInvalidArgType(name, 'number', value);
    }
    if ((min !== undefined && (value < min || Number.isNaN(value))) ||
        (max !== undefined && (value > max || Number.isNaN(value)))) {
      const range = max !== undefined
        ? `>= ${min} && <= ${max}` : `>= ${min}`;
      throw errOutOfRange(name, range, value);
    }
  }

  function validateInteger(value, name, min = undefined, max = undefined) {
    if (typeof value !== 'number' || !Number.isInteger(value)) {
      throw errInvalidArgType(name, 'number', value);
    }
    if ((min !== undefined && value < min) || (max !== undefined && value > max)) {
      const range = max !== undefined
        ? `>= ${min} && <= ${max}` : `>= ${min}`;
      throw errOutOfRange(name, range, value);
    }
  }

  // node internal/validators.js validateObject — plain objects only
  // (functions are rejected: node oracle — events.once(ee, 'x', () => {})
  // throws ERR_INVALID_ARG_TYPE, the invalid-options test relies on it).
  function validateObject(value, name) {
    if (value === null || typeof value !== 'object') {
      throw errInvalidArgType(name, 'object', value);
    }
  }

  function validateAbortSignal(signal, name) {
    if (signal === undefined || signal === null) {
      return;
    }
    if (!(signal instanceof AbortSignal)) {
      throw errInvalidArgTypeInstance(name, 'AbortSignal', signal);
    }
  }

  function validateThisInternalField(self, field, type) {
    if (self === null || self === undefined || self[field] === undefined) {
      throw errInvalidThis(type);
    }
  }

  // node internal/webidl.js webidl.converters.DOMString — ToString with the
  // symbol rejection (TypeError).
  function webidlDOMString(value) {
    if (typeof value === 'symbol') {
      throw new TypeError('Cannot convert a Symbol value to a string');
    }
    return `${value}`;
  }

  // node internal/perf/utils.js now() — milliseconds float; performance.now
  // when the realm provides it, Date.now otherwise (integer ms — only the
  // "typeof number" contract is asserted by tests).
  const now = (typeof performance !== 'undefined' && typeof performance.now === 'function')
    ? () => performance.now()
    : () => Date.now();

  // process.emitWarning with node's semantics: the 'warning' event is
  // deferred to nextTick and carries the warning object/string. String
  // warnings get node's warning-object shape (name = type || 'Warning').
  function ensureProcessEmitWarning() {
    if (typeof process === 'undefined') {
      return;
    }
    if (typeof process.emitWarning === 'function') {
      return; // a native definition wins (anchor: module header).
    }
    process.emitWarning = function emitWarning(warning, type, code, ctor) {
      if (typeof warning === 'string') {
        const err = new Error(warning);
        err.name = type === undefined ? 'Warning' : String(type);
        if (code !== undefined) {
          err.code = code;
        }
        warning = err;
      }
      process.nextTick(() => {
        process.emit('warning', warning);
      });
    };
  }

  // ════════════════════════════════════════════════════════════════════
  // events-module symbols consumed by the event_target half (the full events
  // port is not installed in this split build — the Rust-native constructor
  // remains the `events` module; these two symbols keep the EventTarget
  // max-listeners faces addressable).
  const kMaxEventTargetListeners = Symbol('events.maxEventTargetListeners');
  const kMaxEventTargetListenersWarned =
    Symbol('events.maxEventTargetListenersWarned');

  // `EventEmitter` binding — the Rust-native `events` module (Rust ctor
  // carries defaultMaxListeners = 10). setMaxListeners static is absent on
  // that ctor; provide the module-local implementation NodeEventTarget needs.
  const EventEmitter = (function() {
    try {
      const m = require('events');
      // The four module-level statics are DUAL-NATURE functions in node:
      // they accept an EventEmitter OR an EventTarget (node oracle:
      // events.listenerCount(new EventTarget(), 'x') reads the target's
      // kEvents map; getEventListeners walks the listener list; set/get
      // MaxListeners address kMaxEventTargetListeners). The E10 Rust ctor's
      // statics only know the hidden-state EventEmitter face, and — being a
      // different implementation — CANNOT see this module's symbol-keyed
      // slots. Replace them with the dual implementations below (node
      // semantics verbatim; the EventEmitter arms delegate to the module's
      // own prototype/static faces when present, else behave as the port's
      // would for the Rust ctor's hidden-state shape).
      m.setMaxListeners = function(n = (m.defaultMaxListeners ?? 10), ...targets) {
        if (typeof n !== 'number' || n < 0 || Number.isNaN(n)) {
          const e = new RangeError('The value of "setMaxListeners" is out of range. It must be >= 0. Received ' + n);
          e.code = 'ERR_OUT_OF_RANGE';
          throw e;
        }
        if (targets.length === 0) {
          // No per-target form on the Rust ctor's defaultMaxListeners (a
          // plain data property): rebinding it is still the node shape.
          try { m.defaultMaxListeners = n; } catch { /* read-only ctor */ }
          return;
        }
        for (let i = 0; i < targets.length; i++) {
          const target = targets[i];
          if (isEventTargetBind(target)) {
            target[kMaxEventTargetListeners] = n;
            target[kMaxEventTargetListenersWarned] = false;
          } else if (typeof target.setMaxListeners === 'function') {
            target.setMaxListeners(n);
          } else {
            const e = new TypeError(
              'The "eventTargets" argument must be an instance of EventEmitter or EventTarget. ' +
              'Received ' + determineSpecificType(target));
            e.code = 'ERR_INVALID_ARG_TYPE';
            throw e;
          }
        }
      };
      m.getMaxListeners = function(emitterOrTarget) {
        if (typeof emitterOrTarget?.getMaxListeners === 'function') {
          return emitterOrTarget.getMaxListeners();
        } else if (typeof emitterOrTarget?.[kMaxEventTargetListeners] === 'number') {
          return emitterOrTarget[kMaxEventTargetListeners];
        }
        const e = new TypeError(
          'The "emitter" argument must be an instance of EventEmitter or EventTarget. ' +
          'Received ' + determineSpecificType(emitterOrTarget));
        e.code = 'ERR_INVALID_ARG_TYPE';
        throw e;
      };
      m.listenerCount = function(emitterOrTarget, type) {
        if (typeof emitterOrTarget.listenerCount === 'function') {
          return emitterOrTarget.listenerCount(type);
        }
        if (isEventTargetBind(emitterOrTarget)) {
          return emitterOrTarget[kEvents]?.get(type)?.size ?? 0;
        }
        const e = new TypeError(
          'The "emitter" argument must be an instance of EventEmitter or EventTarget. ' +
          'Received ' + determineSpecificType(emitterOrTarget));
        e.code = 'ERR_INVALID_ARG_TYPE';
        throw e;
      };
      m.getEventListeners = function(emitterOrTarget, type) {
        if (typeof emitterOrTarget.listeners === 'function') {
          return emitterOrTarget.listeners(type);
        }
        if (isEventTargetBind(emitterOrTarget)) {
          const root = emitterOrTarget[kEvents]?.get(type);
          const listeners = [];
          let handler = root?.next;
          while (handler?.listener !== undefined) {
            const l = handler.listener?.deref ? handler.listener.deref() : handler.listener;
            listeners.push(l);
            handler = handler.next;
          }
          return listeners;
        }
        const e = new TypeError(
          'The "emitter" argument must be an instance of EventEmitter or EventTarget. ' +
          'Received ' + determineSpecificType(emitterOrTarget));
        e.code = 'ERR_INVALID_ARG_TYPE';
        throw e;
      };
      return m;
    } catch {
      return function EventEmitter() {};
    }
  })();

  // Brand check used by the dual statics above; defined after the classes —
  // hoisted via function declaration below (isEventTargetBind forwards to
  // the same kIsEventTarget well-known-symbol contract as isEventTarget).
  function isEventTargetBind(obj) {
    return obj?.constructor?.[kIsEventTarget];
  }

  // lib/internal/event_target.js (node v24) — adapted port.
  // Deviations: primordials → realm globals; Safe* → plain builtins;
  // webidl.converters.DOMString / perf now() inlined above. The class
  // bodies, listener linked list, dispatch loop (incl. passive /
  // resistStopPropagation / weak listeners / once / recursion), Event and
  // CustomEvent (private fields, ERR_INVALID_THIS brand checks, statics,
  // toStringTag), NodeEventTarget, defineEventHandler and the exported
  // symbols are verbatim.
  // ════════════════════════════════════════════════════════════════════

  const kIsEventTarget = Symbol.for('nodejs.event_target');
  const kIsNodeEventTarget = Symbol('kIsNodeEventTarget');

  // `EventEmitter` here is the events module's constructor function
  // declaration in this same IIFE scope (node: require('events')).

  const kEvents = Symbol('kEvents');
  const kIsBeingDispatched = Symbol('kIsBeingDispatched');
  const kStop = Symbol('kStop');
  const kTarget = Symbol('kTarget');
  const kHandlers = Symbol('kHandlers');
  const kWeakHandler = Symbol('kWeak');
  const kResistStopPropagation = Symbol('kResistStopPropagation');

  const kHybridDispatch = Symbol.for('nodejs.internal.kHybridDispatch');
  const kRemoveWeakListenerHelper = Symbol('kRemoveWeakListenerHelper');
  const kCreateEvent = Symbol('kCreateEvent');
  const kNewListener = Symbol('kNewListener');
  const kRemoveListener = Symbol('kRemoveListener');
  const kIsNodeStyleListener = Symbol('kIsNodeStyleListener');
  const kTrustEvent = Symbol('kTrustEvent');

  const kType = Symbol('type');
  const kDetail = Symbol('detail');
  const kInPassiveListener = Symbol('kInPassiveListener');

  // util module is installed AFTER node_events in the realm init order
  // (globals.rs install_all_native), so require('util') must stay LAZY here
  // — an eval-time require would fail and sink the whole module (that was
  // the first farm run's failure).
  let utilInspect = null;
  function getInspect() {
    if (utilInspect === null) {
      utilInspect = require('util').inspect;
    }
    return utilInspect;
  }

  const isTrustedSet = new WeakSet();
  const isTrustedDescriptor = {
    __proto__: null,
    configurable: false,
    enumerable: true,
    get: function isTrusted() {
      return isTrustedSet.has(this);
    },
  };

  function isEvent(value) {
    return typeof value?.[kType] === 'string';
  }

  class Event {
    #cancelable = false;
    #bubbles = false;
    #composed = false;
    #defaultPrevented = false;
    #timestamp = now();
    #propagationStopped = false;

    constructor(type, options = undefined) {
      if (arguments.length === 0) {
        throw errMissingArgs('type');
      }
      if (options != null) {
        validateObject(options, 'options');
      }
      this.#bubbles = !!options?.bubbles;
      this.#cancelable = !!options?.cancelable;
      this.#composed = !!options?.composed;

      this[kType] = `${type}`;
      if (options?.[kTrustEvent]) {
        isTrustedSet.add(this);
      }

      this[kTarget] = null;
      this[kIsBeingDispatched] = false;
      this[kInPassiveListener] = false;
    }

    initEvent(type, bubbles = false, cancelable = false) {
      if (arguments.length === 0) {
        throw errMissingArgs('type');
      }

      if (this[kIsBeingDispatched]) {
        return;
      }
      this[kType] = `${type}`;
      this.#bubbles = !!bubbles;
      this.#cancelable = !!cancelable;
    }

    [Symbol.for('nodejs.util.inspect.custom')](depth, options) {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      const name = this.constructor.name;
      if (depth < 0) {
        return name;
      }

      const opts = Object.assign({}, options, {
        depth: Number.isInteger(options.depth) ? options.depth - 1 : options.depth,
      });

      return `${name} ${getInspect()({
        type: this[kType],
        defaultPrevented: this.#defaultPrevented,
        cancelable: this.#cancelable,
        timeStamp: this.#timestamp,
      }, opts)}`;
    }

    stopImmediatePropagation() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      // Spec mention "stopImmediatePropagation should set both "stop
      // propagation" and "stop immediate propagation" flags"
      // cf: from https://dom.spec.whatwg.org/#dom-event-stopimmediatepropagation
      this.stopPropagation();
      this[kStop] = true;
    }

    preventDefault() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      if (!this.#cancelable || this[kInPassiveListener]) return;
      this.#defaultPrevented = true;
    }

    get target() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this[kTarget];
    }

    get currentTarget() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this[kIsBeingDispatched] ? this[kTarget] : null;
    }

    get srcElement() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this[kTarget];
    }

    get type() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this[kType];
    }

    get cancelable() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this.#cancelable;
    }

    get defaultPrevented() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this.#cancelable && this.#defaultPrevented;
    }

    get timeStamp() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this.#timestamp;
    }


    // The following are non-op and unused properties/methods from Web API
    // Event. These are not supported in Node.js and are provided purely for
    // API completeness.
    composedPath() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this[kIsBeingDispatched] ? [this[kTarget]] : [];
    }

    get returnValue() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return !this.#cancelable || !this.#defaultPrevented;
    }

    set returnValue(value) {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }

      if (!value) {
        if (!this.#cancelable || this[kInPassiveListener]) return;
        this.#defaultPrevented = true;
      }
    }

    get bubbles() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this.#bubbles;
    }

    get composed() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this.#composed;
    }

    get eventPhase() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this[kIsBeingDispatched] ? Event.AT_TARGET : Event.NONE;
    }

    get cancelBubble() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      return this.#propagationStopped;
    }

    set cancelBubble(value) {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      if (value) {
        this.#propagationStopped = true;
      }
    }

    stopPropagation() {
      if (!isEvent(this)) {
        throw errInvalidThis('Event');
      }
      this.#propagationStopped = true;
    }
  }

  Object.defineProperties(
    Event.prototype, {
      [Symbol.toStringTag]: {
        __proto__: null,
        writable: false,
        enumerable: false,
        configurable: true,
        value: 'Event',
      },
      initEvent: kEnumerableProperty,
      stopImmediatePropagation: kEnumerableProperty,
      preventDefault: kEnumerableProperty,
      target: kEnumerableProperty,
      currentTarget: kEnumerableProperty,
      srcElement: kEnumerableProperty,
      type: kEnumerableProperty,
      cancelable: kEnumerableProperty,
      defaultPrevented: kEnumerableProperty,
      timeStamp: kEnumerableProperty,
      composedPath: kEnumerableProperty,
      returnValue: kEnumerableProperty,
      bubbles: kEnumerableProperty,
      composed: kEnumerableProperty,
      eventPhase: kEnumerableProperty,
      cancelBubble: kEnumerableProperty,
      stopPropagation: kEnumerableProperty,
      // Don't conform to the spec with isTrusted. The spec defines it as
      // LegacyUnforgeable but defining it in the constructor has a big
      // performance impact and the property doesn't seem to be useful
      // outside of browsers.
      isTrusted: isTrustedDescriptor,
    });

  const staticProps = ['NONE', 'CAPTURING_PHASE', 'AT_TARGET', 'BUBBLING_PHASE'];

  staticProps.forEach((staticProp, index) => {
    Object.defineProperty(Event, staticProp, {
      __proto__: null,
      writable: false,
      configurable: false,
      enumerable: true,
      value: index,
    });
  });

  function isCustomEvent(value) {
    return isEvent(value) && (value?.[kDetail] !== undefined);
  }

  class CustomEvent extends Event {
    constructor(type, options = kEmptyObject) {
      if (arguments.length === 0) {
        throw errMissingArgs('type');
      }
      super(type, options);
      this[kDetail] = options?.detail ?? null;
    }

    get detail() {
      if (!isCustomEvent(this)) {
        throw errInvalidThis('CustomEvent');
      }
      return this[kDetail];
    }
  }

  Object.defineProperties(CustomEvent.prototype, {
    [Symbol.toStringTag]: {
      __proto__: null,
      writable: false,
      enumerable: false,
      configurable: true,
      value: 'CustomEvent',
    },
    detail: kEnumerableProperty,
  });

  // Weak listener cleanup
  // This has to be lazy for snapshots to work
  let weakListenersState = null;
  // The resource needs to retain the callback so that it doesn't
  // get garbage collected now that it's weak.
  let objectToWeakListenerMap = null;
  function weakListeners() {
    weakListenersState ??= new FinalizationRegistry(
      ({ eventTarget, listener, eventType }) => eventTarget.deref()?.[kRemoveWeakListenerHelper](eventType, listener),
    );
    objectToWeakListenerMap ??= new WeakMap();
    return { registry: weakListenersState, map: objectToWeakListenerMap };
  }

  const kFlagOnce = 1 << 0;
  const kFlagCapture = 1 << 1;
  const kFlagPassive = 1 << 2;
  const kFlagNodeStyle = 1 << 3;
  const kFlagWeak = 1 << 4;
  const kFlagRemoved = 1 << 5;
  const kFlagResistStopPropagation = 1 << 6;

  // The listeners for an EventTarget are maintained as a linked list.
  // Unfortunately, the way EventTarget is defined, listeners are accounted
  // using the tuple [handler,capture], and even if we don't actually make
  // use of capture or bubbling, in order to be spec compliant we have to
  // take on the additional complexity of supporting it. Fortunately, using
  // the linked list makes dispatching faster, even if adding/removing is
  // slower.
  class Listener {
    constructor(eventTarget, eventType, previous, listener, once, capture, passive,
                isNodeStyleListener, weak, resistStopPropagation) {
      this.next = undefined;
      if (previous !== undefined) {
        previous.next = this;
      }
      this.previous = previous;
      this.listener = listener;

      let flags = 0b0;
      if (once) {
        flags |= kFlagOnce;
      }
      if (capture) {
        flags |= kFlagCapture;
      }
      if (passive) {
        flags |= kFlagPassive;
      }
      if (isNodeStyleListener) {
        flags |= kFlagNodeStyle;
      }
      if (weak) {
        flags |= kFlagWeak;
      }
      if (resistStopPropagation) {
        flags |= kFlagResistStopPropagation;
      }
      this.flags = flags;

      this.removed = false;

      if (this.weak) {
        this.callback = new WeakRef(listener);
        weakListeners().registry.register(listener, {
          __proto__: null,
          // Weak ref so the listener won't hold the eventTarget alive
          eventTarget: new WeakRef(eventTarget),
          listener: this,
          eventType,
        }, this);
        // Store only a WeakRef to the retainer here. Listener instances are
        // Strongly reachable from the EventTarget's listener list for as long
        // as they're registered, so a plain strong reference would keep the
        // retainer (and listener) alive forever, defeating weak retention
        this.weakKeyRef = new WeakRef(weak);
        let retained = weakListeners().map.get(weak);
        if (retained === undefined) {
          retained = new Set();
          weakListeners().map.set(weak, retained);
        }
        retained.add(listener);
        this.listener = this.callback;
      } else if (typeof listener === 'function') {
        this.callback = listener;
        this.listener = listener;
      } else {
        this.callback = async (...args) => {
          if (listener.handleEvent) {
            await Reflect.apply(listener.handleEvent, listener, args);
          }
        };
        this.listener = listener;
      }
    }

    get once() {
      return Boolean(this.flags & kFlagOnce);
    }
    get capture() {
      return Boolean(this.flags & kFlagCapture);
    }
    get passive() {
      return Boolean(this.flags & kFlagPassive);
    }
    get isNodeStyleListener() {
      return Boolean(this.flags & kFlagNodeStyle);
    }
    get weak() {
      return Boolean(this.flags & kFlagWeak);
    }
    get resistStopPropagation() {
      return Boolean(this.flags & kFlagResistStopPropagation);
    }
    get removed() {
      return Boolean(this.flags & kFlagRemoved);
    }
    set removed(value) {
      if (value) {
        this.flags |= kFlagRemoved;
      } else {
        this.flags &= ~kFlagRemoved;
      }
    }

    same(listener, capture) {
      const myListener = this.weak ? this.listener.deref() : this.listener;
      return myListener === listener && this.capture === capture;
    }

    remove() {
      if (this.previous !== undefined) {
        this.previous.next = this.next;
      }
      if (this.next !== undefined) {
        this.next.previous = this.previous;
      }
      this.removed = true;
      if (this.weak) {
        weakListeners().registry.unregister(this);
        const weakKey = this.weakKeyRef?.deref();
        const listener = this.callback?.deref();
        if (weakKey !== undefined && listener !== undefined) {
          const retained = weakListeners().map.get(weakKey);
          if (retained !== undefined) {
            retained.delete(listener);
            if (retained.size === 0) {
              weakListeners().map.delete(weakKey);
            }
          }
        }
      }
    }
  }

  function initEventTarget(self) {
    self[kEvents] = undefined;
    self[kMaxEventTargetListeners] = EventEmitter.defaultMaxListeners;
    self[kMaxEventTargetListenersWarned] = false;
    self[kHandlers] = undefined;
  }

  function getEventsMap(self) {
    if (self[kEvents] === undefined) {
      self[kEvents] = new Map();
    }
    return self[kEvents];
  }

  class EventTarget {
    // Used in checking whether an object is an EventTarget. This is a
    // well-known symbol as EventTarget may be used cross-realm.
    // Ref: https://github.com/nodejs/node/pull/33661
    static [kIsEventTarget] = true;

    constructor() {
      initEventTarget(this);
    }

    [kNewListener](size, type, listener, once, capture, passive, weak) {
      if (this[kMaxEventTargetListeners] > 0 &&
          size > this[kMaxEventTargetListeners] &&
          !this[kMaxEventTargetListenersWarned]) {
        this[kMaxEventTargetListenersWarned] = true;
        // No error code for this since it is a Warning
        const w = new Error('Possible EventTarget memory leak detected. ' +
                            `${size} ${type} listeners ` +
                            `added to ${getInspect()(this, { depth: -1 })}. MaxListeners is ${this[kMaxEventTargetListeners]}. Use ` +
                            'events.setMaxListeners() to increase limit');
        w.name = 'MaxListenersExceededWarning';
        w.target = this;
        w.type = type;
        w.count = size;
        process.emitWarning(w);
      }
    }
    [kRemoveListener](size, type, listener, capture) {}

    addEventListener(type, listener, options = kEmptyObject) {
      if (!isEventTarget(this)) {
        throw errInvalidThis('EventTarget');
      }
      if (arguments.length < 2) {
        throw errMissingArgs('type', 'listener');
      }

      let once = false;
      let capture = false;
      let passive = false;
      let isNodeStyleListener = false;
      let weak = false;
      let resistStopPropagation = false;

      if (options !== kEmptyObject) {
        // We validateOptions before the validateListener check because the
        // spec requires us to hit getters.
        options = validateEventListenerOptions(options);

        once = options.once;
        capture = options.capture;
        passive = options.passive;
        isNodeStyleListener = options.isNodeStyleListener;
        weak = options.weak;
        resistStopPropagation = options.resistStopPropagation;

        const signal = options.signal;

        validateAbortSignal(signal, 'options.signal');

        if (signal) {
          if (signal.aborted) {
            return;
          }
          const signalOpts = { __proto__: null, once: true };
          signalOpts[kWeakHandler] = this;
          signalOpts[kResistStopPropagation] = true;
          signal.addEventListener('abort', () => {
            this.removeEventListener(type, listener, options);
          }, signalOpts);
        }
      }

      if (!validateEventListener(listener)) {
        // The DOM silently allows passing undefined as a second argument
        // No error code for this since it is a Warning
        const w = new Error(`addEventListener called with ${listener}` +
                            ' which has no effect.');
        w.name = 'AddEventListenerArgumentTypeWarning';
        w.target = this;
        w.type = type;
        process.emitWarning(w);
        return;
      }

      type = webidlDOMString(type);

      const eventsMap = getEventsMap(this);
      let root = eventsMap.get(type);

      if (root === undefined) {
        root = { size: 1, next: undefined, resistStopPropagation: Boolean(resistStopPropagation) };
        // This is the first handler in our linked list.
        new Listener(this, type, root, listener, once, capture, passive,
                     isNodeStyleListener, weak, resistStopPropagation);
        this[kNewListener](
          root.size,
          type,
          listener,
          once,
          capture,
          passive,
          weak);
        eventsMap.set(type, root);
        return;
      }

      let handler = root.next;
      let previous = root;

      // We have to walk the linked list to see if we have a match
      while (handler !== undefined && !handler.same(listener, capture)) {
        previous = handler;
        handler = handler.next;
      }

      if (handler !== undefined) { // Duplicate! Ignore
        return;
      }

      new Listener(this, type, previous, listener, once, capture, passive,
                   isNodeStyleListener, weak, resistStopPropagation);
      root.size++;
      root.resistStopPropagation ||= Boolean(resistStopPropagation);
      this[kNewListener](root.size, type, listener, once, capture, passive, weak);
    }

    removeEventListener(type, listener, options = kEmptyObject) {
      if (!isEventTarget(this)) {
        throw errInvalidThis('EventTarget');
      }
      if (arguments.length < 2) {
        throw errMissingArgs('type', 'listener');
      }
      if (!validateEventListener(listener)) {
        return;
      }

      type = webidlDOMString(type);
      const capture = options?.capture === true;

      if (this[kEvents] === undefined) {
        return;
      }

      const root = this[kEvents].get(type);
      if (root === undefined || root.next === undefined) {
        return;
      }

      let handler = root.next;
      while (handler !== undefined) {
        if (handler.same(listener, capture)) {
          handler.remove();
          root.size--;
          if (root.size === 0) {
            this[kEvents].delete(type);
          }
          this[kRemoveListener](root.size, type, listener, capture);
          break;
        }
        handler = handler.next;
      }
    }

    [kRemoveWeakListenerHelper](type, listener) {
      if (this[kEvents] === undefined) {
        return;
      }

      const root = this[kEvents].get(type);
      if (root === undefined || root.next === undefined) {
        return;
      }

      const capture = listener.capture === true;

      let handler = root.next;
      while (handler !== undefined) {
        if (handler === listener) {
          handler.remove();
          root.size--;
          if (root.size === 0) {
            this[kEvents].delete(type);
          }
          // Undefined is passed as the listener as the listener was GCed
          this[kRemoveListener](root.size, type, undefined, capture);
          break;
        }
        handler = handler.next;
      }
    }

    dispatchEvent(event) {
      if (!isEventTarget(this)) {
        throw errInvalidThis('EventTarget');
      }
      if (arguments.length < 1) {
        throw errMissingArgs('event');
      }

      if (!(event instanceof Event)) {
        throw errInvalidArgTypeInstance('event', 'Event', event);
      }

      if (event[kIsBeingDispatched]) {
        throw errEventRecursion(event.type);
      }

      this[kHybridDispatch](event, event.type, event);
      return event.defaultPrevented !== true;
    }

    [kHybridDispatch](nodeValue, type, event) {
      if (event !== undefined) {
        event[kTarget] = this;
        event[kIsBeingDispatched] = true;
      }

      const root = this[kEvents]?.get(type);
      if (root === undefined || root.next === undefined) {
        if (event !== undefined) {
          event[kIsBeingDispatched] = false;
        }
        return true;
      }

      let handler = root.next;
      let next;

      while (handler !== undefined &&
             (root.resistStopPropagation || handler.passive || event?.[kStop] !== true)) {
        // Cache the next item in case this iteration removes the current one
        next = handler.next;

        if (handler.removed || (event?.[kStop] === true && !handler.resistStopPropagation)) {
          // Deal with the case an event is removed while event handlers are
          // Being processed (removeEventListener called from a listener)
          // And the case of event.stopImmediatePropagation() being called
          // For events not flagged as resistStopPropagation
          handler = next;
          continue;
        }
        if (handler.once) {
          handler.remove();
          root.size--;
          const { listener, capture } = handler;
          this[kRemoveListener](root.size, type, listener, capture);
        }

        let arg;
        try {
          if (handler.isNodeStyleListener) {
            arg = nodeValue;
          } else {
            if (event === undefined) {
              event = this[kCreateEvent](nodeValue, type);
              event[kTarget] = this;
              event[kIsBeingDispatched] = true;
            }
            arg = event;
          }
          const callback = handler.weak ?
            handler.callback.deref() : handler.callback;
          let result;
          if (callback) {
            if (handler.passive && !handler.isNodeStyleListener) {
              arg[kInPassiveListener] = true;
            }
            result = callback.call(this, arg);
            if (!handler.isNodeStyleListener) {
              arg[kIsBeingDispatched] = false;
            }
          }
          if (result !== undefined && result !== null) {
            etAddCatch(result);
          }
        } catch (err) {
          emitUncaughtException(err);
        } finally {
          if (arg?.[kInPassiveListener]) {
            arg[kInPassiveListener] = false;
          }
        }

        handler = next;
      }

      if (event !== undefined) {
        event[kIsBeingDispatched] = false;
      }
    }

    [kCreateEvent](nodeValue, type) {
      return new CustomEvent(type, { detail: nodeValue });
    }
    [Symbol.for('nodejs.util.inspect.custom')](depth, options) {
      const name = this.constructor.name;
      if (depth < 0) {
        return name;
      }

      const opts = Object.assign({}, options, {
        depth: Number.isInteger(options.depth) ? options.depth - 1 : options.depth,
      });

      return `${name} ${getInspect()({}, opts)}`;
    }
  }

  Object.defineProperties(EventTarget.prototype, {
    addEventListener: kEnumerableProperty,
    removeEventListener: kEnumerableProperty,
    dispatchEvent: kEnumerableProperty,
    [Symbol.toStringTag]: {
      __proto__: null,
      writable: false,
      enumerable: false,
      configurable: true,
      value: 'EventTarget',
    },
  });

  function initNodeEventTarget(self) {
    initEventTarget(self);
  }

  class NodeEventTarget extends EventTarget {
    static [kIsNodeEventTarget] = true;
    static defaultMaxListeners = 10;

    constructor() {
      super();
      initNodeEventTarget(this);
    }

    setMaxListeners(n) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      EventEmitter.setMaxListeners(n, this);
    }

    getMaxListeners() {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      return this[kMaxEventTargetListeners];
    }

    eventNames() {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      if (this[kEvents] === undefined) {
        return [];
      }
      return Array.from(this[kEvents].keys());
    }

    listenerCount(type) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      if (this[kEvents] === undefined) {
        return 0;
      }
      const root = this[kEvents].get(String(type));
      return root !== undefined ? root.size : 0;
    }

    off(type, listener, options) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      this.removeEventListener(type, listener, options);
      return this;
    }

    removeListener(type, listener, options) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      this.removeEventListener(type, listener, options);
      return this;
    }

    on(type, listener) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      const opts = { };
      opts[kIsNodeStyleListener] = true;
      this.addEventListener(type, listener, opts);
      return this;
    }

    addListener(type, listener) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      const opts = { };
      opts[kIsNodeStyleListener] = true;
      this.addEventListener(type, listener, opts);
      return this;
    }

    emit(type, arg) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      validateString(type, 'type');
      const hadListeners = this.listenerCount(type) > 0;
      this[kHybridDispatch](arg, type);
      return hadListeners;
    }

    once(type, listener) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      const opts = { once: true };
      opts[kIsNodeStyleListener] = true;
      this.addEventListener(type, listener, opts);
      return this;
    }

    removeAllListeners(type) {
      if (!isNodeEventTarget(this)) {
        throw errInvalidThis('NodeEventTarget');
      }
      if (this[kEvents] === undefined) {
        return this;
      }
      if (type !== undefined) {
        this[kEvents].delete(String(type));
      } else {
        this[kEvents].clear();
      }

      return this;
    }
  }

  Object.defineProperties(NodeEventTarget.prototype, {
    setMaxListeners: kEnumerableProperty,
    getMaxListeners: kEnumerableProperty,
    eventNames: kEnumerableProperty,
    listenerCount: kEnumerableProperty,
    off: kEnumerableProperty,
    removeListener: kEnumerableProperty,
    on: kEnumerableProperty,
    addListener: kEnumerableProperty,
    once: kEnumerableProperty,
    emit: kEnumerableProperty,
    removeAllListeners: kEnumerableProperty,
  });

  // EventTarget API

  function validateEventListener(listener) {
    if (typeof listener === 'function' ||
        typeof listener?.handleEvent === 'function') {
      return true;
    }

    if (listener == null) {
      return false;
    }

    if (typeof listener === 'object') {
      // Require `handleEvent` lazily.
      return true;
    }

    throw errInvalidArgTypeInstance('listener', 'EventListener', listener);
  }

  function validateEventListenerOptions(options) {
    if (typeof options === 'boolean') {
      return { capture: options };
    }

    if (options === null) {
      return kEmptyObject;
    }
    validateObject(options, 'options');
    return {
      once: Boolean(options.once),
      capture: Boolean(options.capture),
      passive: Boolean(options.passive),
      signal: options.signal,
      weak: options[kWeakHandler],
      resistStopPropagation: options[kResistStopPropagation] ?? false,
      isNodeStyleListener: Boolean(options[kIsNodeStyleListener]),
    };
  }

  // Test whether the argument is an event object. This is far from a
  // fool-proof test, for example this input will result in a false positive:
  // > isEventTarget({ constructor: EventTarget })
  // It stands in its current implementation as a compromise.
  // Ref: https://github.com/nodejs/node/pull/33661
  function isEventTarget(obj) {
    return obj?.constructor?.[kIsEventTarget];
  }

  function isNodeEventTarget(obj) {
    return obj?.constructor?.[kIsNodeEventTarget];
  }

  function etAddCatch(promise) {
    const then = promise.then;
    if (typeof then === 'function') {
      then.call(promise, undefined, function(err) {
        // The callback is called with nextTick to avoid a follow-up
        // rejection from this promise.
        emitUncaughtException(err);
      });
    }
  }

  function emitUncaughtException(err) {
    process.nextTick(() => { throw err; });
  }

  function makeEventHandler(handler) {
    // Event handlers are dispatched in the order they were first set
    // See https://github.com/nodejs/node/pull/35949#issuecomment-722496598
    function eventHandler(...args) {
      if (typeof eventHandler.handler !== 'function') {
        return;
      }
      return Reflect.apply(eventHandler.handler, this, args);
    }
    eventHandler.handler = handler;
    return eventHandler;
  }

  function defineEventHandler(emitter, name, event = name) {
    // 8.1.5.1 Event handlers - basically `on[eventName]` attributes
    const propName = `on${name}`;
    function get() {
      validateThisInternalField(this, kHandlers, 'EventTarget');
      return this[kHandlers]?.get(event)?.handler ?? null;
    }
    Object.defineProperty(get, 'name', {
      __proto__: null,
      value: `get ${propName}`,
    });

    function set(value) {
      validateThisInternalField(this, kHandlers, 'EventTarget');
      if (this[kHandlers] === undefined) {
        this[kHandlers] = new Map();
      }
      let wrappedHandler = this[kHandlers].get(event);
      if (wrappedHandler) {
        const eventsMap = getEventsMap(this);
        if (typeof wrappedHandler.handler === 'function') {
          const root = eventsMap.get(event);
          if (root !== undefined) {
            root.size--;
            this[kRemoveListener](root.size, event, wrappedHandler.handler, false);
          }
        }
        wrappedHandler.handler = value;
        if (typeof wrappedHandler.handler === 'function') {
          const root = eventsMap.get(event);
          if (root !== undefined) {
            root.size++;
            this[kNewListener](root.size, event, value, false, false, false, false);
          } else {
            // The wrapper was externally removed from kEvents (e.g. via
            // removeEventListener); re-register it so the handler fires.
            this.addEventListener(event, wrappedHandler);
          }
        }
      } else {
        wrappedHandler = makeEventHandler(value);
        this.addEventListener(event, wrappedHandler);
      }
      this[kHandlers].set(event, wrappedHandler);
    }
    Object.defineProperty(set, 'name', {
      __proto__: null,
      value: `set ${propName}`,
    });

    Object.defineProperty(emitter, propName, {
      __proto__: null,
      get,
      set,
      configurable: true,
      enumerable: true,
    });
  }

  const EventTargetExports = {
    Event,
    CustomEvent,
    EventTarget,
    NodeEventTarget,
    defineEventHandler,
    initEventTarget,
    initNodeEventTarget,
    kCreateEvent,
    kNewListener,
    kTrustEvent,
    kRemoveListener,
    kEvents,
    kWeakHandler,
    kResistStopPropagation,
    isEventTarget,
  };

  // The lazy cross-module reference (node: lazy
  // require('internal/event_target') inside lib/events.js) belongs to the
  // full two-module port; in this split build the events half is not
  // installed, so there is nothing to wire.

  // ════════════════════════════════════════════════════════════════════
  // Bao integration face (no node counterpart — this is the seam between
  // the port and bao's realm; see the module header for rationale). Runs
  // inside ensure_installed — i.e. on the FIRST require, in user context,
  // never during realm init (the install-time evaluation of this source is
  // what poisoned the realm during crash localization; see the header).
  // ════════════════════════════════════════════════════════════════════

  ensureProcessEmitWarning();

  const _g = globalThis;

  // node's global EventTarget/Event/CustomEvent ARE the internal module's
  // classes; replace bao's guarded WHATWG shim (zero external consumers).
  _g.Event = Event;
  _g.EventTarget = EventTarget;
  _g.CustomEvent = CustomEvent;

  // AbortSignal brand integration — node's contract: AbortSignal IS an
  // EventTarget with kMaxEventTargetListeners = 0 (the events-domain tests
  // call events.once/on/getMaxListeners/setMaxListeners on signals and
  // introspect signal[kEvents]). bao's shim AbortSignal stores listeners in
  // `this._listeners` and is not an EventTarget; brand it, route its
  // listener surface through the port, and wrap the constructor to install
  // the node slots. Idempotent via the brand check.
  const _AbortSignal = _g.AbortSignal;
  if (typeof _AbortSignal === 'function' && _AbortSignal[kIsEventTarget] !== true) {
    integrateAbortSignalFace();
  }

  function integrateAbortSignalFace() {
    const _AbortSignal = _g.AbortSignal;
    const ShimAbortSignal = _AbortSignal;
    const abortSignalDomException = (message, name) => {
      if (typeof _g.DOMException === 'function') {
        return new _g.DOMException(message, name);
      }
      const err = new Error(message);
      err.name = name;
      return err;
    };

    ShimAbortSignal[kIsEventTarget] = true;
    // INSTANCE-LEVEL integration — the shim prototype is NOT touched
    // (no setPrototypeOf, no property deletes): prototype-shape mutation on
    // a constructor whose instances are held by native code proved to be the
    // crash trigger for a subsequent realm GC (see the farm A/B: with the
    // prototype-chain version, ANY later script accessing a require-cached
    // module object segfaulted; skipping this block fixed it). Instead each
    // new signal gets the port's EventTarget internals as OWN properties,
    // which shadow the shim's prototype methods without changing them:
    //   - the three DOM-facing methods (addEventListener/removeEventListener/
    //     dispatchEvent), plus the symbol-keyed internals dispatch invokes on
    //     `this` (kNewListener/kRemoveListener/kHybridDispatch/kCreateEvent/
    //     kRemoveWeakListenerHelper) — the shim prototype chain has no
    //     EventTarget links, so the symbol hooks must ride on the instance;
    //   - the node slots via initEventTarget (kEvents map, kHandlers, and
    //     kMaxEventTargetListeners = 0: node contract — AbortSignal ignores
    //     EventEmitter.defaultMaxListeners).
    const ET_PROTO = EventTarget.prototype;
    function integrateSignal(s) {
      initEventTarget(s);
      // node: AbortSignal ignores EventEmitter.defaultMaxListeners.
      s[kMaxEventTargetListeners] = 0;
      s[kMaxEventTargetListenersWarned] = false;
      // NOTE: descriptor shorthand ({ key: fn }) would install an undefined
      // value in this engine (the bare value is treated as a generic
      // descriptor with no value) — every face below is an explicit
      // { value, ... } descriptor (verified against the bao oracle).
      const mk = (v) => ({ value: v, enumerable: false, configurable: true, writable: true });
      const props = {
        addEventListener: mk(ET_PROTO.addEventListener),
        removeEventListener: mk(ET_PROTO.removeEventListener),
        dispatchEvent: mk(ET_PROTO.dispatchEvent),
      };
      props[kNewListener] = mk(ET_PROTO[kNewListener]);
      props[kRemoveListener] = mk(ET_PROTO[kRemoveListener]);
      props[kHybridDispatch] = mk(ET_PROTO[kHybridDispatch]);
      props[kCreateEvent] = mk(ET_PROTO[kCreateEvent]);
      props[kRemoveWeakListenerHelper] = mk(ET_PROTO[kRemoveWeakListenerHelper]);
      Object.defineProperties(s, props);
      return s;
    }

    function BaoAbortSignal(...args) {
      if (new.target === undefined) {
        throw new TypeError("Constructor AbortSignal requires 'new'");
      }
      return integrateSignal(
        Reflect.construct(ShimAbortSignal, args, new.target));
    }
    BaoAbortSignal.prototype = ShimAbortSignal.prototype;
    BaoAbortSignal[kIsEventTarget] = true;
    // Carry the shim's statics (AbortSignal.timeout etc.), then override
    // with node-contract faces.
    for (const k of Object.getOwnPropertyNames(ShimAbortSignal)) {
      if (k === 'length' || k === 'name' || k === 'prototype') continue;
      BaoAbortSignal[k] = ShimAbortSignal[k];
    }

    // Re-port AbortSignal.timeout on top of the ported dispatch (the shim
    // body read this._listeners directly, which no longer receives the
    // listeners). Same validation, same TimeoutError DOMException contract.
    BaoAbortSignal.timeout = function timeout(milliseconds) {
      if (typeof milliseconds !== 'number' || !isFinite(milliseconds) || milliseconds < 0) {
        throw new TypeError('The "milliseconds" argument must be a non-negative finite number');
      }
      const signal = new BaoAbortSignal();
      setTimeout(function() {
        if (signal.aborted) return;
        signal.aborted = true;
        signal.reason = abortSignalDomException('signal timed out', 'TimeoutError');
        signal.dispatchEvent(new Event('abort'));
      }, milliseconds);
      return signal;
    };

    // node 18+: AbortSignal.abort(reason) — an already-aborted signal whose
    // reason defaults to an AbortError DOMException.
    BaoAbortSignal.abort = function abort(reason) {
      const signal = new BaoAbortSignal();
      signal.aborted = true;
      signal.reason = reason !== undefined
        ? reason
        : abortSignalDomException('The operation was aborted', 'AbortError');
      return signal;
    };

    // node 20+/WHATWG: AbortSignal.any(signals) — a fresh signal that
    // aborts with the FIRST input's abort reason. An already-aborted input
    // wins immediately in iteration order (the combined signal is born
    // aborted with that reason; NO abort event fires — node only ever fires
    // 'abort' via the abort algorithm, never on construction). The tracking
    // listeners detach once the combined signal aborts. This explicit
    // override is the integrated face: the statics loop above copies the
    // bootstrap trampoline (if the shim carries one) and this replaces it,
    // exactly like abort/timeout.
    BaoAbortSignal.any = function any(signals) {
      if (signals === null || signals === undefined ||
          typeof signals[Symbol.iterator] !== 'function') {
        const e = new TypeError(
          `The "signals" argument must be an instance of Iterable. ` +
          `Received ${determineSpecificType(signals)}`);
        e.code = 'ERR_INVALID_ARG_TYPE';
        throw e;
      }
      const list = [...signals];
      // node validates every element up-front (validateIterable +
      // per-element validateAbortSignal before any wiring).
      for (let i = 0; i < list.length; i++) {
        if (!(list[i] instanceof AbortSignal)) {
          throw errInvalidArgTypeInstance('signals', 'AbortSignal', list[i]);
        }
      }
      const combined = new BaoAbortSignal();
      for (let i = 0; i < list.length; i++) {
        if (list[i].aborted) {
          combined.aborted = true;
          combined.reason = list[i].reason;
          return combined;
        }
      }
      // Works across both dispatch faces: inputs are usually integrated
      // signals (ported addEventListener → kEvents + kHybridDispatch), but
      // a pre-integration shim signal (this._listeners, plain-object event
      // arg) delivers through its own face — ev.target carries the input
      // either way, and the input's reason propagates verbatim.
      const onInputAbort = (ev) => {
        if (combined.aborted) {
          return;
        }
        combined.aborted = true;
        combined.reason = ev?.target?.reason;
        for (let i = 0; i < list.length; i++) {
          list[i].removeEventListener('abort', onInputAbort);
        }
        combined.dispatchEvent(new Event('abort'));
      };
      for (let i = 0; i < list.length; i++) {
        list[i].addEventListener('abort', onInputAbort);
      }
      return combined;
    };

    Object.defineProperty(_g, 'AbortSignal', {
      value: BaoAbortSignal,
      writable: true,
      configurable: true,
      enumerable: false,
    });

    // AbortController wrapper: route abort() through real dispatch so the
    // port's once/on/removeEventListener semantics (kResistStopPropagation,
    // kEvents bookkeeping) hold. Keeps the shim's contract face: .signal,
    // .abort(reason), signal.aborted/reason, listener delivery, plus node's
    // repeat-abort no-op.
    const _AbortController = _g.AbortController;
    if (typeof _AbortController === 'function') {
      function BaoAbortController() {
        const signal = new BaoAbortSignal();
        this.signal = signal;
        this.abort = function(reason) {
          if (signal.aborted) {
            return; // node: abort after abort is a no-op
          }
          signal.aborted = true;
          signal.reason = reason !== undefined
            ? reason
            : abortSignalDomException('This operation was aborted', 'AbortError');
          signal.dispatchEvent(new Event('abort'));
        };
      }
      BaoAbortController.prototype = _AbortController.prototype;
      Object.defineProperty(_g, 'AbortController', {
        value: BaoAbortController,
        writable: true,
        configurable: true,
        enumerable: false,
      });
    }
  }

  return { event_target: EventTargetExports };
})()
"#;

/// Realm-init hook: installs the ABORT-SIGNAL BOOTSTRAP STATICS ONLY (see
/// `install_abort_signal_bootstrap_statics`). The module body itself MUST
/// NOT be evaluated here — a JS module-body evaluation during
/// `install_all_native` (realm init) poisons the realm (the module header's
/// GC-pressure class — any later module-body variable access then
/// segfaults). The port is evaluated lazily by [`ensure_installed`] on the
/// first `require('internal/event_target')` (require.rs wiring) or on the
/// first bootstrap-trampoline call, both in user context, when realm init
/// has fully completed.
pub fn install(cx: &mut mozjs::context::JSContext) {
    install_abort_signal_bootstrap_statics(cx);
}

// @trace REQ-ENG-006 [api:AbortSignal.abort] [api:AbortSignal.any]
/// Define the AbortSignal bootstrap statics on the realm's AbortSignal
/// constructor: native trampolines for `abort` and `any` (node v24 static
/// face; the globals.rs shim already provides `timeout` at this point and
/// is left untouched).
///
/// Why native defines and not a JS eval: realm-init JS module-body
/// evaluation poisons the realm (GC-pressure class — module header); a
/// native function-object definition at init is the proven-safe shape (the
/// events ctor's `once`/`on` trampolines in node_events::install do exactly
/// this). The trampolines carry no implementation — their first user-call
/// forces [`ensure_installed`] and re-dispatches to the port's integrated
/// static (see `abort_signal_bootstrap_redispatch`).
///
/// Absence guard: a pre-existing function face wins (the same "native
/// definition wins automatically" contract as `ensureProcessEmitWarning`
/// below), and a realm with NO AbortSignal constructor at this point gets
/// nothing — the browser Node Realm installs the globals.rs shim AFTER
/// `install_node_apis` (runtime_bridge order), so there the static face
/// appears only when the port integrates (the CLI/test realm —
/// `globals::install_all` = web → node — has the shim at hook time and gets
/// the full bootstrap face).
#[allow(unsafe_op_in_unsafe_fn)]
fn install_abort_signal_bootstrap_statics(cx: &mut mozjs::context::JSContext) {
    unsafe {
        let global = CurrentGlobalOrNull(cx.raw_cx());
        if global.is_null() {
            return;
        }
        rooted!(&in(cx) let global_root = global);
        let mut ctor_val = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            global_root.handle().into(),
            c"AbortSignal".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctor_val,
            },
        );
        if !ctor_val.is_object() || !mozjs_sys::jsapi::js::IsFunctionObject(ctor_val.to_object()) {
            return;
        }
        rooted!(&in(cx) let ctor_root = ctor_val.to_object());
        // Two explicit guard+define blocks (the node_events::install shape):
        // read the existing property — a real face already present wins
        // automatically — otherwise define the native trampoline (nargs=1:
        // node's abort(reason)/any(signals) length).
        let mut existing_abort = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            ctor_root.handle().into(),
            c"abort".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut existing_abort,
            },
        );
        if !existing_abort.is_object() {
            w2::JS_DefineFunction(
                cx,
                ctor_root.handle(),
                c"abort".as_ptr(),
                Some(abort_signal_static_abort),
                1,
                JSPROP_ENUMERATE as u32,
            );
        }
        let mut existing_any = UndefinedValue();
        JS_GetProperty(
            cx.raw_cx(),
            ctor_root.handle().into(),
            c"any".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut existing_any,
            },
        );
        if !existing_any.is_object() {
            w2::JS_DefineFunction(
                cx,
                ctor_root.handle(),
                c"any".as_ptr(),
                Some(abort_signal_static_any),
                1,
                JSPROP_ENUMERATE as u32,
            );
        }
    }
}

// @trace REQ-ENG-006 [api:AbortSignal.abort] [api:AbortSignal.any]
/// `AbortSignal.abort(reason)` bootstrap trampoline — the pre-integration
/// face. The first call evaluates the port (user context, never realm init)
/// and the integrated constructor replaces the global; this native becomes
/// unreachable.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn abort_signal_static_abort(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    abort_signal_bootstrap_redispatch(cx, argc, &args, "abort")
}

// @trace REQ-ENG-006 [api:AbortSignal.any]
/// `AbortSignal.any(signals)` bootstrap trampoline — same lazy face.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn abort_signal_static_any(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    abort_signal_bootstrap_redispatch(cx, argc, &args, "any")
}

/// Shared trampoline body: force [`ensure_installed`] (the exact
/// `events_static_once` lazy-install discipline — a JSNative frame IS user
/// context; the port body must never be evaluated during realm init), then
/// re-dispatch the call to the port's integrated static of the same name.
/// Failure leaves the engine exception pending and returns false (node
/// parity — a broken bootstrap surfaces as the real error, no silent
/// fallback).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn abort_signal_bootstrap_redispatch(
    cx: *mut JSContext,
    argc: u32,
    args: &CallArgs,
    name: &str,
) -> bool {
    if !ensure_installed(cx) {
        // Pending engine exception — propagate verbatim.
        return false;
    }
    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        throw_type_error(cx, "AbortSignal statics require a global object");
        return false;
    }
    rooted!(&in(cx_ref) let global_root = global);
    let mut ctor_val = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"AbortSignal".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor_val,
        },
    );
    if !ctor_val.is_object() || !mozjs_sys::jsapi::js::IsFunctionObject(ctor_val.to_object()) {
        throw_type_error(cx, "AbortSignal bootstrap: the constructor disappeared");
        return false;
    }
    rooted!(&in(cx_ref) let ctor_root = ctor_val.to_object());
    let name_box = ZBox::from_bytes(format!("{}\0", name).as_bytes());
    let mut static_val = UndefinedValue();
    JS_GetProperty(
        cx,
        ctor_root.handle().into(),
        name_box.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut static_val,
        },
    );
    if !static_val.is_object() || !mozjs_sys::jsapi::js::IsFunctionObject(static_val.to_object()) {
        // ensure_installed succeeded but the static face is gone — loud
        // failure, never fall back and never recurse.
        throw_type_error(
            cx,
            &format!(
                "AbortSignal.{} did not install from the event_target port",
                name
            ),
        );
        return false;
    }
    let fn_obj = static_val.to_object();
    if fn_obj == args.callee() {
        // The port evaluated WITHOUT integrating (structurally unreachable:
        // this trampoline only exists because the shim existed at init, and
        // the shim's presence drives integrateAbortSignalFace). Loud failure
        // instead of infinite recursion.
        throw_type_error(
            cx,
            &format!("AbortSignal.{} bootstrap did not integrate", name),
        );
        return false;
    }
    crate::node_events::redispatch_to_js(cx, argc, args, fn_obj)
}

/// Local TypeError reporter (shape shared with node_events::throw_type_error
/// — each node_* module carries its own).
fn throw_type_error(cx: *mut JSContext, msg: &str) {
    unsafe {
        let c_msg = ZBox::from_bytes(format!("{}\0", msg).as_bytes());
        JS_ReportErrorASCII(cx, c_msg.as_ptr());
    }
}

/// Evaluate the `internal/event_target` port and cache the module object for
/// the require() surface. Idempotent: the first call wins (the gc_store
/// cache short-circuits every later require anyway). SPLIT BUILD:
/// `builtin:events` is NOT touched — the Rust-native constructor (E10 shape)
/// stays the `events` module.
///
/// # Safety / GC contract
/// Called from `require_fn` (a JSNative frame): `cx` is live and the caller
/// propagates a pending exception when evaluation fails (Node parity — the
/// require() caller sees the real engine error, not a loader replacement).
pub fn ensure_installed(cx: *mut JSContext) -> bool {
    // Per-realm once: the gc_store cache is the idempotence boundary; a
    // thread-local flag keeps repeat failed evaluations from re-running the
    // whole source on every require attempt.
    ET_EVAL_DONE.with(|d| {
        if *d.borrow() {
            return;
        }
        *d.borrow_mut() = true;
        let source = EVENT_TARGET_ONLY_JS;
        unsafe {
            let raw_cx = cx;
            let mut source_text = mozjs::rust::transform_str_to_source_text(source);
            let mut rval = UndefinedValue();
            let rval_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            let filename = ZBox::from_bytes("<internal/event_target>".as_bytes());
            let opts = NewCompileOptions(raw_cx, filename.as_ptr(), 1);
            if opts.is_null() {
                JS_ClearPendingException(raw_cx);
                return;
            }
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            mozjs::glue::DeleteCompileOptions(opts);
            if !ok {
                // The engine's exception stays pending; require_fn returns
                // false and the caller re-throws it verbatim (Node parity).
                return;
            }
            if !rval.is_object() {
                JS_ClearPendingException(raw_cx);
                return;
            }
            let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
            let obj = rval.to_object();
            rooted!(&in(wrapped) let obj_rooted = obj);
            cache_builtin(&mut wrapped, "internal/event_target", obj_rooted.get());
        }
    });
    // Success is decided by the caller re-checking the gc_store cache.
    crate::require::get_builtin(cx, "internal/event_target").is_some()
}

thread_local! {
    static ET_EVAL_DONE: ::std::cell::RefCell<bool> = const { ::std::cell::RefCell::new(false) };
}
