// @trace REQ-ENG-007
use ::std::ffi::CString;

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
    return new Readable({
      objectMode: true,
      read: function() {
        var self = this;
        if (Array.isArray(iterable)) {
          for (var i = 0; i < iterable.length; i++) self.push(iterable[i]);
          self.push(null);
        } else {
          self.push(null);
        }
      },
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
        let c_filename = CString::new("node:stream").unwrap_or_default();
        let opts = NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(STREAM_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);
        libc::free(opts as *mut _);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        let exports_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &exports_obj };

        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr };

        for name in &["Readable", "Writable", "Duplex", "Transform", "PassThrough", "EventEmitter", "Stream", "finished", "pipeline"] {
            let cname = CString::new(*name).unwrap_or_default();
            let mut val = UndefinedValue();
            JS_GetProperty(cx_raw, exports_h, cname.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
            if !val.is_undefined() {
                let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
                JS_DefineProperty(cx_raw, mod_h, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
            }
        }

        cache_builtin(cx, "stream", mod_obj.get());
    }
}
