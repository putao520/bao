// @trace REQ-ENG-007 [entity:DNS] [code:bun_dns]
// Hostname → IP resolution for the dns.lookup family goes through `bun_dns`
// (Backend::Libc): we build a `GetAddrInfo` request with `Backend::Libc`, call
// libc::getaddrinfo directly, and walk the result chain via
// `GetAddrInfoResult::from_addr_info`. This replaces the previous
// `std::net::ToSocketAddrs` path (which also called libc getaddrinfo but
// bypassed `bun_dns`'s typed addrinfo model) so the runtime shares one DNS
// surface with bun_http / bun_install. `std::net::Ipv6Addr` is used only for
// canonical IPv6 text rendering in render_address.
//
// Reverse DNS uses libc::getnameinfo (NI_NAMEREQD) for dns.reverse().
// lookupService uses libc::getnameinfo (NI_NAMEREQD | NI_NUMERICSERV) for
// hostname + service name resolution.
// Per-RR-type resolve methods (A/AAAA/CNAME/MX/NAPTR/NS/PTR/SOA/SRV/TXT) use
// c-ares (bun_cares_sys) synchronous integration: Channel::init + ares_query
// per RR type with the matching parse thunk, driven synchronously with
// ares_getsock + poll + ares_process_fd until the callback fires.
use ::std::ffi::CString;
use ::std::ptr::NonNull;
use bun_core::ZBox;
use bun_dns::{
    Backend, Family, GetAddrInfo, GetAddrInfoResult, Options, Protocol, SocketType, addrinfo,
    freeaddrinfo,
};

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{DoubleValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// ── Synchronous c-ares per-RR-type resolver ──────────────────────────
// @trace REQ-ENG-007 [api:dns.resolve*] [code:bun_cares_sys]
//
// c-ares is inherently async: you submit a query, then drive the channel's
// event loop (ares_getsock → poll → ares_process_fd) until the callback fires.
// We wrap this into a synchronous call that blocks the current thread until
// resolution completes or times out, matching Node.js's dns.resolve* API
// semantics where each call is a blocking resolver from the JS perspective
// (the JS layer provides async wrappers via callbacks/promises).

use ::std::cell::RefCell;
use ::std::sync::atomic::{AtomicBool, Ordering};
use ::std::time::{Duration, Instant};
use bun_cares_sys::c_ares_draft as cares;

/// Per-RR-type resolved record data. Each variant holds the parsed fields
/// matching Node.js dns.resolve* return shapes.
enum DnsRRData {
    A(Vec<::std::string::String>),         // IPv4 dotted-quads
    Aaaa(Vec<::std::string::String>),      // IPv6 canonical text
    Cname(::std::string::String),
    Mx(Vec<(u16, ::std::string::String)>), // (priority, exchange)
    Txt(Vec<Vec<::std::string::String>>),  // one Vec per TXT record, each holding that record's chunks
    Ns(Vec<::std::string::String>),        // nameserver hostnames
    Ptr(Vec<::std::string::String>),       // reverse-resolved hostnames
    Soa {
        nsname: ::std::string::String,
        hostmaster: ::std::string::String,
        serial: u32,
        refresh: u32,
        retry: u32,
        expire: u32,
        minttl: u32,
    },
    Srv(Vec<(u16, u16, u16, ::std::string::String)>), // (priority, weight, port, name)
    Naptr(Vec<NaptrRecord>),
}

struct NaptrRecord {
    flags: ::std::string::String,
    service: ::std::string::String,
    regexp: ::std::string::String,
    replacement: ::std::string::String,
    order: u16,
    preference: u16,
}

/// Container type that implements `cares::ChannelContainer` for our
/// synchronous wrapper. Tracks c-ares socket fds so we can poll them.
struct SyncChannelContainer {
    channel: RefCell<*mut cares::Channel>,
    /// Socket fds that c-ares wants us to monitor, paired with (readable, writable).
    sockets: RefCell<Vec<(cares::ares_socket_t, bool, bool)>>,
}

impl cares::ChannelContainer for SyncChannelContainer {
    fn on_dns_socket_state(&self, socket: cares::ares_socket_t, readable: bool, writable: bool) {
        let mut sockets = self.sockets.borrow_mut();
        // Remove old entry for this fd, then re-insert if any direction is still active.
        sockets.retain(|&(fd, _, _)| fd != socket);
        if readable || writable {
            sockets.push((socket, readable, writable));
        }
    }

    fn set_channel(&self, channel: *mut cares::Channel) {
        *self.channel.borrow_mut() = channel;
    }
}

/// NUL-terminate `src` into `buf`, returning a `*const c_char` suitable for
/// c-ares FFI. Truncates at `buf.len() - 1` if `src` is too long.
fn nul_terminate(buf: &mut [u8], src: &[u8]) -> *const ::std::ffi::c_char {
    let len = src.len().min(buf.len() - 1);
    buf[..len].copy_from_slice(&src[..len]);
    buf[len] = 0;
    buf.as_ptr().cast::<::std::ffi::c_char>()
}

// ── Reply handler types ──────────────────────────────────────────────
// Each RR type needs a concrete `ReplyHandler<R>` impl so that
// `ares_reply_callback<R, Handler>` can be monomorphized into a concrete
// `unsafe extern "C" fn` compatible with `ares_callback`. The handler
// writes results into thread-local storage so the synchronous drive loop
// can pick them up.

thread_local! {
    static QUERY_RESULT: RefCell<Option<DnsRRData>> = const { RefCell::new(None) };
    static QUERY_DONE: AtomicBool = const { AtomicBool::new(false) };
    static QUERY_ERROR: RefCell<Option<::std::string::String>> = const { RefCell::new(None) };
}

fn query_reset() {
    QUERY_RESULT.with(|r| *r.borrow_mut() = None);
    QUERY_DONE.with(|d| d.store(false, Ordering::SeqCst));
    QUERY_ERROR.with(|e| *e.borrow_mut() = None);
}

fn query_set_error(err: ::std::string::String) {
    QUERY_ERROR.with(|e| *e.borrow_mut() = Some(err));
    QUERY_DONE.with(|d| d.store(true, Ordering::SeqCst));
}

fn query_set_result(data: DnsRRData) {
    QUERY_RESULT.with(|r| *r.borrow_mut() = Some(data));
    QUERY_DONE.with(|d| d.store(true, Ordering::SeqCst));
}

/// Map a c-ares status to the Node.js `dns` error code string (the value of
/// `err.code`, e.g. "ENOTFOUND") — the c-ares variant names are spelled
/// exactly as the Node codes they map to. `None` means the query completed
/// but the parse produced no records — Node reports that as ENODATA.
fn status_to_code(status: Option<cares::Error>) -> &'static str {
    match status {
        None => "ENODATA",
        Some(cares::Error::ENODATA) => "ENODATA",
        Some(cares::Error::EFORMERR) => "EFORMERR",
        Some(cares::Error::ESERVFAIL) => "ESERVFAIL",
        Some(cares::Error::ENOTFOUND) => "ENOTFOUND",
        Some(cares::Error::ENOTIMP) => "ENOTIMP",
        Some(cares::Error::EREFUSED) => "EREFUSED",
        Some(cares::Error::EBADQUERY) => "EBADQUERY",
        Some(cares::Error::EBADNAME) => "EBADNAME",
        Some(cares::Error::EBADFAMILY) => "EBADFAMILY",
        Some(cares::Error::EBADRESP) => "EBADRESP",
        Some(cares::Error::ECONNREFUSED) => "ECONNREFUSED",
        Some(cares::Error::ETIMEOUT) => "ETIMEOUT",
        Some(cares::Error::EOF) => "EOF",
        Some(cares::Error::EFILE) => "EFILE",
        Some(cares::Error::ENOMEM) => "ENOMEM",
        Some(cares::Error::EDESTRUCTION) => "EDESTRUCTION",
        Some(cares::Error::EBADSTR) => "EBADSTR",
        Some(cares::Error::EBADFLAGS) => "EBADFLAGS",
        Some(cares::Error::ENONAME) => "ENONAME",
        Some(cares::Error::EBADHINTS) => "EBADHINTS",
        Some(cares::Error::ENOTINITIALIZED) => "ENOTINITIALIZED",
        Some(cares::Error::ELOADIPHLPAPI) => "ELOADIPHLPAPI",
        Some(cares::Error::EADDRGETNETWORKPARAMS) => "EADDRGETNETWORKPARAMS",
        Some(cares::Error::ECANCELLED) => "ECANCELLED",
        Some(cares::Error::ESERVICE) => "ESERVICE",
        Some(cares::Error::ENOSERVER) => "ENOSERVER",
    }
}

