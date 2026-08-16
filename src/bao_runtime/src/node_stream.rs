// @trace REQ-ENG-007
use bun_core::ZBox;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// BCE-20260816-STREAM-PUMP — the old polyfill had three defects that made the
// stream classes inert ("data 事件全灭"):
//   1. Flowing-mode entry (on('data') / resume()) set `flowing` but never
//      drained chunks already in the buffer, so the canonical
//      push-before-listen pattern (`r.push(x); r.push(null); r.on('data')`)
//      never emitted anything. Fix: a `_flow()` drain loop invoked from
//      push(), on('data') and resume(); 'end' fires when the buffer drains
//      while flowing.
//   2. The Duplex constructor assigned own `this._write`/`this._final`
//      no-ops, which SHADOW `Transform.prototype._write` (the `_transform`
//      delegator) on the prototype chain — the user transform function was
//      never invoked, so Transform/PassThrough emitted nothing. Fix:
//      own-assignment only when `opts.write` is provided; prototype-level
//      defaults on Writable/Duplex keep the plain-Writable path intact.
//   3. The async iterator dropped the waiter's `resolve` (only `reject` was
//      captured, and on data/end it was merely nulled), so a `for await`
//      racing the producer hung forever. Fix: a waiter queue woken on
//      data/end/error.
const STREAM_JS: &str = r#"
(function() {
  function EE() { this._events = {}; this._maxListeners = 10; }
  EE.prototype.on = EE.prototype.addListener = function(e, fn) {
    (this._events[e] || (this._events[e] = [])).push(fn);
    var ls = this._events[e];
    if (ls.length > this._maxListeners && !this._warned) {
      this._warned = true;
    }
    return this;
  };
  EE.prototype.once = function(e, fn) {
    var self = this;
    function w() { self.removeListener(e, w); fn.apply(this, arguments); }
    fn._onceWrapper = w;
    return this.on(e, w);
  };
  EE.prototype.emit = function(e) {
    var a = Array.prototype.slice.call(arguments, 1);
    var ls = this._events[e];
    // Node semantics: emitting 'error' with no 'error' listener throws the
    // error (uncaught-exception path). pipeline()/pipe() install error
    // forwarders first, so composed streams still propagate instead of
    // throwing at the source. BCE-20260816-EE-ERRORTHROW.
    if (e === "error" && !(ls && ls.length > 0)) {
      throw a[0];
    }
    if (ls) { ls = ls.slice(); for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); }
    return !!ls;
  };
  EE.prototype.removeListener = function(e, fn) {
    var ls = this._events[e];
    if (ls) {
      var idx = ls.indexOf(fn);
      if (idx === -1 && fn._onceWrapper) idx = ls.indexOf(fn._onceWrapper);
      if (idx >= 0) ls.splice(idx, 1);
    }
    return this;
  };
  EE.prototype.removeAllListeners = function(e) {
    if (e) { delete this._events[e]; } else { this._events = {}; }
    return this;
  };
  EE.prototype.listeners = function(e) { return (this._events[e] || []).slice(); };
  EE.prototype.listenerCount = function(e) { return (this._events[e] || []).length; };
  EE.prototype.setMaxListeners = function(n) { this._maxListeners = n; return this; };
  EE.prototype.getMaxListeners = function() { return this._maxListeners; };
  EE.prototype.prependListener = function(e, fn) {
    (this._events[e] || (this._events[e] = [])).unshift(fn);
    return this;
  };
  EE.prototype.prependOnceListener = function(e, fn) {
    var self = this;
    function w() { self.removeListener(e, w); fn.apply(this, arguments); }
    fn._onceWrapper = w;
    return this.prependListener(e, w);
  };
  EE.prototype.eventNames = function() { return Object.keys(this._events); };

  function chunkSize(c) {
    return (typeof c === "string") ? c.length : (c && c.length) || 1;
  }

  function RS(opts) {
    this.buffer = [];
    this.length = 0;
    this.ended = false;
    this.endEmitted = false;
    this.flowing = false;
    this.paused = false;
    this.hwm = (opts && opts.highWaterMark) || 16384;
    this.encoding = null;
    this.objectMode = !!(opts && opts.objectMode);
    this.destroyed = false;
  }

  function Readable(opts) {
    if (!(this instanceof Readable)) return new Readable(opts);
    EE.call(this);
    this._readableState = new RS(opts);
    if (opts && opts.read) this._read = opts.read;
    this.readable = true;
    this.destroyed = false;
  }
  Readable.prototype = Object.create(EE.prototype);
  Readable.prototype.constructor = Readable;
  // Prototype-level default so subclasses (Duplex/Transform) that do not pass
  // opts.read are not shadowed by an own-property no-op.
  Readable.prototype._read = function() {};
  // Flowing-mode pump: emits buffered chunks while flowing, then 'end' once
  // the buffer drains on an ended stream. Re-entrant safe — a data listener
  // that pauses or pushes re-enters with fresh state each iteration.
  Readable.prototype._flow = function() {
    var s = this._readableState;
    while (s.flowing && !s.paused && s.buffer.length > 0) {
      var d = s.buffer.shift();
      s.length -= chunkSize(d);
      this.emit("data", d);
    }
    if (s.flowing && !s.paused && s.ended && s.buffer.length === 0 && !s.endEmitted) {
      s.endEmitted = true;
      this.emit("end");
    }
  };
  Readable.prototype.on = function(e, fn) {
    EE.prototype.on.call(this, e, fn);
    var s = this._readableState;
    if (e === "data") {
      var firstEntry = !s.flowing;
      s.flowing = true;
      s.paused = false;
      if (firstEntry) {
        // Node starts the flow on process.nextTick, not synchronously in
        // on(): code that attaches 'end'/'error' after 'data' in the same
        // tick must observe the whole flow. Pushes landing before the
        // microtask are buffered and drained by _flow() then.
        var self = this;
        Promise.resolve().then(function() { self._read(0); self._flow(); });
      }
    }
    if (e === "readable") this._read(s.hwm);
    return this;
  };
  Readable.prototype.push = function(chunk) {
    var s = this._readableState;
    if (chunk === null) {
      s.ended = true;
      if (s.flowing && !s.paused) this._flow();
      return false;
    }
    s.buffer.push(chunk);
    s.length += chunkSize(chunk);
    this._flow();
    return s.length < s.hwm;
  };
  Readable.prototype.unshift = function(chunk) {
    var s = this._readableState;
    s.buffer.unshift(chunk);
    s.length += chunkSize(chunk);
    return this;
  };
  Readable.prototype.read = function(n) {
    var s = this._readableState;
    if (s.buffer.length > 0) {
      var d = s.buffer.shift();
      s.length -= chunkSize(d);
      if (s.ended && s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); }
      return d;
    }
    if (s.ended && !s.endEmitted) { s.endEmitted = true; this.emit("end"); }
    return null;
  };
  Readable.prototype.pipe = function(dest) {
    var src = this;
    var s = this._readableState;
    src.on("data", ondata);
    src.on("end", onend);
    src.on("error", onerror);
    if (dest.emit) dest.emit("pipe", src);
    dest.on("drain", function() { src.resume(); });
    // Source already fully consumed before pipe attached: 'end' has fired
    // and will not re-fire, so end the destination now.
    if (s.endEmitted) onend();
    function ondata(c) { if (dest.write(c) === false) src.pause(); }
    function onend() { dest.end(); }
    function onerror(e) { dest.emit("error", e); }
    return dest;
  };
  Readable.prototype.resume = function() {
    var s = this._readableState;
    if (!s.flowing || s.paused) {
      s.flowing = true;
      s.paused = false;
      this._read(0);
      this._flow();
    }
    return this;
  };
  Readable.prototype.pause = function() {
    this._readableState.flowing = false;
    this._readableState.paused = true;
    return this;
  };
  Readable.prototype.isPaused = function() { return !!this._readableState.paused; };
  Readable.prototype.setEncoding = function(enc) { this._readableState.encoding = enc; return this; };
  Readable.prototype.destroy = function(err) {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._readableState.destroyed = true;
    this._readableState.buffer = [];
    this.readable = false;
    if (err) this.emit("error", err);
    this.emit("close");
    return this;
  };
  Readable.prototype.wrap = function(stream) {
    var self = this;
    stream.on("data", function(c) { self.push(c); });
    stream.on("end", function() { self.push(null); });
    stream.on("error", function(e) { self.emit("error", e); });
    return this;
  };
  Readable.prototype[Symbol.asyncIterator] = function() {
    var self = this;
    var buf = [];
    var done = false;
    var error = null;
    var waiters = [];
    // Wake the oldest waiter with data / done / error in priority order.
    // BCE-20260816-STREAM-PUMP fix 3: the old implementation dropped the
    // waiter's resolve function, so any `for await` racing the producer
    // never woke up.
    function wake() {
      while (waiters.length > 0) {
        var w = waiters.shift();
        if (error) { w.reject(error); return; }
        if (buf.length > 0) { w.resolve({ value: buf.shift(), done: false }); continue; }
        if (done) { w.resolve({ value: undefined, done: true }); continue; }
        return;
      }
    }
    self.on("data", function(c) { buf.push(c); wake(); });
    self.on("end", function() { done = true; wake(); });
    self.on("error", function(e) { error = e; wake(); });
    return {
      next: function() {
        if (error) return Promise.reject(error);
        if (buf.length > 0) return Promise.resolve({ value: buf.shift(), done: false });
        if (done) return Promise.resolve({ value: undefined, done: true });
        return new Promise(function(res, rej) { waiters.push({ resolve: res, reject: rej }); });
      },
      return: function() { self.destroy(); return Promise.resolve({ done: true }); },
      [Symbol.asyncIterator]: function() { return this; },
    };
  };
  Readable.from = function(iterable, opts) {
    opts = opts || {};
    return new Readable({
      objectMode: opts.objectMode !== false,
      highWaterMark: opts.highWaterMark,
      read: function() {
        var self = this;
        if (Array.isArray(iterable)) {
          for (var i = 0; i < iterable.length; i++) self.push(iterable[i]);
          self.push(null);
        } else if (iterable && typeof iterable[Symbol.asyncIterator] === 'function') {
          var ai = iterable[Symbol.asyncIterator]();
          function pump() {
            ai.next().then(function(result) {
              if (result.done) { self.push(null); return; }
              self.push(result.value);
              pump();
            }).catch(function(err) { self.destroy(err); });
          }
          pump();
        } else if (iterable && typeof iterable[Symbol.iterator] === 'function') {
          var si = iterable[Symbol.iterator]();
          var item;
          while (!(item = si.next()).done) {
            self.push(item.value);
          }
          self.push(null);
        } else {
          self.push(null);
        }
      },
    });
  };

  Readable.fromWeb = function(readableStream, opts) {
    opts = opts || {};
    return new Readable({
      objectMode: true,
      highWaterMark: opts.highWaterMark,
      read: function() {
        var self = this;
        var reader = readableStream.getReader();
        function pump() {
          reader.read().then(function(result) {
            if (result.done) { self.push(null); return; }
            self.push(result.value);
            pump();
          }).catch(function(err) { self.destroy(err); });
        }
        pump();
      }
    });
  };

  Readable.toWeb = function(readable) {
    return new ReadableStream({
      start: function(controller) {
        readable.on('data', function(chunk) { controller.enqueue(chunk); });
        readable.on('end', function() { controller.close(); });
        readable.on('error', function(err) { controller.error(err); });
      }
    });
  };

  function WS(opts) {
    this.pending = 0;
    this.needDrain = false;
    this.ended = false;
    this.finished = false;
    this.hwm = (opts && opts.highWaterMark) || 16384;
    this.corked = 0;
    this.corkBuffer = [];
    this.objectMode = !!(opts && opts.objectMode);
    this.destroyed = false;
    this.defaultEncoding = (opts && opts.defaultEncoding) || "utf8";
  }

  function Writable(opts) {
    if (!(this instanceof Writable)) return new Writable(opts);
    EE.call(this);
    this._writableState = new WS(opts);
    // Own-property assignment only when the user supplied one, so subclass
    // prototype overrides (Transform.prototype._write) are never shadowed.
    if (opts && opts.write) this._write = opts.write;
    if (opts && opts.writev) this._writev = opts.writev;
    if (opts && opts.final) this._final = opts.final;
    this.writable = true;
    this.destroyed = false;
  }
  Writable.prototype = Object.create(EE.prototype);
  Writable.prototype.constructor = Writable;
  Writable.prototype._write = function(c, e, cb) { cb(); };
  Writable.prototype._final = function(cb) { cb(); };
  Writable.prototype.write = function(chunk, encoding, cb) {
    if (typeof encoding === "function") { cb = encoding; encoding = null; }
    var s = this._writableState;
    if (s.ended) { if (cb) cb(new Error("write after end")); return false; }
    if (s.corked > 0) { s.corkBuffer.push({ chunk: chunk, cb: cb }); return true; }
    var self = this;
    s.pending++;
    this._write(chunk, encoding || null, function(err) {
      s.pending--;
      if (err) self.emit("error", err);
      else if (s.pending === 0 && s.needDrain) { s.needDrain = false; self.emit("drain"); }
      if (cb) cb(err);
    });
    var ok = s.pending < s.hwm;
    if (!ok) s.needDrain = true;
    return ok;
  };
  Writable.prototype.setDefaultEncoding = function(enc) { this._writableState.defaultEncoding = enc; return this; };
  Writable.prototype.cork = function() { this._writableState.corked++; };
  Writable.prototype.uncork = function() {
    var s = this._writableState;
    if (s.corked > 0) {
      s.corked--;
      if (s.corked === 0) {
        var items = s.corkBuffer.slice();
        s.corkBuffer = [];
        for (var i = 0; i < items.length; i++) {
          this.write(items[i].chunk, null, items[i].cb);
        }
      }
    }
  };
  Writable.prototype.end = function(chunk, encoding, cb) {
    var s = this._writableState;
    if (typeof chunk === "function") { cb = chunk; chunk = null; }
    if (typeof encoding === "function") { cb = encoding; encoding = null; }
    if (chunk !== null && chunk !== undefined) this.write(chunk, encoding);
    if (s.corked > 0) this.uncork();
    s.ended = true;
    this.writable = false;
    var self = this;
    this._final(function(err) {
      // finish/close fire on a microtask (Node: process.nextTick after the
      // final write callback), so listeners attached later in the same tick
      // as end() still observe them.
      Promise.resolve().then(function() {
        s.finished = true;
        if (cb) cb(err);
        self.emit("finish");
        self.emit("close");
      });
    });
    return this;
  };
  Writable.prototype.destroy = function(err) {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._writableState.destroyed = true;
    this.writable = false;
    if (err) this.emit("error", err);
    this.emit("close");
    return this;
  };
  Writable.prototype._destroy = function(err, cb) { cb(err); };

  Writable.fromWeb = function(writableStream, opts) {
    opts = opts || {};
    return new Writable({
      highWaterMark: opts.highWaterMark,
      decodeStrings: opts.decodeStrings,
      write: function(chunk, enc, cb) {
        var result = writableStream.getWriter().write(chunk);
        if (result && typeof result.then === 'function') {
          result.then(function() { cb(); }, function(err) { cb(err); });
        } else {
          cb();
        }
      }
    });
  };

  Writable.toWeb = function(writable) {
    return new WritableStream({
      write: function(chunk) {
        return new Promise(function(resolve, reject) {
          var ok = writable.write(chunk);
          if (ok === false) {
            writable.once('drain', resolve);
          } else {
            resolve();
          }
        });
      },
      close: function() {
        return new Promise(function(resolve) {
          writable.end(resolve);
        });
      },
      abort: function(reason) {
        writable.destroy(reason);
        return Promise.resolve();
      }
    });
  };

  function Duplex(opts) {
    if (!(this instanceof Duplex)) return new Duplex(opts);
    Readable.call(this, opts);
    this._writableState = new WS(opts);
    // BCE-20260816-STREAM-PUMP fix 2: only take own `_write`/`_final` when
    // the user supplied them. The old unconditional no-op assignment here
    // shadowed Transform.prototype._write on every Transform/PassThrough,
    // so transform functions were never invoked.
    if (opts && opts.write) this._write = opts.write;
    if (opts && opts.writev) this._writev = opts.writev;
    if (opts && opts.final) this._final = opts.final;
    this.writable = true;
  }
  Duplex.prototype = Object.create(Readable.prototype);
  var skip = {on:1, once:1, emit:1, removeListener:1, removeAllListeners:1, addListener:1,
    constructor:1, listeners:1, listenerCount:1, eventNames:1, setMaxListeners:1, getMaxListeners:1,
    prependListener:1, prependOnceListener:1};
  for (var k in Writable.prototype) {
    if (!skip[k]) Duplex.prototype[k] = Writable.prototype[k];
  }
  Duplex.prototype.constructor = Duplex;

  Duplex.from = function(webStreams) {
    if (webStreams && webStreams.readable && webStreams.writable) {
      var readable = webStreams.readable;
      var writable = webStreams.writable;
      return new Duplex({
        read: function() {
          var self = this;
          var reader = (typeof ReadableStream !== 'undefined' && readable instanceof ReadableStream)
            ? readable.getReader() : null;
          if (reader) {
            function pump() {
              reader.read().then(function(result) {
                if (result.done) { self.push(null); return; }
                self.push(result.value);
                pump();
              }).catch(function(err) { self.destroy(err); });
            }
            pump();
          } else {
            readable.on('data', function(chunk) { self.push(chunk); });
            readable.on('end', function() { self.push(null); });
            readable.on('error', function(err) { self.destroy(err); });
          }
        },
        write: function(chunk, enc, cb) {
          if (typeof writable.write === 'function') {
            var result = writable.write(chunk);
            if (result && typeof result.then === 'function') {
              result.then(function() { cb(); }, function(err) { cb(err); });
            } else {
              cb();
            }
          } else { cb(); }
        }
      });
    }
    return new Duplex({
      read: function() {},
      write: function(chunk, enc, cb) { cb(); }
    });
  };

  function Transform(opts) {
    if (!(this instanceof Transform)) return new Transform(opts);
    Duplex.call(this, opts);
    this._transform = (opts && opts.transform) || function(c, e, cb) { cb(null, c); };
    this._flush = (opts && opts.flush) || function(cb) { cb(); };
  }
  Transform.prototype = Object.create(Duplex.prototype);
  Transform.prototype.constructor = Transform;
  Transform.prototype._writeTransform = function(chunk, enc, cb) {
    var self = this;
    this._transform(chunk, enc, function(err, data) {
      if (err) { self.emit("error", err); return; }
      if (data !== null && data !== undefined) self.push(data);
      cb();
    });
  };
  Transform.prototype._write = function(chunk, enc, cb) {
    this._writeTransform(chunk, enc, cb);
  };
  Transform.prototype.end = function(chunk, enc, cb) {
    var self = this;
    if (typeof chunk === "function") { cb = chunk; chunk = null; }
    if (typeof enc === "function") { cb = enc; enc = null; }
    var s = this._writableState;
    function finish(err) {
      // finish/close on a microtask — same rationale as Writable.end.
      Promise.resolve().then(function() {
        if (err) self.emit("error", err);
        if (cb) cb(err);
        self.emit("finish");
        self.emit("close");
      });
    }
    if (chunk) {
      this._writeTransform(chunk, enc, function() {
        self._flush(function(err) {
          self.push(null);
          s.ended = true;
          self.writable = false;
          finish(err);
        });
      });
    } else {
      self._flush(function(err) {
        self.push(null);
        s.ended = true;
        self.writable = false;
        finish(err);
      });
    }
    return this;
  };

  function PassThrough(opts) { if (!(this instanceof PassThrough)) return new PassThrough(opts); Transform.call(this, opts); }
  PassThrough.prototype = Object.create(Transform.prototype);
  PassThrough.prototype.constructor = PassThrough;

  function finished(stream, opts, cb) {
    if (typeof opts === "function") { cb = opts; opts = {}; }
    opts = opts || {};
    if (!stream) { if (cb) cb(new Error("stream is required")); return; }
    var finished = false;
    function done(err) {
      if (finished) return;
      finished = true;
      if (cb) cb(err);
    }
    stream.on("end", function() { if (!opts.writable) done(null); });
    stream.on("finish", function() { if (!opts.readable) done(null); });
    stream.on("error", done);
    stream.on("close", function() { done(finished ? null : new Error("premature close")); });
  }

  // BCE-20260816-STREAM-PIPELINE — pipeline previously called streams[i].on()
  // directly, so any non-stream source crashed with "streams[i].on is not a
  // function" before the callback err path could report it (async generator /
  // iterable sources are core Node pipeline inputs). normalizeStream adapts
  // them the way Node does: generator/async-iterable/iterable → Readable.from,
  // function (generator fn / duplex factory) → invoked and normalized.
  function normalizeStream(x) {
    if (typeof x === "function") x = x();
    if (x && typeof x.on === "function") return x;
    if (x && (typeof x[Symbol.asyncIterator] === "function" || typeof x[Symbol.iterator] === "function")) {
      return Readable.from(x);
    }
    return x;
  }

  function pipeline() {
    var streams = Array.prototype.slice.call(arguments);
    var cb = typeof streams[streams.length - 1] === "function" ? streams.pop() : null;
    if (streams.length < 2) { if (cb) cb(new Error("pipeline requires at least 2 streams")); return; }
    // Adapt iterable/generator sources before any .on() wiring. A stream
    // argument that is neither stream nor iterable keeps its type so the
    // callback receives the real error instead of a crash.
    var badStream = null;
    for (var ni = 0; ni < streams.length; ni++) {
      try {
        streams[ni] = normalizeStream(streams[ni]);
      } catch (normErr) {
        badStream = normErr;
        break;
      }
      if (!streams[ni] || typeof streams[ni].on !== "function") {
        badStream = new Error("pipeline: stream at index " + ni + " is not a stream");
        break;
      }
    }
    if (badStream) {
      if (cb) cb(badStream);
      else throw badStream;
      return;
    }
    var source = streams[0];
    var dest = streams[streams.length - 1];
    var errored = false;
    function onerror(err) {
      if (errored) return;
      errored = true;
      cleanup();
      if (cb) cb(err);
    }
    for (var i = 0; i < streams.length - 1; i++) {
      streams[i].on("error", onerror);
      streams[i].pipe(streams[i + 1]);
    }
    streams[streams.length - 1].on("error", onerror);
    streams[streams.length - 1].on("finish", function() {
      cleanup();
      if (cb) cb(null);
    });
    streams[streams.length - 1].on("end", function() {
      cleanup();
      if (cb) cb(null);
    });
    function cleanup() {
      for (var i = 0; i < streams.length; i++) {
        streams[i].removeListener("error", onerror);
      }
    }
    return dest;
  }

  function compose() {
    var streams = Array.prototype.slice.call(arguments);
    if (streams.length === 0) return new PassThrough();
    if (streams.length === 1) return streams[0];
    return pipeline.apply(null, streams);
  }

  function addAbortSignal(signal, stream) {
    if (signal && typeof signal.addEventListener === 'function') {
      signal.addEventListener('abort', function() {
        stream.destroy(new Error('The operation was aborted'));
      }, { once: true });
    }
    return stream;
  }

  var promises = {
    pipeline: function() {
      var streams = Array.prototype.slice.call(arguments);
      return new Promise(function(resolve, reject) {
        pipeline.apply(null, streams.concat([function(err) {
          if (err) reject(err);
          else resolve();
        }]));
      });
    },
    finished: function(stream, opts) {
      return new Promise(function(resolve, reject) {
        finished(stream, opts, function(err) {
          if (err) reject(err);
          else resolve();
        });
      });
    }
  };

  return {
    Readable: Readable,
    Writable: Writable,
    Duplex: Duplex,
    Transform: Transform,
    PassThrough: PassThrough,
    Stream: Readable,
    EventEmitter: EE,
    finished: finished,
    pipeline: pipeline,
    compose: compose,
    addAbortSignal: addAbortSignal,
    promises: promises,
  };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();
        let c_filename = ZBox::from_bytes("node:stream".as_bytes());
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(STREAM_JS);
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

        for name in &[
            "Readable",
            "Writable",
            "Duplex",
            "Transform",
            "PassThrough",
            "EventEmitter",
            "Stream",
            "finished",
            "pipeline",
            "compose",
            "addAbortSignal",
            "promises",
        ] {
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
                    mod_obj.handle().into(),
                    cname.as_ptr(),
                    val_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // Web Streams API exposure on node:stream — Node.js re-exports the
        // global Web Streams constructors (ReadableStream, WritableStream,
        // TransformStream) as named properties of `require('stream')`.
        //
        // BCE-20260816-STREAM-WEB: realms without servo's native page streams
        // (CLI mode, browser privileged evaluate realm) previously fell back
        // to an inline polyfill whose WritableStream.prototype.write
        // referenced an out-of-scope `controller` (ReferenceError — every
        // TransformStream write rejected) while ReadableStream.read() fell
        // into a setTimeout(tick, 0) poll that never terminated — any
        // TransformStream usage hung the whole event loop. The full
        // WHATWG implementation ported from Bun (web_streams.js) exists for
        // exactly this; install it and re-export the real globals.
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            // Idempotent: web_streams.js guards every constructor with
            // `typeof _g.X === "undefined"`, so realms that already carry
            // servo's native streams keep them untouched.
            crate::web_streams::install_web_streams(cx, global_root.handle());
            for name in &[
                "ReadableStream",
                "WritableStream",
                "TransformStream",
                "ByteLengthQueuingStrategy",
                "CountQueuingStrategy",
            ] {
                let cname = ZBox::from_bytes(name.as_bytes());
                let mut val = UndefinedValue();
                JS_GetProperty(
                    cx_raw,
                    global_root.handle().into(),
                    cname.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut val,
                    },
                );
                if val.is_object() {
                    rooted!(&in(cx) let val_root = val);
                    JS_DefineProperty(
                        cx_raw,
                        mod_obj.handle().into(),
                        cname.as_ptr(),
                        val_root.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            // WebStream alias (Node.js sometimes uses this name).
            let ws_cname = ZBox::from_bytes("WebStream\0".as_bytes());
            let rs_cname = ZBox::from_bytes("ReadableStream\0".as_bytes());
            let mut rs_val = UndefinedValue();
            JS_GetProperty(
                cx_raw,
                mod_obj.handle().into(),
                rs_cname.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rs_val,
                },
            );
            if rs_val.is_object() {
                rooted!(&in(cx) let rs_root = rs_val);
                JS_DefineProperty(
                    cx_raw,
                    mod_obj.handle().into(),
                    ws_cname.as_ptr(),
                    rs_root.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "stream", mod_obj.get());
    }
}
