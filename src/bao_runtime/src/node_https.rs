// @trace REQ-ENG-007
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::conversions::unsafe_jsstr_to_string;
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

  // Node-shaped request/get on the REAL async transport: `__https_request`
  // returns a pending Promise (fetch-shaped Response on resolve). The
  // client faces (ClientRequest/IncomingMessage semantics, callback form,
  // request()/get() signature normalization) come from the shared factory
  // in the http module — one implementation, two schemes.
  var __clientImpl = null;
  function impl() {
    if (__clientImpl) return __clientImpl;
    var http = null;
    try { http = require("http"); } catch (e) {}
    if (http && typeof http.__makeClient === "function") {
      __clientImpl = http.__makeClient(function (u, m, hj, b, t) {
        return __https_request(u, m, hj, b, t);
      }, "https:");
    }
    return __clientImpl;
  }

  function request(options, callback) {
    var i = impl();
    if (!i) throw new Error("https: client transport unavailable (http module failed to load)");
    return i.request.apply(null, arguments);
  }

  function get(options, callback) {
    var i = impl();
    if (!i) throw new Error("https: client transport unavailable (http module failed to load)");
    return i.get.apply(null, arguments);
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

// @trace REQ-ENG-010 [api:https.request async] [entity:FetchTasklet]
//
// BCE-20260618-007: `https.request` previously called
// `perform_https_request` → `stealth_http_request` directly inside the
// JS-native frame, blocking the JS thread on the full TLS round-trip.
// Now it returns a *pending* Promise and schedules the work on a detached
// worker via `fetch_async::start` (FetchTasklet pattern). The Promise
// resolves to a Response object (same shape as fetch()).
//
// The legacy `perform_https_request` JSON-string builder is kept for any
// internal non-JS caller that still wants the serialized form, but the
// JS-native entry no longer touches it — C2 invariant satisfied.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn https_request(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let url = if argc > 0 && (*args.get(0).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(0).ptr).to_string()))
    } else {
        String::new()
    };

    let method = if argc > 1 && (*args.get(1).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(1).ptr).to_string()))
    } else {
        "GET".to_string()
    };

    let headers_json = if argc > 2 && (*args.get(2).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(2).ptr).to_string()))
    } else {
        "{}".to_string()
    };

    // Body (arg 3): string (UTF-8) or Buffer/TypedArray/DataView/ArrayBuffer —
    // byte-exact via the house extractor (same contract as node_http's
    // http_request). The previous string-only read silently emptied every
    // binary request body; unrecognized objects fail loudly.
    let body_bytes: Option<Vec<u8>> = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_undefined() || v.is_null() {
            None
        } else if v.is_string() {
            let s = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(v.to_string()));
            (!s.is_empty()).then(|| s.into_bytes())
        } else if v.is_object() {
            match crate::node_buffer::collect_byte_view(cx, v) {
                Some(bytes) => (!bytes.is_empty()).then_some(bytes),
                None => {
                    JS_ReportErrorUTF8(
                        cx,
                        c"%s".as_ptr(),
                        c"https: request body must be a string, Buffer, TypedArray or ArrayBuffer"
                            .as_ptr(),
                    );
                    return false;
                }
            }
        } else {
            JS_ReportErrorUTF8(
                cx,
                c"%s".as_ptr(),
                c"https: request body must be a string, Buffer, TypedArray or ArrayBuffer".as_ptr(),
            );
            return false;
        }
    } else {
        None
    };

    // Node TLS options (arg 4, JSON from the client shim):
    // {rejectUnauthorized, ca: [pem...], servername}. Rides the same
    // FetchTlsInit the undici-subset `init.tls` uses — private CA anchoring
    // and verification opt-out are Node https semantics, not fetch's.
    let tls_opts_json = if argc > 4 && (*args.get(4).ptr).is_string() {
        unsafe_jsstr_to_string(cx, NonNull::new_unchecked((*args.get(4).ptr).to_string()))
    } else {
        String::new()
    };
    let tls_init: Option<crate::fetch_async::FetchTlsInit> = if tls_opts_json.is_empty() {
        None
    } else {
        let v: ::serde_json::Value = serde_json::from_str(&tls_opts_json).unwrap_or_default();
        let reject_unauthorized = v
            .get("rejectUnauthorized")
            .and_then(|x| x.as_bool());
        let servername = v
            .get("servername")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let ca_pems: Vec<String> = v
            .get("ca")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if reject_unauthorized.is_none() && servername.is_none() && ca_pems.is_empty() {
            None
        } else {
            let mut ca_der: Vec<Box<[u8]>> = Vec::new();
            for pem in &ca_pems {
                for der in bao_boringssl_bridge::pem_parse_certs(pem) {
                    ca_der.push(der.into_boxed_slice());
                }
            }
            Some(crate::fetch_async::FetchTlsInit {
                ca_certs_der: ca_der.into_boxed_slice(),
                reject_unauthorized,
                servername,
            })
        }
    };

    // Build the PENDING Promise. The network round-trip runs off the JS thread.
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let null_global = ::std::ptr::null_mut::<JSObject>());
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_global.handle().into());
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let promise_val = mozjs::jsval::ObjectValue(promise);

    // Map the JS method string to bun_http::Method (pure, no I/O).
    let bun_method = match method.as_str() {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    // Parse the headers JSON once, on the JS thread (cheap), so the worker
    // receives a ready Vec and does no JSON parsing. No network I/O here.
    let headers_vec: Vec<(String, String)> = if !headers_json.is_empty() {
        serde_json::from_str::<::std::collections::HashMap<String, String>>(&headers_json)
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    // No stealth profile for https.request (Node API parity). Stealth applies
    // to the stealth_http / fetch path.
    let profile: Option<bao_stealth::StealthProfile> = None;

    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    // The worker runs stealth_http_request off-thread; the JS thread returns
    // immediately with the pending Promise. TLS options (Node https
    // rejectUnauthorized/ca/servername) ride start_fetch's FetchTlsInit.
    unsafe {
        crate::fetch_async::start_fetch(
            cx,
            promise_val,
            profile,
            bun_method,
            url,
            headers_vec,
            body_bytes,
            None,
            tls_init,
        );
    }

    args.rval().set(promise_val);
    true
}

/// Internal *synchronous* HTTPS request wrapper: serializes the result as a
/// JSON string (the legacy shape returned by the pre-async `https.request`).
///
/// Retained per BCE-007 CONTRACT-5: internal non-JS callers that need a
/// synchronous, JSON-serialized response can use this without affecting the
/// JS-native async path. Currently no internal caller routes through it, so
/// it is `#[allow(dead_code)]` until an internal sync consumer appears.
// @trace REQ-ENG-010 [api:https.request sync wrapper (CONTRACT-5)]
#[allow(dead_code)]
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
        &None,
        bun_method,
        url,
        &headers_vec,
        if body.is_empty() {
            None
        } else {
            Some(body.as_bytes())
        },
    );

    match result {
        Ok(resp) => {
            let status_code = resp.status_code;
            let headers_json_parts: Vec<String> = resp
                .headers
                .iter()
                .map(|(k, v)| format!("\"{}\":\"{}\"", escape_json(k), escape_json(v)))
                .collect();
            let headers_str = headers_json_parts.join(",");
            // @trace REQ-PERF-001 [entity:HttpResponse]
            // 直接对 &[u8] 做 UTF-8 decode + JSON escape,消除 String::from_utf8_lossy()
            // .to_string() 的中转拷贝。lossy UTF-8 decode 用 Cow,Cow::Borrowed 零拷贝,
            // Cow::Owned 仅在非 UTF-8 时一次分配。
            let body_lossy = String::from_utf8_lossy(&resp.body);
            let response_body: &str = &body_lossy;

            format!(
                "{{\"statusCode\":{},\"statusMessage\":\"{}\",\"httpVersion\":\"1.1\",\"headers\":{{{}}},\"body\":\"{}\"}}",
                status_code,
                escape_json(&resp.status_text),
                headers_str,
                escape_json(response_body)
            )
        }
        Err(e) => {
            format!(
                "{{\"statusCode\":0,\"statusMessage\":\"\",\"httpVersion\":\"\",\"headers\":{{}},\"body\":\"\",\"error\":\"{}\"}}",
                escape_json(&e)
            )
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

        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__https_request".as_ptr(),
            Some(https_request),
            5,
            0,
        );

        // The HTTPS_JS IIFE resolves this host bridge as a FREE variable —
        // the `typeof __https_request === "function"` probe inside the IIFE
        // looks at the GLOBAL, never at this module object. Defining it only
        // on mod_obj left the probe false, so every https.request() came
        // back with statusCode 0 and an empty body. Mirror it onto the
        // global (non-enumerable, configurable) so the IIFE sees it (same
        // class as the http2 fix, commit 854677b0).
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__https_request".as_ptr(),
                Some(https_request),
                5,
                0,
            );
        }

        let c_filename = ZBox::from_bytes("node:https".as_bytes());
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
        rooted!(&in(cx) let exports_rooted = exports_obj);

        for name in &[
            "request",
            "get",
            "Agent",
            "globalAgent",
            "Server",
            "createServer",
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

        cache_builtin(cx, "https", mod_obj.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