/// Helper: read a NUL-terminated `*mut u8` from c-ares as a Rust String.
unsafe fn c_ares_str_to_string(ptr: *mut u8) -> ::std::string::String {
    if ptr.is_null() {
        return String::new();
    }
    ::std::ffi::CStr::from_ptr(ptr.cast::<::std::ffi::c_char>())
        .to_string_lossy()
        .into_owned()
}

// ── A/AAAA: hostent-with-ttls handlers (dns.resolve4/resolve6) ────────
//
// A/AAAA ride the same c-ares stack as every other RR type (ares_query + the
// per-type parse thunk); the former libc getaddrinfo arm was a second
// resolver inside one resolve family. ares_parse_a_reply/ares_parse_aaaa_reply
// allocate the hostent for the caller; the Box<hostent_with_ttls> Drop runs
// ares_free_hostent when the handler returns — the same caller-owned lease as
// the NS path (callback_wrapper_ns + ares_parse_ns_reply).

struct AHostentWithTtlsHandler;

impl cares::HostentWithTtlsHandler for AHostentWithTtlsHandler {
    const PARSE: fn(&[u8]) -> ::std::result::Result<Box<cares::hostent_with_ttls>, cares::Error> =
        cares::hostent_with_ttls::parse_a;

    fn on_hostent_with_ttls(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: Option<Box<cares::hostent_with_ttls>>,
    ) {
        if status.is_some() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        let Some(with_ttls) = results else {
            // Query completed but parse produced nothing — Node's ENODATA.
            query_set_error(status_to_code(None).to_string());
            return;
        };
        // SAFETY: with_ttls.hostent was allocated by PARSE (ares_parse_a_reply).
        let addrs = unsafe { hostent_inet_addrs(with_ttls.hostent) };
        // The Box drop below ends the hostent lease (ares_free_hostent); the
        // addresses are owned Strings copied out before that.
        if addrs.is_empty() {
            // Resolved fine but no address records (e.g. CNAME-only answer):
            // Node reports ENODATA, not [].
            query_set_error("ENODATA".to_string());
            return;
        }
        query_set_result(DnsRRData::A(addrs));
    }
}

struct AaaaHostentWithTtlsHandler;

impl cares::HostentWithTtlsHandler for AaaaHostentWithTtlsHandler {
    const PARSE: fn(&[u8]) -> ::std::result::Result<Box<cares::hostent_with_ttls>, cares::Error> =
        cares::hostent_with_ttls::parse_aaaa;

    fn on_hostent_with_ttls(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: Option<Box<cares::hostent_with_ttls>>,
    ) {
        if status.is_some() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        let Some(with_ttls) = results else {
            query_set_error(status_to_code(None).to_string());
            return;
        };
        // SAFETY: with_ttls.hostent was allocated by PARSE (ares_parse_aaaa_reply).
        let addrs = unsafe { hostent_inet_addrs(with_ttls.hostent) };
        if addrs.is_empty() {
            query_set_error("ENODATA".to_string());
            return;
        }
        query_set_result(DnsRRData::Aaaa(addrs));
    }
}

/// Walk a c-ares hostent's `h_addr_list` and render each address to canonical
/// text (4-byte entries as dotted-quad IPv4, 16-byte as compressed IPv6) —
/// the same shapes as `render_address`/`render_cache_ip`. Read-only: the
/// hostent lease belongs to the caller (parse_*_reply allocation).
///
/// # Safety
/// `hostent` must be null or a live hostent allocated by ares_parse_*_reply.
unsafe fn hostent_inet_addrs(hostent: *mut cares::struct_hostent) -> Vec<::std::string::String> {
    let mut addrs = Vec::new();
    if hostent.is_null() {
        return addrs;
    }
    // SAFETY: caller contract — live parse_*_reply hostent.
    let h = unsafe { &*hostent };
    if h.h_addr_list.is_null() {
        return addrs;
    }
    let mut entry_ptr = h.h_addr_list;
    // SAFETY: h_addr_list is a NUL-terminated array of pointers, each to
    // h_length address bytes owned by the hostent (ares_parse_*_reply contract).
    while !unsafe { *entry_ptr }.is_null() {
        let entry: *mut u8 = unsafe { *entry_ptr }.cast();
        match h.h_length as usize {
            4 => {
                // SAFETY: entry points at h_length == 4 address bytes.
                let octets: [u8; 4] = unsafe { *entry.cast::<[u8; 4]>() };
                addrs.push(format!(
                    "{}.{}.{}.{}",
                    octets[0], octets[1], octets[2], octets[3]
                ));
            }
            16 => {
                // SAFETY: entry points at h_length == 16 address bytes.
                let bytes: [u8; 16] = unsafe { *entry.cast::<[u8; 16]>() };
                addrs.push(::std::net::Ipv6Addr::from(bytes).to_string());
            }
            // parse_a/parse_aaaa hostents only ever carry 4- or 16-byte
            // addresses; anything else is a foreign hostent — skip rather
            // than render garbage.
            _ => {}
        }
        entry_ptr = unsafe { entry_ptr.add(1) };
    }
    addrs
}

// ── CNAME: hostent handler (stores h_name as Cname) ──────────────────

struct CnameHostentHandler;

impl cares::HostentHandler for CnameHostentHandler {
    fn on_hostent(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_hostent,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        // SAFETY: results is non-null per check above.
        let h = unsafe { &*results };
        let cname = unsafe { c_ares_str_to_string(h.h_name.cast::<u8>()) };
        // Ownership: this handler is only registered via host_callback_wrapper
        // on ares_gethostbyname, so the hostent is owned by c-ares itself —
        // ares_gethostbyname(3): "the ares library will free it when the
        // callback returns". Freeing here double-frees once end_hquery's
        // teardown runs and was the mi_free invalid-pointer SIGSEGV on real
        // CNAME/PTR answers. We copy h_name into an owned String above and
        // must not touch `results` afterwards.
        query_set_result(DnsRRData::Cname(cname));
    }
}

// ── NS: hostent handler (collects nameserver hostnames) ──────────────

struct NsHostentHandler;

impl cares::HostentHandler for NsHostentHandler {
    fn on_hostent(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_hostent,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        // SAFETY: results is non-null per check above.
        let h = unsafe { &*results };
        let mut names = Vec::new();
        // ares_parse_ns_reply(3) fills h_name with the QUERIED DOMAIN and
        // puts the NS server hostnames in h_aliases (probe-verified against
        // system c-ares 1.27.0: query NS github.com → h_name="github.com",
        // h_aliases=[dns2.p08.nsone.net, ns-1283.awsdns-32.org, ...]).
        // Node's resolveNs returns the servers, so collect aliases only.
        if !h.h_aliases.is_null() {
            let mut alias_ptr = h.h_aliases;
            while !unsafe { *alias_ptr }.is_null() {
                names.push(
                    unsafe { ::std::ffi::CStr::from_ptr(*alias_ptr) }
                        .to_string_lossy()
                        .into_owned(),
                );
                alias_ptr = unsafe { alias_ptr.add(1) };
            }
        }
        // Ownership: this handler is registered ONLY via callback_wrapper_ns,
        // whose hostent comes from ares_parse_ns_reply — allocated FOR THE
        // CALLER and freed with ares_free_hostent (unlike the
        // ares_gethostbyname callback hostent that c-ares itself owns; see
        // CnameHostentHandler). All names are copied into owned Strings
        // above, so freeing here is the correct end of the lease.
        unsafe { cares::ares_free_hostent(results) };
        query_set_result(DnsRRData::Ns(names));
    }
}

// ── PTR: hostent handler (reverse-resolved hostnames) ────────────────

struct PtrHostentHandler;

impl cares::HostentHandler for PtrHostentHandler {
    fn on_hostent(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_hostent,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        // SAFETY: results is non-null per check above.
        let h = unsafe { &*results };
        let mut names = Vec::new();
        // Channel::get_host_by_addr → ares_gethostbyaddr: h_name is the
        // primary reverse hostname and h_aliases carries the PTR targets —
        // c-ares echoes the primary as aliases[0] (probe-verified:
        // 8.8.8.8 → h_name=h_aliases[0]="dns.google"). Collecting aliases
        // only therefore matches Node's resolvePtr without duplicating
        // h_name; fall back to h_name when the alias list is empty.
        if !h.h_aliases.is_null() {
            let mut alias_ptr = h.h_aliases;
            while !unsafe { *alias_ptr }.is_null() {
                names.push(
                    unsafe { ::std::ffi::CStr::from_ptr(*alias_ptr) }
                        .to_string_lossy()
                        .into_owned(),
                );
                alias_ptr = unsafe { alias_ptr.add(1) };
            }
        }
        if names.is_empty() && !h.h_name.is_null() {
            names.push(unsafe { c_ares_str_to_string(h.h_name.cast::<u8>()) });
        }
        // Ownership: registered via host_callback_wrapper on
        // ares_gethostbyaddr — the hostent is owned by c-ares and freed
        // after the callback returns (same contract as CnameHostentHandler);
        // do not free here.
        query_set_result(DnsRRData::Ptr(names));
    }
}

