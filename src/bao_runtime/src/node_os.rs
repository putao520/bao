// @trace REQ-ENG-007
use ::std::ptr::NonNull;
use bun_core::ZBox;

use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

// Windows arm (W2.5, node/bun windows parity): hostname = GetComputerNameExW
// (DNS hostname), release/version = RtlGetVersion major.minor.build,
// uptime/mem = GetTickCount64 + GlobalMemoryStatusEx, cpus model = cpuid
// brand string, networkInterfaces = GetAdaptersAddresses (MAC/loopback/
// prefix from the SDK record fields), userInfo uid/gid = -1 with shell
// empty (no POSIX ids / shell on windows), loadavg = [0,0,0], getPriority =
// GetPriorityClass mapped onto the nice scale. posix faces are untouched.
pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let os_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if os_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"hostname".as_ptr(),
            Some(os_hostname),
            0,
            0,
        );
        w2::JS_DefineFunction(cx, os_obj.handle(), c"type".as_ptr(), Some(os_type), 0, 0);
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"platform".as_ptr(),
            Some(os_platform),
            0,
            0,
        );
        w2::JS_DefineFunction(cx, os_obj.handle(), c"arch".as_ptr(), Some(os_arch), 0, 0);
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"release".as_ptr(),
            Some(os_release),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"uptime".as_ptr(),
            Some(os_uptime),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"totalmem".as_ptr(),
            Some(os_totalmem),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"freemem".as_ptr(),
            Some(os_freemem),
            0,
            0,
        );
        w2::JS_DefineFunction(cx, os_obj.handle(), c"cpus".as_ptr(), Some(os_cpus), 0, 0);
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"networkInterfaces".as_ptr(),
            Some(os_network_interfaces),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"homedir".as_ptr(),
            Some(os_homedir),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"tmpdir".as_ptr(),
            Some(os_tmpdir),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"userInfo".as_ptr(),
            Some(os_user_info),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"loadavg".as_ptr(),
            Some(os_loadavg),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"endianness".as_ptr(),
            Some(os_endianness),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"devNull".as_ptr(),
            Some(os_dev_null),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"getPriority".as_ptr(),
            Some(os_get_priority),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"availableParallelism".as_ptr(),
            Some(os_available_parallelism),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"machine".as_ptr(),
            Some(os_machine),
            0,
            0,
        );
        w2::JS_DefineFunction(
            cx,
            os_obj.handle(),
            c"version".as_ptr(),
            Some(os_version),
            0,
            0,
        );

        let eol = if cfg!(windows) { "\r\n" } else { "\n" };
        let eol_str = JS_NewStringCopyN(
            cx.raw_cx(),
            eol.as_ptr() as *const ::std::os::raw::c_char,
            eol.len(),
        );
        if !eol_str.is_null() {
            let val = StringValue(&*eol_str);
            rooted!(&in(cx) let v = val);
            JS_DefineProperty(
                cx.raw_cx(),
                os_obj.handle().into(),
                c"EOL".as_ptr(),
                v.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
            );
        }

        let dev_null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let dn_str = JS_NewStringCopyN(
            cx.raw_cx(),
            dev_null.as_ptr() as *const ::std::os::raw::c_char,
            dev_null.len(),
        );
        if !dn_str.is_null() {
            let val = StringValue(&*dn_str);
            rooted!(&in(cx) let v = val);
            JS_DefineProperty(
                cx.raw_cx(),
                os_obj.handle().into(),
                c"devNull".as_ptr(),
                v.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
            );
        }
    }

    // os.constants
    unsafe {
        rooted!(&in(cx) let constants_obj = w2::JS_NewPlainObject(cx));
        if !constants_obj.get().is_null() {
            rooted!(&in(cx) let sig_obj = w2::JS_NewPlainObject(cx));
            if !sig_obj.get().is_null() {
                let raw = cx.raw_cx();
                let signals = [
                    ("SIGHUP", 1),
                    ("SIGINT", 2),
                    ("SIGQUIT", 3),
                    ("SIGILL", 4),
                    ("SIGTRAP", 5),
                    ("SIGABRT", 6),
                    ("SIGBUS", 7),
                    ("SIGFPE", 8),
                    ("SIGKILL", 9),
                    ("SIGUSR1", 10),
                    ("SIGSEGV", 11),
                    ("SIGUSR2", 12),
                    ("SIGPIPE", 13),
                    ("SIGALRM", 14),
                    ("SIGTERM", 15),
                ];
                for (name, val) in &signals {
                    let v = Int32Value(*val);
                    rooted!(&in(cx) let rv = v);
                    JS_DefineProperty(
                        raw,
                        sig_obj.handle().into(),
                        ZBox::from_bytes(name.as_bytes()).as_ptr(),
                        rv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                w2::JS_DefineProperty3(
                    cx,
                    constants_obj.handle(),
                    c"signals".as_ptr(),
                    sig_obj.handle(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            w2::JS_DefineProperty3(
                cx,
                os_obj.handle(),
                c"constants".as_ptr(),
                constants_obj.handle(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    cache_builtin(cx, "os", os_obj.get());
}

unsafe fn return_string(cx: *mut JSContext, s: &str, args: &CallArgs) {
    unsafe {
        let utf16: Vec<u16> = s.encode_utf16().collect();
        let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
        args.rval().set(if js_str.is_null() {
            UndefinedValue()
        } else {
            StringValue(&*js_str)
        });
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_hostname(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let hostname = libc_binding::get_hostname();
    return_string(cx, &hostname, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_type(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let os_type = if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else if cfg!(target_os = "windows") {
        "Windows_NT"
    } else {
        "Unknown"
    };
    return_string(cx, os_type, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_platform(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let platform = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "unknown"
    };
    return_string(cx, platform, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_arch(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let arch = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    };
    return_string(cx, arch, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_release(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let release = libc_binding::get_os_release();
    return_string(cx, &release, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_uptime(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let info = libc_binding::get_sysinfo();
    args.rval().set(Int32Value(info.uptime as i32));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_totalmem(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let info = libc_binding::get_sysinfo();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let val = mozjs::jsval::DoubleValue(info.totalram as f64);
    rooted!(&in(wrapped_cx) let v = val);
    args.rval().set(v.get());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_freemem(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let info = libc_binding::get_sysinfo();
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let val = mozjs::jsval::DoubleValue(info.freeram as f64);
    rooted!(&in(wrapped_cx) let v = val);
    args.rval().set(v.get());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_cpus(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let nproc = match ::std::thread::available_parallelism() {
        Ok(n) => n.get(),
        Err(_) => 1,
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, nproc));
    let model = libc_binding::get_cpu_model();
    for i in 0..nproc {
        rooted!(&in(cx_ref) let cpu = mozjs_sys::jsapi::JS_NewPlainObject(cx));
        if !cpu.get().is_null() {
            let model_str = JS_NewStringCopyN(
                cx,
                model.as_ptr() as *const ::std::os::raw::c_char,
                model.len(),
            );
            if !model_str.is_null() {
                let val = StringValue(&*model_str);
                rooted!(&in(cx_ref) let mv = val);
                JS_DefineProperty(
                    cx,
                    cpu.handle().into(),
                    c"model".as_ptr(),
                    mv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            rooted!(&in(cx_ref) let sv = Int32Value(i as i32));
            JS_DefineProperty(
                cx,
                cpu.handle().into(),
                c"speed".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );

            rooted!(&in(cx_ref) let times = mozjs_sys::jsapi::JS_NewPlainObject(cx));
            if !times.get().is_null() {
                for &(name, val) in &[
                    ("user", 0i32),
                    ("nice", 0),
                    ("sys", 0),
                    ("idle", 0),
                    ("irq", 0),
                ] {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    rooted!(&in(cx_ref) let tv = Int32Value(val));
                    JS_DefineProperty(
                        cx,
                        times.handle().into(),
                        c_name.as_ptr(),
                        tv.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            let times_val = ObjectValue(times.get());
            rooted!(&in(cx_ref) let tv = times_val);
            JS_DefineProperty(
                cx,
                cpu.handle().into(),
                c"times".as_ptr(),
                tv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let cpu_val = ObjectValue(cpu.get());
        rooted!(&in(cx_ref) let cv = cpu_val);
        JS_DefineElement(
            cx,
            arr.handle().into(),
            i as u32,
            cv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

// Windows type aliases for the shared sockaddr-formatting helpers (libc's
// windows module lacks both).
#[cfg(not(windows))]
use libc::{in6_addr, in_addr_t};
#[cfg(windows)]
use bun_windows_sys::ws2_32::in6_addr;
#[cfg(windows)]
type in_addr_t = u32;

// BCE-20260816-OS-NETIF — os_networkInterfaces previously returned a bare
// empty object (silently fake). Real enumeration via libc getifaddrs(3),
// grouped per interface name with the Node shape:
//   { "lo": [{ address, netmask, family: "IPv4"|"IPv6", mac, internal,
//              cidr, scopeid? }] }
// mac comes from the AF_PACKET entry (link-layer address), internal is the
// IFF_LOOPBACK flag, cidr is address/prefixlen (netmask popcount).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_network_interfaces(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

// ────────────────── os.networkInterfaces (node shape) ──────────────────

struct IfaceEntry {
    address: String,
    netmask: String,
    family: &'static str,
    prefix_len: u8,
    scopeid: Option<u32>,
}


/// Node-shaped interface enumeration on POSIX via getifaddrs(3). Grouped per
/// interface name with the MAC from the AF_PACKET entry and IFF_LOOPBACK for
/// `internal` (see the os_networkInterfaces BCE note above).
#[cfg(not(windows))]
fn collect_posix_ifaddrs(
    ifaces: &mut Vec<(String, String, bool, Vec<IfaceEntry>)>,
) -> bool {
    let mut ifap: *mut libc::ifaddrs = ::std::ptr::null_mut();
    let ok = unsafe { libc::getifaddrs(&mut ifap) } == 0;
    if !ok || ifap.is_null() {
        return false;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        let name = unsafe {
            if ifa.ifa_name.is_null() {
                continue;
            }
            ::std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned()
        };
        let sa = ifa.ifa_addr as *const libc::sockaddr;
        if sa.is_null() {
            continue;
        }
        let family = unsafe { (*sa).sa_family as i32 };
        let flags = ifa.ifa_flags;
        let slot = match ifaces.iter_mut().find(|(n, _, _, _)| *n == name) {
            Some(slt) => slt,
            None => {
                ifaces.push((name.clone(), "00:00:00:00:00:00".to_string(), false, Vec::new()));
                ifaces.last_mut().unwrap()
            }
        };
        if flags & libc::IFF_LOOPBACK as u32 != 0 {
            slot.2 = true;
        }
        match family {
            libc::AF_PACKET => {
                let sll = sa as *const libc::sockaddr_ll;
                let mac = unsafe {
                    format!(
                        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        (*sll).sll_addr[0], (*sll).sll_addr[1], (*sll).sll_addr[2],
                        (*sll).sll_addr[3], (*sll).sll_addr[4], (*sll).sll_addr[5]
                    )
                };
                slot.1 = mac;
            }
            libc::AF_INET => {
                let sin = sa as *const libc::sockaddr_in;
                let addr = unsafe { ipv4_to_string((*sin).sin_addr.s_addr) };
                let (mask, prefix) = if ifa.ifa_netmask.is_null() {
                    ("0.0.0.0".to_string(), 0u8)
                } else {
                    let snm = ifa.ifa_netmask as *const libc::sockaddr_in;
                    unsafe {
                        let m = ipv4_to_string((*snm).sin_addr.s_addr);
                        let p = (*snm).sin_addr.s_addr.count_ones() as u8;
                        (m, p)
                    }
                };
                slot.3.push(IfaceEntry {
                    address: addr,
                    netmask: mask,
                    family: "IPv4",
                    prefix_len: prefix,
                    scopeid: None,
                });
            }
            libc::AF_INET6 => {
                let sin6 = sa as *const libc::sockaddr_in6;
                let addr = unsafe { ipv6_to_string(&(*sin6).sin6_addr) };
                let (mask, prefix) = if ifa.ifa_netmask.is_null() {
                    ("::".to_string(), 0u8)
                } else {
                    let snm = ifa.ifa_netmask as *const libc::sockaddr_in6;
                    unsafe {
                        let mut pl = 0u8;
                        for b in (*snm).sin6_addr.s6_addr.iter() {
                            pl += b.count_ones() as u8;
                        }
                        (ipv6_to_string(&(*snm).sin6_addr), pl)
                    }
                };
                let scopeid = unsafe { (*sin6).sin6_scope_id };
                slot.3.push(IfaceEntry {
                    address: addr,
                    netmask: mask,
                    family: "IPv6",
                    prefix_len: prefix,
                    scopeid: if scopeid > 0 { Some(scopeid) } else { None },
                });
            }
            _ => {}
        }
    }
    unsafe { libc::freeifaddrs(ifap) };
    true
}

// ── Windows: GetAdaptersAddresses (iphlpapi) enumeration ─────────────────
//
// Mirror declares only the walked prefix of each SDK record (x64 layout,
// offsets per the SDK headers in the cross sysroot — IP_ADAPTER_ADDRESSES_LH
// / IP_ADAPTER_UNICAST_ADDRESS_LH). OnLinkPrefixLength is a plain UINT8 at
// offset 56 of the unicast record (union 0..8, Next 8..16, Address 16..32,
// PrefixOrigin/SuffixOrigin/DadState 32..44, lifetimes 44..56).
#[cfg(windows)]
mod netif {
    use core::ffi::c_void;
    use bun_windows_sys::ws2_32::sockaddr;

    pub const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24; // ipifcons.h
    pub const ERROR_BUFFER_OVERFLOW: i32 = 122; // winerror.h
    pub const GAA_FLAGS: u32 = 0x0002 | 0x0004 | 0x0008 | 0x0010; // skip anycast/multicast/dns + include prefix

    #[repr(C)]
    pub struct SocketAddress {
        pub lp_sockaddr: *mut sockaddr,
        pub i_sockaddr_length: i32,
    }

    /// IP_ADAPTER_UNICAST_ADDRESS_LH prefix + the tail byte we read.
    /// Size = 64 on x64 (OnLinkPrefixLength at 56, padded tail).
    #[repr(C)]
    pub struct IpAdapterUnicastAddress {
        pub length_and_flags: u64, // union { ULONGLONG Alignment; {Length,Flags} }
        pub next: *mut IpAdapterUnicastAddress,
        pub address: SocketAddress,
        pub prefix_origin: i32,
        pub suffix_origin: i32,
        pub dad_state: i32,
        pub valid_lifetime: u32,
        pub preferred_lifetime: u32,
        pub lease_lifetime: u32,
        pub on_link_prefix_length: u8,
        __pad: [u8; 7],
    }

    /// IP_ADAPTER_ADDRESSES_LH prefix (x64) — walked fields only; the record
    /// continues past `if_type` in the SDK but is never read here.
    #[repr(C)]
    pub struct IpAdapterAddresses {
        pub length_and_if_index: u64, // union { Alignment; {Length,IfIndex} }
        pub next: *mut IpAdapterAddresses,
        pub adapter_name: *mut u8,
        pub first_unicast_address: *mut IpAdapterUnicastAddress,
        _anycast: *mut c_void,
        _multicast: *mut c_void,
        _dns_server: *mut c_void,
        _dns_suffix: *mut u16,
        _description: *mut u16,
        pub friendly_name: *mut u16,
        pub physical_address: [u8; 8],
        pub physical_address_length: u32,
        _flags: u32,
        _mtu: u32,
        pub if_type: u32,
    }

    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        pub unsafe fn GetAdaptersAddresses(
            family: u32,
            flags: u32,
            reserved: *mut c_void,
            adapter_addresses: *mut IpAdapterAddresses,
            size_pointer: *mut u32,
        ) -> i32;
    }
}

/// Windows counterpart of `collect_posix_ifaddrs` (node parity): friendly
/// name, MAC, loopback flag and unicast address/netmask/cidr per adapter.
#[cfg(windows)]
fn collect_adapters(ifaces: &mut Vec<(String, String, bool, Vec<IfaceEntry>)>) -> bool {
    use netif::*;

    const AF_UNSPEC: u32 = 0;
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    unsafe {
        let mut size = 0u32;
        // SAFETY: null record + size out-pointer = the documented size probe.
        let rc = GetAdaptersAddresses(
            AF_UNSPEC,
            GAA_FLAGS,
            ::std::ptr::null_mut(),
            ::std::ptr::null_mut(),
            &mut size,
        );
        if !(rc == ERROR_BUFFER_OVERFLOW && size > 0) {
            return false;
        }
        let mut buf = vec![0u8; size as usize];
        let head = buf.as_mut_ptr() as *mut IpAdapterAddresses;
        // SAFETY: buffer sized per the probe; records initialized by the call.
        let rc = GetAdaptersAddresses(
            AF_UNSPEC,
            GAA_FLAGS,
            ::std::ptr::null_mut(),
            head,
            &mut size,
        );
        if rc != 0 {
            return false;
        }
        let mut cur = head;
        while !cur.is_null() {
            let a = &*cur;
            let name = if a.friendly_name.is_null() {
                String::new()
            } else {
                let len = (0..).take_while(|&i| *a.friendly_name.add(i) != 0).count();
                String::from_utf16_lossy(::std::slice::from_raw_parts(a.friendly_name, len))
            };
            let mac = if a.physical_address_length >= 6 {
                format!(
                    "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    a.physical_address[0],
                    a.physical_address[1],
                    a.physical_address[2],
                    a.physical_address[3],
                    a.physical_address[4],
                    a.physical_address[5]
                )
            } else {
                "00:00:00:00:00:00".to_string()
            };
            let internal = a.if_type == IF_TYPE_SOFTWARE_LOOPBACK;
            let slot = match ifaces.iter_mut().find(|(n, _, _, _)| *n == name) {
                Some(slt) => slt,
                None => {
                    ifaces.push((name, mac, false, Vec::new()));
                    ifaces.last_mut().unwrap()
                }
            };
            if internal {
                slot.2 = true;
            }
            let mut uni = a.first_unicast_address;
            while !uni.is_null() {
                let u = &*uni;
                let sa = u.address.lp_sockaddr;
                if !sa.is_null() {
                    let family = (*sa).sa_family as u32;
                    match family {
                        2 => {
                            // AF_INET — sockaddr_in {family u16, port u16, addr u32}
                            let sin = sa.cast::<bun_windows_sys::ws2_32::sockaddr_in>();
                            let octets = (*sin).sin_addr.s_addr.to_be();
                            slot.3.push(IfaceEntry {
                                address: format!(
                                    "{}.{}.{}.{}",
                                    (octets >> 24) & 0xff,
                                    (octets >> 16) & 0xff,
                                    (octets >> 8) & 0xff,
                                    octets & 0xff
                                ),
                                netmask: prefix_to_ipv4_mask(u.on_link_prefix_length),
                                family: "IPv4",
                                prefix_len: u.on_link_prefix_length,
                                scopeid: None,
                            });
                        }
                        23 => {
                            // AF_INET6 — sockaddr_in6 {family,port,flowinfo,addr,scopeid}
                            let sin6 = sa.cast::<bun_windows_sys::ws2_32::sockaddr_in6>();
                            slot.3.push(IfaceEntry {
                                address: ipv6_to_string(&(*sin6).sin6_addr),
                                netmask: prefix_to_ipv6_mask(u.on_link_prefix_length),
                                family: "IPv6",
                                prefix_len: u.on_link_prefix_length,
                                scopeid: if (*sin6).sin6_scope_id > 0 {
                                    Some((*sin6).sin6_scope_id)
                                } else {
                                    None
                                },
                            });
                        }
                        _ => {}
                    }
                }
                uni = u.next;
            }
            cur = a.next;
        }
        true
    }
}

/// `n` one-bits then zeros (netmask text form for the node cidr field).
#[cfg(windows)]
fn prefix_to_ipv4_mask(prefix: u8) -> String {
    let n = prefix.min(32) as u32;
    let mask = if n == 0 { 0 } else { (!0u32) << (32 - n) };
    let b = mask.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

/// IPv6 netmask text form (node renders the full compressed mask).
#[cfg(windows)]
fn prefix_to_ipv6_mask(prefix: u8) -> String {
    let n = prefix.min(128) as u32;
    let mut groups = [0u16; 8];
    for (i, g) in groups.iter_mut().enumerate() {
        let bits = n.saturating_sub(i as u32 * 16).min(16);
        if bits > 0 {
            *g = ((!0u16) << (16 - bits)) as u16;
        }
    }
    ::std::net::Ipv6Addr::from(groups).to_string()
}
    // (name, mac, internal, entries) — platform enumeration fills this in
    // getifaddrs / GetAdaptersAddresses order.
    let mut ifaces: Vec<(String, String, bool, Vec<IfaceEntry>)> = Vec::new();
    #[cfg(windows)]
    let ok = collect_adapters(&mut ifaces);
    #[cfg(not(windows))]
    let ok = collect_posix_ifaddrs(&mut ifaces);
    if !ok {
        // Enumeration failed: expose an empty object, never a fake interface
        // (BCE-20260816-OS-NETIF: silently-fake results are forbidden).
    }
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if !obj.get().is_null() {
        for (name, mac, internal, entries) in &ifaces {
            let mut oscx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(oscx) let arr = w2::NewArrayObject1(&mut oscx, entries.len().max(1)));
            if arr.get().is_null() {
                continue;
            }
            for (idx, ent) in entries.iter().enumerate() {
                rooted!(&in(wrapped_cx) let ent_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
                if ent_obj.get().is_null() {
                    continue;
                }
                for (key, val_str) in &[
                    ("address", &ent.address),
                    ("netmask", &ent.netmask),
                    ("family", &ent.family.to_string()),
                    ("mac", mac),
                    ("cidr", &format!("{}/{}", ent.address, ent.prefix_len)),
                ] {
                    let c_key = ZBox::from_bytes(key.as_bytes());
                    let utf16: Vec<u16> = val_str.encode_utf16().collect();
                    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
                    if !js_str.is_null() {
                        let val = StringValue(&*js_str);
                        rooted!(&in(wrapped_cx) let v = val);
                        JS_DefineProperty(
                            cx,
                            ent_obj.handle().into(),
                            c_key.as_ptr(),
                            v.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
                {
                    let c_key = ZBox::from_bytes(b"internal");
                    rooted!(&in(wrapped_cx) let v = mozjs::jsval::BooleanValue(*internal));
                    JS_DefineProperty(
                        cx,
                        ent_obj.handle().into(),
                        c_key.as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                if let Some(scopeid) = ent.scopeid {
                    let c_key = ZBox::from_bytes(b"scopeid");
                    rooted!(&in(wrapped_cx) let v = Int32Value(scopeid as i32));
                    JS_DefineProperty(
                        cx,
                        ent_obj.handle().into(),
                        c_key.as_ptr(),
                        v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                rooted!(&in(wrapped_cx) let elem = ObjectValue(ent_obj.get()));
                JS_DefineElement(
                    cx,
                    arr.handle().into(),
                    idx as u32,
                    elem.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let c_name = ZBox::from_bytes(name.as_bytes());
            rooted!(&in(wrapped_cx) let arr_val = ObjectValue(arr.get()));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c_name.as_ptr(),
                arr_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

/// Format an IPv4 s_addr (network byte order) as dotted quad.
unsafe fn ipv4_to_string(s_addr: in_addr_t) -> String {
    let be = s_addr.to_be();
    format!(
        "{}.{}.{}.{}",
        (be >> 24) & 0xff,
        (be >> 16) & 0xff,
        (be >> 8) & 0xff,
        be & 0xff
    )
}

/// Format an IPv6 address in Node style (compressed, lowercase, RFC 5952
/// longest-zero-run compression — ::1 / fe80::... shapes).
unsafe fn ipv6_to_string(addr: &in6_addr) -> String {
    let g = addr.s6_addr;
    let mut groups = [0u16; 8];
    for i in 0..8 {
        groups[i] = ((g[i * 2] as u16) << 8) | g[i * 2 + 1] as u16;
    }
    let (mut best_start, mut best_len) = (usize::MAX, 0usize);
    let mut i = 0;
    while i < 8 {
        if groups[i] == 0 {
            let start = i;
            while i < 8 && groups[i] == 0 {
                i += 1;
            }
            let len = i - start;
            if len > best_len {
                best_len = len;
                best_start = start;
            }
        } else {
            i += 1;
        }
    }
    let mut out = ::std::string::String::new();
    let mut j = 0;
    while j < 8 {
        if j == best_start && best_len > 1 {
            out.push_str("::");
            j += best_len;
            continue;
        }
        if !out.is_empty() && !out.ends_with(':') {
            out.push(':');
        }
        out.push_str(&format!("{:x}", groups[j]));
        j += 1;
    }
    if out.is_empty() {
        out.push_str("::");
    }
    out
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_homedir(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let home = bun_core::getenv_z(bun_core::zstr!("HOME"))
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .or_else(|| {
            bun_core::getenv_z(bun_core::zstr!("USERPROFILE"))
                .map(|s| String::from_utf8_lossy(s).into_owned())
        })
        .unwrap_or_else(|| "/root".to_string());
    return_string(cx, &home, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_tmpdir(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let tmp = bun_core::getenv_z(bun_core::zstr!("TMPDIR"))
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .or_else(|| {
            bun_core::getenv_z(bun_core::zstr!("TEMP"))
                .map(|s| String::from_utf8_lossy(s).into_owned())
        })
        .unwrap_or_else(|| "/tmp".to_string());
    return_string(cx, &tmp, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_user_info(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if !obj.get().is_null() {
        let username = libc_binding::get_username();
        // node windows parity: uid/gid are -1 there (no POSIX ids), shell null
        #[cfg(windows)]
        let (uid, gid) = (-1i32, -1i32);
        #[cfg(not(windows))]
        let uid = unsafe { libc::getuid() } as i32;
        #[cfg(not(windows))]
        let gid = unsafe { libc::getgid() } as i32;
        let home = bun_core::getenv_z(bun_core::zstr!("HOME"))
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .or_else(|| {
                bun_core::getenv_z(bun_core::zstr!("USERPROFILE"))
                    .map(|s| String::from_utf8_lossy(s).into_owned())
            })
            .unwrap_or_else(|| String::new());
        #[cfg(windows)]
        let shell = String::new();
        #[cfg(not(windows))]
        let shell = bun_core::getenv_z(bun_core::zstr!("SHELL"))
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_else(|| "/bin/sh".to_string());

        for (name, val_str) in &[
            ("username", &username),
            ("homedir", &home),
            ("shell", &shell),
        ] {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let utf16: Vec<u16> = val_str.encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            if !js_str.is_null() {
                let val = StringValue(&*js_str);
                rooted!(&in(wrapped_cx) let v = val);
                JS_DefineProperty(
                    cx,
                    obj.handle().into(),
                    c_name.as_ptr(),
                    v.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        for (name, val) in &[("uid", uid), ("gid", gid)] {
            let c_name = ZBox::from_bytes(name.as_bytes());
            rooted!(&in(wrapped_cx) let v = Int32Value(*val));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c_name.as_ptr(),
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_loadavg(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr = w2::NewArrayObject1(&mut wrapped_cx, 3));
    let loadavg = libc_binding::get_loadavg();
    for (i, &val) in loadavg.iter().enumerate() {
        let dval = mozjs::jsval::DoubleValue(val);
        rooted!(&in(wrapped_cx) let v = dval);
        JS_DefineElement(
            cx,
            arr.handle().into(),
            i as u32,
            v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_endianness(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let endian = if cfg!(target_endian = "little") {
        "LE"
    } else {
        "BE"
    };
    return_string(cx, endian, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_dev_null(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let dev = if cfg!(windows) { "NUL" } else { "/dev/null" };
    return_string(cx, dev, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_get_priority(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    #[cfg(windows)]
    let priority = libc_binding::get_priority_windows();
    #[cfg(not(windows))]
    let priority = unsafe { libc::getpriority(0 /*PRIO_PROCESS*/, 0) };
    args.rval().set(Int32Value(priority));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_available_parallelism(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let n = match ::std::thread::available_parallelism() {
        Ok(n) => n.get() as i32,
        Err(_) => 1,
    };
    args.rval().set(Int32Value(n));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_machine(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let machine = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    };
    return_string(cx, machine, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_version(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let version = libc_binding::get_os_version();
    return_string(cx, &version, &args);
    true
}

pub(crate) mod libc_binding {
    #[cfg(windows)]
    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// ComputerNameDnsHostname(1) etc. — winnt.h COMPUTER_NAME_FORMAT.
        pub unsafe fn GetComputerNameExW(
            name_type: u32,
            buffer: *mut u16,
            size: *mut u32,
        ) -> i32;
    }


    #[cfg(not(windows))]
    pub fn get_username() -> String {
        unsafe {
            let uid = libc::getuid();
            let pw = libc::getpwuid(uid);
            if !pw.is_null() {
                let name = ::std::ffi::CStr::from_ptr((*pw).pw_name);
                name.to_string_lossy().into_owned()
            } else {
                bun_core::getenv_z(bun_core::zstr!("USER"))
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .or_else(|| {
                        bun_core::getenv_z(bun_core::zstr!("LOGNAME"))
                            .map(|s| String::from_utf8_lossy(s).into_owned())
                    })
                    .unwrap_or_else(|| "unknown".to_string())
            }
        }
    }

    #[cfg(not(windows))]
    pub fn get_hostname() -> String {
        let mut buf = [0u8; 256];
        unsafe {
            if libc::gethostname(buf.as_mut_ptr() as *mut ::std::os::raw::c_char, buf.len()) == 0 {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..len]).into_owned()
            } else {
                "unknown".to_string()
            }
        }
    }

    #[cfg(not(windows))]
    pub fn get_os_release() -> String {
        let mut buf = [0u8; 256];
        unsafe {
            if libc::syscall(
                libc::SYS_uname,
                buf.as_mut_ptr() as *mut ::std::os::raw::c_void,
            ) == 0
            {
                let utsname = buf.as_ptr() as *const libc::utsname;
                let release = ::std::ffi::CStr::from_ptr((*utsname).release.as_ptr());
                release.to_string_lossy().into_owned()
            } else {
                let mut uname = ::std::mem::MaybeUninit::<libc::utsname>::uninit();
                if libc::uname(uname.as_mut_ptr()) == 0 {
                    let uname = uname.assume_init();
                    let release = ::std::ffi::CStr::from_ptr(uname.release.as_ptr());
                    release.to_string_lossy().into_owned()
                } else {
                    "unknown".to_string()
                }
            }
        }
    }

    #[cfg(not(windows))]
    pub struct SysInfo {
        pub totalram: u64,
        pub freeram: u64,
        pub uptime: u64,
    }

    #[cfg(not(windows))]
    pub fn get_sysinfo() -> SysInfo {
        let mut info = ::std::mem::MaybeUninit::<libc::sysinfo>::uninit();
        unsafe {
            if libc::sysinfo(info.as_mut_ptr()) == 0 {
                let info = info.assume_init();
                let mem_unit = if info.mem_unit == 0 {
                    1
                } else {
                    info.mem_unit as u64
                };
                SysInfo {
                    totalram: info.totalram * mem_unit,
                    freeram: info.freeram * mem_unit,
                    uptime: info.uptime as u64,
                }
            } else {
                SysInfo {
                    totalram: 0,
                    freeram: 0,
                    uptime: 0,
                }
            }
        }
    }

    #[cfg(not(windows))]
    pub fn get_loadavg() -> [f64; 3] {
        let mut avg = [0.0f64; 3];
        unsafe {
            libc::getloadavg(avg.as_mut_ptr(), 3);
        }
        avg
    }

    #[cfg(not(windows))]
    pub fn get_cpu_model() -> String {
        if let Ok(content) = bun_sys::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("model name")
                    && let Some((_, val)) = line.split_once(':')
                {
                    return val.trim().to_string();
                }
            }
        }
        "unknown".to_string()
    }

    #[cfg(not(windows))]
    pub fn get_os_version() -> String {
        let mut uname = ::std::mem::MaybeUninit::<libc::utsname>::uninit();
        unsafe {
            if libc::uname(uname.as_mut_ptr()) == 0 {
                let uname = uname.assume_init();
                let version = ::std::ffi::CStr::from_ptr(uname.version.as_ptr());
                version.to_string_lossy().into_owned()
            } else {
                "unknown".to_string()
            }
        }
    }

    // ── windows arms (W2.5) ──────────────────────────────────────────────
    // node parity: hostname = DNS hostname (GetComputerNameExW); release =
    // major.minor.build (RtlGetVersion — GetVersionExW is manifest-gated);
    // sysinfo = GlobalMemoryStatusEx + GetTickCount64; cpu model = cpuid
    // brand string; loadavg = [0,0,0] (no load concept on windows);
    // username = %USERNAME% (GetUserNameW's flat name differs from the
    // POSIX login name node exposes).

    #[cfg(windows)]
    pub fn get_username() -> String {
        bun_core::getenv_z(bun_core::zstr!("USERNAME"))
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_else(|| "unknown".to_string())
    }

    #[cfg(windows)]
    pub fn get_hostname() -> String {
        // ComputerNameDnsHostname = 1 (winnt.h COMPUTER_NAME_FORMAT)
        let mut buf = [0u16; 256];
        let mut size = buf.len() as u32;
        // SAFETY: buffer/len valid; declared kernel32 entry point.
        let ok = unsafe { GetComputerNameExW(1, buf.as_mut_ptr(), &mut size) != 0 };
        if ok {
            String::from_utf16_lossy(&buf[..size as usize])
        } else {
            bun_core::getenv_z(bun_core::zstr!("COMPUTERNAME"))
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .unwrap_or_else(|| "unknown".to_string())
        }
    }

    /// `RTL_OSVERSIONINFOW` minimal mirror (size, major, minor, build,
    /// platform id, CSD string).
    #[cfg(windows)]
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        csd_version: [u16; 128],
    }

    #[cfg(windows)]
    fn rtl_version() -> (u32, u32, u32) {
        let mut vi = OsVersionInfoW {
            size: ::std::mem::size_of::<OsVersionInfoW>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform_id: 0,
            csd_version: [0; 128],
        };
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
        }
        // SAFETY: info is a valid OsVersionInfoW sized per the contract.
        let rc = unsafe { RtlGetVersion(&mut vi) };
        if rc == 0 {
            (vi.major, vi.minor, vi.build)
        } else {
            (0, 0, 0)
        }
    }

    #[cfg(windows)]
    pub fn get_os_release() -> String {
        let (major, minor, build) = rtl_version();
        if major == 0 {
            "unknown".to_string()
        } else {
            format!("{}.{}.{}", major, minor, build)
        }
    }

    #[cfg(windows)]
    pub fn get_os_version() -> String {
        let (major, minor, build) = rtl_version();
        if major == 0 {
            "unknown".to_string()
        } else {
            format!("{}.{}.{}", major, minor, build)
        }
    }

    #[cfg(windows)]
    pub struct SysInfo {
        pub totalram: u64,
        pub freeram: u64,
        pub uptime: u64,
    }

    #[cfg(windows)]
    pub fn get_sysinfo() -> SysInfo {
        // MEMORYSTATUSEX (win32) — only total/avail physical are consumed.
        #[repr(C)]
        struct MemoryStatusEx {
            length: u32,
            memory_load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page: u64,
            avail_page: u64,
            total_virtual: u64,
            avail_virtual: u64,
            avail_extended_virtual: u64,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
            fn GetTickCount64() -> u64;
        }
        let mut ms = MemoryStatusEx {
            length: ::std::mem::size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page: 0,
            avail_page: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // SAFETY: buf sized to dwLength; declared kernel32 entry point.
        let ok = unsafe { GlobalMemoryStatusEx(&mut ms) } != 0;
        // SAFETY: no args.
        let uptime = unsafe { GetTickCount64() } / 1000;
        SysInfo {
            totalram: if ok { ms.total_phys } else { 0 },
            freeram: if ok { ms.avail_phys } else { 0 },
            uptime: if ok { uptime } else { 0 },
        }
    }

    #[cfg(windows)]
    pub fn get_loadavg() -> [f64; 3] {
        // node windows parity: no load concept — zeros.
        [0.0; 3]
    }

    #[cfg(windows)]
    pub fn get_cpu_model() -> String {
        // cpuid leaves 0x80000002..0x80000004 = 48-byte brand string (x86_64).
        #[cfg(target_arch = "x86_64")]
        {
            let mut brand = [0u8; 48];
            // SAFETY: documented extended brand-string leaves; x86_64 only.
            unsafe {
                for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
                    let out = ::std::arch::x86_64::__cpuid(leaf);
                    brand[i * 16..i * 16 + 4].copy_from_slice(&out.eax.to_ne_bytes());
                    brand[i * 16 + 4..i * 16 + 8].copy_from_slice(&out.ebx.to_ne_bytes());
                    brand[i * 16 + 8..i * 16 + 12].copy_from_slice(&out.ecx.to_ne_bytes());
                    brand[i * 16 + 12..i * 16 + 16].copy_from_slice(&out.edx.to_ne_bytes());
                }
            }
            let end = brand.iter().position(|&b| b == 0).unwrap_or(48);
            let trimmed = String::from_utf8_lossy(&brand[..end]).trim().to_string();
            if trimmed.is_empty() {
                "unknown".to_string()
            } else {
                trimmed
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            "unknown".to_string()
        }
    }

    /// node windows parity: os.getPriority maps the process priority class
    /// onto the nice scale (kernel32 GetPriorityClass).
    #[cfg(windows)]
    pub fn get_priority_windows() -> i32 {
        const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;
        const REALTIME_PRIORITY_CLASS: u32 = 0x0000_0100;
        const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut core::ffi::c_void;
            fn GetPriorityClass(hProcess: *mut core::ffi::c_void) -> u32;
        }
        // SAFETY: pseudo process handle; declared kernel32 entry point.
        let class = unsafe { GetPriorityClass(GetCurrentProcess()) };
        match class {
            IDLE_PRIORITY_CLASS => 19,
            BELOW_NORMAL_PRIORITY_CLASS => 10,
            ABOVE_NORMAL_PRIORITY_CLASS => -7,
            HIGH_PRIORITY_CLASS => -14,
            REALTIME_PRIORITY_CLASS => -20,
            _ => 0, // NORMAL
        }
    }
}

#[cfg(test)]
mod tests {
    use super::libc_binding::*;

    #[test]
    fn test_get_username_not_empty() {
        let name = get_username();
        assert!(!name.is_empty(), "username should not be empty");
        assert_ne!(name, "unknown", "username should not fallback to unknown");
    }

    #[test]
    fn test_get_hostname_not_empty() {
        let host = get_hostname();
        assert!(!host.is_empty(), "hostname should not be empty");
    }

    #[test]
    fn test_get_os_release_not_empty() {
        let rel = get_os_release();
        assert!(!rel.is_empty(), "os release should not be empty");
    }

    #[test]
    fn test_get_sysinfo_positive() {
        let info = get_sysinfo();
        assert!(info.totalram > 0, "totalram should be > 0");
        assert!(info.freeram > 0, "freeram should be > 0");
        assert!(info.uptime > 0, "uptime should be > 0");
    }

    #[test]
    fn test_get_loadavg_values() {
        let avg = get_loadavg();
        // Load averages are typically >= 0 on a running system
        assert!(avg[0] >= 0.0, "1min load avg should be >= 0");
    }

    #[test]
    fn test_get_cpu_model_not_unknown() {
        let model = get_cpu_model();
        // On Linux, /proc/cpuinfo should have model name
        assert_ne!(model, "unknown", "CPU model should not be unknown on Linux");
    }

    #[test]
    fn test_get_os_version_not_empty() {
        let ver = get_os_version();
        assert!(!ver.is_empty(), "os version should not be empty");
    }

    #[test]
    fn test_sysinfo_freeram_less_than_total() {
        let info = get_sysinfo();
        assert!(info.freeram <= info.totalram, "freeram should <= totalram");
    }

    // ─── libc_binding extended edge case tests ───────────────────
    // @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

    #[test]
    fn test_get_hostname_no_null_bytes() {
        let host = get_hostname();
        assert!(
            !host.contains('\0'),
            "hostname should not contain null bytes"
        );
    }

    #[test]
    fn test_get_os_release_format() {
        let rel = get_os_release();
        // Linux kernel version format: X.Y.Z...
        assert!(
            rel.contains('.') || rel == "unknown",
            "release should contain dots or be unknown"
        );
    }

    #[test]
    fn test_get_sysinfo_uptime_reasonable() {
        let info = get_sysinfo();
        // Uptime should be less than 10 years in seconds (reasonable bound)
        let ten_years_secs: u64 = 10 * 365 * 24 * 3600;
        assert!(info.uptime < ten_years_secs, "uptime should be reasonable");
    }

    #[test]
    fn test_get_sysinfo_totalram_reasonable() {
        let info = get_sysinfo();
        // Total RAM should be less than 10 TB (reasonable bound)
        let ten_tb: u64 = 10 * 1024 * 1024 * 1024 * 1024;
        assert!(info.totalram < ten_tb, "totalram should be reasonable");
    }

    #[test]
    fn test_get_loadavg_three_values() {
        let avg = get_loadavg();
        // All three load averages should be finite
        assert!(avg[0].is_finite(), "1min load avg should be finite");
        assert!(avg[1].is_finite(), "5min load avg should be finite");
        assert!(avg[2].is_finite(), "15min load avg should be finite");
    }

    #[test]
    fn test_get_cpu_model_not_empty() {
        let model = get_cpu_model();
        assert!(!model.is_empty(), "CPU model should not be empty");
    }

    #[test]
    fn test_get_os_version_not_unknown() {
        let ver = get_os_version();
        // On a real Linux system, version should not be "unknown"
        assert_ne!(ver, "unknown", "os version should not be unknown on Linux");
    }

    #[test]
    fn test_sysinfo_struct_fields_consistent() {
        let info = get_sysinfo();
        // mem_unit is already factored in, so totalram should be >= raw totalram
        assert!(info.totalram > 0);
        assert!(info.freeram > 0);
        assert!(info.uptime > 0);
    }
}
