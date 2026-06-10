// @trace REQ-ENG-007
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use bun_dns::{Family, SocketType, address_to_string, addrinfo as AddrInfo, getaddrinfo, AI_ADDRCONFIG, freeaddrinfo};
use bun_core::is_ip_address;

use crate::require::cache_builtin;

/// Synchronous DNS lookup via bun_dns::getaddrinfo, using bun_dns data model
/// for address formatting. Returns (ip_string, family_number) for the first
/// result, or None on error.
fn dns_lookup_sync(hostname: &str, family: Family) -> Option<(String, i32)> {
    let c_hostname = bun_core::ZBox::from_bytes(hostname.as_bytes());
    let mut hints: AddrInfo = unsafe { core::mem::zeroed() };
    hints.ai_family = family.to_libc();
    hints.ai_socktype = SocketType::Stream.to_libc();
    hints.ai_flags = AI_ADDRCONFIG;

    let mut result: *mut AddrInfo = core::ptr::null_mut();
    let ret = unsafe {
        getaddrinfo(c_hostname.as_ptr(), core::ptr::null(), &hints, &mut result)
    };
    if ret != 0 || result.is_null() {
        return None;
    }

    // SAFETY: result is non-null, points to valid addrinfo chain from getaddrinfo
    let first = unsafe { &*result };
    if first.ai_addr.is_null() {
        unsafe { freeaddrinfo(result) };
        return None;
    }

    let addr = unsafe { bun_dns::Address::init_posix(first.ai_addr.cast()) };
    let ip_bun = address_to_string(&addr).ok();
    let ip_utf8 = ip_bun.as_ref().map(|s| s.to_utf8());
    let ip_str = ip_utf8.as_ref()
        .map(|s| String::from_utf8_lossy(s.slice()).into_owned())
        .unwrap_or_default();

    let family_num = match first.ai_family {
        f if f == Family::Inet.to_libc() => 4,
        f if f == Family::Inet6.to_libc() => 6,
        _ => 4,
    };

    unsafe { freeaddrinfo(result) };
    Some((ip_str, family_num))
}

/// Synchronous DNS resolve via bun_dns::getaddrinfo, using bun_dns data model.
/// Returns a Vec of IP address strings.
fn dns_resolve_sync(hostname: &str, family: Family) -> Vec<String> {
    let c_hostname = bun_core::ZBox::from_bytes(hostname.as_bytes());
    let mut hints: AddrInfo = unsafe { core::mem::zeroed() };
    hints.ai_family = family.to_libc();
    hints.ai_socktype = SocketType::Stream.to_libc();
    hints.ai_flags = AI_ADDRCONFIG;

    let mut result: *mut AddrInfo = core::ptr::null_mut();
    let ret = unsafe {
        getaddrinfo(c_hostname.as_ptr(), core::ptr::null(), &hints, &mut result)
    };
    if ret != 0 || result.is_null() {
        return Vec::new();
    }

    let mut ips = Vec::new();
    let mut current = result;
    while !current.is_null() {
        // SAFETY: current is non-null, points into getaddrinfo result chain
        let info = unsafe { &*current };
        if !info.ai_addr.is_null() {
            let addr = unsafe { bun_dns::Address::init_posix(info.ai_addr.cast()) };
            if let Ok(s) = address_to_string(&addr) {
                let utf8 = s.to_utf8();
                ips.push(String::from_utf8_lossy(utf8.slice()).into_owned());
            }
        }
        current = info.ai_next;
    }

    unsafe { freeaddrinfo(result) };
    ips
}

// ---------------------------------------------------------------------------
// JS polyfill — classes + API surface
// ---------------------------------------------------------------------------

