// @trace REQ-ENG-006 [api:node:diagnostics_channel]
//
// Node.js diagnostics_channel module — JS source evaluation pattern.
// Implements channel(), hasSubscribers(), subscribe(), unsubscribe(),
// tracingChannel(), Channel class, and ActiveChannel prototype swap.
// Mirrors Bun's diagnostics_channel.ts API surface.

use ::std::ptr::NonNull;
use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    let source = r#"(function() {
var channels = new Map();

function channel(name) {
  if (typeof name !== 'string' && typeof name !== 'symbol') {
    throw new TypeError('The "name" argument must be of type string or symbol');
  }
  var existing = channels.get(name);
  if (existing) return existing;
  var ch = new Channel(name);
  channels.set(name, ch);
  return ch;
}

function hasSubscribers(name) {
  var ch = channels.get(name);
  return ch ? ch.hasSubscribers : false;
}

function subscribe(name, subscription) {
  if (typeof subscription !== 'function') {
    throw new TypeError('The "subscription" argument must be of type function');
  }
  var ch = channel(name);
  ch.subscribe(subscription);
}

function unsubscribe(name, subscription) {
  var ch = channels.get(name);
  if (!ch) return false;
  return ch.unsubscribe(subscription);
}

// Channel (inactive — no subscribers)
function Channel(name) {
  this.name = name;
  this._subscribers = [];
  this._stores = new Map();
  this._active = false;
}

Channel.prototype.subscribe = function subscribe(subscription) {
  if (typeof subscription !== 'function') {
    throw new TypeError('The "subscription" argument must be of type function');
  }
  this._subscribers.push(subscription);
  this._active = true;
};

Channel.prototype.unsubscribe = function unsubscribe(subscription) {
  var idx = this._subscribers.indexOf(subscription);
  if (idx === -1) return false;
  this._subscribers.splice(idx, 1);
  if (this._subscribers.length === 0 && this._stores.size === 0) {
    this._active = false;
  }
  return true;
};

Channel.prototype.bindStore = function bindStore(store, transform) {
  this._stores.set(store, transform || null);
  this._active = true;
};

Channel.prototype.unbindStore = function unbindStore(store) {
  if (!this._stores.delete(store)) return false;
  if (this._subscribers.length === 0 && this._stores.size === 0) {
    this._active = false;
  }
  return true;
};

Object.defineProperty(Channel.prototype, 'hasSubscribers', {
  get: function() { return this._subscribers.length > 0; },
  enumerable: true,
  configurable: true
});

Channel.prototype.publish = function publish(data) {
  for (var i = 0; i < this._subscribers.length; i++) {
    try {
      this._subscribers[i](data, this.name);
    } catch (e) {
      if (typeof process !== 'undefined' && process.nextTick) {
        process.nextTick(function(err) { throw err; }, e);
      }
    }
  }
};

Channel.prototype.runStores = function runStores(data, fn, thisArg) {
  var args = [];
  for (var i = 3; i < arguments.length; i++) args.push(arguments[i]);
  // Run store bindings
  this._stores.forEach(function(transform, store) {
    var val = transform ? transform(data) : data;
    if (store && typeof store.run === 'function') {
      store.run(val, function() {
        fn.apply(thisArg, args);
      });
    } else {
      fn.apply(thisArg, args);
    }
  });
  if (this._stores.size === 0) {
    fn.apply(thisArg, args);
  }
  this.publish(data);
};

// TracingChannel
function TracingChannel(nameOrChannels) {
  if (typeof nameOrChannels === 'string') {
    this.start = channel('tracing:' + nameOrChannels + ':start');
    this.end = channel('tracing:' + nameOrChannels + ':end');
    this.asyncStart = channel('tracing:' + nameOrChannels + ':asyncStart');
    this.asyncEnd = channel('tracing:' + nameOrChannels + ':asyncEnd');
    this.error = channel('tracing:' + nameOrChannels + ':error');
  } else if (nameOrChannels && typeof nameOrChannels === 'object') {
    this.start = nameOrChannels.start;
    this.end = nameOrChannels.end;
    this.asyncStart = nameOrChannels.asyncStart;
    this.asyncEnd = nameOrChannels.asyncEnd;
    this.error = nameOrChannels.error;
  }
}

TracingChannel.prototype.subscribe = function subscribe(handlers) {
  if (handlers.start) this.start.subscribe(handlers.start);
  if (handlers.end) this.end.subscribe(handlers.end);
  if (handlers.asyncStart) this.asyncStart.subscribe(handlers.asyncStart);
  if (handlers.asyncEnd) this.asyncEnd.subscribe(handlers.asyncEnd);
  if (handlers.error) this.error.subscribe(handlers.error);
};

TracingChannel.prototype.unsubscribe = function unsubscribe(handlers) {
  var allOk = true;
  if (handlers.start) allOk = this.start.unsubscribe(handlers.start) && allOk;
  if (handlers.end) allOk = this.end.unsubscribe(handlers.end) && allOk;
  if (handlers.asyncStart) allOk = this.asyncStart.unsubscribe(handlers.asyncStart) && allOk;
  if (handlers.asyncEnd) allOk = this.asyncEnd.unsubscribe(handlers.asyncEnd) && allOk;
  if (handlers.error) allOk = this.error.unsubscribe(handlers.error) && allOk;
  return allOk;
};

TracingChannel.prototype.traceSync = function traceSync(fn, context, thisArg) {
  var args = [];
  for (var i = 3; i < arguments.length; i++) args.push(arguments[i]);
  this.start.publish(context);
  try {
    var result = fn.apply(thisArg, args);
    this.end.publish(context);
    return result;
  } catch (e) {
    this.error.publish({ ...context, error: e });
    throw e;
  }
};

TracingChannel.prototype.tracePromise = function tracePromise(fn, context, thisArg) {
  var self = this;
  var args = [];
  for (var i = 3; i < arguments.length; i++) args.push(arguments[i]);
  self.start.publish(context);
  return Promise.resolve().then(function() {
    return fn.apply(thisArg, args);
  }).then(function(result) {
    self.end.publish(context);
    return result;
  }, function(e) {
    self.error.publish({ ...context, error: e });
    throw e;
  });
};

TracingChannel.prototype.traceCallback = function traceCallback(fn, position, context, thisArg) {
  var self = this;
  var args = [];
  for (var i = 4; i < arguments.length; i++) args.push(arguments[i]);
  position = position || 0;
  var callback = args[position];
  if (typeof callback === 'function') {
    args[position] = function() {
      self.asyncStart.publish(context);
      try {
        return callback.apply(this, arguments);
      } finally {
        self.asyncEnd.publish(context);
      }
    };
  }
  return fn.apply(thisArg, args);
};

return {
  channel: channel,
  hasSubscribers: hasSubscribers,
  subscribe: subscribe,
  unsubscribe: unsubscribe,
  tracingChannel: function(nameOrChannels) { return new TracingChannel(nameOrChannels); },
  Channel: Channel
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
        let opts =
            mozjs::glue::NewCompileOptions(raw_cx, c"<node:diagnostics_channel>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            libc::free(opts as *mut _);
            if ok && rval.is_object() {
                let obj = rval.to_object();
                cache_builtin(cx, "diagnostics_channel", obj);
            }
        }
    }
}
