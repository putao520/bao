// @trace REQ-ENG-007
use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// BCE-20260816-STRINGDECODER — the old implementation buffered DECODED
// STRINGS, but decoding a buffer whose last multi-byte UTF-8 sequence is
// split across write() boundaries happens eagerly in Buffer.toString(): the
// partial bytes become U+FFFD before any "partial" bookkeeping, so
// write([0xE4,0xB8]) + write([0xAD]) returned two replacement characters
// instead of '' + '中'. Correct StringDecoder semantics hang the INCOMPLETE
// TRAILING BYTES and re-decode them joined with the next write — that
// requires byte-level buffering, implemented here on top of Buffer.
const STRING_DECODER_JS: &str = r#"
(function() {
  function toBytes(buf) {
    if (buf instanceof Uint8Array) return buf;
    if (typeof buf === 'string') return Buffer.from(buf, 'utf8');
    return new Uint8Array(0);
  }

  // Length of the longest prefix of `bytes` that ends on a complete UTF-8
  // character boundary. A trailing partial sequence (lead byte + only some of
  // its continuation bytes) is NOT included; malformed sequences are treated
  // as complete (Buffer.toString renders U+FFFD, matching Node).
  function completeUtf8Length(bytes) {
    var n = bytes.length;
    if (n === 0) return 0;
    var i = n - 1;
    var back = 0;
    while (i > 0 && (bytes[i] & 0xC0) === 0x80 && back < 3) { i--; back++; }
    var b = bytes[i];
    var need = 1;
    if (b >= 0xF0) need = 4;
    else if (b >= 0xE0) need = 3;
    else if (b >= 0xC0) need = 2;
    if (n - i >= need) return n;
    for (var j = i + 1; j < n; j++) {
      if ((bytes[j] & 0xC0) !== 0x80) return n;
    }
    return i;
  }

  function StringDecoder(encoding) {
    var enc = (encoding || 'utf8').toLowerCase();
    if (enc === 'utf-8' || enc === 'utf_8') enc = 'utf8';
    if (enc === 'ucs2' || enc === 'ucs-2') enc = 'utf16le';
    this.encoding = enc;
    this._partial = new Uint8Array(0);
  }

  StringDecoder.prototype.write = function(buf) {
    if (this.encoding !== 'utf8') {
      return Buffer.from(toBytes(buf)).toString(this.encoding);
    }
    var bytes = toBytes(buf);
    var combined = new Uint8Array(this._partial.length + bytes.length);
    combined.set(this._partial, 0);
    combined.set(bytes, this._partial.length);
    var complete = completeUtf8Length(combined);
    this._partial = combined.slice(complete);
    if (complete === 0) return '';
    return Buffer.from(combined.buffer, combined.byteOffset, complete).toString('utf8');
  };

  StringDecoder.prototype.end = function(buf) {
    var str = buf ? this.write(buf) : '';
    if (this._partial.length > 0) {
      // Leftover partial bytes decode as U+FFFD (Node semantics) and clear.
      str += Buffer.from(this._partial).toString('utf8');
      this._partial = new Uint8Array(0);
    }
    return str;
  };

  StringDecoder.prototype.text = function(buf, offset) {
    if (!offset || offset < 0) offset = 0;
    var bytes = toBytes(buf);
    if (offset >= bytes.length) {
      var keep = this._partial;
      this._partial = new Uint8Array(0);
      return keep.length ? Buffer.from(keep).toString('utf8') : '';
    }
    return this.write(bytes.subarray(offset));
  };

  StringDecoder.prototype.fill = function(buf) {
    return this.write(buf);
  };

  return {
    StringDecoder: StringDecoder,
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

        let c_filename = ZBox::from_bytes("node:string_decoder".as_bytes());
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(STRING_DECODER_JS);
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

        {
            let name = &"StringDecoder";
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

        cache_builtin(cx, "string_decoder", mod_obj.get());
    }
}
