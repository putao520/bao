// @trace REQ-ENG-21 [api:Bun.DNSResolver]
// Bun.DNSResolver — DNS 查询 API(SPEC REQ-ENG-21 C1-C4,内核 bun_dns crate)。
//
// 桥接 workspace `bun_dns` crate(零修改复用):GetAddrInfo/Options/Family
// 类型化请求模型 + 进程级共享 DNS cache(`bun_dns::cache`,与 node:dns
// lookup / bun_http / servo 同一 resolver 面,node_dns.rs 同款通道)。
// 解析走 Backend::Libc 的 getaddrinfo(POSIX libc / Windows GetAddrInfoW
// 宽字符管线,node_dns.rs W2.5 同款),结果链经 `GetAddrInfoResult::
// from_addr_info` 游走。
//
// JS 面:
//   * `new Bun.DNSResolver()` — 实例(SPEC C2;构造器无契约化 options 面)
//   * `resolver.resolve(host)`   → Promise<DNSRecord[]>  (A+AAAA 全族,C3)
//   * `resolver.resolve4(host)`  → Promise<DNSRecord[]>  (仅 IPv4,C4)
//   * `resolver.resolve6(host)`  → Promise<DNSRecord[]>  (仅 IPv6,C4)
//
// DNSRecord = { address: string, family: 4|6, ttl: number }。libc
// getaddrinfo 不返回 TTL(node_dns.rs 同款边界),per-record ttl 如实记 0,
// 共享 cache 的过期由引擎侧 cap(`BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS`)
// 管理——禁编造 TTL。
//
// 解析在调用内同步完成后以已结算 Promise 交付(解析纯 CPU/系统调用面;
// node:dns 的 resolve* 家族在本仓同为调用内阻塞形态)。解析失败走
// Promise reject 真实错误(平台 rc 原文),不静默返空。
#[cfg(not(windows))]
use libc::getaddrinfo as sys_getaddrinfo;

use bun_core::ZBox;
use bun_dns::{Backend, Family, GetAddrInfo, GetAddrInfoResult, Options, Protocol, SocketType};

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

use crate::bun_api::js_string_from_utf8;

// `mozjs::jsapi::*` glob 导入了零泛参的 `mozilla::Result` 别名,与本文件
// 的 `Result` 语义冲突——统一显式路径(node_dns.rs 同款纪律)。
use ::std::result::Result;

/// JSFUN_CONSTRUCTOR (0x400): constructible so `new Bun.DNSResolver()`
/// dispatches here (node_buffer.rs / bun_glob_api.rs pattern — SM's
/// [[Construct]] for natives honours the rval we set).
const JSFUN_CONSTRUCTOR: u32 = 0x400;

// ── 平台解析调用(POSIX getaddrinfo / Windows GetAddrInfoW 宽管线) ──────

/// `bun_dns` cfg 分发的 addrinfo(POSIX libc / winsock A 布局)——本文件
/// 双平台共用的链节点类型。
use bun_dns::addrinfo as platform_addrinfo;

/// Windows resolution backend: `GetAddrInfoW`/`FreeAddrInfoW` (ws2_32) —
/// 宽字符管线(bao_uloop::addrinfo win_dns 同款:ANSI 导出经 ANSI codepage
/// 解码非 ASCII 主机名,宽管线保 UTF-8 语义)。`ADDRINFOW` 与 `bun_dns`
/// cfg 分发的 `addrinfo` 布局同构(仅 canonname 指针型差异,本面不读)。
#[cfg(windows)]
mod sys_getaddrinfo_win {
    /// `ADDRINFOW` (`ws2tcpip.h`): `GetAddrInfoW`'s node type.
    pub type ADDRINFOW = super::platform_addrinfo;

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        /// 0 on success; otherwise the WSA error code. Node/service are
        /// NUL-terminated UTF-16; service NULL mirrors the POSIX arm.
        pub unsafe fn GetAddrInfoW(
            pwNodeName: *const u16,
            pwServiceName: *const u16,
            pHints: *const ADDRINFOW,
            ppResult: *mut *mut ADDRINFOW,
        ) -> core::ffi::c_int;
        /// Frees a chain returned by `GetAddrInfoW`.
        pub unsafe fn FreeAddrInfoW(pAddrInfo: *mut ADDRINFOW);
    }
}

