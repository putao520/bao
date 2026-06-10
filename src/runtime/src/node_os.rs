// @trace REQ-ENG-007
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, Int32Value, ObjectValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let os_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if os_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, os_obj.handle(), c"hostname".as_ptr(), Some(os_hostname), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"type".as_ptr(), Some(os_type), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"platform".as_ptr(), Some(os_platform), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"arch".as_ptr(), Some(os_arch), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"release".as_ptr(), Some(os_release), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"uptime".as_ptr(), Some(os_uptime), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"totalmem".as_ptr(), Some(os_totalmem), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"freemem".as_ptr(), Some(os_freemem), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"cpus".as_ptr(), Some(os_cpus), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"networkInterfaces".as_ptr(), Some(os_network_interfaces), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"homedir".as_ptr(), Some(os_homedir), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"tmpdir".as_ptr(), Some(os_tmpdir), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"userInfo".as_ptr(), Some(os_user_info), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"loadavg".as_ptr(), Some(os_loadavg), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"endianness".as_ptr(), Some(os_endianness), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"devNull".as_ptr(), Some(os_dev_null), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"getPriority".as_ptr(), Some(os_get_priority), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"availableParallelism".as_ptr(), Some(os_available_parallelism), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"machine".as_ptr(), Some(os_machine), 0, 0);
        w2::JS_DefineFunction(cx, os_obj.handle(), c"version".as_ptr(), Some(os_version), 0, 0);

        let eol = if cfg!(windows) { "\r\n" } else { "\n" };
        let eol_str = JS_NewStringCopyN(cx.raw_cx(), eol.as_ptr() as *const ::std::os::raw::c_char, eol.len());
        if !eol_str.is_null() {
            let val = StringValue(&*eol_str);
            rooted!(&in(cx) let v = val);
            JS_DefineProperty(
                cx.raw_cx(), os_obj.handle().into(), c"EOL".as_ptr(),
                v.handle().into(), (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
            );
        }

        let dev_null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let dn_str = JS_NewStringCopyN(cx.raw_cx(), dev_null.as_ptr() as *const ::std::os::raw::c_char, dev_null.len());
        if !dn_str.is_null() {
            let val = StringValue(&*dn_str);
            rooted!(&in(cx) let v = val);
            JS_DefineProperty(
                cx.raw_cx(), os_obj.handle().into(), c"devNull".as_ptr(),
                v.handle().into(), (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
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
                let signals = [("SIGHUP", 1), ("SIGINT", 2), ("SIGQUIT", 3), ("SIGILL", 4), ("SIGTRAP", 5), ("SIGABRT", 6), ("SIGBUS", 7), ("SIGFPE", 8), ("SIGKILL", 9), ("SIGUSR1", 10), ("SIGSEGV", 11), ("SIGUSR2", 12), ("SIGPIPE", 13), ("SIGALRM", 14), ("SIGTERM", 15)];
                for (name, val) in &signals {
                    let v = Int32Value(*val);
                    rooted!(&in(cx) let rv = v);
                    let sig_ptr = sig_obj.get();
                    let sig_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sig_ptr };
                    JS_DefineProperty(raw, sig_h, bun_core::ZBox::from_bytes(name.as_bytes()).as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
                }
                w2::JS_DefineProperty3(cx, constants_obj.handle(), c"signals".as_ptr(), sig_obj.handle(), JSPROP_ENUMERATE as u32);
            }
            w2::JS_DefineProperty3(cx, os_obj.handle(), c"constants".as_ptr(), constants_obj.handle(), JSPROP_ENUMERATE as u32);
        }
    }

    cache_builtin(cx, "os", os_obj.get());
}