// ── MX handler ───────────────────────────────────────────────────────

struct MxHandler;

impl cares::ReplyHandler<cares::struct_ares_mx_reply> for MxHandler {
    fn on_reply(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_ares_mx_reply,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        let mut mx_list = Vec::new();
        let mut cur = results;
        while !cur.is_null() {
            // SAFETY: cur is non-null, walks the linked list.
            let mx = unsafe { &*cur };
            let host = unsafe { c_ares_str_to_string(mx.host) };
            mx_list.push((mx.priority, host));
            cur = mx.next;
        }
        // SAFETY: free the linked list.
        unsafe { cares::ares_free_data(results.cast::<::std::ffi::c_void>()) };
        query_set_result(DnsRRData::Mx(mx_list));
    }
}

// ── TXT handler ──────────────────────────────────────────────────────

struct TxtHandler;

impl cares::ReplyHandler<cares::struct_ares_txt_ext> for TxtHandler {
    fn on_reply(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_ares_txt_ext,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        // Group chunks into records: `record_start` marks the first chunk of
        // each DNS TXT record (a record may carry multiple ≤255-byte chunks).
        // Node.js dns.resolveTxt returns one inner array per record.
        let mut records: Vec<Vec<::std::string::String>> = Vec::new();
        let mut cur = results;
        while !cur.is_null() {
            // SAFETY: cur is non-null, walks the linked list.
            let txt = unsafe { &*cur };
            if txt.record_start != 0 || records.is_empty() {
                records.push(Vec::new());
            }
            records
                .last_mut()
                .expect("record group just opened")
                .push(::std::string::String::from_utf8_lossy(txt.txt_bytes()).into_owned());
            cur = txt.next;
        }
        // SAFETY: free the linked list.
        unsafe { cares::ares_free_data(results.cast::<::std::ffi::c_void>()) };
        query_set_result(DnsRRData::Txt(records));
    }
}

// ── SOA handler ──────────────────────────────────────────────────────

struct SoaHandler;

impl cares::ReplyHandler<cares::struct_ares_soa_reply> for SoaHandler {
    fn on_reply(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_ares_soa_reply,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        // SAFETY: results is non-null SOA reply.
        let soa = unsafe { &*results };
        // Copy EVERY field out before ares_free_data — the old code read the
        // five numeric fields from `soa` after freeing the reply (the two
        // strings happened to be copied pre-free, which is why nsname /
        // hostmaster came back right while serial/refresh/retry/expire/
        // minttl read the allocator's post-free fill pattern — the
        // -538976289 / 0xDFDFDFDF garbage). Reading freed memory is UB;
        // snapshot all fields first, then end the lease.
        let nsname = unsafe { c_ares_str_to_string(soa.nsname) };
        let hostmaster = unsafe { c_ares_str_to_string(soa.hostmaster) };
        let serial = soa.serial;
        let refresh = soa.refresh;
        let retry = soa.retry;
        let expire = soa.expire;
        let minttl = soa.minttl;
        // SAFETY: free the SOA reply.
        unsafe { cares::ares_free_data(results.cast::<::std::ffi::c_void>()) };
        query_set_result(DnsRRData::Soa {
            nsname,
            hostmaster,
            serial,
            refresh,
            retry,
            expire,
            minttl,
        });
    }
}

// ── SRV handler ──────────────────────────────────────────────────────

struct SrvHandler;

impl cares::ReplyHandler<cares::struct_ares_srv_reply> for SrvHandler {
    fn on_reply(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_ares_srv_reply,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        let mut srv_list = Vec::new();
        let mut cur = results;
        while !cur.is_null() {
            // SAFETY: cur is non-null, walks linked list.
            let srv = unsafe { &*cur };
            let host = unsafe { c_ares_str_to_string(srv.host) };
            srv_list.push((srv.priority, srv.weight, srv.port, host));
            cur = srv.next;
        }
        // SAFETY: free the linked list.
        unsafe { cares::ares_free_data(results.cast::<::std::ffi::c_void>()) };
        query_set_result(DnsRRData::Srv(srv_list));
    }
}

// ── NAPTR handler ────────────────────────────────────────────────────

struct NaptrHandler;

impl cares::ReplyHandler<cares::struct_ares_naptr_reply> for NaptrHandler {
    fn on_reply(
        &mut self,
        status: Option<cares::Error>,
        _timeouts: i32,
        results: *mut cares::struct_ares_naptr_reply,
    ) {
        if status.is_some() || results.is_null() {
            query_set_error(status_to_code(status).to_string());
            return;
        }
        let mut naptr_list = Vec::new();
        let mut cur = results;
        while !cur.is_null() {
            // SAFETY: cur is non-null, walks linked list.
            let naptr = unsafe { &*cur };
            naptr_list.push(NaptrRecord {
                flags: unsafe { c_ares_str_to_string(naptr.flags) },
                service: unsafe { c_ares_str_to_string(naptr.service) },
                regexp: unsafe { c_ares_str_to_string(naptr.regexp) },
                replacement: unsafe { c_ares_str_to_string(naptr.replacement) },
                order: naptr.order,
                preference: naptr.preference,
            });
            cur = naptr.next;
        }
        // SAFETY: free the linked list.
        unsafe { cares::ares_free_data(results.cast::<::std::ffi::c_void>()) };
        query_set_result(DnsRRData::Naptr(naptr_list));
    }
}

// ── Resolve RR type dispatch ─────────────────────────────────────────

