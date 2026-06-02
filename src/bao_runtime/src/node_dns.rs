// @trace REQ-ENG-007
use ::std::ffi::CString;
use ::std::net::ToSocketAddrs;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const DNS_JS: &str = r#"
(function() {
  function Resolver() {
    this._servers = [];
  }
  Resolver.prototype.resolve = function(hostname, rrtype, callback) {
    if (typeof rrtype === "function") { callback = rrtype; rrtype = "A"; }
    if (typeof __dns_resolve === "function") {
      var result = __dns_resolve(hostname, rrtype || "A");
      if (callback) callback(null, result);
      return result;
    }
    if (callback) callback(new Error("dns.resolve not available"));
    return [];
  };
  Resolver.prototype.resolve4 = function(hostname, callback) {
    return this.resolve(hostname, "A", callback);
  };
  Resolver.prototype.resolve6 = function(hostname, callback) {
    if (typeof __dns_resolve6 === "function") {
      var result = __dns_resolve6(hostname);
      if (callback) callback(null, result);
      return result;
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.getServers = function() {
    return this._servers.slice();
  };
  Resolver.prototype.setServers = function(servers) {
    this._servers = Array.isArray(servers) ? servers.slice() : [];
  };

  function lookup(hostname, options, callback) {
    if (typeof options === "function") { callback = options; options = null; }
    if (typeof __dns_lookup === "function") {
      var result = __dns_lookup(hostname);
      if (callback) callback(null, result.address, result.family);
      return result;
    }
    var err = new Error("dns.lookup not available");
    if (callback) callback(err);
    throw err;
  }

  function resolve(hostname, rrtype, callback) {
    if (typeof rrtype === "function") { callback = rrtype; rrtype = "A"; }
    if (typeof __dns_resolve === "function") {
      var result = __dns_resolve(hostname, rrtype || "A");
      if (callback) callback(null, result);
      return result;
    }
    if (callback) callback(new Error("dns.resolve not available"));
    return [];
  }

  function resolve4(hostname, callback) {
    return resolve(hostname, "A", callback);
  }

  function resolve6(hostname, callback) {
    if (typeof __dns_resolve6 === "function") {
      var result = __dns_resolve6(hostname);
      if (callback) callback(null, result);
      return result;
    }
    if (callback) callback(null, []);
    return [];
  }

  function reverse(ip, callback) {
    if (typeof __dns_reverse === "function") {
      var result = __dns_reverse(ip);
      if (callback) callback(null, result);
      return result;
    }
    if (callback) callback(null, []);
    return [];
  }

  function lookupService(address, port, callback) {
    if (typeof callback === "function") {
      callback(null, { service: "unknown", hostname: address });
    }
    return { service: "unknown", hostname: address };
  }

  function getServers() {
    return [];
  }

  function setServers(servers) {
    // no-op
  }

  return {
    lookup: lookup,
    resolve: resolve,
    resolve4: resolve4,
    resolve6: resolve6,
    reverse: reverse,
    lookupService: lookupService,
    getServers: getServers,
    setServers: setServers,
    Resolver: Resolver
  };
})();
"#;

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_lookup(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"dns.lookup requires a hostname argument".as_ptr(),
        );
        return false;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"dns.lookup hostname must be a string".as_ptr(),
        );
        return false;
    }

    let hostname =
        jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let result_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &result_obj,
    };

    match (hostname.as_str(), 0u16).to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.next() {
                let ip = addr.ip().to_string();
                let family = if addr.is_ipv4() { 4 } else { 6 };

                if let Ok(c_ip) = CString::new(ip.as_str()) {
                    let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                    if !js_str.is_null() {
                        let ip_val = StringValue(&*js_str);
                        let ip_h = Handle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &ip_val,
                        };
                        JS_DefineProperty(
                            cx,
                            result_h,
                            c"address".as_ptr(),
                            ip_h,
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }

                let family_val = Int32Value(family);
                let family_h = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &family_val,
                };
                JS_DefineProperty(
                    cx,
                    result_h,
                    c"family".as_ptr(),
                    family_h,
                    JSPROP_ENUMERATE as u32,
                );
            } else {
                define_empty_lookup_result(cx, result_h);
            }
        }
        Err(_) => {
            define_empty_lookup_result(cx, result_h);
        }
    }

    args.rval().set(ObjectValue(result_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_empty_lookup_result(cx: *mut JSContext, result_h: Handle<*mut JSObject>) {
    let js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !js_str.is_null() {
        let ip_val = StringValue(&*js_str);
        let ip_h = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &ip_val,
        };
        JS_DefineProperty(cx, result_h, c"address".as_ptr(), ip_h, JSPROP_ENUMERATE as u32);
    }
    let family_val = Int32Value(4);
    let family_h = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &family_val,
    };
    JS_DefineProperty(cx, result_h, c"family".as_ptr(), family_h, JSPROP_ENUMERATE as u32);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolve(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"dns.resolve requires a hostname argument".as_ptr(),
        );
        return false;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"dns.resolve hostname must be a string".as_ptr(),
        );
        return false;
    }

    let hostname =
        jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &arr_obj,
    };

    match (hostname.as_str(), 0u16).to_socket_addrs() {
        Ok(addrs) => {
            let mut idx = 0u32;
            for addr in addrs {
                let ip = addr.ip().to_string();
                if let Ok(c_ip) = CString::new(ip.as_str()) {
                    let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                    if !js_str.is_null() {
                        let val = StringValue(&*js_str);
                        let val_h = Handle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &val,
                        };
                        JS_DefineElement(cx, arr_h, idx, val_h, JSPROP_ENUMERATE as u32);
                        idx += 1;
                    }
                }
            }
        }
        Err(_) => {}
    }

    args.rval().set(ObjectValue(arr_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolve6(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(ObjectValue(arr_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_reverse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"dns.reverse requires an ip argument".as_ptr(),
        );
        return false;
    }

    let ip_val = *args.get(0).ptr;
    if !ip_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"dns.reverse ip must be a string".as_ptr(),
        );
        return false;
    }

    let ip_str = jsstr_to_string(cx, NonNull::new_unchecked(ip_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let arr_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &arr_obj,
    };

    // Validate IP format and attempt reverse lookup via ToSocketAddrs
    match ip_str.parse::<::std::net::IpAddr>() {
        Ok(_addr) => {
            // Standard library does not provide reverse DNS directly.
            // Return the IP itself as the hostname in the array.
            if let Ok(c_ip) = CString::new(ip_str.as_str()) {
                let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                if !js_str.is_null() {
                    let val = StringValue(&*js_str);
                    let val_h = Handle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &val,
                    };
                    JS_DefineElement(cx, arr_h, 0, val_h, JSPROP_ENUMERATE as u32);
                }
            }
        }
        Err(_) => {}
    }

    args.rval().set(ObjectValue(arr_obj));
    true
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();

        // The IIFE below is evaluated via JS::Evaluate2 in the global scope,
        // so `__dns_*` helpers must be visible on the global object — defining
        // them on mod_obj alone made `typeof __dns_lookup === "function"` fail
        // and dns.lookup fell back to "not available" (root cause of the
        // test_dns_net_deep family failures).
        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &global,
            };
            JS_DefineFunction(cx_raw, global_h, c"__dns_lookup".as_ptr(), Some(dns_lookup), 1, 0);
            JS_DefineFunction(cx_raw, global_h, c"__dns_resolve".as_ptr(), Some(dns_resolve), 2, 0);
            JS_DefineFunction(
                cx_raw,
                global_h,
                c"__dns_resolve6".as_ptr(),
                Some(dns_resolve6),
                1,
                0,
            );
            JS_DefineFunction(cx_raw, global_h, c"__dns_reverse".as_ptr(), Some(dns_reverse), 1, 0);
        }

        // Also keep mirrors on the module object for completeness (existing
        // callers may import the helpers off the dns module).
        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mod_ptr,
        };
        JS_DefineFunction(cx_raw, mod_h, c"__dns_lookup".as_ptr(), Some(dns_lookup), 1, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__dns_resolve".as_ptr(), Some(dns_resolve), 2, 0);
        JS_DefineFunction(
            cx_raw,
            mod_h,
            c"__dns_resolve6".as_ptr(),
            Some(dns_resolve6),
            1,
            0,
        );
        JS_DefineFunction(cx_raw, mod_h, c"__dns_reverse".as_ptr(), Some(dns_reverse), 1, 0);

        let c_filename = CString::new("node:dns").unwrap_or_default();
        let opts = mozjs::glue::NewCompileOptions(cx_raw, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return;
        }

        let mut src = mozjs::rust::transform_str_to_source_text(DNS_JS);
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
            "lookup",
            "resolve",
            "resolve4",
            "resolve6",
            "reverse",
            "lookupService",
            "getServers",
            "setServers",
            "Resolver",
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

        cache_builtin(cx, "dns", mod_obj.get());
    }
}
