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

  return {
    request: request,
    get: get,
    Agent: Agent,
    globalAgent: globalAgent,
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
    let minreq_method = match method {
        "GET" => minreq::Method::Get,
        "POST" => minreq::Method::Post,
        "PUT" => minreq::Method::Put,
        "DELETE" => minreq::Method::Delete,
        "PATCH" => minreq::Method::Patch,
        "HEAD" => minreq::Method::Head,
        "TRACE" => minreq::Method::Trace,
        "OPTIONS" => minreq::Method::Options,
        _ => minreq::Method::Get,
    };

    let mut req = minreq::Request::new(minreq_method, url);

    if !body.is_empty() {
        req = req.with_body(body.as_bytes());
    }

    if !headers_json.is_empty() {
        if let Ok(headers) = serde_json_like_parse(headers_json) {
            for (key, value) in headers {
                req = req.with_header(key.as_str(), value.as_str());
            }
        }
    }

    req = req.with_timeout(30);

    match req.send() {
        Ok(response) => {
            let status_code = response.status_code;
            let reason = match status_code {
                200 => "OK",
                201 => "Created",
                204 => "No Content",
                301 => "Moved Permanently",
                302 => "Found",
                304 => "Not Modified",
                400 => "Bad Request",
                401 => "Unauthorized",
                403 => "Forbidden",
                404 => "Not Found",
                405 => "Method Not Allowed",
                500 => "Internal Server Error",
                502 => "Bad Gateway",
                503 => "Service Unavailable",
                _ => "",
            };

            let mut headers_map = Vec::new();
            for (key, value) in response.headers.iter() {
                headers_map.push(format!("\"{}\":\"{}\"", escape_json(key), escape_json(value)));
            }
            let headers_str = headers_map.join(",");

            let response_body = String::from_utf8_lossy(response.as_bytes()).into_owned();

            format!(
                "{{\"statusCode\":{},\"statusMessage\":\"{}\",\"httpVersion\":\"1.1\",\"headers\":{{{}}},\"body\":\"{}\"}}",
                status_code,
                reason,
                headers_str,
                escape_json(&response_body)
            )
        }
        Err(e) => {
            format!("{{\"statusCode\":0,\"statusMessage\":\"\",\"httpVersion\":\"\",\"headers\":{{}},\"body\":\"\",\"error\":\"{}\"}}", escape_json(&e.to_string()))
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

fn serde_json_like_parse(json: &str) -> ::std::result::Result<Vec<(String, String)>, ()> {
    let mut result = Vec::new();
    let trimmed = json.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(());
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if inner.trim().is_empty() {
        return Ok(result);
    }

    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    let mut token_start = 0;
    let mut current_key: Option<String> = None;

    for (i, ch) in inner.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '{' || ch == '[' {
            depth += 1;
        } else if ch == '}' || ch == ']' {
            depth -= 1;
        } else if depth == 0 && ch == ':' {
            let key_str = inner[token_start..i].trim().trim_matches('"').to_string();
            current_key = Some(key_str);
            token_start = i + 1;
        } else if depth == 0 && ch == ',' {
            if let Some(ref _key) = current_key {
                let val_str = inner[token_start..i].trim().trim_matches('"').to_string();
                result.push((current_key.take().unwrap(), val_str));
            }
            token_start = i + 1;
            current_key = None;
        }
    }

    if let Some(key) = current_key {
        let val_str = inner[token_start..].trim().trim_matches('"').to_string();
        result.push((key, val_str));
    }

    Ok(result)
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

        cache_builtin("https", mod_obj.get());
    }
}