/// `bun_dns` cfg 分发的 addrinfo(POSIX libc / winsock A 布局)——本文件
/// 双平台共用的链节点类型(见上方 use)。
///
/// 平台解析调用统一形:POSIX `getaddrinfo` / Windows `GetAddrInfoW`。
///
/// SAFETY: `host_arg` 是平台原生 NUL 终止名(`*const c_char` /
/// `*const u16`);hints/result 是合法指针。
#[cfg(not(windows))]
unsafe fn platform_resolve(
    host_arg: *const core::ffi::c_char,
    hints: *const platform_addrinfo,
    result: &mut *mut platform_addrinfo,
) -> core::ffi::c_int {
    // SAFETY: host_arg NUL 终止,hints/result 合法。
    unsafe { sys_getaddrinfo(host_arg, ::std::ptr::null(), hints, result) }
}

#[cfg(windows)]
unsafe fn platform_resolve(
    host_arg: *const u16,
    hints: *const platform_addrinfo,
    result: &mut *mut platform_addrinfo,
) -> core::ffi::c_int {
    // SAFETY: host_arg 是 NUL 终止 UTF-16;hints/result 合法。
    unsafe { sys_getaddrinfo_win::GetAddrInfoW(host_arg, ::std::ptr::null(), hints, result) }
}

/// `freeaddrinfo`/`FreeAddrInfoW` — 释放平台解析调用返回的链
/// (`bun_dns` 的 cfg 分发 re-export 覆盖双平台)。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn free_chain(result: *mut platform_addrinfo) {
    // SAFETY: result 是 getaddrinfo/GetAddrInfoW 分配的存活链。
    unsafe { bun_dns::freeaddrinfo(result) };
}

// ── 解析核心 ─────────────────────────────────────────────────────────────

/// 族过滤解析请求。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FamilyFilter {
    Any,
    V4,
    V6,
}

impl FamilyFilter {
    fn to_bun(self) -> Family {
        match self {
            FamilyFilter::Any => Family::Unspecified,
            FamilyFilter::V4 => Family::Inet,
            FamilyFilter::V6 => Family::Inet6,
        }
    }

    /// 该过滤的 family 标记(Any 由逐条结果判族)。
    fn marker(self) -> Option<i32> {
        match self {
            FamilyFilter::V4 => Some(4),
            FamilyFilter::V6 => Some(6),
            FamilyFilter::Any => None,
        }
    }
}

/// 单条解析结果(libc getaddrinfo 无 TTL,恒 0,见文件头注释)。
struct DnsRecord {
    address: ::std::string::String,
    family: i32,
    ttl: i32,
}

/// `bun_dns::cache::IpAddr` → 渲染文本 + family 标记(node_dns.rs
/// render_cache_ip/cache_ip_family 同款;cache 命中路径零系统调用)。
fn render_cache_ip(ip: &bun_dns::cache::IpAddr) -> (::std::string::String, i32) {
    match ip {
        bun_dns::cache::IpAddr::V4(o) => (format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]), 4),
        bun_dns::cache::IpAddr::V6(bytes) => {
            let segs: [u16; 8] =
                core::array::from_fn(|i| u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]));
            (::std::net::Ipv6Addr::from(segs).to_string(), 6)
        }
    }
}

/// `bun_dns::Address` → 渲染文本 + family 标记(node_dns.rs render_address
/// 同款镜像,不引入 BunString 进 JS 桥)。
fn render_address(addr: &bun_dns::Address) -> Option<(::std::string::String, i32)> {
    if let Some(v4) = addr.as_in4() {
        // SAFETY: sin_addr is 4 POD bytes on every target.
        let octets: [u8; 4] = unsafe { *::std::ptr::addr_of!(v4.sin_addr).cast::<[u8; 4]>() };
        return Some((format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]), 4));
    }
    if let Some(v6) = addr.as_in6() {
        // SAFETY: sin6_addr is 16 POD bytes (in6_addr).
        let bytes: [u8; 16] = unsafe { *::std::ptr::addr_of!(v6.sin6_addr).cast::<[u8; 16]>() };
        let segs: [u16; 8] =
            core::array::from_fn(|i| u16::from_be_bytes([bytes[i * 2], bytes[i * 2 + 1]]));
        return Some((::std::net::Ipv6Addr::from(segs).to_string(), 6));
    }
    None
}