unsafe fn return_string(cx: *mut JSContext, s: &str, args: &CallArgs) { unsafe {
    let utf16: Vec<u16> = s.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
}}

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
    let os_type = if cfg!(target_os = "linux") { "Linux" }
        else if cfg!(target_os = "macos") { "Darwin" }
        else if cfg!(target_os = "windows") { "Windows_NT" }
        else { "Unknown" };
    return_string(cx, os_type, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_platform(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let platform = if cfg!(target_os = "linux") { "linux" }
        else if cfg!(target_os = "macos") { "darwin" }
        else if cfg!(target_os = "windows") { "win32" }
        else { "unknown" };
    return_string(cx, platform, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_arch(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let arch = if cfg!(target_arch = "x86_64") { "x64" }
        else if cfg!(target_arch = "aarch64") { "arm64" }
        else if cfg!(target_arch = "x86") { "ia32" }
        else if cfg!(target_arch = "arm") { "arm" }
        else { "unknown" };
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
            let model_str = JS_NewStringCopyN(cx, model.as_ptr() as *const ::std::os::raw::c_char, model.len());
            if !model_str.is_null() {
                let val = StringValue(&*model_str);
                rooted!(&in(cx_ref) let mv = val);
                JS_DefineProperty(cx, cpu.handle().into(), c"model".as_ptr(), mv.handle().into(), JSPROP_ENUMERATE as u32);
            }
            rooted!(&in(cx_ref) let sv = Int32Value(i as i32));
            JS_DefineProperty(cx, cpu.handle().into(), c"speed".as_ptr(), sv.handle().into(), JSPROP_ENUMERATE as u32);

            rooted!(&in(cx_ref) let times = mozjs_sys::jsapi::JS_NewPlainObject(cx));
            if !times.get().is_null() {
                for &(name, val) in &[("user", 0i32), ("nice", 0), ("sys", 0), ("idle", 0), ("irq", 0)] {
                    let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
                    rooted!(&in(cx_ref) let tv = Int32Value(val));
                    JS_DefineProperty(cx, times.handle().into(), c_name.as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
                }
            }
            let times_val = ObjectValue(times.get());
            rooted!(&in(cx_ref) let tv = times_val);
            JS_DefineProperty(cx, cpu.handle().into(), c"times".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);
        }
        let cpu_val = ObjectValue(cpu.get());
        rooted!(&in(cx_ref) let cv = cpu_val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, cv.handle().into(), JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_network_interfaces(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_homedir(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // D51: bun_core::env_var::HOME::get() handles HOME/USERPROFILE cross-platform
    let home = bun_core::env_var::HOME::get()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_else(|| "/root".to_string());
    return_string(cx, &home, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_tmpdir(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // D51: bun_core::env_var handles TMPDIR/TEMP cross-platform
    let tmp = bun_core::env_var::TMPDIR::get()
        .or_else(|| bun_core::env_var::TEMP::get())
        .map(|b| String::from_utf8_lossy(b).into_owned())
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
        let uid = bun_sys::safe_libc::getuid();
        let gid = bun_sys::safe_libc::getgid();
        let home = bun_core::env_var::HOME::get()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let shell = bun_core::env_var::SHELL::get()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_else(|| "/bin/sh".to_string());

        for (name, val_str) in &[("username", &username), ("homedir", &home), ("shell", &shell)] {
            let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
            let utf16: Vec<u16> = val_str.encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            if !js_str.is_null() {
                let val = StringValue(&*js_str);
                rooted!(&in(wrapped_cx) let v = val);
                JS_DefineProperty(cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        for (name, val) in &[("uid", uid as i32), ("gid", gid as i32)] {
            let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
            rooted!(&in(wrapped_cx) let v = Int32Value(*val));
            JS_DefineProperty(cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
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
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_endianness(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let endian = if cfg!(target_endian = "little") { "LE" } else { "BE" };
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
    // D85: bun_libuv_sys::uv_os_getpriority replaces libc::getpriority (cross-platform)
    let val = bun_libuv_sys::uv_os_getpriority(bun_libuv_sys::uv_os_getpid());
    args.rval().set(Int32Value(val));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_available_parallelism(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
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
    let machine = if cfg!(target_arch = "x86_64") { "x86_64" }
        else if cfg!(target_arch = "aarch64") { "aarch64" }
        else if cfg!(target_arch = "x86") { "i686" }
        else if cfg!(target_arch = "arm") { "arm" }
        else { "unknown" };
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
    

    pub fn get_username() -> String {
        // D38: bun_sys::safe_libc::getuid is safe fn (no unsafe needed)
        let uid = bun_sys::safe_libc::getuid();
        // D86: bun_libuv_sys::uv_os_get_passwd2 replaces libc::getpwuid (cross-platform)
        let mut pwd: bun_libuv_sys::uv_passwd_t = unsafe { core::mem::zeroed() };
        pwd.uid = uid as bun_libuv_sys::uv_uid_t;
        let rc = bun_libuv_sys::uv_os_get_passwd2(&mut pwd);
        if rc == 0 && !pwd.username.is_null() {
            let name = unsafe { bun_core::ZStr::from_c_ptr(pwd.username) };
            let result = name.as_cstr().to_string_lossy().into_owned();
            unsafe { bun_libuv_sys::uv_os_free_passwd(&mut pwd) };
            result
        } else {
            // D51: bun_core::env_var::USER::get() handles USER/LOGNAME/USERNAME cross-platform
            bun_core::env_var::USER::get()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| "unknown".to_string())
        }
    }

    pub fn get_hostname() -> String {
        let mut buf = [0u8; 256];
        // D38: bun_sys::gethostname replaces libc::gethostname
        if bun_sys::gethostname(&mut buf).is_ok() {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..len]).into_owned()
        } else {
            "unknown".to_string()
        }
    }

    pub fn get_os_release() -> String {
        // D38: bun_core::ffi::uname() replaces libc::syscall(SYS_uname) + libc::uname
        let u = bun_core::ffi::uname();
        let release = unsafe { bun_core::ZStr::from_c_ptr(u.release.as_ptr().cast()) };
        release.as_cstr().to_string_lossy().into_owned()
    }

    pub struct SysInfo {
        pub totalram: u64,
        pub freeram: u64,
        pub uptime: u64,
    }

    pub fn get_sysinfo() -> SysInfo {
        // D38: bun_sys::sysinfo() replaces libc::sysinfo
        match bun_sys::sysinfo() {
            Ok(info) => {
                let mem_unit = if info.mem_unit == 0 { 1 } else { info.mem_unit as u64 };
                SysInfo {
                    totalram: info.totalram * mem_unit,
                    freeram: info.freeram * mem_unit,
                    uptime: info.uptime as u64,
                }
            }
            Err(_) => SysInfo { totalram: 0, freeram: 0, uptime: 0 }
        }
    }

    pub fn get_loadavg() -> [f64; 3] {
        let mut avg = [0.0f64; 3];
        // D94: bun_sys::c::getloadavg replaces libc::getloadavg (cross-platform)
        unsafe { bun_sys::c::getloadavg(avg.as_mut_ptr(), 3) };
        avg
    }

    pub fn get_cpu_model() -> String {
        if let Ok(file) = bun_sys::File::open(
            bun_core::zstr!("/proc/cpuinfo"),
            bun_sys::O::RDONLY,
            0,
        ) {
            if let Ok(bytes) = file.read_to_end_small() {
                let content = String::from_utf8_lossy(&bytes);
                for line in content.lines() {
                    if line.starts_with("model name")
                        && let Some((_, val)) = line.split_once(':') {
                            return val.trim().to_string();
                        }
                }
            }
        }
        "unknown".to_string()
    }

    pub fn get_os_version() -> String {
        // D38: bun_core::ffi::uname() replaces libc::uname
        let u = bun_core::ffi::uname();
        let version = unsafe { bun_core::ZStr::from_c_ptr(u.version.as_ptr().cast()) };
        version.as_cstr().to_string_lossy().into_owned()
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
        assert!(!host.contains('\0'), "hostname should not contain null bytes");
    }

    #[test]
    fn test_get_os_release_format() {
        let rel = get_os_release();
        // Linux kernel version format: X.Y.Z...
        assert!(rel.contains('.') || rel == "unknown", "release should contain dots or be unknown");
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
