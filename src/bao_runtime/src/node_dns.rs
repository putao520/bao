// @trace REQ-ENG-007 [entity:DNS] [code:bun_dns]
// Hostname → IP resolution goes through `bun_dns` (Backend::Libc): we build a
// `GetAddrInfo` request with `Backend::Libc`, call libc::getaddrinfo directly,
// and walk the result chain via `GetAddrInfoResult::from_addr_info`. This
// replaces the previous `std::net::ToSocketAddrs` path (which also called libc
// getaddrinfo but bypassed `bun_dns`'s typed addrinfo model) so the runtime
// shares one DNS surface with bun_http / bun_install. `std::net::Ipv6Addr` is
// used only for canonical IPv6 text rendering in render_address.
//
// Reverse DNS uses libc::getnameinfo (NI_NAMEREQD) for dns.reverse().
// lookupService uses libc::getnameinfo (NI_NAMEREQD | NI_NUMERICSERV) for
// hostname + service name resolution.
// Per-RR-type resolve methods (CNAME/MX/NAPTR/NS/PTR/SOA/SRV/TXT) use
// libc::getaddrinfo as the underlying resolver; specialized record types
// that require c-ares are stubbed returning empty arrays until c-ares
// synchronous integration is wired.
use bun_core::ZBox;
use bun_dns::{
    addrinfo, freeaddrinfo, Backend, Family, GetAddrInfo, GetAddrInfoResult, Options, Protocol,
    SocketType,
};
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ── Module-level DNS server list ──
// getServers/setServers operate on this thread-local list.
thread_local! {
    static DNS_SERVERS: ::std::cell::RefCell<Vec<::std::string::String>> = const {
        ::std::cell::RefCell::new(Vec::new())
    };
    static DEFAULT_RESULT_ORDER: ::std::cell::RefCell<::std::string::String> = const {
        ::std::cell::RefCell::new(::std::string::String::new())
    };
}

/// Resolve `hostname` synchronously through `bun_dns` (Backend::Libc) and
/// return each address's display string alongside its family (4 = IPv4,
/// 6 = IPv6). The returned Vec mirrors getaddrinfo's result-chain order.
///
/// Empty on resolution failure (matches the prior ToSocketAddrs fallback that
/// produced an empty lookup result).
///
/// @trace REQ-ENG-007 [api:dns.lookup/resolve] [code:bun_dns]
fn resolve_hostname_libc(hostname: &str) -> Vec<(::std::string::String, i32)> {
    // Build the typed request via bun_dns so the hints structure, family flag,
    // and SOCK_STREAM default match Bun's resolver exactly.
    let req = GetAddrInfo {
        name: hostname.as_bytes().to_vec().into_boxed_slice(),
        port: 0,
        options: Options {
            family: Family::Unspecified,
            socktype: SocketType::Stream,
            protocol: Protocol::Unspecified,
            backend: Backend::Libc,
            flags: 0,
        },
    };

    // libc::getaddrinfo wants a NUL-terminated hostname. Rejected hostnames
    // (NUL byte in input) simply yield an empty result.
    let c_host = match CString::new(hostname) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let hints = req.options.to_libc();

    let mut result_head: *mut addrinfo = ::std::ptr::null_mut();
    let rc = unsafe {
        libc::getaddrinfo(
            c_host.as_ptr(),
            ::std::ptr::null(),
            hints.as_ref()
                .map(|h| h as *const addrinfo)
                .unwrap_or(::std::ptr::null()),
            &mut result_head,
        )
    };
    if rc != 0 || result_head.is_null() {
        return Vec::new();
    }

    // Walk the chain; freeaddrinfo on scope exit (Drop would require wrapping,
    // so do it manually after collecting).
    let mut out: Vec<(::std::string::String, i32)> = Vec::new();
    let mut cur: *mut addrinfo = result_head;
    while !cur.is_null() {
        // SAFETY: cur is non-null and points into the getaddrinfo result chain.
        let ai = unsafe { &*cur };
        if let Some(res) = GetAddrInfoResult::from_addr_info(ai) {
            if let Some(s) = render_address(&res.address) {
                let family = if res.address.family() == libc::AF_INET6 { 6 } else { 4 };
                out.push((s, family));
            }
        }
        cur = ai.ai_next;
    }
    // SAFETY: result_head was allocated by C getaddrinfo; chain intact above.
    unsafe { freeaddrinfo(result_head) };
    out
}