/// 域过滤解析核心:Any 族共享 cache 优先(读命中零系统调用;写仅 Any 族
/// 回填,族过滤查询不回写——禁以过滤后的部分集合覆写共享 cache 全集),
/// 未命中走 node_dns.rs `resolve_hostname_libc` 同款 getaddrinfo 通道
/// (libc 后端 + SOCK_STREAM 默认,Bun resolver 对齐)。
/// Err = 真实解析失败(平台 rc 原文)。
fn resolve_host(host: &str, filter: FamilyFilter) -> Result<Vec<DnsRecord>, ::std::string::String> {
    let marker = filter.marker();

    // 进程级共享 cache 询问(与 node:dns / bun_http / servo 融合点);
    // 族过滤在命中路径上按条目族判(Any 族 cache 条目可能混合)。
    if let Some(addrs) = bun_dns::cache::lookup(host.as_bytes()) {
        let records: Vec<DnsRecord> = addrs
            .iter()
            .map(render_cache_ip)
            .filter(|(_, fam)| marker.is_none_or(|m| *fam == m))
            .map(|(address, family)| DnsRecord { address, family, ttl: 0 })
            .collect();
        if !records.is_empty() {
            return Ok(records);
        }
    }

    let req = GetAddrInfo {
        name: host.as_bytes().to_vec().into_boxed_slice(),
        port: 0,
        options: Options {
            family: filter.to_bun(),
            socktype: SocketType::Stream,
            protocol: Protocol::Unspecified,
            backend: Backend::Libc,
            flags: 0,
        },
    };

    // getaddrinfo 要 NUL 终止的主机名;内嵌 NUL 永不可解析,以失败交付
    // (node_dns.rs 同款边界)。
    #[cfg(not(windows))]
    let host_c = match ::std::ffi::CString::new(host) {
        Ok(c) => c,
        Err(_) => return Err(resolve_error(&host, -1)),
    };
    #[cfg(not(windows))]
    let host_arg = host_c.as_ptr();
    #[cfg(windows)]
    let host_arg: Vec<u16> = {
        if host.as_bytes().contains(&0) {
            return Err(resolve_error(&host, -1));
        }
        let mut w: Vec<u16> =
            String::from_utf8_lossy(host.as_bytes()).encode_utf16().collect();
        w.push(0);
        w
    };
    // GetAddrInfoW 取 `*const u16`;shadow 成裸指针(Vec 被 shadowed 绑定持有,
    // 存活至作用域末,调用期指针有效;与 POSIX 臂 `host_c.as_ptr()` 同形)。
    #[cfg(windows)]
    let host_arg = host_arg.as_ptr();

    let hints = req.options.to_libc();
    let mut result_head: *mut platform_addrinfo = ::std::ptr::null_mut();
    let rc = unsafe {
        platform_resolve(
            host_arg,
            hints
                .as_ref()
                .map(|h| h as *const platform_addrinfo)
                .unwrap_or(::std::ptr::null()),
            &mut result_head,
        )
    };
    if rc != 0 {
        return Err(resolve_error(&host, rc));
    }
    if result_head.is_null() {
        return Ok(Vec::new());
    }

    let mut records: Vec<DnsRecord> = Vec::new();
    let mut cache_ips: Vec<bun_dns::cache::IpAddr> = Vec::new();
    let mut cur = result_head;
    while !cur.is_null() {
        // SAFETY: cur 非空,指向 getaddrinfo 结果链节点。
        let ai = unsafe { &*cur };
        if let Some(res) = GetAddrInfoResult::from_addr_info(ai) {
            if let Some((address, family)) = render_address(&res.address) {
                if marker.is_none_or(|m| family == m) {
                    records.push(DnsRecord { address, family, ttl: 0 });
                }
            }
            // cache 抽取(node_dns.rs address_to_cache_ip 同形态,v4/v6 字节);
            // 仅 Any 族查询回填共享 cache(见函数头注释)。
            if filter == FamilyFilter::Any {
                if let Some(v4) = res.address.as_in4() {
                    // SAFETY: sin_addr is 4 POD bytes.
                    let octets: [u8; 4] =
                        unsafe { *::std::ptr::addr_of!(v4.sin_addr).cast::<[u8; 4]>() };
                    cache_ips.push(bun_dns::cache::IpAddr::V4(octets));
                } else if let Some(v6) = res.address.as_in6() {
                    // SAFETY: sin6_addr is 16 POD bytes.
                    let bytes: [u8; 16] =
                        unsafe { *::std::ptr::addr_of!(v6.sin6_addr).cast::<[u8; 16]>() };
                    cache_ips.push(bun_dns::cache::IpAddr::V6(bytes));
                }
            }
        }
        cur = ai.ai_next;
    }
    // SAFETY: result_head 由平台解析调用分配,链在上游游走中保持完整。
    unsafe { free_chain(result_head) };
    if filter == FamilyFilter::Any {
        // getaddrinfo 无 TTL(见 bun_dns GetAddrInfoResult::ttl)— cache
        // 施加引擎侧 cap(node_dns.rs 同款)。
        bun_dns::cache::insert(host.as_bytes(), cache_ips, None);
    }
    Ok(records)
}

