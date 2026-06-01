// @trace REQ-ENG-007
use ::std::ffi::CString;
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

  function Server(opts, reqListener) {
    if (typeof opts === "function") { reqListener = opts; opts = {}; }
    this._opts = opts || {};
    this.listening = false;
    if (reqListener) this.on("request", reqListener);
  }
  Server.prototype.listen = function(port, host, cb) {
    this.listening = true;
    this._port = port;
    if (typeof host === "function") { cb = host; }
    if (cb) cb();
    return this;
  };
  Server.prototype.close = function(cb) { this.listening = false; if (cb) cb(); return this; };
  Server.prototype.on = function(e, fn) { (this._listeners || (this._listeners = {}))[e] = fn; return this; };
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

    let headers_json = if argc > 2 && (*args.get(2).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(2).ptr).to_string()))
    } else {
        "{}".to_string()
    };

    let body = if argc > 3 && (*args.get(3).ptr).is_string() {
        jsstr_to_string(cx, NonNull::new_unchecked((*args.get(3).ptr).to_string()))
    } else {
        String::new()
    };

    let result = perform_https_request(&url, &method, &headers_json, &body);

    let Ok(c_result) = CString::new(result.as_str()) else {
        args.rval().set(UndefinedValue());
        return true;
    };
    let js_result = JS_NewStringCopyZ(cx, c_result.as_ptr());
    if !js_result.is_null() {
        args.rval().set(StringValue(&*js_result));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

fn perform_https_request(url: &str, method: &str, headers_json: &str, body: &str) -> String {
    let bun_method = match method {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    let headers_map: ::std::collections::HashMap<String, String> = if !headers_json.is_empty() {
        serde_json::from_str(headers_json).unwrap_or_default()
    } else {
        ::std::collections::HashMap::new()
    };
    let headers_vec: Vec<(String, String)> = headers_map.into_iter().collect();

    let result = crate::stealth_http::stealth_http_request(
        &None, bun_method, url, &headers_vec, if body.is_empty() { None } else { Some(body.as_bytes()) },
    );

    match result {
        Ok(resp) => {
            let status_code = resp.status_code;
            let headers_json_parts: Vec<String> = resp.headers.iter()
                .map(|(k, v)| format!("\"{}\":\"{}\"", escape_json(k), escape_json(v)))
                .collect();
            let headers_str = headers_json_parts.join(",");
            let response_body = String::from_utf8_lossy(&resp.body).to_string();

            format!(
                "{{\"statusCode\":{},\"statusMessage\":\"{}\",\"httpVersion\":\"1.1\",\"headers\":{{{}}},\"body\":\"{}\"}}",
                status_code,
                escape_json(&resp.status_text),
                headers_str,
                escape_json(&response_body)
            )
        }
        Err(e) => {
            format!("{{\"statusCode\":0,\"statusMessage\":\"\",\"httpVersion\":\"\",\"headers\":{{}},\"body\":\"\",\"error\":\"{}\"}}", escape_json(&e))
        }
    }
}

fn escape_json(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                result.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => result.push(ch),
        }
    }
    result
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

        let c_filename = CString::new("node:https").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(HTTPS_JS);
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
            let cname = CString::new(*name).unwrap_or_default();
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
