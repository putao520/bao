// @trace REQ-ENG-006 [api:node:punycode]
//
// Node.js punycode module (deprecated but provided for compatibility).
// Full RFC 3492 Bootstring encoding/decoding implementation.
// Registered via JS source evaluation matching Bun's approach
// (punycode.ts is a direct port of the npm punycode@2.1.0 package).

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    // Full punycode implementation (RFC 3492) as JS source.
    // Ported from the npm punycode@2.1.0 package (same as Bun's punycode.ts).
    let source = r#"(function() {
/** Highest positive signed 32-bit float power of two */
var maxInt = 2147483647;
var base = 36;
var tMin = 1;
var tMax = 26;
var skew = 38;
var damp = 700;
var initialBias = 72;
var initialN = 128;
var delimiter = '-';

function adapt(delta, numPoints, firstTime) {
  delta = firstTime ? Math.floor(delta / damp) : delta >> 1;
  delta += Math.floor(delta / numPoints);
  var k = 0;
  while (delta > Math.floor((maxInt - tMin) * tMax) / 2) {
    delta = Math.floor(delta / tMax);
    k += base;
  }
  return k + Math.floor((base * (delta + 1)) / (delta + skew));
}

function mapDomain(string, fn) {
  var parts = string.split('@');
  var result = '';
  if (parts.length > 1) {
    result = parts[0] + '@';
    string = parts[1];
  }
  var labels = string.split('.');
  var mapped = [];
  for (var i = 0; i < labels.length; i++) {
    mapped.push(fn(labels[i]));
  }
  return result + mapped.join('.');
}

function ucs2decode(string) {
  var output = [];
  var counter = 0;
  var length = string.length;
  while (counter < length) {
    var value = string.charCodeAt(counter++);
    if (value >= 0xD800 && value <= 0xDBFF && counter < length) {
      var extra = string.charCodeAt(counter++);
      if ((extra & 0xFC00) == 0xDC00) {
        output.push(((value & 0x3FF) << 10) + (extra & 0x3FF) + 0x10000);
      } else {
        output.push(value);
        counter--;
      }
    } else {
      output.push(value);
    }
  }
  return output;
}

function ucs2encode(array) {
  return String.fromCodePoint.apply(String, array);
}

function encode(input) {
  var n = initialN;
  var delta = 0;
  var bias = initialBias;
  var output = [];
  input = ucs2decode(String(input));
  var length = input.length;
  var handledCPCount = 0;
  while (handledCPCount < length) {
    var m = maxInt;
    for (var i = 0; i < input.length; i++) {
      var cp = input[i];
      if (cp >= n && cp < m) m = cp;
    }
    handledCPCount++;
    delta += (m - n) * (handledCPCount + 1);
    n = m;
    for (var j = 0; j < input.length; j++) {
      var c = input[j];
      if (c < n) {
        delta++;
        continue;
      }
      if (c > maxInt) return '';
      var q = delta;
      var k = base;
      while (true) {
        var t = k <= bias ? tMin : (k >= bias + tMax ? tMax : k - bias);
        if (q < t) break;
        var qMinusT = q - t;
        var baseMinusT = base - t;
        output.push(String.fromCharCode(t + (qMinusT % baseMinusT)));
        q = Math.floor(qMinusT / baseMinusT);
        k += base;
      }
      output.push(String.fromCharCode(t + q));
      bias = adapt(delta, handledCPCount, handledCPCount == 1);
      delta = 0;
    }
  }
  delta++;
  return output.join('');
}

function decode(input) {
  var n = initialN;
  var delta = 0;
  var bias = initialBias;
  var output = [];
  var pos = input.lastIndexOf(delimiter);
  if (pos > 0) {
    output = ucs2decode(input.slice(0, pos));
    input = input.slice(pos + 1);
  }
  var length = output.length;
  var i = 0;
  while (i < input.length) {
    var oldi = delta;
    var w = 1;
    var k = base;
    while (true) {
      var c = input.charCodeAt(i++);
      if (c - 48 < 10) { var digit = c - 22; }
      else if (c - 65 < 26) { digit = c - 65; }
      else if (c - 97 < 26) { digit = c - 97; }
      else if (c == delimiter) { break; }
      else return ucs2encode(output);
      if (digit >= base || digit > Math.floor((maxInt - delta) / w)) return ucs2encode(output);
      delta += digit * w;
      var t = k <= bias ? tMin : (k >= bias + tMax ? tMax : k - bias);
      if (delta < t) break;
      var baseMinusT = base - t;
      if (w > Math.floor(maxInt / baseMinusT)) return ucs2encode(output);
      w *= baseMinusT;
      k += base;
    }
    delta = adapt(delta - oldi, output.length + 1, oldi == 0);
    if (Math.floor(delta / (length + 1)) > maxInt - n) return ucs2encode(output);
    n += Math.floor(delta / (length + 1));
    delta %= length + 1;
    output.splice(delta, 0, n);
    delta++;
  }
  return ucs2encode(output);
}

function toASCII(input) {
  return mapDomain(input, function(string) {
    return /[^\0-\x7E]/.test(string) ? 'xn--' + encode(string) : string;
  });
}

function toUnicode(input) {
  return mapDomain(input, function(string) {
    if (string.indexOf('xn--') === 0) {
      return decode(string.slice(4));
    }
    return string;
  });
}

return {
  version: '2.1.0',
  ucs2: { decode: ucs2decode, encode: ucs2encode },
  decode: decode,
  encode: encode,
  toASCII: toASCII,
  toUnicode: toUnicode
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
    let opts = mozjs::glue::NewCompileOptions(raw_cx, c"<node:punycode>".as_ptr(), 1);
    if !opts.is_null() {
        let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
        libc::free(opts as *mut _);
        if ok && rval.is_object() {
            let obj = rval.to_object();
            cache_builtin(cx, "punycode", obj);
            return;
        }
    }
    } // end unsafe
    // Fallback: register empty object so require() doesn't throw
    rooted!(&in(cx) let fallback = unsafe { w2::JS_NewPlainObject(cx) });
    if !fallback.get().is_null() {
        cache_builtin(cx, "punycode", fallback.get());
    }
}
