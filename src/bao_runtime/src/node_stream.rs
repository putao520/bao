// @trace REQ-ENG-007
use bun_core::ZBox;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

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
    this._read = (opts && opts.read) || function() {};
    this.readable = true;
    this.destroyed = false;
  }
  Readable.prototype = Object.create(EE.prototype);
  Readable.prototype.constructor = Readable;
  Readable.prototype.on = function(e, fn) {
    EE.prototype.on.call(this, e, fn);
    var s = this._readableState;
    if (e === "data") {
      s.flowing = true;
      s.paused = false;
      this._read(0);
    }
    if (e === "readable") this._read(s.hwm);
    return this;
  };
  Readable.prototype.push = function(chunk) {
    var s = this._readableState;
    if (chunk === null) {
      s.ended = true;
      if (s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); }
      return false;
    }
    s.buffer.push(chunk);
    s.length += (typeof chunk === "string") ? chunk.length : (chunk && chunk.length) || 1;
    if (s.flowing && !s.paused) {
      var d = s.buffer.shift();
      s.length -= (typeof d === "string") ? d.length : (d && d.length) || 1;
      this.emit("data", d);
      if (s.ended && s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); }
    }
    return s.length < s.hwm;
  };
  Readable.prototype.unshift = function(chunk) {
    var s = this._readableState;
    s.buffer.unshift(chunk);
    s.length += (typeof chunk === "string") ? chunk.length : 1;
    return this;
  };
  Readable.prototype.read = function(n) {
    var s = this._readableState;
    if (s.buffer.length > 0) {
      var d = s.buffer.shift();
      s.length -= (typeof d === "string") ? d.length : (d && d.length) || 1;
      if (s.ended && s.buffer.length === 0 && !s.endEmitted) { s.endEmitted = true; this.emit("end"); }
      return d;
    }
    return null;
  };
  Readable.prototype.pipe = function(dest) {
    var src = this;
    src.on("data", ondata);
    src.on("end", onend);
    src.on("error", onerror);
    if (dest.emit) dest.emit("pipe", src);
    function ondata(c) { if (dest.write(c) === false) src.pause(); }
    function onend() { dest.end(); }
    function onerror(e) { dest.emit("error", e); }
    dest.on("drain", function() { src.resume(); });
    return dest;
  };
  Readable.prototype.resume = function() {
    var s = this._readableState;
    if (!s.flowing) { s.flowing = true; s.paused = false; this._read(0); }
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
    var reject = null;
    self.on("data", function(c) { buf.push(c); if (reject) { reject = null; } });
    self.on("end", function() { done = true; if (reject) { reject = null; } });
    self.on("error", function(e) { if (reject) reject(e); });
    return {
      next: function() {
        if (buf.length > 0) return Promise.resolve({ value: buf.shift(), done: false });
        if (done) return Promise.resolve({ value: undefined, done: true });
        return new Promise(function(res, rej) { reject = rej; });
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
    this.buffer = [];
    this.writing = false;
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
    this._write = (opts && opts.write) || function(c, e, cb) { cb(); };
    this._writev = (opts && opts.writev) || null;
    this._final = (opts && opts.final) || function(cb) { cb(); };
    this.writable = true;
    this.destroyed = false;
  }
  Writable.prototype = Object.create(EE.prototype);
  Writable.prototype.constructor = Writable;
  Writable.prototype.write = function(chunk, encoding, cb) {
    var s = this._writableState;
    if (s.ended) { if (cb) cb(new Error("write after end")); return false; }
    if (s.corked > 0) { s.corkBuffer.push({ chunk: chunk, cb: cb }); return true; }
    var self = this;
    this._write(chunk, encoding || null, function(err) {
      if (err) self.emit("error", err);
      else self.emit("drain");
      if (cb) cb(err);
    });
    return s.buffer.length < s.hwm;
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
    if (chunk) this.write(chunk, encoding);
    s.ended = true;
    this.writable = false;
    var self = this;
    this._final(function(err) {
      s.finished = true;
      if (cb) cb(err);
      self.emit("finish");
      self.emit("close");
    });
    return this;
  };
  Writable.prototype.destroy = function(err) {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._writableState.destroyed = true;
    this._writableState.buffer = [];
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
    this._write = (opts && opts.write) || function(c, e, cb) { cb(); };
    this._final = (opts && opts.final) || function(cb) { cb(); };
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
    if (chunk) {
      this._writeTransform(chunk, enc, function() {
        self._flush(function(err) {
          if (err) self.emit("error", err);
          self.push(null);
          s.ended = true;
          self.writable = false;
          if (cb) cb(err);
          self.emit("finish");
          self.emit("close");
        });
      });
    } else {
      self._flush(function(err) {
        if (err) self.emit("error", err);
        self.push(null);
        s.ended = true;
        self.writable = false;
        if (cb) cb(err);
        self.emit("finish");
        self.emit("close");
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

  function pipeline() {
    var streams = Array.prototype.slice.call(arguments);
    var cb = typeof streams[streams.length - 1] === "function" ? streams.pop() : null;
    if (streams.length < 2) { if (cb) cb(new Error("pipeline requires at least 2 streams")); return; }
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
        // TransformStream) as named properties of `require('stream')`. We
        // first try to forward whatever the global already provides (servo
        // installs the real WHATWG streams); if missing (CLI mode), we attach
        // a minimal pure-JS polyfill so downstream code that uses streams has
        // a working surface. See ~/code/rust/bun/src/js/node/stream.ts (Bun
        // forwards the same way).
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let mut has_readable = false;
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
                    if name == &"ReadableStream" {
                        has_readable = true;
                    }
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
            // If no global ReadableStream was found (CLI mode), install a
            // pure-JS WHATWG-flavoured polyfill directly on the stream module
            // and re-export it as a global so subsequent code (e.g. Blob's
            // `stream()` method in globals.rs) sees the same constructor.
            if !has_readable {
                let web_streams_src = r#"(function(){
  function ReadableStream(underlyingSource, strategy) {
    this._state = 'readable';
    this._disturbed = false;
    this._reader = undefined;
    this._storedError = undefined;
    this._chunks = [];
    this._closed = false;
    this._started = false;
    var self = this;
    var controller = {
      desiredSize: (strategy && typeof strategy.highWaterMark === 'number') ? strategy.highWaterMark : 1,
      enqueue: function(chunk) { if (!self._closed) self._chunks.push(chunk); },
      close: function() { self._closed = true; self._state = 'closed'; },
      error: function(e) { self._storedError = e; self._state = 'errored'; }
    };
    this._controller = controller;
    if (underlyingSource && typeof underlyingSource.start === 'function') {
      try { var r = underlyingSource.start(controller); if (r && typeof r.then === 'function') r.then(function(){ self._started = true; }); else self._started = true; } catch(e) { controller.error(e); }
    } else { self._started = true; }
  }
  ReadableStream.prototype.getReader = function() {
    var stream = this;
    return {
      get closed() {
        return stream._closed ? Promise.resolve() : new Promise(function(_, rej){ stream._rejectClose = rej; });
      },
      read: function() {
        stream._disturbed = true;
        return new Promise(function(resolve, reject) {
          function tick() {
            if (stream._chunks.length > 0) resolve({ value: stream._chunks.shift(), done: false });
            else if (stream._closed) resolve({ value: undefined, done: true });
            else if (stream._state === 'errored') reject(stream._storedError);
            else setTimeout(tick, 0);
          }
          tick();
        });
      },
      cancel: function(reason) { stream._closed = true; return Promise.resolve(); },
      releaseLock: function() {}
    };
  };
  ReadableStream.prototype.cancel = function(reason) { this._closed = true; return Promise.resolve(); };
  ReadableStream.prototype.pipeTo = function(dest) {
    var reader = this.getReader();
    var self = this;
    function pump() {
      return reader.read().then(function(r) {
        if (r.done) { if (typeof dest.close === 'function') dest.close(); return; }
        if (typeof dest.write === 'function') dest.write(r.value);
        return pump();
      });
    }
    return pump();
  };
  ReadableStream.prototype.pipeThrough = function(transform) {
    this.pipeTo(transform.writable);
    return transform.readable;
  };
  ReadableStream.prototype.tee = function() {
    return [this, this];
  };
  Object.defineProperty(ReadableStream.prototype, 'locked', { get: function() { return !!this._reader; } });

  function WritableStream(underlyingSink, strategy) {
    this._state = 'writable';
    this._written = [];
    this._closed = false;
    this._sink = underlyingSink || {};
    var self = this;
    var controller = {
      error: function(e) { self._state = 'errored'; self._storedError = e; }
    };
    this._controller = controller;
  }
  WritableStream.prototype.write = function(chunk) {
    var self = this;
    return new Promise(function(resolve, reject) {
      try {
        if (typeof self._sink.write === 'function') {
          var r = self._sink.write(chunk, controller);
          if (r && typeof r.then === 'function') r.then(resolve, reject);
          else { self._written.push(chunk); resolve(); }
        } else { self._written.push(chunk); resolve(); }
      } catch(e) { reject(e); }
    });
  };
  WritableStream.prototype.close = function() { this._closed = true; this._state = 'closed'; return Promise.resolve(); };
  WritableStream.prototype.abort = function(reason) { this._state = 'errored'; return Promise.resolve(); };
  Object.defineProperty(WritableStream.prototype, 'locked', { get: function() { return false; } });
  WritableStream.prototype.getWriter = function() {
    var stream = this;
    return {
      get closed() { return stream._closed ? Promise.resolve() : new Promise(function(){}); },
      write: function(chunk) { return stream.write(chunk); },
      close: function() { return stream.close(); },
      releaseLock: function() {},
      abort: function(r) { return stream.abort(r); }
    };
  };

  function TransformStream(transformer, strategy) {
    var self = this;
    this._transformer = transformer || {};
    this.readable = new ReadableStream();
    this.writable = new WritableStream({
      write: function(chunk) {
        if (typeof self._transformer.transform === 'function') {
          self._transformer.transform(chunk, {
            enqueue: function(c) { self.readable._controller.enqueue(c); },
            error: function(e) { self.readable._controller.error(e); }
          });
        } else {
          self.readable._controller.enqueue(chunk);
        }
      }
    });
  }

  return { ReadableStream: ReadableStream, WritableStream: WritableStream, TransformStream: TransformStream };
})()"#;
                let mut wsrc = mozjs::rust::transform_str_to_source_text(web_streams_src);
                let mut wval = UndefinedValue();
                let wh = MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut wval,
                };
                let wopts = NewCompileOptions(cx_raw, c"<web-streams>".as_ptr(), 1);
                if !wopts.is_null() {
                    if JS::Evaluate2(cx_raw, wopts, &mut wsrc, wh) && wval.is_object() {
                        let exports = wval.to_object();
                        rooted!(&in(cx) let exports_root = exports);
                        for name in &["ReadableStream", "WritableStream", "TransformStream"] {
                            let cname = ZBox::from_bytes(name.as_bytes());
                            let mut v = UndefinedValue();
                            JS_GetProperty(
                                cx_raw,
                                exports_root.handle().into(),
                                cname.as_ptr(),
                                MutableHandle::<Value> {
                                    _phantom_0: ::std::marker::PhantomData,
                                    ptr: &mut v,
                                },
                            );
                            if v.is_object() {
                                rooted!(&in(cx) let vr = v);
                                JS_DefineProperty(
                                    cx_raw,
                                    mod_obj.handle().into(),
                                    cname.as_ptr(),
                                    vr.handle().into(),
                                    JSPROP_ENUMERATE as u32,
                                );
                                JS_DefineProperty(
                                    cx_raw,
                                    global_root.handle().into(),
                                    cname.as_ptr(),
                                    vr.handle().into(),
                                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                                );
                            }
                        }
                    }
                    libc::free(wopts as *mut _);
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
