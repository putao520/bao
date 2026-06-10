// @trace REQ-ENG-007
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const HTTPS_JS: &str = r#"
(function() {
  function Agent(opts) {
    this.maxSockets = (opts && opts.maxSockets) || Infinity;
    this.sockets = {};
    this.requests = {};
  }
  Agent.prototype.createConnection = function(port, host, cb) {
    var net = null;
    try { net = require("net"); } catch(e) {}
    if (net && net.connect) return net.connect(port, host, cb);
    if (cb) cb(new Error("no transport"));
    return null;
  };
  Agent.prototype.destroy = function() {};

  var globalAgent = new Agent();

  function buildURL(options) {
    var host = options.hostname || options.host || "localhost";
    var port = options.port ? ":" + options.port : "";
    var path = options.path || "/";
    return "https://" + host + port + path;
  }

  function extractHeaders(options) {
    var headers = options.headers || {};
    if (!headers["Host"]) {
      var host = options.hostname || options.host || "localhost";
      if (options.port) host += ":" + options.port;
      headers["Host"] = host;
    }
    return headers;
  }

  function request(options, callback) {
    if (typeof options === "string") {
      options = { hostname: options, path: "/" };
    }

    var url = buildURL(options);
    var method = (options.method || "GET").toUpperCase();
    var headers = extractHeaders(options);
    var body = options.body || "";
    var timeout = options.timeout || options.timeoutMs || 30000;

    // Handle Buffer body
    if (body && typeof body !== "string") {
      try { body = String(body); } catch(e) { body = ""; }
    }

    var headersJSON = "{}";
    try { headersJSON = JSON.stringify(headers); } catch(e) {}

    var resultJSON = "";
    if (typeof __https_request === "function") {
      resultJSON = __https_request(url, method, headersJSON, body);
    }

    var result = {};
    try { result = JSON.parse(resultJSON); } catch(e) {
      result = { statusCode: 0, headers: {}, body: resultJSON };
    }

    var req = {};
    req.method = method;
    req.path = options.path || "/";
    req.headers = headers;

    var res = {};
    res.statusCode = result.statusCode || 0;
    res.statusMessage = result.statusMessage || "";
    res.headers = result.headers || {};
    res.httpVersion = result.httpVersion || "1.1";
    res.complete = true;

    var chunks = [];
    res._bodyText = result.body || "";

    res.on = function(event, listener) {
      if (event === "data" && listener) {
        if (res._bodyText) listener(res._bodyText);
      }
      if (event === "end" && listener) listener();
      return res;
    };
    res.pipe = function(dest) { return dest; };
    res.destroy = function() {};

    req.on = function(event, listener) {
      if (event === "response" && listener) listener(res);
      if (event === "error" && result.error && listener) listener(new Error(result.error));
      if (event === "close") {}
      return req;
    };
    req.end = function(data) {
      if (data) body = data;
      return req;
    };
    req.write = function(data) { return req; };
    req.destroy = function() {};
    req.setTimeout = function(ms, cb) { if (cb) cb(); return req; };
    req.setNoDelay = function() { return req; };
    req.setSocketKeepAlive = function() { return req; };

    if (callback) callback(res);

    return req;
  }

  function get(options, callback) {
    if (typeof options === "string") {
      options = { hostname: options, method: "GET", path: "/" };
    } else {
      options = Object.assign({}, options, { method: "GET" });
    }
    return request(options, callback);
  }

  var EE = null;
  try { EE = require("events").EventEmitter; } catch(e) {
    EE = function EE() { this._events = {}; };
    EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); } return this; };
  }

  function Server(opts, reqListener) {
    if (typeof opts === "function") { reqListener = opts; opts = {}; }
    EE.call(this);
    this._opts = opts || {};
    this.listening = false;
    if (reqListener) this.on("request", reqListener);
  }
  Server.prototype = Object.create(EE.prototype);
  Server.prototype.constructor = Server;
  Server.prototype.listen = function(port, host, cb) {
    this.listening = true;
    this._port = port;
    if (typeof host === "function") { cb = host; }
    if (cb) cb();
    return this;
  };
  Server.prototype.close = function(cb) { this.listening = false; if (cb) cb(); return this; };
  Server.prototype.setTimeout = function(ms, cb) { if (cb) cb(); return this; };

  function createServer(opts, reqListener) {
    return new Server(opts, reqListener);
  }

  return {
    request: request,
    get: get,
    Agent: Agent,
    globalAgent: globalAgent,
    Server: Server,
    createServer: createServer,
  };
})();
"#;

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn https_request(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let url = if argc > 0 && (*args.get(0).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(0).ptr).to_string()))
    } else {
        String::new()
    };

    let method = if argc > 1 && (*args.get(1).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        "GET".to_string()
    };

    // Extract headers directly from JS object — no JSON serialize/deserialize round-trip.
    // Replaces hand-written parse_simple_json_object (铁律0: use JS API instead of hand-written parser).
    let mut headers_vec: Vec<(String, String)> = Vec::new();
    if argc > 2 {
        let headers_val = *args.get(2).ptr;
        if headers_val.is_object() {
            let obj = headers_val.to_object();
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut ids = mozjs::rust::IdVector::new(cx);
            if GetPropertyKeys(cx, obj_h, JSITER_OWNONLY as u32, ids.handle_mut()) {
                for jsid in &*ids {
                    if !jsid.is_string() { continue; }
                    let key_str = jsid.to_string();
                    let key = jsstr_to_string(cx, NonNull::new_unchecked(key_str));
                    let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
                    let mut val = mozjs::jsval::UndefinedValue();
                    JS_GetProperty(cx, obj_h, c_key.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val });
                    let value = if val.is_string() {
                        jsstr_to_string(cx, NonNull::new_unchecked(val.to_string()))
                    } else {
                        String::new()
                    };
                    if !key.is_empty() {
                        headers_vec.push((key, value));
                    }
                }
            }
        }
    }

    let body = if argc > 3 && (*args.get(3).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(3).ptr).to_string()))
    } else {
        String::new()
    };

    let result = perform_https_request(&url, &method, &headers_vec, &body);

    let c_result = bun_core::ZBox::from_bytes(result.as_str().as_bytes());
    let js_result = JS_NewStringCopyZ(cx, c_result.as_ptr());
    if !js_result.is_null() {
        args.rval().set(StringValue(&*js_result));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

fn perform_https_request(url: &str, method: &str, headers: &[(String, String)], body: &str) -> String {
    let bun_method = match method {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    let result = crate::stealth_http::stealth_http_request(
        &None, bun_method, url, headers, if body.is_empty() { None } else { Some(body.as_bytes()) },
    );

    match result {
        Ok(resp) => {
            // 铁律0: use bun_core::fmt::js_printer::write_json_string for JSON object building
            use core::fmt::Write;
            use bun_core::fmt::js_printer::write_json_string;
            use bun_core::fmt::strings::Encoding;
            let mut json = String::with_capacity(256);
            json.push_str("{\"statusCode\":");
            write!(json, "{}", resp.status_code).unwrap();
            json.push_str(",\"statusMessage\":");
            write_json_string(resp.status_text.as_bytes(), &mut json, Encoding::Utf8).unwrap();
            json.push_str(",\"httpVersion\":\"1.1\",\"headers\":{");
            let mut first = true;
            for (k, v) in &resp.headers {
                if !first { json.push(','); }
                first = false;
                write_json_string(k.as_bytes(), &mut json, Encoding::Utf8).unwrap();
                json.push(':');
                write_json_string(v.as_bytes(), &mut json, Encoding::Utf8).unwrap();
            }
            json.push_str("},\"body\":");
            write_json_string(&resp.body, &mut json, Encoding::Utf8).unwrap();
            json.push('}');
            json
        }
        Err(e) => {
            use core::fmt::Write;
            use bun_core::fmt::js_printer::write_json_string;
            use bun_core::fmt::strings::Encoding;
            let mut json = String::with_capacity(128);
            json.push_str("{\"statusCode\":0,\"statusMessage\":\"\",\"httpVersion\":\"\",\"headers\":{},\"body\":\"\",\"error\":");
            write_json_string(e.as_bytes(), &mut json, Encoding::Utf8).unwrap();
            json.push('}');
            json
        }
    }
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();

        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mod_ptr,
        };
        JS_DefineFunction(
            cx_raw,
            mod_h,
            c"__https_request".as_ptr(),
            Some(https_request),
            4,
            0,
        );

        let c_filename = c"node:https".as_ptr();
        let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx_raw, c_filename, 1) as *mut _) else {
            return;
        };
        let opts = _opts_guard.as_ptr() as *const JS::ReadOnlyCompileOptions;

        let mut src = mozjs::rust::transform_str_to_source_text(HTTPS_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx_raw, opts, &mut src, rval_handle);

        if !ok || !rval.is_object() {
            return;
        }

        let exports_obj = rval.to_object();
        let exports_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &exports_obj,
        };

        let mod_ptr2 = mod_obj.get();
        let mod_h2 = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mod_ptr2,
        };

        for name in &[
            "request",
            "get",
            "Agent",
            "globalAgent",
            "Server",
            "createServer",
        ] {
            let cname = bun_core::ZBox::from_bytes(name.as_bytes());
            let mut val = UndefinedValue();
            JS_GetProperty(
                cx_raw,
                exports_h,
                cname.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut val,
                },
            );
            if !val.is_undefined() {
                let val_h = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &val,
                };
                JS_DefineProperty(
                    cx_raw,
                    mod_h2,
                    cname.as_ptr(),
                    val_h,
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        cache_builtin(cx, "https", mod_obj.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: same as deleted escape_json but now inline for tests only
    fn escape_json(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        bun_core::fmt::encode_json_string_chars(&mut out, s.as_bytes()).ok();
        out
    }

    #[test]
    fn escape_json_plain_string() {
        assert_eq!(escape_json("hello"), "hello");
    }

    #[test]
    fn escape_json_double_quote() {
        assert_eq!(escape_json(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn escape_json_backslash() {
        assert_eq!(escape_json(r"path\to\file"), r"path\\to\\file");
    }

    #[test]
    fn escape_json_newline() {
        assert_eq!(escape_json("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn escape_json_carriage_return() {
        assert_eq!(escape_json("hello\rworld"), "hello\\rworld");
    }

    #[test]
    fn escape_json_tab() {
        assert_eq!(escape_json("col1\tcol2"), "col1\\tcol2");
    }

    #[test]
    fn escape_json_control_chars() {
        let input = "bell\x07bell";
        let escaped = escape_json(input);
        assert!(escaped.contains("\\u0007"));
    }

    #[test]
    fn escape_json_empty() {
        assert_eq!(escape_json(""), "");
    }

    #[test]
    fn escape_json_mixed() {
        let input = r#"{"key":"val\nue"}"#;
        let expected = r#"{\"key\":\"val\\nue\"}"#;
        assert_eq!(escape_json(input), expected);
    }

    #[test]
    fn escape_json_unicode_preserved() {
        assert_eq!(escape_json("你好"), "你好");
    }
}