const DNS_JS: &str = r#"
(function() {
  var EE = null;
  try { EE = require("events").EventEmitter; } catch(e) {
    EE = function EE() { this._events = {}; };
    EE.prototype.on = function(e, fn) { (this._events[e] || (this._events[e] = [])).push(fn); return this; };
    EE.prototype.emit = function(e) { var a = Array.prototype.slice.call(arguments, 1); var ls = this._events[e]; if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a); return !!ls; };
    EE.prototype.removeListener = function(e, fn) { var ls = this._events[e]; if (ls) { var i = ls.indexOf(fn); if (i >= 0) ls.splice(i, 1); } return this; };
  }

  function Resolver() {
    EE.call(this);
    this._servers = [];
  }
  Resolver.prototype = Object.create(EE.prototype);
  Resolver.prototype.constructor = Resolver;
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
  Resolver.prototype.reverse = function(ip, callback) {
    if (typeof __dns_reverse === "function") {
      var result = __dns_reverse(ip);
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
    // no-op — libc getaddrinfo uses system resolvers
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

// ---------------------------------------------------------------------------
// Native sync functions — use bun_dns + libc getaddrinfo
// ---------------------------------------------------------------------------

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_lookup(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"dns.lookup requires a hostname argument".as_ptr());
        return false;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        JS_ReportErrorUTF8(cx, c"dns.lookup hostname must be a string".as_ptr());
        return false;
    }

    let hostname = jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let result_obj = JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let result_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &result_obj,
    };

    match dns_lookup_sync(&hostname, Family::Unspecified) {
        Some((ip, family)) => {
            let c_ip = bun_core::ZBox::from_bytes(ip.as_str().as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
            if !js_str.is_null() {
                let ip_val = StringValue(&*js_str);
                let ip_h = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &ip_val,
                };
                JS_DefineProperty(cx, result_h, c"address".as_ptr(), ip_h, JSPROP_ENUMERATE as u32);
            }
            let family_val = Int32Value(family);
            let family_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &family_val,
            };
            JS_DefineProperty(cx, result_h, c"family".as_ptr(), family_h, JSPROP_ENUMERATE as u32);
        }
        None => define_empty_lookup_result(cx, result_h),
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
        JS_ReportErrorUTF8(cx, c"dns.resolve requires a hostname argument".as_ptr());
        return false;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        JS_ReportErrorUTF8(cx, c"dns.resolve hostname must be a string".as_ptr());
        return false;
    }

    let hostname = jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    // Parse rrtype to determine Family
    let family = if argc >= 2 {
        let rrtype_val = *args.get(1).ptr;
        if rrtype_val.is_string() {
            let rrtype = jsstr_to_string(cx, NonNull::new_unchecked(rrtype_val.to_string()));
            match rrtype.as_str() {
                "AAAA" | "AAAAA" => Family::Inet6,
                "A" => Family::Inet,
                _ => Family::Unspecified,
            }
        } else {
            Family::Unspecified
        }
    } else {
        Family::Unspecified
    };

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

    let ips = dns_resolve_sync(&hostname, family);
    for (idx, ip) in ips.iter().enumerate() {
        let c_ip = bun_core::ZBox::from_bytes(ip.as_str().as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            let val_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &val,
            };
            JS_DefineElement(cx, arr_h, idx as u32, val_h, JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(arr_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolve6(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
        args.rval().set(if arr_obj.is_null() { UndefinedValue() } else { ObjectValue(arr_obj) });
        return true;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
        args.rval().set(if arr_obj.is_null() { UndefinedValue() } else { ObjectValue(arr_obj) });
        return true;
    }

    let hostname = jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

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

    let ips = dns_resolve_sync(&hostname, Family::Inet6);
    for (idx, ip) in ips.iter().enumerate() {
        let c_ip = bun_core::ZBox::from_bytes(ip.as_str().as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            let val_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &val,
            };
            JS_DefineElement(cx, arr_h, idx as u32, val_h, JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(arr_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_reverse(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"dns.reverse requires an ip argument".as_ptr());
        return false;
    }

    let ip_val = *args.get(0).ptr;
    if !ip_val.is_string() {
        JS_ReportErrorUTF8(cx, c"dns.reverse ip must be a string".as_ptr());
        return false;
    }

    let ip_str = jsstr_to_string(cx, NonNull::new_unchecked(ip_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Reverse DNS: libc getaddrinfo resolves host→ip, not ip→host.
    // For real reverse DNS, we'd need c-ares gethostbyaddr (async).
    // Fallback: validate IP and return it as hostname (matches previous behavior).
    let is_valid_ip = is_ip_address(ip_str.as_bytes());
    if is_valid_ip {
        let c_ip = bun_core::ZBox::from_bytes(ip_str.as_str().as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
        if !js_str.is_null() {
            let val = StringValue(&*js_str);
            let val_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &val,
            };
            let arr_h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &arr_obj,
            };
            JS_DefineElement(cx, arr_h, 0, val_h, JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(arr_obj));
    true
}

// ---------------------------------------------------------------------------
// Module install
// ---------------------------------------------------------------------------

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let mod_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if mod_obj.get().is_null() {
        return;
    }

    unsafe {
        let cx_raw = cx.raw_cx();

        let global = CurrentGlobalOrNull(cx_raw);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &global,
            };
            JS_DefineFunction(cx_raw, global_h, c"__dns_lookup".as_ptr(), Some(dns_lookup), 1, 0);
            JS_DefineFunction(cx_raw, global_h, c"__dns_resolve".as_ptr(), Some(dns_resolve), 2, 0);
            JS_DefineFunction(cx_raw, global_h, c"__dns_resolve6".as_ptr(), Some(dns_resolve6), 1, 0);
            JS_DefineFunction(cx_raw, global_h, c"__dns_reverse".as_ptr(), Some(dns_reverse), 1, 0);
        }

        let mod_ptr = mod_obj.get();
        let mod_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mod_ptr,
        };
        JS_DefineFunction(cx_raw, mod_h, c"__dns_lookup".as_ptr(), Some(dns_lookup), 1, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__dns_resolve".as_ptr(), Some(dns_resolve), 2, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__dns_resolve6".as_ptr(), Some(dns_resolve6), 1, 0);
        JS_DefineFunction(cx_raw, mod_h, c"__dns_reverse".as_ptr(), Some(dns_reverse), 1, 0);

        let c_filename = c"node:dns".as_ptr();
        let Some(_opts_guard) = crate::compile_options_guard::CompileOptionsGuard::new(mozjs::glue::NewCompileOptions(cx_raw, c_filename, 1) as *mut _) else {
            return;
        };
        let opts = _opts_guard.as_ptr() as *const JS::ReadOnlyCompileOptions;

        let mut src = mozjs::rust::transform_str_to_source_text(DNS_JS);
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
        let mod_h2 = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &mod_ptr2 };

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
                JS_DefineProperty(cx_raw, mod_h2, cname.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
            }
        }

        cache_builtin(cx, "dns", mod_obj.get());
    }
}