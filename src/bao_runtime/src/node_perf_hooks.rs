// @trace REQ-ENG-007 [api:node:perf_hooks]
//
// perf_hooks module — real PerformanceMark/Measure entry buffer.
//
// Silent-fake eradication (audit item 5): the previous implementation
// returned ad-hoc plain objects from mark()/measure() with NUMERIC entryType
// (0/1) and never stored anything — performance.getEntries() did not exist,
// so marks/measures vanished. This rewrite:
//
//   - augments the GLOBAL performance object (installed by
//     web_api::install_performance) with mark/measure/getEntries/
//     getEntriesByName/getEntriesByType/clearMarks/clearMeasures, so
//     `require("perf_hooks").performance === globalThis.performance` (Node
//     identity) and entries are visible from both references;
//   - mark()/measure() produce real PerformanceMark / PerformanceMeasure
//     instances (entryType is the string 'mark' / 'measure', per the spec);
//   - entries accumulate in an insertion-ordered buffer that getEntries*
//     read and clearMarks/clearMeasures remove from;
//   - the module object re-exports now/mark/measure delegates plus
//     PerformanceEntry/PerformanceMark/PerformanceMeasure classes, a live
//     nodeTiming, timerify (real timed wrapper storing 'function' entries),
//     and timeOrigin.
//
// Module source is a JS IIFE (same pattern as node_async_hooks / node_test).

use bun_core::ZBox;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;

use crate::require::cache_builtin;

const PERF_SOURCE: &str = r#"
(function() {
  var _g = globalThis;

  // ── the performance object: augment the global one (Node identity) ──
  var perf = _g.performance;
  if (!perf || typeof perf.now !== 'function') {
    // No global performance in this scope: create a self-contained one so the
    // module surface stays functional (now() falls back to Date.now-based
    // monotonic-ish clock).
    perf = { now: function() { return Date.now(); } };
  }

  var _entries = [];

  // ── entry classes ──
  function PerformanceEntry() {
    throw new TypeError('Illegal constructor');
  }
  function PerformanceMark(name, options) {
    if (typeof name !== 'string') {
      throw new TypeError('The "name" argument must be of type string');
    }
    options = options || {};
    this.name = name;
    this.entryType = 'mark';
    this.startTime = (typeof options.startTime === 'number' && options.startTime >= 0)
      ? options.startTime
      : perf.now();
    this.duration = 0;
    this.detail = options.detail;
  }
  function PerformanceMeasure(name, startTime, duration, detail) {
    this.name = name;
    this.entryType = 'measure';
    this.startTime = startTime;
    this.duration = duration;
    this.detail = detail;
  }
  PerformanceMark.prototype = Object.create(PerformanceEntry.prototype);
  PerformanceMark.prototype.constructor = PerformanceMark;
  PerformanceMeasure.prototype = Object.create(PerformanceEntry.prototype);
  PerformanceMeasure.prototype.constructor = PerformanceMeasure;
  PerformanceEntry.prototype.name = undefined;
  PerformanceEntry.prototype.entryType = undefined;
  PerformanceEntry.prototype.startTime = undefined;
  PerformanceEntry.prototype.duration = undefined;

  // ── mark / measure ──
  function mark(name, options) {
    var entry = new PerformanceMark(name, options);
    _entries.push(entry);
    return entry;
  }

  // Resolve a start/end descriptor: number → itself; string → startTime of
  // the most recent mark with that name; undefined → undefined.
  function _resolvePoint(t) {
    if (typeof t === 'number') return t;
    if (typeof t === 'string') {
      for (var i = _entries.length - 1; i >= 0; i--) {
        if (_entries[i].entryType === 'mark' && _entries[i].name === t) {
          return _entries[i].startTime;
        }
      }
      throw new TypeError('The marker "' + t + '" does not exist');
    }
    return undefined;
  }

  function measure(name, startOrOptions, endArg) {
    if (typeof name !== 'string') {
      throw new TypeError('The "name" argument must be of type string');
    }
    var opts = (startOrOptions !== null && typeof startOrOptions === 'object') ? startOrOptions : null;
    var startRaw = opts ? opts.start : startOrOptions;
    var endRaw = opts ? opts.end : endArg;
    var detail = opts ? opts.detail : undefined;
    var start = _resolvePoint(startRaw);
    var end = _resolvePoint(endRaw);
    if (start === undefined && end === undefined) {
      start = 0;
      end = perf.now();
    } else if (start === undefined) {
      start = 0;
    } else if (end === undefined) {
      end = perf.now();
    }
    var entry = new PerformanceMeasure(name, start, end - start, detail);
    _entries.push(entry);
    return entry;
  }

  // ── buffer queries / clears (Node API surface) ──
  function getEntries() { return _entries.slice(); }
  function getEntriesByName(name, type) {
    var out = [];
    for (var i = 0; i < _entries.length; i++) {
      if (_entries[i].name !== name) continue;
      if (type !== undefined && _entries[i].entryType !== type) continue;
      out.push(_entries[i]);
    }
    return out;
  }
  function getEntriesByType(type) {
    var out = [];
    for (var i = 0; i < _entries.length; i++) {
      if (_entries[i].entryType === type) out.push(_entries[i]);
    }
    return out;
  }
  function clearMarks(name) {
    _entries = _entries.filter(function(e) {
      return !(e.entryType === 'mark' && (name === undefined || e.name === name));
    });
  }
  function clearMeasures(name) {
    _entries = _entries.filter(function(e) {
      return !(e.entryType === 'measure' && (name === undefined || e.name === name));
    });
  }

  // ── install on the performance object (identity preserved) ──
  perf.mark = mark;
  perf.measure = measure;
  perf.getEntries = getEntries;
  perf.getEntriesByName = getEntriesByName;
  perf.getEntriesByType = getEntriesByType;
  perf.clearMarks = clearMarks;
  perf.clearMeasures = clearMeasures;
  if (typeof perf.timeOrigin !== 'number') {
    // Epoch-ms of the runtime clock origin. The global performance.now() is
    // epoch-absolute here (wall clock), so the origin is the module-init
    // epoch — a real, positive, monotonic-pairing timestamp.
    perf.timeOrigin = Date.now();
  }

  // ── timerify: real timed wrapper storing 'function' entries ──
  function timerify(fn) {
    if (typeof fn !== 'function') {
      throw new TypeError('The "original" argument must be of type function');
    }
    return function() {
      var t0 = perf.now();
      try {
        return fn.apply(this, arguments);
      } finally {
        var entry = new PerformanceMeasure(fn.name || 'anonymous', t0, perf.now() - t0, null);
        entry.entryType = 'function';
        _entries.push(entry);
      }
    };
  }

  return {
    performance: perf,
    now: function() { return perf.now(); },
    mark: mark,
    measure: measure,
    PerformanceEntry: PerformanceEntry,
    PerformanceMark: PerformanceMark,
    PerformanceMeasure: PerformanceMeasure,
    nodeTiming: {
      name: 'node',
      entryType: 'node',
      startTime: 0,
      get duration() { return perf.now(); }
    },
    timerify: timerify
  };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    unsafe {
        let raw_cx = cx.raw_cx();
        let c_filename = ZBox::from_bytes("node:perf_hooks".as_bytes());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(PERF_SOURCE);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            log::warn!("perf_hooks: failed to evaluate module source");
            return;
        }

        let exports_obj = rval.to_object();
        rooted!(&in(cx) let exports_root = exports_obj);
        cache_builtin(cx, "perf_hooks", exports_root.get());
    }
}
