use ::std::ffi::CString;
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
                    JS_DefineProperty(raw, sig_h, CString::new(*name).unwrap_or_default().as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);
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
        Ok(n) => n.get() as usize,
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
                    let Ok(c_name) = CString::new(name) else { continue };
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
    let home = ::std::env::var("HOME")
        .or_else(|_| ::std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/root".to_string());
    return_string(cx, &home, &args);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn os_tmpdir(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let tmp = ::std::env::var("TMPDIR")
        .or_else(|_| ::std::env::var("TEMP"))
        .unwrap_or_else(|_| "/tmp".to_string());
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
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let home = ::std::env::var("HOME").unwrap_or_else(|_| String::new());
        let shell = ::std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        for (name, val_str) in &[("username", &username), ("homedir", &home), ("shell", &shell)] {
            let Ok(c_name) = CString::new(*name) else { continue };
            let utf16: Vec<u16> = val_str.encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            if !js_str.is_null() {
                let val = StringValue(&*js_str);
                rooted!(&in(wrapped_cx) let v = val);
                JS_DefineProperty(cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        for (name, val) in &[("uid", uid as i32), ("gid", gid as i32)] {
            let Ok(c_name) = CString::new(*name) else { continue };
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
    let priority = unsafe { libc::getpriority(0, 0) };
    args.rval().set(Int32Value(priority));
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

mod libc_binding {
    

    pub fn get_username() -> String {
        unsafe {
            let uid = libc::getuid();
            let pw = libc::getpwuid(uid);
            if !pw.is_null() {
                let name = ::std::ffi::CStr::from_ptr((*pw).pw_name);
                name.to_string_lossy().into_owned()
            } else {
                ::std::env::var("USER")
                    .or_else(|_| ::std::env::var("LOGNAME"))
                    .unwrap_or_else(|_| "unknown".to_string())
            }
        }
    }

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

    pub fn get_os_release() -> String {
        let mut buf = [0u8; 256];
        unsafe {
            if libc::syscall(libc::SYS_uname, buf.as_mut_ptr() as *mut ::std::os::raw::c_void) == 0 {
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

    pub struct SysInfo {
        pub totalram: u64,
        pub freeram: u64,
        pub uptime: u64,
    }

    pub fn get_sysinfo() -> SysInfo {
        let mut info = ::std::mem::MaybeUninit::<libc::sysinfo>::uninit();
        unsafe {
            if libc::sysinfo(info.as_mut_ptr()) == 0 {
                let info = info.assume_init();
                let mem_unit = if info.mem_unit == 0 { 1 } else { info.mem_unit as u64 };
                SysInfo {
                    totalram: info.totalram * mem_unit,
                    freeram: info.freeram * mem_unit,
                    uptime: info.uptime as u64,
                }
            } else {
                SysInfo { totalram: 0, freeram: 0, uptime: 0 }
            }
        }
    }

    pub fn get_loadavg() -> [f64; 3] {
        let mut avg = [0.0f64; 3];
        unsafe {
            libc::getloadavg(avg.as_mut_ptr(), 3);
        }
        avg
    }

    pub fn get_cpu_model() -> String {
        if let Ok(content) = ::std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("model name") {
                    if let Some((_, val)) = line.split_once(':') {
                        return val.trim().to_string();
                    }
                }
            }
        }
        "unknown".to_string()
    }

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
}
