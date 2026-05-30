use ::std::ffi::CString;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const STREAM_JS: &str = r#"
(function() {
  function EE() { this._events = {}; }
  EE.prototype.on = function(e, fn) {
    (this._events[e] || (this._events[e] = [])).push(fn);
    return this;
  };
  EE.prototype.once = function(e, fn) {
    var self = this;
    function w() { self.removeListener(e, w); fn.apply(this, arguments); }
    return this.on(e, w);
  };
  EE.prototype.emit = function(e) {
    var a = Array.prototype.slice.call(arguments, 1);
    var ls = this._events[e];
    if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a);
    return !!ls;
  };
  EE.prototype.removeListener = function(e, fn) {
    var ls = this._events[e];
    if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); }
    return this;
  };

  function RS(opts) {
    this.buffer = [];
    this.length = 0;
    this.ended = false;
    this.flowing = false;
    this.hwm = (opts && opts.highWaterMark) || 16384;
  }

  function Readable(opts) {
    EE.call(this);
    this._readableState = new RS(opts);
  }
  Readable.prototype = Object.create(EE.prototype);
  Readable.prototype.constructor = Readable;
  Readable.prototype.on = function(e, fn) {
    EE.prototype.on.call(this, e, fn);
    if (e === "data") this._readableState.flowing = true;
    return this;
  };
  Readable.prototype.push = function(chunk) {
    var s = this._readableState;
    if (chunk === null) {
      s.ended = true;
      if (s.buffer.length === 0) this.emit("end");
      return false;
    }
    s.buffer.push(chunk);
    s.length += (typeof chunk === "string") ? chunk.length : 1;
    if (s.flowing) {
      var d = s.buffer.shift();
      s.length -= (typeof d === "string") ? d.length : 1;
      this.emit("data", d);
      if (s.ended && s.buffer.length === 0) this.emit("end");
    }
    return s.length < s.hwm;
  };
  Readable.prototype.read = function() {
    var s = this._readableState;
    if (s.buffer.length > 0) {
      var d = s.buffer.shift();
      s.length -= (typeof d === "string") ? d.length : 1;
      return d;
    }
    return null;
  };
  Readable.prototype.pipe = function(dest) {
    this.on("data", function(c) { dest.write(c); });
    this.on("end", function() { dest.end(); });
    return dest;
  };
  Readable.prototype.resume = function() { this._readableState.flowing = true; return this; };
  Readable.prototype.pause = function() { this._readableState.flowing = false; return this; };
  Readable.prototype.destroy = function(err) {
    this._readableState.ended = true;
    this._readableState.buffer = [];
    if (err) this.emit("error", err);
    this.emit("close");
    return this;
  };

  function WS(opts) {
    this.buffer = [];
    this.writing = false;
    this.ended = false;
    this.hwm = (opts && opts.highWaterMark) || 16384;
  }

  function Writable(opts) {
    EE.call(this);
    this._writableState = new WS(opts);
    this._write = (opts && opts.write) || function(c, e, cb) { cb(); };
    this._final = (opts && opts.final) || function(cb) { cb(); };
  }
  Writable.prototype = Object.create(EE.prototype);
  Writable.prototype.constructor = Writable;
  Writable.prototype.write = function(chunk) {
    if (this._writableState.ended) return false;
    this._writableState.buffer.push(chunk);
    this._write(chunk, null, function() {});
    return true;
  };
  Writable.prototype.end = function(chunk) {
    if (chunk) this.write(chunk);
    this._writableState.ended = true;
    var self = this;
    this._final(function() { self.emit("finish"); self.emit("close"); });
    return this;
  };
  Writable.prototype.destroy = function(err) {
    this._writableState.ended = true;
    this._writableState.buffer = [];
    if (err) this.emit("error", err);
    this.emit("close");
    return this;
  };

  function Duplex(opts) {
    Readable.call(this, opts);
    Writable.call(this, opts);
  }
  Duplex.prototype = Object.create(Readable.prototype);
  var skip = {on:1, once:1, emit:1, removeListener:1, constructor:1};
  for (var k in Writable.prototype) {
    if (!skip[k]) Duplex.prototype[k] = Writable.prototype[k];
  }
  Duplex.prototype.constructor = Duplex;

  function Transform(opts) {
    Duplex.call(this, opts);
    this._transform = (opts && opts.transform) || function(c, e, cb) { cb(null, c); };
  }
  Transform.prototype = Object.create(Duplex.prototype);
  Transform.prototype.constructor = Transform;
  Transform.prototype._writeTransform = function(chunk, cb) {
    var self = this;
    this._transform(chunk, null, function(err, data) {
      if (err) { self.emit("error", err); return; }
      if (data) self.push(data);
      cb();
    });
  };
  Transform.prototype.write = function(chunk) {
    if (this._writableState.ended) return false;
    this._writeTransform(chunk, function() {});
    return true;
  };
  Transform.prototype.end = function(chunk) {
    var self = this;
    if (chunk) {
      this._writeTransform(chunk, function() {
        self.push(null);
        self.emit("finish");
        self.emit("close");
      });
    } else {
      self.push(null);
      self.emit("finish");
      self.emit("close");
    }
    return this;
  };

  function PassThrough(opts) { Transform.call(this, opts); }
  PassThrough.prototype = Object.create(Transform.prototype);
  PassThrough.prototype.constructor = PassThrough;

  return {
    Readable: Readable,
    Writable: Writable,
    Duplex: Duplex,
    Transform: Transform,
    PassThrough: PassThrough,
    Stream: Readable,
    EventEmitter: EE,
    finished: function(stream, cb) {
      if (!stream) return;
      stream.on("end", cb || function() {});
      stream.on("finish", cb || function() {});
      stream.on("error", cb || function() {});
    },
    pipeline: function() {
      var streams = Array.prototype.slice.call(arguments);
      var cb = typeof streams[streams.length - 1] === "function" ? streams.pop() : null;
      for (var i = 0; i < streams.length - 1; i++) {
        streams[i].pipe(streams[i + 1]);
      }
      if (cb) cb(null);
    },
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