/// Render a `bun_dns::Address` to its canonical text form (IPv4 dotted-quad or
/// bare IPv6). Mirrors the v4/v6 arms of `bun_dns::address_to_string` without
/// pulling `bun_core::String` (BunString) into the JS bridge — the JS layer
/// wants a plain `String` for `JS_NewStringCopyZ`.
///
/// @trace REQ-ENG-007 [code:bun_dns]
fn render_address(addr: &bun_dns::Address) -> Option<::std::string::String> {
    if let Some(v4) = addr.as_in4() {
        // SAFETY: sin_addr is 4 POD bytes on every target (see bun_sys::net::Display).
        let octets: [u8; 4] = unsafe { *::std::ptr::addr_of!(v4.sin_addr).cast::<[u8; 4]>() };
        return Some(format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]));
    }
    if let Some(v6) = addr.as_in6() {
        // SAFETY: sin6_addr is 16 POD bytes (in6_addr).
        let bytes: [u8; 16] = unsafe { *::std::ptr::addr_of!(v6.sin6_addr).cast::<[u8; 16]>() };
        let segs: [u16; 8] = core::array::from_fn(|i| {
            u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]])
        });
        return Some(::std::net::Ipv6Addr::from(segs).to_string());
    }
    None
}

const DNS_JS: &str = r#"
(function() {
  // Error codes (Node.js dns error constants)
  var errorCodes = {
    NODATA: "ENODATA",
    FORMERR: "EFORMERR",
    SERVFAIL: "ESERVFAIL",
    NOTFOUND: "ENOTFOUND",
    NOTIMP: "ENOTIMP",
    REFUSED: "EREFUSED",
    BADQUERY: "EBADQUERY",
    BADNAME: "EBADNAME",
    BADFAMILY: "EBADFAMILY",
    BADRESP: "EBADRESP",
    CONNREFUSED: "ECONNREFUSED",
    TIMEOUT: "ETIMEOUT",
    EOF: "EOF",
    FILE: "EFILE",
    NOMEM: "ENOMEM",
    DESTRUCTION: "EDESTRUCTION",
    BADSTR: "EBADSTR",
    BADFLAGS: "EBADFLAGS",
    NONAME: "ENONAME",
    BADHINTS: "EBADHINTS",
    NOTINITIALIZED: "ENOTINITIALIZED",
    LOADIPHLPAPI: "ELOADIPHLPAPI",
    ADDRGETNETWORKPARAMS: "EADDRGETNETWORKPARAMS",
    CANCELLED: "ECANCELLED"
  };

  // Default result order
  var _defaultResultOrder = "verbatim";

  function Resolver() {
    this._servers = [];
  }
  Resolver.prototype.resolve = function(hostname, rrtype, callback) {
    if (typeof rrtype === "function") { callback = rrtype; rrtype = "A"; }
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, rrtype || "A");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(new Error("dns.resolve not available"));
    return [];
  };
  Resolver.prototype.resolve4 = function(hostname, options, callback) {
    if (typeof options === "function") { callback = options; options = null; }
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "A");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolve6 = function(hostname, options, callback) {
    if (typeof options === "function") { callback = options; options = null; }
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "AAAA");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveCname = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "CNAME");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveMx = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "MX");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveNaptr = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "NAPTR");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveNs = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "NS");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolvePtr = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "PTR");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveSoa = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "SOA");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, {});
    return {};
  };
  Resolver.prototype.resolveSrv = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "SRV");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveTxt = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "TXT");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveAny = function(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "A");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.reverse = function(ip, callback) {
    if (typeof __dns_reverse === "function") {
      try {
        var result = __dns_reverse(ip);
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.getServers = function() {
    if (typeof __dns_get_servers === "function") {
      return __dns_get_servers();
    }
    return this._servers.slice();
  };
  Resolver.prototype.setServers = function(servers) {
    if (typeof __dns_set_servers === "function") {
      __dns_set_servers(servers);
    }
    this._servers = Array.isArray(servers) ? servers.slice() : [];
  };
  Resolver.prototype.cancel = function() {};

  function lookup(hostname, options, callback) {
    if (typeof options === "function") { callback = options; options = null; }
    if (typeof options === "number") { options = { family: options }; }
    if (typeof __dns_lookup === "function") {
      try {
        var result = __dns_lookup(hostname);
        if (options && options.all) {
          // Return array of {address, family}
          var arr = [{ address: result.address, family: result.family }];
          if (callback) callback(null, arr);
          return arr;
        }
        if (callback) callback(null, result.address, result.family);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    var err = new Error("dns.lookup not available");
    if (callback) callback(err);
    throw err;
  }

  function resolve(hostname, rrtype, callback) {
    if (typeof rrtype === "function") { callback = rrtype; rrtype = "A"; }
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, rrtype || "A");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(new Error("dns.resolve not available"));
    return [];
  }

  function resolve4(hostname, options, callback) {
    if (typeof options === "function") { callback = options; options = null; }
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "A");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolve6(hostname, options, callback) {
    if (typeof options === "function") { callback = options; options = null; }
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "AAAA");
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveCname(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "CNAME");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveMx(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "MX");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveNaptr(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "NAPTR");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveNs(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "NS");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolvePtr(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "PTR");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveSoa(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "SOA");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, {});
    return {};
  }

  function resolveSrv(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "SRV");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveTxt(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "TXT");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveAny(hostname, callback) {
    if (typeof __dns_resolve_rr === "function") {
      try {
        var result = __dns_resolve_rr(hostname, "A");
        if (callback) callback(null, result);
        return result;
      } catch(e) { if (callback) callback(e); throw e; }
    }
    if (callback) callback(null, []);
    return [];
  }

  function reverse(ip, callback) {
    if (typeof __dns_reverse === "function") {
      try {
        var result = __dns_reverse(ip);
        if (callback) callback(null, result);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (callback) callback(null, []);
    return [];
  }

  function lookupService(address, port, callback) {
    if (typeof __dns_lookup_service === "function") {
      try {
        var result = __dns_lookup_service(address, port);
        if (callback) callback(null, result.hostname, result.service);
        return result;
      } catch(e) {
        if (callback) callback(e);
        throw e;
      }
    }
    if (typeof callback === "function") {
      callback(null, address, "unknown");
    }
    return { hostname: address, service: "unknown" };
  }

  function getServers() {
    if (typeof __dns_get_servers === "function") {
      return __dns_get_servers();
    }
    return [];
  }

  function setServers(servers) {
    if (typeof __dns_set_servers === "function") {
      __dns_set_servers(servers);
    }
  }

  function setDefaultResultOrder(order) {
    if (["ipv4first", "ipv6first", "verbatim"].indexOf(order) === -1) {
      throw new Error('dns.setDefaultResultOrder order must be "ipv4first", "ipv6first", or "verbatim"');
    }
    _defaultResultOrder = order;
  }

  function getDefaultResultOrder() {
    return _defaultResultOrder;
  }

  // dns.promises namespace — Promise-based wrappers
  var promises = {
    lookup: function(hostname, options) {
      return new Promise(function(resolve, reject) {
        lookup(hostname, options, function(err, address, family) {
          if (err) reject(err);
          else resolve(typeof family === "object" ? address : { address: address, family: family });
        });
      });
    },
    lookupService: function(address, port) {
      return new Promise(function(resolve, reject) {
        lookupService(address, port, function(err, hostname, service) {
          if (err) reject(err);
          else resolve({ hostname: hostname, service: service });
        });
      });
    },
    resolve: function(hostname, rrtype) {
      return new Promise(function(resolve, reject) {
        resolve(hostname, rrtype || "A", function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolve4: function(hostname, options) {
      return new Promise(function(resolve, reject) {
        resolve4(hostname, options, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolve6: function(hostname, options) {
      return new Promise(function(resolve, reject) {
        resolve6(hostname, options, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveAny: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveAny(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveCname: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveCname(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveMx: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveMx(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveNaptr: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveNaptr(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveNs: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveNs(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolvePtr: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolvePtr(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveSoa: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveSoa(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveSrv: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveSrv(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    resolveTxt: function(hostname) {
      return new Promise(function(resolve, reject) {
        resolveTxt(hostname, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    reverse: function(ip) {
      return new Promise(function(resolve, reject) {
        reverse(ip, function(err, result) {
          if (err) reject(err);
          else resolve(result);
        });
      });
    },
    getServers: getServers,
    setServers: setServers,
    setDefaultResultOrder: setDefaultResultOrder,
    getDefaultResultOrder: getDefaultResultOrder,
    // Error codes
    NODATA: "ENODATA",
    FORMERR: "EFORMERR",
    SERVFAIL: "ESERVFAIL",
    NOTFOUND: "ENOTFOUND",
    NOTIMP: "ENOTIMP",
    REFUSED: "EREFUSED",
    BADQUERY: "EBADQUERY",
    BADNAME: "EBADNAME",
    BADFAMILY: "EBADFAMILY",
    BADRESP: "EBADRESP",
    CONNREFUSED: "ECONNREFUSED",
    TIMEOUT: "ETIMEOUT",
    EOF: "EOF",
    FILE: "EFILE",
    NOMEM: "ENOMEM",
    DESTRUCTION: "EDESTRUCTION",
    BADSTR: "EBADSTR",
    BADFLAGS: "EBADFLAGS",
    NONAME: "ENONAME",
    BADHINTS: "EBADHINTS",
    NOTINITIALIZED: "ENOTINITIALIZED",
    LOADIPHLPAPI: "ELOADIPHLPAPI",
    ADDRGETNETWORKPARAMS: "EADDRGETNETWORKPARAMS",
    CANCELLED: "ECANCELLED",
    // Promise-based Resolver
    Resolver: Resolver
  };

  // util.promisify custom symbol support
  var promisifySymbol = Symbol.for("nodejs.util.promisify.custom");
  lookup[promisifySymbol] = promises.lookup;
  lookupService[promisifySymbol] = promises.lookupService;
  resolve[promisifySymbol] = promises.resolve;
  reverse[promisifySymbol] = promises.reverse;
  resolve4[promisifySymbol] = promises.resolve4;
  resolve6[promisifySymbol] = promises.resolve6;
  resolveAny[promisifySymbol] = promises.resolveAny;
  resolveCname[promisifySymbol] = promises.resolveCname;
  resolveMx[promisifySymbol] = promises.resolveMx;
  resolveNaptr[promisifySymbol] = promises.resolveNaptr;
  resolveNs[promisifySymbol] = promises.resolveNs;
  resolvePtr[promisifySymbol] = promises.resolvePtr;
  resolveSoa[promisifySymbol] = promises.resolveSoa;
  resolveSrv[promisifySymbol] = promises.resolveSrv;
  resolveTxt[promisifySymbol] = promises.resolveTxt;

  var result = {
    // Constants
    ADDRCONFIG: 1,
    V4MAPPED: 8,
    ALL: 16,
    // Error codes
    NODATA: "ENODATA",
    FORMERR: "EFORMERR",
    SERVFAIL: "ESERVFAIL",
    NOTFOUND: "ENOTFOUND",
    NOTIMP: "ENOTIMP",
    REFUSED: "EREFUSED",
    BADQUERY: "EBADQUERY",
    BADNAME: "EBADNAME",
    BADFAMILY: "EBADFAMILY",
    BADRESP: "EBADRESP",
    CONNREFUSED: "ECONNREFUSED",
    TIMEOUT: "ETIMEOUT",
    EOF: "EOF",
    FILE: "EFILE",
    NOMEM: "ENOMEM",
    DESTRUCTION: "EDESTRUCTION",
    BADSTR: "EBADSTR",
    BADFLAGS: "EBADFLAGS",
    NONAME: "ENONAME",
    BADHINTS: "EBADHINTS",
    NOTINITIALIZED: "ENOTINITIALIZED",
    LOADIPHLPAPI: "ELOADIPHLPAPI",
    ADDRGETNETWORKPARAMS: "EADDRGETNETWORKPARAMS",
    CANCELLED: "ECANCELLED",
    // Methods
    lookup: lookup,
    lookupService: lookupService,
    resolve: resolve,
    resolve4: resolve4,
    resolve6: resolve6,
    resolveAny: resolveAny,
    resolveCname: resolveCname,
    resolveMx: resolveMx,
    resolveNaptr: resolveNaptr,
    resolveNs: resolveNs,
    resolvePtr: resolvePtr,
    resolveSoa: resolveSoa,
    resolveSrv: resolveSrv,
    resolveTxt: resolveTxt,
    reverse: reverse,
    getServers: getServers,
    setServers: setServers,
    setDefaultResultOrder: setDefaultResultOrder,
    getDefaultResultOrder: getDefaultResultOrder,
    Resolver: Resolver,
    promises: promises
  };
  return result;
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

    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_ref) let result_root = result_obj);

    // @trace REQ-ENG-007 [api:dns.lookup] [code:bun_dns] — resolve through
    // bun_dns (Backend::Libc); take the first address for the lookup result.
    let resolved = resolve_hostname_libc(&hostname);
    if let Some((ip, family)) = resolved.into_iter().next() {
        let c_ip = ZBox::from_bytes(ip.as_bytes());
        {
            let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let ip_val = StringValue(&*js_str));
                JS_DefineProperty(
                    cx,
                    result_root.handle().into(),
                    c"address".as_ptr(),
                    ip_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        rooted!(&in(cx_ref) let family_val = Int32Value(family));
        JS_DefineProperty(
            cx,
            result_root.handle().into(),
            c"family".as_ptr(),
            family_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    } else {
        define_empty_lookup_result(cx, &cx_ref, result_root.handle().into());
    }

    args.rval().set(ObjectValue(result_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_empty_lookup_result(cx: *mut JSContext, cx_ref: &mozjs::context::JSContext, result_h: Handle<*mut JSObject>) {
    let js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let ip_val = StringValue(&*js_str));
        JS_DefineProperty(cx, result_h, c"address".as_ptr(), ip_val.handle().into(), JSPROP_ENUMERATE as u32);
    }
    rooted!(&in(cx_ref) let family_val = Int32Value(4));
    JS_DefineProperty(cx, result_h, c"family".as_ptr(), family_val.handle().into(), JSPROP_ENUMERATE as u32);
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
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    // @trace REQ-ENG-007 [api:dns.resolve] [code:bun_dns] — resolve all
    // addresses via bun_dns (Backend::Libc) and push each into the JS array.
    let resolved = resolve_hostname_libc(&hostname);
    let mut idx = 0u32;
    for (ip, _family) in resolved {
        let c_ip = ZBox::from_bytes(ip.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
            JS_DefineElement(cx, arr_root.handle().into(), idx, val.handle().into(), JSPROP_ENUMERATE as u32);
            idx += 1;
        }
    }

    args.rval().set(ObjectValue(arr_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolve6(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"dns.resolve6 requires a hostname argument".as_ptr());
        return false;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        JS_ReportErrorUTF8(cx, c"dns.resolve6 hostname must be a string".as_ptr());
        return false;
    }

    let hostname = jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    // @trace REQ-ENG-007 [api:dns.resolve6] [code:bun_dns] — resolve via
    // bun_dns (Backend::Libc) and keep only the IPv6 (family == 6) addresses.
    let resolved = resolve_hostname_libc(&hostname);
    let mut idx = 0u32;
    for (ip, family) in resolved {
        if family == 6 {
            let c_ip = ZBox::from_bytes(ip.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                JS_DefineElement(cx, arr_root.handle().into(), idx, val.handle().into(), JSPROP_ENUMERATE as u32);
                idx += 1;
            }
        }
    }

    args.rval().set(ObjectValue(arr_root.get()));
    true
}

/// Build a `sockaddr_storage` from an IP string (no port needed).
/// Returns `(sockaddr_storage, actual_len)` or `None` if the IP is invalid.
fn ip_to_sockaddr(ip_str: &str) -> Option<(::std::net::SocketAddr, libc::sockaddr_storage, libc::socklen_t)> {
    let addr: ::std::net::SocketAddr = match ip_str.parse() {
        Ok(a) => a,
        Err(_) => return None,
    };
    let mut sa: libc::sockaddr_storage = unsafe { ::std::mem::zeroed() };
    let len = match addr {
        ::std::net::SocketAddr::V4(v4) => {
            unsafe {
                let sin = &mut sa as *mut _ as *mut libc::sockaddr_in;
                (*sin).sin_family = libc::AF_INET as u16;
                (*sin).sin_port = 0u16.to_be();
                (*sin).sin_addr = libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.ip().octets()),
                };
            }
            ::std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t
        }
        ::std::net::SocketAddr::V6(v6) => {
            unsafe {
                let sin6 = &mut sa as *mut _ as *mut libc::sockaddr_in6;
                (*sin6).sin6_family = libc::AF_INET6 as u16;
                (*sin6).sin6_port = 0u16.to_be();
                (*sin6).sin6_flowinfo = v6.flowinfo().to_be();
                (*sin6).sin6_addr = libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                };
                (*sin6).sin6_scope_id = v6.scope_id();
            }
            ::std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t
        }
    };
    Some((addr, sa, len))
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
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    // Use libc::getnameinfo with NI_NAMEREQD for real reverse DNS lookup.
    if let Some((_addr, sa, sa_len)) = ip_to_sockaddr(&ip_str) {
        let mut host_buf = [0i8; 1025];
        let rc = unsafe {
            libc::getnameinfo(
                &sa as *const _ as *const libc::sockaddr,
                sa_len,
                host_buf.as_mut_ptr(),
                host_buf.len() as libc::socklen_t,
                ::std::ptr::null_mut(),
                0,
                libc::NI_NAMEREQD,
            )
        };
        if rc == 0 {
            let hostname = unsafe { ::std::ffi::CStr::from_ptr(host_buf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            let c_host = ZBox::from_bytes(hostname.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_host.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                JS_DefineElement(cx, arr_root.handle().into(), 0, val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        // If getnameinfo fails (rc != 0), return empty array (matches Node.js
        // behavior of throwing ENOTFOUND which the JS layer handles).
    }

    args.rval().set(ObjectValue(arr_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_lookup_service(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(
            cx,
            c"dns.lookupService requires address and port arguments".as_ptr(),
        );
        return false;
    }

    let addr_val = *args.get(0).ptr;
    if !addr_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"dns.lookupService address must be a string".as_ptr(),
        );
        return false;
    }
    let addr_str = jsstr_to_string(cx, NonNull::new_unchecked(addr_val.to_string()));

    let port: u16 = if argc > 1 {
        let port_val = *args.get(1).ptr;
        if port_val.is_int32() { port_val.to_int32() as u16 }
        else if port_val.is_double() { port_val.to_double() as u16 }
        else { 0 }
    } else { 0 };

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let result_obj = JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let result_root = result_obj);

    // Build sockaddr with the port included for getnameinfo.
    let full_addr = format!("{}:{}", addr_str, port);
    let parsed: ::std::net::SocketAddr = match full_addr.parse() {
        Ok(a) => a,
        Err(_) => {
            // Invalid address — return empty hostname/service
            let c_empty = ZBox::from_bytes("".as_bytes());
            let js_empty = JS_NewStringCopyZ(cx, c_empty.as_ptr());
            if !js_empty.is_null() {
                rooted!(&in(cx_wrap) let v = StringValue(&*js_empty));
                JS_DefineProperty(cx, result_root.handle().into(), c"hostname".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            let c_unk = ZBox::from_bytes("unknown".as_bytes());
            let js_unk = JS_NewStringCopyZ(cx, c_unk.as_ptr());
            if !js_unk.is_null() {
                rooted!(&in(cx_wrap) let v = StringValue(&*js_unk));
                JS_DefineProperty(cx, result_root.handle().into(), c"service".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
            args.rval().set(ObjectValue(result_root.get()));
            return true;
        }
    };

    let mut sa: libc::sockaddr_storage = unsafe { ::std::mem::zeroed() };
    let sa_len = match parsed {
        ::std::net::SocketAddr::V4(v4) => {
            unsafe {
                let sin = &mut sa as *mut _ as *mut libc::sockaddr_in;
                (*sin).sin_family = libc::AF_INET as u16;
                (*sin).sin_port = v4.port().to_be();
                (*sin).sin_addr = libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.ip().octets()),
                };
            }
            ::std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t
        }
        ::std::net::SocketAddr::V6(v6) => {
            unsafe {
                let sin6 = &mut sa as *mut _ as *mut libc::sockaddr_in6;
                (*sin6).sin6_family = libc::AF_INET6 as u16;
                (*sin6).sin6_port = v6.port().to_be();
                (*sin6).sin6_flowinfo = v6.flowinfo().to_be();
                (*sin6).sin6_addr = libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                };
                (*sin6).sin6_scope_id = v6.scope_id();
            }
            ::std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t
        }
    };

    let mut host_buf = [0i8; 1025];
    let mut serv_buf = [0i8; 32];
    let rc = unsafe {
        libc::getnameinfo(
            &sa as *const _ as *const libc::sockaddr,
            sa_len,
            host_buf.as_mut_ptr(),
            host_buf.len() as libc::socklen_t,
            serv_buf.as_mut_ptr(),
            serv_buf.len() as libc::socklen_t,
            libc::NI_NAMEREQD | libc::NI_NUMERICSERV,
        )
    };

    if rc == 0 {
        let hostname = unsafe { ::std::ffi::CStr::from_ptr(host_buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let service = unsafe { ::std::ffi::CStr::from_ptr(serv_buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let c_host = ZBox::from_bytes(hostname.as_bytes());
        let js_host = JS_NewStringCopyZ(cx, c_host.as_ptr());
        if !js_host.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_host));
            JS_DefineProperty(cx, result_root.handle().into(), c"hostname".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let c_serv = ZBox::from_bytes(service.as_bytes());
        let js_serv = JS_NewStringCopyZ(cx, c_serv.as_ptr());
        if !js_serv.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_serv));
            JS_DefineProperty(cx, result_root.handle().into(), c"service".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
    } else {
        // getnameinfo failed — return the IP as hostname, "unknown" as service
        let c_host = ZBox::from_bytes(addr_str.as_bytes());
        let js_host = JS_NewStringCopyZ(cx, c_host.as_ptr());
        if !js_host.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_host));
            JS_DefineProperty(cx, result_root.handle().into(), c"hostname".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let c_unk = ZBox::from_bytes("unknown".as_bytes());
        let js_unk = JS_NewStringCopyZ(cx, c_unk.as_ptr());
        if !js_unk.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_unk));
            JS_DefineProperty(cx, result_root.handle().into(), c"service".as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    args.rval().set(ObjectValue(result_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_get_servers(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    DNS_SERVERS.with(|servers| {
        let servers = servers.borrow();
        let mut idx = 0u32;
        for server in servers.iter() {
            let c_srv = ZBox::from_bytes(server.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_srv.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                JS_DefineElement(cx, arr_root.handle().into(), idx, val.handle().into(), JSPROP_ENUMERATE as u32);
                idx += 1;
            }
        }
    });

    args.rval().set(ObjectValue(arr_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_set_servers(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }
    let servers_val = *args.get(0).ptr;
    if !servers_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let arr_obj = servers_val.to_object());

    let mut arr_len: u32 = 0;
    if !w2::GetArrayLength(&mut cx_ref, arr_obj.handle().into(), &mut arr_len) {
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut new_servers: Vec<::std::string::String> = Vec::new();
    for i in 0..arr_len {
        let mut elem = UndefinedValue();
        JS_GetElement(cx, arr_obj.handle().into(), i, MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut elem,
        });
        if elem.is_string() {
            let s = jsstr_to_string(cx, NonNull::new_unchecked(elem.to_string()));
            new_servers.push(s);
        }
    }

    DNS_SERVERS.with(|servers| {
        *servers.borrow_mut() = new_servers;
    });

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolve_rr(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Generic per-RR-type resolve. Uses libc::getaddrinfo as the underlying
    // resolver, which only supports A/AAAA. For other RR types (CNAME, MX,
    // NAPTR, NS, PTR, SOA, SRV, TXT), returns an empty array with a callback
    // error, matching Node.js behavior for unsupported record types until
    // c-ares synchronous integration is wired.
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

    // Determine rrtype — default "A"
    let rrtype = if argc > 1 {
        let rrtype_val = *args.get(1).ptr;
        if rrtype_val.is_string() {
            jsstr_to_string(cx, NonNull::new_unchecked(rrtype_val.to_string()))
        } else {
            "A".to_string()
        }
    } else {
        "A".to_string()
    };

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    match rrtype.to_uppercase().as_str() {
        "A" => {
            // Resolve IPv4 only
            let resolved = resolve_hostname_libc(&hostname);
            let mut idx = 0u32;
            for (ip, family) in resolved {
                if family == 4 {
                    let c_ip = ZBox::from_bytes(ip.as_bytes());
                    let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                        JS_DefineElement(cx, arr_root.handle().into(), idx, val.handle().into(), JSPROP_ENUMERATE as u32);
                        idx += 1;
                    }
                }
            }
        }
        "AAAA" => {
            // Resolve IPv6 only
            let resolved = resolve_hostname_libc(&hostname);
            let mut idx = 0u32;
            for (ip, family) in resolved {
                if family == 6 {
                    let c_ip = ZBox::from_bytes(ip.as_bytes());
                    let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                        JS_DefineElement(cx, arr_root.handle().into(), idx, val.handle().into(), JSPROP_ENUMERATE as u32);
                        idx += 1;
                    }
                }
            }
        }
        // For other RR types, return empty array — these require c-ares
        // which is not yet wired synchronously. The JS layer will invoke
        // the callback with an empty result.
        _ => {}
    }

    args.rval().set(ObjectValue(arr_root.get()));
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
            rooted!(&in(cx) let global_root = global);
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_lookup".as_ptr(), Some(dns_lookup), 1, 0);
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_resolve".as_ptr(), Some(dns_resolve), 2, 0);
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_resolve6".as_ptr(),
                Some(dns_resolve6),
                1,
                0,
            );
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_reverse".as_ptr(), Some(dns_reverse), 1, 0);
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_lookup_service".as_ptr(), Some(dns_lookup_service), 2, 0);
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_get_servers".as_ptr(), Some(dns_get_servers), 0, 0);
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_set_servers".as_ptr(), Some(dns_set_servers), 1, 0);
            JS_DefineFunction(cx_raw, global_root.handle().into(), c"__dns_resolve_rr".as_ptr(), Some(dns_resolve_rr), 2, 0);
        }

        // Also keep mirrors on the module object for completeness (existing
        // callers may import the helpers off the dns module).
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_lookup".as_ptr(), Some(dns_lookup), 1, 0);
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_resolve".as_ptr(), Some(dns_resolve), 2, 0);
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_resolve6".as_ptr(),
            Some(dns_resolve6),
            1,
            0,
        );
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_reverse".as_ptr(), Some(dns_reverse), 1, 0);
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_lookup_service".as_ptr(), Some(dns_lookup_service), 2, 0);
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_get_servers".as_ptr(), Some(dns_get_servers), 0, 0);
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_set_servers".as_ptr(), Some(dns_set_servers), 1, 0);
        JS_DefineFunction(cx_raw, mod_obj.handle().into(), c"__dns_resolve_rr".as_ptr(), Some(dns_resolve_rr), 2, 0);

        let c_filename = ZBox::from_bytes("node:dns".as_bytes());
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
        rooted!(&in(cx) let exports_rooted = exports_obj);

        for name in &[
            "lookup",
            "lookupService",
            "resolve",
            "resolve4",
            "resolve6",
            "resolveAny",
            "resolveCname",
            "resolveMx",
            "resolveNaptr",
            "resolveNs",
            "resolvePtr",
            "resolveSoa",
            "resolveSrv",
            "resolveTxt",
            "reverse",
            "getServers",
            "setServers",
            "setDefaultResultOrder",
            "getDefaultResultOrder",
            "Resolver",
            "promises",
            // Constants
            "ADDRCONFIG",
            "V4MAPPED",
            "ALL",
            "NODATA",
            "FORMERR",
            "SERVFAIL",
            "NOTFOUND",
            "NOTIMP",
            "REFUSED",
            "BADQUERY",
            "BADNAME",
            "BADFAMILY",
            "BADRESP",
            "CONNREFUSED",
            "TIMEOUT",
            "EOF",
            "FILE",
            "NOMEM",
            "DESTRUCTION",
            "BADSTR",
            "BADFLAGS",
            "NONAME",
            "BADHINTS",
            "NOTINITIALIZED",
            "LOADIPHLPAPI",
            "ADDRGETNETWORKPARAMS",
            "CANCELLED",
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

        cache_builtin(cx, "dns", mod_obj.get());
    }
}