/// 平台解析失败文本(真实 rc 原文;Win = WSA 错误码,POSIX = EAI_* 数值)。
fn resolve_error(host: &str, rc: core::ffi::c_int) -> ::std::string::String {
    format!("getaddrinfo failed for \"{host}\" (rc={rc})")
}

// ── JS 面 ────────────────────────────────────────────────────────────────

/// resolve 家族失败分类:参数面(invalid arg)vs 解析面(resolver)。
enum ResolveFailure {
    InvalidArg(::std::string::String),
    Resolver(::std::string::String),
}

impl ResolveFailure {
    fn message(&self) -> &str {
        match self {
            ResolveFailure::InvalidArg(m) | ResolveFailure::Resolver(m) => m,
        }
    }

    /// node dns 错误码形态(参数面 = ERR_INVALID_ARG_TYPE,解析面 = ENOTFOUND)。
    fn code(&self) -> &'static str {
        match self {
            ResolveFailure::InvalidArg(_) => "ERR_INVALID_ARG_TYPE",
            ResolveFailure::Resolver(_) => "ENOTFOUND",
        }
    }
}

/// 构造记录对象 `{ address, family, ttl }` 并挂到数组第 `i` 位。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_record(cx: *mut JSContext, arr: *mut JSObject, i: u32, rec: &DnsRecord) {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let obj = JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        return;
    }
    let js_addr = js_string_from_utf8(cx, &rec.address);
    if !js_addr.is_null() {
        rooted!(&in(cx_ref) let av = StringValue(&*js_addr));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"address".as_ptr(),
            av.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    rooted!(&in(cx_ref) let fv = Int32Value(rec.family));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"family".as_ptr(),
        fv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let tv = Int32Value(rec.ttl));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c"ttl".as_ptr(),
        tv.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let ev = ObjectValue(obj.get()));
    rooted!(&in(cx_ref) let arr_root = arr);
    let _ = JS_DefineElement(
        cx,
        arr_root.handle().into(),
        i,
        ev.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

/// resolve 家族共享体:校验主机参数 → 解析 → 记录数组。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dns_resolve_body(
    cx: *mut JSContext,
    argc: u32,
    args: &CallArgs,
    filter: FamilyFilter,
) -> Result<*mut JSObject, ResolveFailure> {
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        return Err(ResolveFailure::InvalidArg(
            "Bun.DNSResolver.resolve expects a hostname string".to_string(),
        ));
    }
    let host = crate::js_to_rust_string(cx, *args.get(0).ptr);
    if host.is_empty() {
        return Err(ResolveFailure::InvalidArg(
            "Bun.DNSResolver.resolve expects a non-empty hostname".to_string(),
        ));
    }
    let records = resolve_host(&host, filter).map_err(ResolveFailure::Resolver)?;

    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, records.len()));
    if arr.get().is_null() {
        return Err(ResolveFailure::Resolver(
            "Bun.DNSResolver: failed to allocate result array".to_string(),
        ));
    }
    for (i, rec) in records.iter().enumerate() {
        define_record(cx, arr.get(), i as u32, rec);
    }
    Ok(arr.get())
}

