// @trace REQ-ENG-006 [api:node:constants]
//
// Node.js `constants` module — system constants (O_* flags, S_* permissions,
// signals, errno, priority). Values are sourced from libc on Linux.

use bun_core::ZBox;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, Int32Value, ObjectValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let constants_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if constants_obj.get().is_null() {
        return;
    }

    unsafe {
        // fs sub-object: O_* flags + S_* permissions
        rooted!(&in(cx) let fs_obj = w2::JS_NewPlainObject(cx));
        if !fs_obj.get().is_null() {
            // O_* flags (Linux values from libc)
            define_int_prop(cx, fs_obj.get(), "O_RDONLY", libc::O_RDONLY as i32);
            define_int_prop(cx, fs_obj.get(), "O_WRONLY", libc::O_WRONLY as i32);
            define_int_prop(cx, fs_obj.get(), "O_RDWR", libc::O_RDWR as i32);
            define_int_prop(cx, fs_obj.get(), "O_CREAT", libc::O_CREAT as i32);
            define_int_prop(cx, fs_obj.get(), "O_EXCL", libc::O_EXCL as i32);
            define_int_prop(cx, fs_obj.get(), "O_NOCTTY", libc::O_NOCTTY as i32);
            define_int_prop(cx, fs_obj.get(), "O_TRUNC", libc::O_TRUNC as i32);
            define_int_prop(cx, fs_obj.get(), "O_APPEND", libc::O_APPEND as i32);
            define_int_prop(cx, fs_obj.get(), "O_DIRECTORY", libc::O_DIRECTORY as i32);
            define_int_prop(cx, fs_obj.get(), "O_NOFOLLOW", libc::O_NOFOLLOW as i32);
            define_int_prop(cx, fs_obj.get(), "O_SYNC", libc::O_SYNC as i32);
            define_int_prop(cx, fs_obj.get(), "O_DSYNC", libc::O_DSYNC as i32);
            define_int_prop(cx, fs_obj.get(), "O_SYMLINK", libc::O_NOFOLLOW as i32); // No O_SYMLINK on Linux; use O_NOFOLLOW
            define_int_prop(cx, fs_obj.get(), "O_DIRECT", libc::O_DIRECT as i32);
            define_int_prop(cx, fs_obj.get(), "O_NONBLOCK", libc::O_NONBLOCK as i32);

            // S_* permissions
            define_int_prop(cx, fs_obj.get(), "S_IRWXU", libc::S_IRWXU as i32);
            define_int_prop(cx, fs_obj.get(), "S_IRUSR", libc::S_IRUSR as i32);
            define_int_prop(cx, fs_obj.get(), "S_IWUSR", libc::S_IWUSR as i32);
            define_int_prop(cx, fs_obj.get(), "S_IXUSR", libc::S_IXUSR as i32);
            define_int_prop(cx, fs_obj.get(), "S_IRWXG", libc::S_IRWXG as i32);
            define_int_prop(cx, fs_obj.get(), "S_IRGRP", libc::S_IRGRP as i32);
            define_int_prop(cx, fs_obj.get(), "S_IWGRP", libc::S_IWGRP as i32);
            define_int_prop(cx, fs_obj.get(), "S_IXGRP", libc::S_IXGRP as i32);
            define_int_prop(cx, fs_obj.get(), "S_IRWXO", libc::S_IRWXO as i32);
            define_int_prop(cx, fs_obj.get(), "S_IROTH", libc::S_IROTH as i32);
            define_int_prop(cx, fs_obj.get(), "S_IWOTH", libc::S_IWOTH as i32);
            define_int_prop(cx, fs_obj.get(), "S_IXOTH", libc::S_IXOTH as i32);

            // Copy flags
            define_int_prop(cx, fs_obj.get(), "COPYFILE_EXCL", 1);
            define_int_prop(cx, fs_obj.get(), "COPYFILE_FICLONE", 2);
            define_int_prop(cx, fs_obj.get(), "COPYFILE_FICLONE_FORCE", 4);

            rooted!(&in(cx) let fs_val = ObjectValue(fs_obj.get()));
            JS_DefineProperty(cx.raw_cx(), constants_obj.handle().into(), c"fs".as_ptr(), fs_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // os sub-object: signals, errno, priority
        rooted!(&in(cx) let os_obj = w2::JS_NewPlainObject(cx));
        if !os_obj.get().is_null() {
            // errno constants
            define_int_prop(cx, os_obj.get(), "E2BIG", libc::E2BIG as i32);
            define_int_prop(cx, os_obj.get(), "EACCES", libc::EACCES as i32);
            define_int_prop(cx, os_obj.get(), "EADDRINUSE", libc::EADDRINUSE as i32);
            define_int_prop(cx, os_obj.get(), "EADDRNOTAVAIL", libc::EADDRNOTAVAIL as i32);
            define_int_prop(cx, os_obj.get(), "EAFNOSUPPORT", libc::EAFNOSUPPORT as i32);
            define_int_prop(cx, os_obj.get(), "EAGAIN", libc::EAGAIN as i32);
            define_int_prop(cx, os_obj.get(), "EALREADY", libc::EALREADY as i32);
            define_int_prop(cx, os_obj.get(), "EBADF", libc::EBADF as i32);
            define_int_prop(cx, os_obj.get(), "EBADMSG", libc::EBADMSG as i32);
            define_int_prop(cx, os_obj.get(), "EBUSY", libc::EBUSY as i32);
            define_int_prop(cx, os_obj.get(), "ECANCELED", libc::ECANCELED as i32);
            define_int_prop(cx, os_obj.get(), "ECHILD", libc::ECHILD as i32);
            define_int_prop(cx, os_obj.get(), "ECONNABORTED", libc::ECONNABORTED as i32);
            define_int_prop(cx, os_obj.get(), "ECONNREFUSED", libc::ECONNREFUSED as i32);
            define_int_prop(cx, os_obj.get(), "ECONNRESET", libc::ECONNRESET as i32);
            define_int_prop(cx, os_obj.get(), "EDEADLK", libc::EDEADLK as i32);
            define_int_prop(cx, os_obj.get(), "EDESTADDRREQ", libc::EDESTADDRREQ as i32);
            define_int_prop(cx, os_obj.get(), "EDOM", libc::EDOM as i32);
            define_int_prop(cx, os_obj.get(), "EEXIST", libc::EEXIST as i32);
            define_int_prop(cx, os_obj.get(), "EFAULT", libc::EFAULT as i32);
            define_int_prop(cx, os_obj.get(), "EFBIG", libc::EFBIG as i32);
            define_int_prop(cx, os_obj.get(), "EHOSTUNREACH", libc::EHOSTUNREACH as i32);
            define_int_prop(cx, os_obj.get(), "EIDRM", libc::EIDRM as i32);
            define_int_prop(cx, os_obj.get(), "EILSEQ", libc::EILSEQ as i32);
            define_int_prop(cx, os_obj.get(), "EINPROGRESS", libc::EINPROGRESS as i32);
            define_int_prop(cx, os_obj.get(), "EINTR", libc::EINTR as i32);
            define_int_prop(cx, os_obj.get(), "EINVAL", libc::EINVAL as i32);
            define_int_prop(cx, os_obj.get(), "EIO", libc::EIO as i32);
            define_int_prop(cx, os_obj.get(), "EISCONN", libc::EISCONN as i32);
            define_int_prop(cx, os_obj.get(), "EISDIR", libc::EISDIR as i32);
            define_int_prop(cx, os_obj.get(), "ELOOP", libc::ELOOP as i32);
            define_int_prop(cx, os_obj.get(), "EMFILE", libc::EMFILE as i32);
            define_int_prop(cx, os_obj.get(), "EMLINK", libc::EMLINK as i32);
            define_int_prop(cx, os_obj.get(), "EMSGSIZE", libc::EMSGSIZE as i32);
            define_int_prop(cx, os_obj.get(), "ENAMETOOLONG", libc::ENAMETOOLONG as i32);
            define_int_prop(cx, os_obj.get(), "ENETDOWN", libc::ENETDOWN as i32);
            define_int_prop(cx, os_obj.get(), "ENETRESET", libc::ENETRESET as i32);
            define_int_prop(cx, os_obj.get(), "ENETUNREACH", libc::ENETUNREACH as i32);
            define_int_prop(cx, os_obj.get(), "ENFILE", libc::ENFILE as i32);
            define_int_prop(cx, os_obj.get(), "ENOBUFS", libc::ENOBUFS as i32);
            define_int_prop(cx, os_obj.get(), "ENODATA", libc::ENODATA as i32);
            define_int_prop(cx, os_obj.get(), "ENODEV", libc::ENODEV as i32);
            define_int_prop(cx, os_obj.get(), "ENOENT", libc::ENOENT as i32);
            define_int_prop(cx, os_obj.get(), "ENOEXEC", libc::ENOEXEC as i32);
            define_int_prop(cx, os_obj.get(), "ENOLCK", libc::ENOLCK as i32);
            define_int_prop(cx, os_obj.get(), "ENOLINK", libc::ENOLINK as i32);
            define_int_prop(cx, os_obj.get(), "ENOMEM", libc::ENOMEM as i32);
            define_int_prop(cx, os_obj.get(), "ENOMSG", libc::ENOMSG as i32);
            define_int_prop(cx, os_obj.get(), "ENOPROTOOPT", libc::ENOPROTOOPT as i32);
            define_int_prop(cx, os_obj.get(), "ENOSPC", libc::ENOSPC as i32);
            define_int_prop(cx, os_obj.get(), "ENOSR", libc::ENOSR as i32);
            define_int_prop(cx, os_obj.get(), "ENOSTR", libc::ENOSTR as i32);
            define_int_prop(cx, os_obj.get(), "ENOSYS", libc::ENOSYS as i32);
            define_int_prop(cx, os_obj.get(), "ENOTCONN", libc::ENOTCONN as i32);
            define_int_prop(cx, os_obj.get(), "ENOTDIR", libc::ENOTDIR as i32);
            define_int_prop(cx, os_obj.get(), "ENOTEMPTY", libc::ENOTEMPTY as i32);
            define_int_prop(cx, os_obj.get(), "ENOTSOCK", libc::ENOTSOCK as i32);
            define_int_prop(cx, os_obj.get(), "ENOTSUP", libc::ENOTSUP as i32);
            define_int_prop(cx, os_obj.get(), "ENOTTY", libc::ENOTTY as i32);
            define_int_prop(cx, os_obj.get(), "ENXIO", libc::ENXIO as i32);
            define_int_prop(cx, os_obj.get(), "EOPNOTSUPP", libc::EOPNOTSUPP as i32);
            define_int_prop(cx, os_obj.get(), "EOVERFLOW", libc::EOVERFLOW as i32);
            define_int_prop(cx, os_obj.get(), "EPERM", libc::EPERM as i32);
            define_int_prop(cx, os_obj.get(), "EPIPE", libc::EPIPE as i32);
            define_int_prop(cx, os_obj.get(), "EPROTO", libc::EPROTO as i32);
            define_int_prop(cx, os_obj.get(), "EPROTONOSUPPORT", libc::EPROTONOSUPPORT as i32);
            define_int_prop(cx, os_obj.get(), "EPROTOTYPE", libc::EPROTOTYPE as i32);
            define_int_prop(cx, os_obj.get(), "ERANGE", libc::ERANGE as i32);
            define_int_prop(cx, os_obj.get(), "EROFS", libc::EROFS as i32);
            define_int_prop(cx, os_obj.get(), "ESPIPE", libc::ESPIPE as i32);
            define_int_prop(cx, os_obj.get(), "ESRCH", libc::ESRCH as i32);
            define_int_prop(cx, os_obj.get(), "ESTALE", libc::ESTALE as i32);
            define_int_prop(cx, os_obj.get(), "ETIME", libc::ETIME as i32);
            define_int_prop(cx, os_obj.get(), "ETIMEDOUT", libc::ETIMEDOUT as i32);
            define_int_prop(cx, os_obj.get(), "ETXTBSY", libc::ETXTBSY as i32);
            define_int_prop(cx, os_obj.get(), "EWOULDBLOCK", libc::EWOULDBLOCK as i32);
            define_int_prop(cx, os_obj.get(), "EXDEV", libc::EXDEV as i32);

            // Priority constants (from sys/resource.h)
            define_int_prop(cx, os_obj.get(), "PRIORITY_LOW", 19);
            define_int_prop(cx, os_obj.get(), "PRIORITY_BELOW_NORMAL", 10);
            define_int_prop(cx, os_obj.get(), "PRIORITY_NORMAL", 0);
            define_int_prop(cx, os_obj.get(), "PRIORITY_ABOVE_NORMAL", -7);
            define_int_prop(cx, os_obj.get(), "PRIORITY_HIGH", -14);
            define_int_prop(cx, os_obj.get(), "PRIORITY_HIGHEST", -20);

            // signals sub-object
            rooted!(&in(cx) let signals_obj = w2::JS_NewPlainObject(cx));
            if !signals_obj.get().is_null() {
                define_int_prop(cx, signals_obj.get(), "SIGHUP", libc::SIGHUP as i32);
                define_int_prop(cx, signals_obj.get(), "SIGINT", libc::SIGINT as i32);
                define_int_prop(cx, signals_obj.get(), "SIGQUIT", libc::SIGQUIT as i32);
                define_int_prop(cx, signals_obj.get(), "SIGILL", libc::SIGILL as i32);
                define_int_prop(cx, signals_obj.get(), "SIGTRAP", libc::SIGTRAP as i32);
                define_int_prop(cx, signals_obj.get(), "SIGABRT", libc::SIGABRT as i32);
                define_int_prop(cx, signals_obj.get(), "SIGIOT", libc::SIGIOT as i32);
                define_int_prop(cx, signals_obj.get(), "SIGBUS", libc::SIGBUS as i32);
                define_int_prop(cx, signals_obj.get(), "SIGFPE", libc::SIGFPE as i32);
                define_int_prop(cx, signals_obj.get(), "SIGKILL", libc::SIGKILL as i32);
                define_int_prop(cx, signals_obj.get(), "SIGUSR1", libc::SIGUSR1 as i32);
                define_int_prop(cx, signals_obj.get(), "SIGSEGV", libc::SIGSEGV as i32);
                define_int_prop(cx, signals_obj.get(), "SIGUSR2", libc::SIGUSR2 as i32);
                define_int_prop(cx, signals_obj.get(), "SIGPIPE", libc::SIGPIPE as i32);
                define_int_prop(cx, signals_obj.get(), "SIGALRM", libc::SIGALRM as i32);
                define_int_prop(cx, signals_obj.get(), "SIGTERM", libc::SIGTERM as i32);
                define_int_prop(cx, signals_obj.get(), "SIGCHLD", libc::SIGCHLD as i32);
                define_int_prop(cx, signals_obj.get(), "SIGCONT", libc::SIGCONT as i32);
                define_int_prop(cx, signals_obj.get(), "SIGSTOP", libc::SIGSTOP as i32);
                define_int_prop(cx, signals_obj.get(), "SIGTSTP", libc::SIGTSTP as i32);
                define_int_prop(cx, signals_obj.get(), "SIGTTIN", libc::SIGTTIN as i32);
                define_int_prop(cx, signals_obj.get(), "SIGTTOU", libc::SIGTTOU as i32);
                define_int_prop(cx, signals_obj.get(), "SIGURG", libc::SIGURG as i32);
                define_int_prop(cx, signals_obj.get(), "SIGXCPU", libc::SIGXCPU as i32);
                define_int_prop(cx, signals_obj.get(), "SIGXFSZ", libc::SIGXFSZ as i32);
                define_int_prop(cx, signals_obj.get(), "SIGVTALRM", libc::SIGVTALRM as i32);
                define_int_prop(cx, signals_obj.get(), "SIGPROF", libc::SIGPROF as i32);
                define_int_prop(cx, signals_obj.get(), "SIGWINCH", libc::SIGWINCH as i32);
                define_int_prop(cx, signals_obj.get(), "SIGIO", libc::SIGIO as i32);
                define_int_prop(cx, signals_obj.get(), "SIGSYS", libc::SIGSYS as i32);

                rooted!(&in(cx) let sig_val = ObjectValue(signals_obj.get()));
                JS_DefineProperty(cx.raw_cx(), os_obj.handle().into(), c"signals".as_ptr(), sig_val.handle().into(), JSPROP_ENUMERATE as u32);
            }

            rooted!(&in(cx) let os_val = ObjectValue(os_obj.get()));
            JS_DefineProperty(cx.raw_cx(), constants_obj.handle().into(), c"os".as_ptr(), os_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // crypto sub-object (common constants)
        rooted!(&in(cx) let crypto_obj = w2::JS_NewPlainObject(cx));
        if !crypto_obj.get().is_null() {
            define_int_prop(cx, crypto_obj.get(), "OPENSSL_VERSION_NUMBER", 0);
            define_int_prop(cx, crypto_obj.get(), "SSL_OP_ALL", 0);

            rooted!(&in(cx) let crypto_val = ObjectValue(crypto_obj.get()));
            JS_DefineProperty(cx.raw_cx(), constants_obj.handle().into(), c"crypto".as_ptr(), crypto_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // zlib sub-object
        rooted!(&in(cx) let zlib_obj = w2::JS_NewPlainObject(cx));
        if !zlib_obj.get().is_null() {
            define_int_prop(cx, zlib_obj.get(), "Z_NO_FLUSH", 0);
            define_int_prop(cx, zlib_obj.get(), "Z_PARTIAL_FLUSH", 1);
            define_int_prop(cx, zlib_obj.get(), "Z_SYNC_FLUSH", 2);
            define_int_prop(cx, zlib_obj.get(), "Z_FULL_FLUSH", 3);
            define_int_prop(cx, zlib_obj.get(), "Z_FINISH", 4);
            define_int_prop(cx, zlib_obj.get(), "Z_BLOCK", 5);
            define_int_prop(cx, zlib_obj.get(), "Z_OK", 0);
            define_int_prop(cx, zlib_obj.get(), "Z_STREAM_END", 1);
            define_int_prop(cx, zlib_obj.get(), "Z_NEED_DICT", 2);
            define_int_prop(cx, zlib_obj.get(), "Z_ERRNO", -1);
            define_int_prop(cx, zlib_obj.get(), "Z_STREAM_ERROR", -2);
            define_int_prop(cx, zlib_obj.get(), "Z_DATA_ERROR", -3);
            define_int_prop(cx, zlib_obj.get(), "Z_MEM_ERROR", -4);
            define_int_prop(cx, zlib_obj.get(), "Z_BUF_ERROR", -5);
            define_int_prop(cx, zlib_obj.get(), "Z_VERSION_ERROR", -6);
            define_int_prop(cx, zlib_obj.get(), "Z_NO_COMPRESSION", 0);
            define_int_prop(cx, zlib_obj.get(), "Z_BEST_SPEED", 1);
            define_int_prop(cx, zlib_obj.get(), "Z_BEST_COMPRESSION", 9);
            define_int_prop(cx, zlib_obj.get(), "Z_DEFAULT_COMPRESSION", -1);
            define_int_prop(cx, zlib_obj.get(), "Z_DEFAULT_STRATEGY", 0);
            define_int_prop(cx, zlib_obj.get(), "Z_FILTERED", 1);
            define_int_prop(cx, zlib_obj.get(), "Z_HUFFMAN_ONLY", 2);
            define_int_prop(cx, zlib_obj.get(), "Z_RLE", 3);
            define_int_prop(cx, zlib_obj.get(), "Z_FIXED", 4);

            rooted!(&in(cx) let zlib_val = ObjectValue(zlib_obj.get()));
            JS_DefineProperty(cx.raw_cx(), constants_obj.handle().into(), c"zlib".as_ptr(), zlib_val.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    cache_builtin(cx, "constants", constants_obj.get());
}

/// Define an integer property on a JS object (enumerable).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_int_prop(cx: &mut mozjs::context::JSContext, obj_ptr: *mut JSObject, name: &str, val: i32) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let raw_cx = cx.raw_cx();
    rooted!(&in(cx) let obj = obj_ptr);
    rooted!(&in(cx) let v = Int32Value(val));
    JS_DefineProperty(raw_cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
}