/// Resolve a hostname for the given RR type synchronously using c-ares.
/// Returns `Ok(DnsRRData)` on success, `Err(error_code_string)` on failure.
fn resolve_rr_cares(
    hostname: &str,
    ns_type: cares::NSType,
) -> ::std::result::Result<DnsRRData, ::std::string::String> {
    query_reset();

    let container = SyncChannelContainer {
        channel: RefCell::new(::std::ptr::null_mut()),
        sockets: RefCell::new(Vec::new()),
    };

    if let Some(err) = cares::Channel::init(
        &container,
        cares::ChannelOptions {
            timeout: Some(5000),
            tries: Some(2),
        },
    ) {
        return Err(status_to_code(Some(err)).to_string());
    }

    let channel_ptr = *container.channel.borrow();
    // SAFETY: channel_ptr is a live channel from ares_init_options. Channel is
    // a ZST (!Freeze) so &mut from raw is sound (no data to conflict).
    let channel_ref = unsafe { &mut *channel_ptr };

    // Submit the query based on RR type. A/AAAA use ares_query +
    // hostent_with_ttls::callback_wrapper (ares_parse_a_reply/
    // ares_parse_aaaa_reply hostent, caller-owned via Box Drop); CNAME uses
    // ares_gethostbyname (which returns a c-ares-owned struct_hostent);
    // MX/TXT/SOA/SRV/NAPTR use ares_query with the generic
    // ares_reply_callback<R, Handler> thunk; NS uses ares_query +
    // callback_wrapper_ns (ares_parse_ns_reply hostent, caller-owned); PTR
    // uses Channel::get_host_by_addr (ares_gethostbyaddr, c-ares-owned
    // hostent).
    match ns_type {
        cares::NSType::ns_t_a => {
            let mut handler = AHostentWithTtlsHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_a,
                    Some(
                        cares::hostent_with_ttls::callback_wrapper::<AHostentWithTtlsHandler>,
                    ),
                    ::std::ptr::from_mut::<AHostentWithTtlsHandler>(&mut handler)
                        .cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_aaaa => {
            let mut handler = AaaaHostentWithTtlsHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_aaaa,
                    Some(
                        cares::hostent_with_ttls::callback_wrapper::<AaaaHostentWithTtlsHandler>,
                    ),
                    ::std::ptr::from_mut::<AaaaHostentWithTtlsHandler>(&mut handler)
                        .cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_cname => {
            let mut handler = CnameHostentHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: FFI call; name_ptr is NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_gethostbyname(
                    channel_ptr,
                    name_ptr,
                    cares::AF::INET,
                    Some(cares::struct_hostent::host_callback_wrapper::<CnameHostentHandler>),
                    ::std::ptr::from_mut::<CnameHostentHandler>(&mut handler)
                        .cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_ns => {
            let mut handler = NsHostentHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // A real NS query — ares_gethostbyname here issued an A query and
            // handed back the host's own name/aliases (resolveNs("x.com")
            // returned ["x.com"]). callback_wrapper_ns parses the NS answer
            // with ares_parse_ns_reply, whose hostent carries the nameserver
            // hostnames in h_aliases (and is caller-owned; see the handler).
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_ns,
                    Some(cares::struct_hostent::callback_wrapper_ns::<NsHostentHandler>),
                    ::std::ptr::from_mut::<NsHostentHandler>(&mut handler)
                        .cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_ptr => {
            let mut handler = PtrHostentHandler;
            // Real reverse resolution via ares_gethostbyaddr — the old path
            // re-queried the input STRING as CNAME, so resolvePtr echoed the
            // IP back. get_host_by_addr parses the address (v4 or v6) and
            // submits the in-addr.arpa/ip6.arpa PTR query; the hostent
            // contract is c-ares-owned (see PtrHostentHandler).
            channel_ref.get_host_by_addr(hostname.as_bytes(), &mut handler);
        }
        cares::NSType::ns_t_mx => {
            let mut handler = MxHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_mx,
                    Some(cares::ares_reply_callback::<cares::struct_ares_mx_reply, MxHandler>),
                    ::std::ptr::from_mut::<MxHandler>(&mut handler).cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_txt => {
            let mut handler = TxtHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_txt,
                    Some(cares::ares_reply_callback::<cares::struct_ares_txt_ext, TxtHandler>),
                    ::std::ptr::from_mut::<TxtHandler>(&mut handler).cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_soa => {
            let mut handler = SoaHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_soa,
                    Some(cares::ares_reply_callback::<cares::struct_ares_soa_reply, SoaHandler>),
                    ::std::ptr::from_mut::<SoaHandler>(&mut handler).cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_srv => {
            let mut handler = SrvHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_srv,
                    Some(cares::ares_reply_callback::<cares::struct_ares_srv_reply, SrvHandler>),
                    ::std::ptr::from_mut::<SrvHandler>(&mut handler).cast::<::std::ffi::c_void>(),
                );
            }
        }
        cares::NSType::ns_t_naptr => {
            let mut handler = NaptrHandler;
            let mut name_buf = [0u8; 1024];
            let name_ptr = nul_terminate(&mut name_buf, hostname.as_bytes());
            // SAFETY: ares_query FFI; name_ptr NUL-terminated; handler outlives query.
            unsafe {
                cares::ares_query(
                    channel_ptr,
                    name_ptr,
                    cares::NSClass::ns_c_in,
                    cares::NSType::ns_t_naptr,
                    Some(
                        cares::ares_reply_callback::<cares::struct_ares_naptr_reply, NaptrHandler>,
                    ),
                    ::std::ptr::from_mut::<NaptrHandler>(&mut handler).cast::<::std::ffi::c_void>(),
                );
            }
        }
        _ => {
            // Unknown RR type — destroy channel and return error.
            unsafe { cares::Channel::destroy(channel_ptr) };
            return Err("ENOTIMP".to_string());
        }
    }

    // ── Drive c-ares event loop synchronously ────────────────────────
    // Poll the fds c-ares is watching and call ares_process_fd until the
    // callback fires (QUERY_DONE becomes true) or we time out.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !QUERY_DONE.with(|d| d.load(Ordering::SeqCst)) {
        if Instant::now() >= deadline {
            // SAFETY: cancel all pending queries, which will invoke callbacks
            // with ECANCELLED status, setting QUERY_DONE.
            cares::ares_cancel(channel_ref);
            // Process the cancellation callbacks.
            drive_cares_channel(channel_ref, &container);
            break;
        }

        drive_cares_channel(channel_ref, &container);

        if !QUERY_DONE.with(|d| d.load(Ordering::SeqCst)) {
            // No callbacks fired yet — brief sleep to avoid busy-looping.
            // c-ares may open new sockets during processing, so we re-check
            // sockets next iteration.
            ::std::thread::sleep(Duration::from_millis(1));
        }
    }

    // SAFETY: all queries are complete (done or cancelled). Destroy the channel.
    unsafe { cares::Channel::destroy(channel_ptr) };

    // Collect the result.
    if let Some(err) = QUERY_ERROR.with(|e| e.borrow_mut().take()) {
        Err(err)
    } else {
        QUERY_RESULT
            .with(|r| r.borrow_mut().take())
            .ok_or_else(|| "ENODATA".to_string())
    }
}

/// Drive the c-ares channel by polling its sockets and processing ready fds.
fn drive_cares_channel(channel: &mut cares::Channel, container: &SyncChannelContainer) {
    // Use ares_getsock to get the authoritative fd list from c-ares.
    let mut ares_socks = [0 as cares::ares_socket_t; cares::ARES_GETSOCK_MAXNUM as usize];
    // SAFETY: channel is live; ares_socks is a valid array.
    let bitmask = unsafe {
        cares::ares_getsock(channel, ares_socks.as_mut_ptr(), cares::ARES_GETSOCK_MAXNUM)
    };

    // Build poll array from ares_getsock output.
    let mut poll_fds: Vec<libc::pollfd> = Vec::new();
    for i in 0..cares::ARES_GETSOCK_MAXNUM as usize {
        let fd = ares_socks[i];
        if fd == cares::ARES_SOCKET_BAD {
            break;
        }
        let readable = cares::ares_getsock_readable(bitmask, i as ::std::ffi::c_int) != 0;
        let writable = cares::ares_getsock_writable(bitmask, i as ::std::ffi::c_int) != 0;
        if readable || writable {
            poll_fds.push(libc::pollfd {
                fd: fd as libc::c_int,
                events: (if readable { libc::POLLIN } else { 0 })
                    | (if writable { libc::POLLOUT } else { 0 }),
                revents: 0,
            });
        }
    }

    if poll_fds.is_empty() {
        // No fds to poll — just process timeouts.
        cares::ares_process_fd(channel, cares::ARES_SOCKET_BAD, cares::ARES_SOCKET_BAD);
        return;
    }

    // Poll with a short timeout (10ms) so we don't block too long.
    let rc = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as libc::nfds_t, 10) };
    if rc >= 0 {
        // Process each fd that has events.
        for pfd in &poll_fds {
            let readable = pfd.revents & libc::POLLIN != 0;
            let writable = pfd.revents & libc::POLLOUT != 0;
            let has_err = pfd.revents & (libc::POLLERR | libc::POLLHUP) != 0;
            if readable || writable || has_err {
                // On error/hup, signal both readable and writable so c-ares
                // can detect the socket failure.
                cares::ares_process_fd(
                    channel,
                    if readable || has_err {
                        pfd.fd as cares::ares_socket_t
                    } else {
                        cares::ARES_SOCKET_BAD
                    },
                    if writable || has_err {
                        pfd.fd as cares::ares_socket_t
                    } else {
                        cares::ARES_SOCKET_BAD
                    },
                );
            }
        }
    }
    // Always process timeouts after polling.
    cares::ares_process_fd(channel, cares::ARES_SOCKET_BAD, cares::ARES_SOCKET_BAD);
    // Suppress unused-variable warning for container (its sockets field is
    // used by on_dns_socket_state callback through the channel).
    let _ = &container.sockets;
}

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
/// Failure is `Err(raw_gai_rc)` — the getaddrinfo EAI_* return code, a
/// truthful failure signal for the libc stack (the RR resolve family no
/// longer consumes it; its A/AAAA arms ride c-ares like every other RR
/// type). Lookup callers collapse it back to an empty result via
/// `unwrap_or_default`, preserving dns.lookup's behavior.
///
/// @trace REQ-ENG-007 [api:dns.lookup] [code:bun_dns]
fn resolve_hostname_libc(
    hostname: &str,
) -> ::std::result::Result<Vec<(::std::string::String, i32)>, i32> {
    // Shared-cache consultation (fusion point with the usockets and
    // servo/hyper paths — one resolver per process, not per stack). Hit →
    // render straight from the cached addresses, no system call.
    if let Some(addrs) = bun_dns::cache::lookup(hostname.as_bytes()) {
        return Ok(addrs
            .iter()
            .map(|ip| (render_cache_ip(ip), cache_ip_family(ip)))
            .collect());
    }

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

    // libc::getaddrinfo wants a NUL-terminated hostname. A NUL byte in the
    // input can never be a resolvable name — report it as EAI_NONAME (the
    // "name not known" class) rather than swallowing it.
    let c_host = match CString::new(hostname) {
        Ok(c) => c,
        Err(_) => return Err(libc::EAI_NONAME),
    };
    let hints = req.options.to_libc();

    let mut result_head: *mut addrinfo = ::std::ptr::null_mut();
    let rc = unsafe {
        libc::getaddrinfo(
            c_host.as_ptr(),
            ::std::ptr::null(),
            hints
                .as_ref()
                .map(|h| h as *const addrinfo)
                .unwrap_or(::std::ptr::null()),
            &mut result_head,
        )
    };
    if rc != 0 {
        return Err(rc);
    }
    if result_head.is_null() {
        return Ok(Vec::new());
    }

    // Walk the chain; freeaddrinfo on scope exit (Drop would require wrapping,
    // so do it manually after collecting).
    let mut out: Vec<(::std::string::String, i32)> = Vec::new();
    let mut cache_ips: Vec<bun_dns::cache::IpAddr> = Vec::new();
    let mut cur: *mut addrinfo = result_head;
    while !cur.is_null() {
        // SAFETY: cur is non-null and points into the getaddrinfo result chain.
        let ai = unsafe { &*cur };
        if let Some(res) = GetAddrInfoResult::from_addr_info(ai) {
            if let Some(s) = render_address(&res.address) {
                let family = if res.address.family() == libc::AF_INET6 {
                    6
                } else {
                    4
                };
                out.push((s, family));
            }
            if let Some(ip) = address_to_cache_ip(&res.address) {
                cache_ips.push(ip);
            }
        }
        cur = ai.ai_next;
    }
    // SAFETY: result_head was allocated by C getaddrinfo; chain intact above.
    unsafe { freeaddrinfo(result_head) };
    // getaddrinfo returns no TTL (see bun_dns GetAddrInfoResult::ttl) — the
    // cache applies its engine cap (BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS).
    bun_dns::cache::insert(hostname.as_bytes(), cache_ips, None);
    Ok(out)
}

/// Map a raw `getaddrinfo`/`getnameinfo` EAI_* return code to the Node dns
/// error-code string for OS resolver paths. EAI_NONAME → ENOTFOUND and
/// EAI_NODATA → ENODATA follow the observable Node 24.5.0 resolve oracle;
/// the rest keep their getSystemErrorName spellings (libuv translates
/// EAI_MEMORY → ENOMEM), and an unrecognized code surfaces its raw value
/// rather than a placeholder.
/// https://github.com/nodejs/node/blob/v24.5.0/lib/internal/errors.js#L795-L823
fn gai_error_to_dns_code(rc: i32) -> ::std::string::String {
    match rc {
        libc::EAI_NONAME => "ENOTFOUND".to_string(),
        libc::EAI_NODATA => "ENODATA".to_string(),
        libc::EAI_AGAIN => "EAI_AGAIN".to_string(),
        libc::EAI_MEMORY => "ENOMEM".to_string(),
        libc::EAI_BADFLAGS => "EAI_BADFLAGS".to_string(),
        libc::EAI_FAIL => "EAI_FAIL".to_string(),
        libc::EAI_FAMILY => "EAI_FAMILY".to_string(),
        libc::EAI_SERVICE => "EAI_SERVICE".to_string(),
        libc::EAI_SOCKTYPE => "EAI_SOCKTYPE".to_string(),
        libc::EAI_SYSTEM => "EAI_SYSTEM".to_string(),
        libc::EAI_OVERFLOW => "EAI_OVERFLOW".to_string(),
        _ => format!("EAI_{}", rc),
    }
}

/// Render a shared-cache address to canonical text (same forms as
/// [`render_address`], from the raw cached bytes instead of a sockaddr).
fn render_cache_ip(ip: &bun_dns::cache::IpAddr) -> ::std::string::String {
    match ip {
        bun_dns::cache::IpAddr::V4(o) => format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]),
        bun_dns::cache::IpAddr::V6(bytes) => {
            let segs: [u16; 8] =
                core::array::from_fn(|i| u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]));
            ::std::net::Ipv6Addr::from(segs).to_string()
        }
    }
}