/// 真实 Error 载体:JS_ReportErrorUTF8 制造 pending exception 后捕获
/// (harvest-a-pending-exception 形态),附 code 字段。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn make_error_value(cx: *mut JSContext, code: &str, msg: &str) -> JSVal {
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    let mut exn = UndefinedValue();
    let got = JS_GetPendingException(
        cx,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut exn,
        },
    );
    JS_ClearPendingException(cx);
    if !got || !exn.is_object() {
        return exn;
    }
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let exn_obj = exn.to_object());
    let c_code = ZBox::from_bytes(code.as_bytes());
    let code_js = JS_NewStringCopyZ(cx, c_code.as_ptr());
    if !code_js.is_null() {
        rooted!(&in(cx_ref) let code_val = StringValue(&*code_js));
        JS_DefineProperty(
            cx,
            exn_obj.handle().into(),
            c"code".as_ptr(),
            code_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    ObjectValue(exn_obj.get())
}

/// resolve 家族 Promise 交付形态:同步解析 + 同步 settle(成功 Resolve /
/// 失败 Reject,真实错误)。Promise 对象恒交付(异步面契约)。
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dns_resolve_promise(
    cx: *mut JSContext,
    argc: u32,
    args: &CallArgs,
    filter: FamilyFilter,
) -> bool {
    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let promise = JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        JS_ClearPendingException(cx);
        args.rval().set(UndefinedValue());
        return true;
    }

    match dns_resolve_body(cx, argc, args, filter) {
        Ok(arr) => {
            rooted!(&in(cx_ref) let rv = ObjectValue(arr));
            let _ = JS::ResolvePromise(cx, promise.handle().into(), rv.handle().into());
        }
        Err(fail) => {
            let err_val = make_error_value(cx, fail.code(), fail.message());
            rooted!(&in(cx_ref) let err_root = err_val);
            let _ = JS::RejectPromise(cx, promise.handle().into(), err_root.handle().into());
        }
    }
    args.rval().set(ObjectValue(promise.get()));
    true
}

macro_rules! dns_resolve_native {
    ($name:ident, $filter:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            dns_resolve_promise(cx, argc, &args, $filter)
        }
    };
}

dns_resolve_native!(dns_resolve, FamilyFilter::Any);
dns_resolve_native!(dns_resolve4, FamilyFilter::V4);
dns_resolve_native!(dns_resolve6, FamilyFilter::V6);

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dns_resolver_ctor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped;
    rooted!(&in(cx_ref) let g = JS_NewPlainObject(cx_ref));
    if g.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"resolve".as_ptr(),
        Some(dns_resolve),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"resolve4".as_ptr(),
        Some(dns_resolve4),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx_ref,
        g.handle(),
        c"resolve6".as_ptr(),
        Some(dns_resolve6),
        1,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(ObjectValue(g.get()));
    true
}

/// Install `Bun.DNSResolver` (constructor) on the Bun object.
///
/// # Safety
/// Caller must ensure `cx` is a valid JSContext and `bun_obj` a live object.
pub unsafe fn install(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    let ctor = JS_NewFunction(
        cx.raw_cx(),
        Some(dns_resolver_ctor),
        0,
        JSFUN_CONSTRUCTOR,
        c"DNSResolver".as_ptr(),
    );
    if ctor.is_null() {
        return;
    }
    let ctor_obj = JS_GetFunctionObject(ctor);
    if ctor_obj.is_null() {
        return;
    }
    rooted!(&in(cx) let ctor_root = ctor_obj);
    JS_DefineProperty3(
        cx,
        bun_obj,
        c"DNSResolver".as_ptr(),
        ctor_root.handle(),
        JSPROP_ENUMERATE as u32,
    );
}