/// Node-style family marker (4/6) for a shared-cache address.
fn cache_ip_family(ip: &bun_dns::cache::IpAddr) -> i32 {
    match ip {
        bun_dns::cache::IpAddr::V4(_) => 4,
        bun_dns::cache::IpAddr::V6(_) => 6,
    }
}

/// Extract the raw bytes of a resolved `bun_dns::Address` for the shared
/// cache. Returns None for non-IP families (AF_UNIX etc.).
fn address_to_cache_ip(addr: &bun_dns::Address) -> Option<bun_dns::cache::IpAddr> {
    if let Some(v4) = addr.as_in4() {
        // SAFETY: sin_addr is 4 POD bytes on every target (see render_address).
        let octets: [u8; 4] = unsafe { *::std::ptr::addr_of!(v4.sin_addr).cast::<[u8; 4]>() };
        return Some(bun_dns::cache::IpAddr::V4(octets));
    }
    if let Some(v6) = addr.as_in6() {
        // SAFETY: sin6_addr is 16 POD bytes (see render_address).
        let bytes: [u8; 16] = unsafe { *::std::ptr::addr_of!(v6.sin6_addr).cast::<[u8; 16]>() };
        return Some(bun_dns::cache::IpAddr::V6(bytes));
    }
    None
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
        return Some(format!(
            "{}.{}.{}.{}",
            octets[0], octets[1], octets[2], octets[3]
        ));
    }
    if let Some(v6) = addr.as_in6() {
        // SAFETY: sin6_addr is 16 POD bytes (in6_addr).
        let bytes: [u8; 16] = unsafe { *::std::ptr::addr_of!(v6.sin6_addr).cast::<[u8; 16]>() };
        let segs: [u16; 8] =
            core::array::from_fn(|i| u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]));
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

  // Node ERR_INVALID_ARG_TYPE (TypeError) helper — mirrors Node's
  // determineSpecificType (lib/internal/validators.js) for every shape the
  // resolve*/reverse callback checks can reject.
  function _invalidArgType(name, expected, value) {
    var received;
    if (value === null) {
      received = "null";
    } else if (Array.isArray(value)) {
      received = "an instance of Array";
    } else {
      switch (typeof value) {
        case "function":
          received = "function " + (value.name || "");
          break;
        case "string":
          // Node inspect truncates >27 chars: first 24 + "..."
          var shown = value.length > 27 ? value.slice(0, 24) + "..." : value;
          received = "type string ('" + shown + "')";
          break;
        case "number":
          received = "type number (" + String(value) + ")";
          break;
        case "boolean":
          received = "type boolean (" + String(value) + ")";
          break;
        case "undefined":
          received = "type undefined";
          break;
        default:
          received = "an instance of " +
            ((value && value.constructor && value.constructor.name) || "Object");
      }
    }
    var err = new TypeError('The "' + name + '" argument must be of type ' +
      expected + '. Received ' + received);
    err.code = "ERR_INVALID_ARG_TYPE";
    throw err;
  }

  // Callback-slot validation: a present-but-non-function callback throws
  // ERR_INVALID_ARG_TYPE exactly like Node's validateFunction.
  function _validateCallback(callback) {
    if (callback !== undefined && typeof callback !== "function") {
      _invalidArgType("callback", "function", callback);
    }
  }

  // Shared __dns_resolve_rr driver. On resolution failure the native side
  // throws an Error already carrying Node's `code` (e.g. ENOTFOUND) and
  // Node's "<syscall> <CODE> <hostname>" message. Node's resolve* never
  // throws synchronously on resolution failure — the error goes to the
  // callback (the promise API rejects through that same callback) — so we
  // route it there and return the per-type empty fallback.
  function _resolveRR(hostname, rrtype, fallback, callback) {
    var result;
    try {
      result = __dns_resolve_rr(hostname, rrtype);
    } catch (e) {
      if (callback) callback(e);
      return fallback;
    }
    if (callback) callback(null, result);
    return result;
  }

  function Resolver() {
    this._servers = [];
  }
  Resolver.prototype.resolve = function(hostname, rrtype, callback) {
    if (typeof rrtype === "function") { callback = rrtype; rrtype = "A"; }
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, rrtype || "A", [], callback);
    }
    if (callback) callback(new Error("dns.resolve not available"));
    return [];
  };
  Resolver.prototype.resolve4 = function(hostname, options, callback) {
    if (arguments.length <= 2) { callback = options; options = null; }
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "A", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolve6 = function(hostname, options, callback) {
    if (arguments.length <= 2) { callback = options; options = null; }
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "AAAA", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveCname = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "CNAME", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveMx = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "MX", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveNaptr = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "NAPTR", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveNs = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "NS", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolvePtr = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "PTR", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveSoa = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "SOA", {}, callback);
    }
    if (callback) callback(null, {});
    return {};
  };
  Resolver.prototype.resolveSrv = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "SRV", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveTxt = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "TXT", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.resolveAny = function(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "A", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  };
  Resolver.prototype.reverse = function(ip, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
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
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, rrtype || "A", [], callback);
    }
    if (callback) callback(new Error("dns.resolve not available"));
    return [];
  }

  function resolve4(hostname, options, callback) {
    if (arguments.length <= 2) { callback = options; options = null; }
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "A", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolve6(hostname, options, callback) {
    if (arguments.length <= 2) { callback = options; options = null; }
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "AAAA", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveCname(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "CNAME", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveMx(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "MX", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveNaptr(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "NAPTR", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveNs(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "NS", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolvePtr(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "PTR", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveSoa(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "SOA", {}, callback);
    }
    if (callback) callback(null, {});
    return {};
  }

  function resolveSrv(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "SRV", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveTxt(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "TXT", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function resolveAny(hostname, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
    if (typeof __dns_resolve_rr === "function") {
      return _resolveRR(hostname, "A", [], callback);
    }
    if (callback) callback(null, []);
    return [];
  }

  function reverse(ip, callback) {
    if (arguments.length > 2) callback = arguments[2];
    _validateCallback(callback);
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
    // NB: executor params are fulfill/reject — naming the first one `resolve`
    // shadows the outer dns.resolve below, so the DNS query never runs and the
    // promise fulfills with the raw hostname string instead of the RR records.
    resolve: function(hostname, rrtype) {
      return new Promise(function(fulfill, reject) {
        resolve(hostname, rrtype || "A", function(err, result) {
          if (err) reject(err);
          else fulfill(result);
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
        JS_ReportErrorUTF8(cx, c"dns.lookup requires a hostname argument".as_ptr());
        return false;
    }

    let hostname_val = *args.get(0).ptr;
    if !hostname_val.is_string() {
        JS_ReportErrorUTF8(cx, c"dns.lookup hostname must be a string".as_ptr());
        return false;
    }

    let hostname = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if result_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_ref) let result_root = result_obj);

    // @trace REQ-ENG-007 [api:dns.lookup] [code:bun_dns] — resolve through
    // bun_dns (Backend::Libc); take the first address for the lookup result.
    // lookup keeps its empty-result-on-failure behavior (unwrap_or_default);
    // the resolve* arms are the consumers that surface the EAI error.
    let resolved = resolve_hostname_libc(&hostname).unwrap_or_default();
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
unsafe fn define_empty_lookup_result(
    cx: *mut JSContext,
    cx_ref: &mozjs::context::JSContext,
    result_h: Handle<*mut JSObject>,
) {
    let js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let ip_val = StringValue(&*js_str));
        JS_DefineProperty(
            cx,
            result_h,
            c"address".as_ptr(),
            ip_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    rooted!(&in(cx_ref) let family_val = Int32Value(4));
    JS_DefineProperty(
        cx,
        result_h,
        c"family".as_ptr(),
        family_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
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

    let hostname = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    // @trace REQ-ENG-007 [api:dns.resolve] [code:bun_dns] — resolve all
    // addresses via bun_dns (Backend::Libc) and push each into the JS array.
    let resolved = resolve_hostname_libc(&hostname).unwrap_or_default();
    let mut idx = 0u32;
    for (ip, _family) in resolved {
        let c_ip = ZBox::from_bytes(ip.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
            JS_DefineElement(
                cx,
                arr_root.handle().into(),
                idx,
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
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

    let hostname = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    // @trace REQ-ENG-007 [api:dns.resolve6] [code:bun_dns] — resolve via
    // bun_dns (Backend::Libc) and keep only the IPv6 (family == 6) addresses.
    let resolved = resolve_hostname_libc(&hostname).unwrap_or_default();
    let mut idx = 0u32;
    for (ip, family) in resolved {
        if family == 6 {
            let c_ip = ZBox::from_bytes(ip.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                JS_DefineElement(
                    cx,
                    arr_root.handle().into(),
                    idx,
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                idx += 1;
            }
        }
    }

    args.rval().set(ObjectValue(arr_root.get()));
    true
}

/// Build a `sockaddr_storage` from an IP string (no port needed).
/// Returns `(sockaddr_storage, actual_len)` or `None` if the IP is invalid.
///
/// Node's `dns.reverse(ip)` takes a bare IP — no port. Parse as `IpAddr` and
/// attach port 0; parsing as `SocketAddr` would reject every bare IP (it
/// requires `host:port`), which made reverse lookups never run at all.
fn ip_to_sockaddr(
    ip_str: &str,
) -> Option<(
    ::std::net::SocketAddr,
    libc::sockaddr_storage,
    libc::socklen_t,
)> {
    let ip: ::std::net::IpAddr = ip_str.parse().ok()?;
    let addr: ::std::net::SocketAddr = match ip {
        ::std::net::IpAddr::V4(v4) => {
            ::std::net::SocketAddrV4::new(v4, 0).into()
        }
        ::std::net::IpAddr::V6(v6) => {
            ::std::net::SocketAddrV6::new(v6, 0, 0, 0).into()
        }
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
        JS_ReportErrorUTF8(cx, c"dns.reverse requires an ip argument".as_ptr());
        return false;
    }

    let ip_val = *args.get(0).ptr;
    if !ip_val.is_string() {
        JS_ReportErrorUTF8(cx, c"dns.reverse ip must be a string".as_ptr());
        return false;
    }

    let ip_str = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(ip_val.to_string()));

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
    if arr_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_wrap) let arr_root = arr_obj);

    // Use libc::getnameinfo with NI_NAMEREQD for real reverse DNS lookup.
    let Some((_addr, sa, sa_len)) = ip_to_sockaddr(&ip_str) else {
        return throw_resolve_error(cx, "getHostByAddr", "EINVAL", &ip_str);
    };
    let mut host_buf = [0i8; 1025];
    let rc = unsafe {
        libc::getnameinfo(
            ::std::ptr::from_ref(&sa).cast::<libc::sockaddr>(),
            sa_len,
            host_buf.as_mut_ptr(),
            host_buf.len() as libc::socklen_t,
            ::std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if rc != 0 {
        let code = gai_error_to_dns_code(rc);
        return throw_resolve_error(cx, "getHostByAddr", &code, &ip_str);
    }

    let hostname = unsafe { ::std::ffi::CStr::from_ptr(host_buf.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    let c_host = ZBox::from_bytes(hostname.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_host.as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
        JS_DefineElement(
            cx,
            arr_root.handle().into(),
            0,
            val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
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
        JS_ReportErrorUTF8(cx, c"dns.lookupService address must be a string".as_ptr());
        return false;
    }
    let addr_str = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(addr_val.to_string()));

    let port: u16 = if argc > 1 {
        let port_val = *args.get(1).ptr;
        if port_val.is_int32() {
            port_val.to_int32() as u16
        } else if port_val.is_double() {
            port_val.to_double() as u16
        } else {
            0
        }
    } else {
        0
    };

    let cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
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
                JS_DefineProperty(
                    cx,
                    result_root.handle().into(),
                    c"hostname".as_ptr(),
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let c_unk = ZBox::from_bytes("unknown".as_bytes());
            let js_unk = JS_NewStringCopyZ(cx, c_unk.as_ptr());
            if !js_unk.is_null() {
                rooted!(&in(cx_wrap) let v = StringValue(&*js_unk));
                JS_DefineProperty(
                    cx,
                    result_root.handle().into(),
                    c"service".as_ptr(),
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
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
            ::std::ptr::from_ref(&sa).cast::<libc::sockaddr>(),
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
            JS_DefineProperty(
                cx,
                result_root.handle().into(),
                c"hostname".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let c_serv = ZBox::from_bytes(service.as_bytes());
        let js_serv = JS_NewStringCopyZ(cx, c_serv.as_ptr());
        if !js_serv.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_serv));
            JS_DefineProperty(
                cx,
                result_root.handle().into(),
                c"service".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    } else {
        // getnameinfo failed — return the IP as hostname, "unknown" as service
        let c_host = ZBox::from_bytes(addr_str.as_bytes());
        let js_host = JS_NewStringCopyZ(cx, c_host.as_ptr());
        if !js_host.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_host));
            JS_DefineProperty(
                cx,
                result_root.handle().into(),
                c"hostname".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let c_unk = ZBox::from_bytes("unknown".as_bytes());
        let js_unk = JS_NewStringCopyZ(cx, c_unk.as_ptr());
        if !js_unk.is_null() {
            rooted!(&in(cx_wrap) let v = StringValue(&*js_unk));
            JS_DefineProperty(
                cx,
                result_root.handle().into(),
                c"service".as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
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
                JS_DefineElement(
                    cx,
                    arr_root.handle().into(),
                    idx,
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
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
        JS_GetElement(
            cx,
            arr_obj.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if elem.is_string() {
            let s = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(elem.to_string()));
            new_servers.push(s);
        }
    }

    DNS_SERVERS.with(|servers| {
        *servers.borrow_mut() = new_servers;
    });

    args.rval().set(UndefinedValue());
    true
}

/// Report a Node-style resolver failure — message `"<syscall> <CODE> <hostname>"`
/// (e.g. `queryNs ENOTFOUND this-domain….invalid`, Node's exact format) with
/// the machine-readable `code` property attached to the pending Error object —
/// and return `false` so the JS caller observes a thrown exception whose
/// `err.code` the DNS_JS catch sites route to the callback / promise rejection.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_resolve_error(
    cx: *mut JSContext,
    syscall: &str,
    code: &str,
    hostname: &str,
) -> bool {
    let msg = format!("{} {} {}", syscall, code, hostname);
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    // "%s" format with one NUL-terminated argument — hostname bytes (which
    // may contain '%') are never interpreted as format specifiers.
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    // Attach `code` to the pending Error so the JS catch site reads err.code
    // without parsing the message.
    let mut pending = UndefinedValue();
    JS_GetPendingException(
        cx,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut pending,
        },
    );
    if pending.is_object() {
        let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(cx_ref) let err_obj = pending.to_object());
        let c_code = ZBox::from_bytes(code.as_bytes());
        let js_code = JS_NewStringCopyZ(cx, c_code.as_ptr());
        if !js_code.is_null() {
            rooted!(&in(cx_ref) let code_val = StringValue(&*js_code));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"code".as_ptr(),
                code_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolve_rr(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Generic per-RR-type resolve. Every RR type — including A/AAAA — goes
    // through c-ares synchronous resolution (resolve_rr_cares), so one
    // resolve family rides one resolver stack; the libc path
    // (resolve_hostname_libc) serves only the lookup family (NSS contract).
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
    let hostname = unsafe_jsstr_to_string(cx, NonNull::new_unchecked(hostname_val.to_string()));

    // Determine rrtype — default "A"
    let rrtype = if argc > 1 {
        let rrtype_val = *args.get(1).ptr;
        if rrtype_val.is_string() {
            unsafe_jsstr_to_string(cx, NonNull::new_unchecked(rrtype_val.to_string()))
        } else {
            "A".to_string()
        }
    } else {
        "A".to_string()
    };

    let mut cx_wrap = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    match rrtype.to_uppercase().as_str() {
        "A" => {
            // Real A query on the c-ares stack (same resolver as every other
            // RR type). Error plumbing keeps Node's resolve4 oracle shape: the
            // c-ares statuses surface as-is in "queryA <code> <host>"
            // (ENOTFOUND for NXDOMAIN, ENODATA for a NOERROR answer without
            // A records) — the exact codes Node's c-ares-based resolve4
            // reports, since Node issues the same query. The former libc arm
            // had to bridge gai EAI_* codes onto these spellings.
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_a) {
                Ok(DnsRRData::A(addrs)) => {
                    let mut idx = 0u32;
                    for ip in addrs {
                        let c_ip = ZBox::from_bytes(ip.as_bytes());
                        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                        if !js_str.is_null() {
                            rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                            JS_DefineElement(
                                cx,
                                arr_root.handle().into(),
                                idx,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                            idx += 1;
                        }
                    }
                }
                Err(code) => return throw_resolve_error(cx, "queryA", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "AAAA" => {
            // Real AAAA query on the c-ares stack — same error plumbing as
            // the A arm; the syscall is "queryAaaa" (Node's exact casing, not
            // "queryAAAA").
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_aaaa) {
                Ok(DnsRRData::Aaaa(addrs)) => {
                    let mut idx = 0u32;
                    for ip in addrs {
                        let c_ip = ZBox::from_bytes(ip.as_bytes());
                        let js_str = JS_NewStringCopyZ(cx, c_ip.as_ptr());
                        if !js_str.is_null() {
                            rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                            JS_DefineElement(
                                cx,
                                arr_root.handle().into(),
                                idx,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                            idx += 1;
                        }
                    }
                }
                Err(code) => return throw_resolve_error(cx, "queryAaaa", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        // ── c-ares RR types ──────────────────────────────────────────
        "CNAME" => {
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_cname) {
                Ok(DnsRRData::Cname(cname)) => {
                    let c_cname = ZBox::from_bytes(cname.as_bytes());
                    let js_str = JS_NewStringCopyZ(cx, c_cname.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                        JS_DefineElement(
                            cx,
                            arr_root.handle().into(),
                            0,
                            val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
                // Node: resolveCname on a missing domain errors —
                // "queryCname ENOTFOUND <host>" with err.code (not []).
                Err(code) => return throw_resolve_error(cx, "queryCname", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "MX" => {
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_mx) {
                Ok(DnsRRData::Mx(mx_list)) => {
                    let mut idx = 0u32;
                    for (priority, exchange) in mx_list {
                        let entry_obj = JS_NewPlainObject(cx);
                        if entry_obj.is_null() {
                            continue;
                        }
                        rooted!(&in(cx_wrap) let entry_root = entry_obj);
                        rooted!(&in(cx_wrap) let prio_val = Int32Value(priority as i32));
                        JS_DefineProperty(
                            cx,
                            entry_root.handle().into(),
                            c"priority".as_ptr(),
                            prio_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        let c_exchange = ZBox::from_bytes(exchange.as_bytes());
                        let js_exchange = JS_NewStringCopyZ(cx, c_exchange.as_ptr());
                        if !js_exchange.is_null() {
                            rooted!(&in(cx_wrap) let ex_val = StringValue(&*js_exchange));
                            JS_DefineProperty(
                                cx,
                                entry_root.handle().into(),
                                c"exchange".as_ptr(),
                                ex_val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        rooted!(&in(cx_wrap) let entry_jsval = ObjectValue(entry_root.get()));
                        JS_DefineElement(
                            cx,
                            arr_root.handle().into(),
                            idx,
                            entry_jsval.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        idx += 1;
                    }
                }
                Err(code) => return throw_resolve_error(cx, "queryMx", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "TXT" => {
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_txt) {
                Ok(DnsRRData::Txt(records)) => {
                    let mut idx = 0u32;
                    // Node.js dns.resolveTxt returns array of arrays of strings:
                    // each outer element = one TXT record, each inner array
                    // holds that record's character-string chunks.
                    for record in records {
                        let rec_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
                        if rec_obj.is_null() {
                            continue;
                        }
                        rooted!(&in(cx_wrap) let rec_root = rec_obj);
                        let mut chunk_idx = 0u32;
                        for chunk in record {
                            let c_txt = ZBox::from_bytes(chunk.as_bytes());
                            let js_str = JS_NewStringCopyZ(cx, c_txt.as_ptr());
                            if !js_str.is_null() {
                                rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                                JS_DefineElement(
                                    cx,
                                    rec_root.handle().into(),
                                    chunk_idx,
                                    val.handle().into(),
                                    JSPROP_ENUMERATE as u32,
                                );
                                chunk_idx += 1;
                            }
                        }
                        rooted!(&in(cx_wrap) let rec_val = ObjectValue(rec_root.get()));
                        JS_DefineElement(
                            cx,
                            arr_root.handle().into(),
                            idx,
                            rec_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        idx += 1;
                    }
                }
                Err(code) => return throw_resolve_error(cx, "queryTxt", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "NS" => {
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_ns) {
                Ok(DnsRRData::Ns(ns_list)) => {
                    let mut idx = 0u32;
                    for ns in ns_list {
                        let c_ns = ZBox::from_bytes(ns.as_bytes());
                        let js_str = JS_NewStringCopyZ(cx, c_ns.as_ptr());
                        if !js_str.is_null() {
                            rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                            JS_DefineElement(
                                cx,
                                arr_root.handle().into(),
                                idx,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                            idx += 1;
                        }
                    }
                }
                Err(code) => return throw_resolve_error(cx, "queryNs", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "SOA" => {
            let result_obj = JS_NewPlainObject(cx);
            if result_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let result_root = result_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_soa) {
                Ok(DnsRRData::Soa {
                    nsname,
                    hostmaster,
                    serial,
                    refresh,
                    retry,
                    expire,
                    minttl,
                }) => {
                    let c_nsname = ZBox::from_bytes(nsname.as_bytes());
                    let js_nsname = JS_NewStringCopyZ(cx, c_nsname.as_ptr());
                    if !js_nsname.is_null() {
                        rooted!(&in(cx_wrap) let v = StringValue(&*js_nsname));
                        JS_DefineProperty(
                            cx,
                            result_root.handle().into(),
                            c"nsname".as_ptr(),
                            v.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                    let c_hm = ZBox::from_bytes(hostmaster.as_bytes());
                    let js_hm = JS_NewStringCopyZ(cx, c_hm.as_ptr());
                    if !js_hm.is_null() {
                        rooted!(&in(cx_wrap) let v = StringValue(&*js_hm));
                        JS_DefineProperty(
                            cx,
                            result_root.handle().into(),
                            c"hostmaster".as_ptr(),
                            v.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                    // SOA numerics are u32 (RFC 1035 serial can exceed
                    // i32::MAX); DoubleValue carries the full range exactly
                    // as JS numbers — Node prints them unsigned.
                    rooted!(&in(cx_wrap) let v = DoubleValue(serial as f64));
                    JS_DefineProperty(
                        cx,
                        result_root.handle().into(),
                        c"serial".as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                    rooted!(&in(cx_wrap) let v = DoubleValue(refresh as f64));
                    JS_DefineProperty(
                        cx,
                        result_root.handle().into(),
                        c"refresh".as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                    rooted!(&in(cx_wrap) let v = DoubleValue(retry as f64));
                    JS_DefineProperty(
                        cx,
                        result_root.handle().into(),
                        c"retry".as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                    rooted!(&in(cx_wrap) let v = DoubleValue(expire as f64));
                    JS_DefineProperty(
                        cx,
                        result_root.handle().into(),
                        c"expire".as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                    rooted!(&in(cx_wrap) let v = DoubleValue(minttl as f64));
                    JS_DefineProperty(
                        cx,
                        result_root.handle().into(),
                        c"minttl".as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                Err(code) => return throw_resolve_error(cx, "querySoa", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(result_root.get()));
        }
        "SRV" => {
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_srv) {
                Ok(DnsRRData::Srv(srv_list)) => {
                    let mut idx = 0u32;
                    for (priority, weight, port, name) in srv_list {
                        let entry_obj = JS_NewPlainObject(cx);
                        if entry_obj.is_null() {
                            continue;
                        }
                        rooted!(&in(cx_wrap) let entry_root = entry_obj);
                        rooted!(&in(cx_wrap) let prio_val = Int32Value(priority as i32));
                        JS_DefineProperty(
                            cx,
                            entry_root.handle().into(),
                            c"priority".as_ptr(),
                            prio_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        rooted!(&in(cx_wrap) let wt_val = Int32Value(weight as i32));
                        JS_DefineProperty(
                            cx,
                            entry_root.handle().into(),
                            c"weight".as_ptr(),
                            wt_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        rooted!(&in(cx_wrap) let port_val = Int32Value(port as i32));
                        JS_DefineProperty(
                            cx,
                            entry_root.handle().into(),
                            c"port".as_ptr(),
                            port_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        let c_name = ZBox::from_bytes(name.as_bytes());
                        let js_name = JS_NewStringCopyZ(cx, c_name.as_ptr());
                        if !js_name.is_null() {
                            rooted!(&in(cx_wrap) let nm_val = StringValue(&*js_name));
                            JS_DefineProperty(
                                cx,
                                entry_root.handle().into(),
                                c"name".as_ptr(),
                                nm_val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        rooted!(&in(cx_wrap) let entry_jsval = ObjectValue(entry_root.get()));
                        JS_DefineElement(
                            cx,
                            arr_root.handle().into(),
                            idx,
                            entry_jsval.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        idx += 1;
                    }
                }
                Err(code) => return throw_resolve_error(cx, "querySrv", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "NAPTR" => {
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_naptr) {
                Ok(DnsRRData::Naptr(naptr_list)) => {
                    let mut idx = 0u32;
                    for naptr in naptr_list {
                        let entry_obj = JS_NewPlainObject(cx);
                        if entry_obj.is_null() {
                            continue;
                        }
                        rooted!(&in(cx_wrap) let entry_root = entry_obj);
                        let c_flags = ZBox::from_bytes(naptr.flags.as_bytes());
                        let js_flags = JS_NewStringCopyZ(cx, c_flags.as_ptr());
                        if !js_flags.is_null() {
                            rooted!(&in(cx_wrap) let v = StringValue(&*js_flags));
                            JS_DefineProperty(
                                cx,
                                entry_root.handle().into(),
                                c"flags".as_ptr(),
                                v.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        let c_svc = ZBox::from_bytes(naptr.service.as_bytes());
                        let js_svc = JS_NewStringCopyZ(cx, c_svc.as_ptr());
                        if !js_svc.is_null() {
                            rooted!(&in(cx_wrap) let v = StringValue(&*js_svc));
                            JS_DefineProperty(
                                cx,
                                entry_root.handle().into(),
                                c"service".as_ptr(),
                                v.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        let c_re = ZBox::from_bytes(naptr.regexp.as_bytes());
                        let js_re = JS_NewStringCopyZ(cx, c_re.as_ptr());
                        if !js_re.is_null() {
                            rooted!(&in(cx_wrap) let v = StringValue(&*js_re));
                            JS_DefineProperty(
                                cx,
                                entry_root.handle().into(),
                                c"regexp".as_ptr(),
                                v.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        let c_rep = ZBox::from_bytes(naptr.replacement.as_bytes());
                        let js_rep = JS_NewStringCopyZ(cx, c_rep.as_ptr());
                        if !js_rep.is_null() {
                            rooted!(&in(cx_wrap) let v = StringValue(&*js_rep));
                            JS_DefineProperty(
                                cx,
                                entry_root.handle().into(),
                                c"replacement".as_ptr(),
                                v.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        rooted!(&in(cx_wrap) let v = Int32Value(naptr.order as i32));
                        JS_DefineProperty(
                            cx,
                            entry_root.handle().into(),
                            c"order".as_ptr(),
                            v.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        rooted!(&in(cx_wrap) let v2 = Int32Value(naptr.preference as i32));
                        JS_DefineProperty(
                            cx,
                            entry_root.handle().into(),
                            c"preference".as_ptr(),
                            v2.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        rooted!(&in(cx_wrap) let entry_jsval = ObjectValue(entry_root.get()));
                        JS_DefineElement(
                            cx,
                            arr_root.handle().into(),
                            idx,
                            entry_jsval.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                        idx += 1;
                    }
                }
                Err(code) => return throw_resolve_error(cx, "queryNaptr", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        "PTR" => {
            // Real reverse resolution: ares_gethostbyaddr on the IP (v4/v6)
            // via Channel::get_host_by_addr. The old path re-queried the
            // input STRING as a CNAME, so resolvePtr echoed the IP back.
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            match resolve_rr_cares(&hostname, cares::NSType::ns_t_ptr) {
                Ok(DnsRRData::Ptr(ptr_names)) => {
                    let mut idx = 0u32;
                    for ptr_name in ptr_names {
                        let c_ptr = ZBox::from_bytes(ptr_name.as_bytes());
                        let js_str = JS_NewStringCopyZ(cx, c_ptr.as_ptr());
                        if !js_str.is_null() {
                            rooted!(&in(cx_wrap) let val = StringValue(&*js_str));
                            JS_DefineElement(
                                cx,
                                arr_root.handle().into(),
                                idx,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                            idx += 1;
                        }
                    }
                }
                // Node: resolvePtr with no PTR records errors —
                // "queryPtr ENOTFOUND <ip>" with err.code.
                Err(code) => return throw_resolve_error(cx, "queryPtr", &code, &hostname),
                Ok(_) => {}
            }
            args.rval().set(ObjectValue(arr_root.get()));
        }
        _ => {
            // Unknown RR type — return empty array.
            let arr_obj = w2::NewArrayObject1(&mut cx_wrap, 0);
            if arr_obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            rooted!(&in(cx_wrap) let arr_root = arr_obj);
            args.rval().set(ObjectValue(arr_root.get()));
        }
    }

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
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_lookup".as_ptr(),
                Some(dns_lookup),
                1,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_resolve".as_ptr(),
                Some(dns_resolve),
                2,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_resolve6".as_ptr(),
                Some(dns_resolve6),
                1,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_reverse".as_ptr(),
                Some(dns_reverse),
                1,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_lookup_service".as_ptr(),
                Some(dns_lookup_service),
                2,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_get_servers".as_ptr(),
                Some(dns_get_servers),
                0,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_set_servers".as_ptr(),
                Some(dns_set_servers),
                1,
                0,
            );
            JS_DefineFunction(
                cx_raw,
                global_root.handle().into(),
                c"__dns_resolve_rr".as_ptr(),
                Some(dns_resolve_rr),
                2,
                0,
            );
        }

        // Also keep mirrors on the module object for completeness (existing
        // callers may import the helpers off the dns module).
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_lookup".as_ptr(),
            Some(dns_lookup),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_resolve".as_ptr(),
            Some(dns_resolve),
            2,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_resolve6".as_ptr(),
            Some(dns_resolve6),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_reverse".as_ptr(),
            Some(dns_reverse),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_lookup_service".as_ptr(),
            Some(dns_lookup_service),
            2,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_get_servers".as_ptr(),
            Some(dns_get_servers),
            0,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_set_servers".as_ptr(),
            Some(dns_set_servers),
            1,
            0,
        );
        JS_DefineFunction(
            cx_raw,
            mod_obj.handle().into(),
            c"__dns_resolve_rr".as_ptr(),
            Some(dns_resolve_rr),
            2,
            0,
        );

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
