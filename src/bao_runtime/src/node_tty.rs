// @trace REQ-ENG-007
use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let tty_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if tty_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, tty_obj.handle(), c"isatty".as_ptr(), Some(tty_isatty), 1, 0);

        // ReadStream constructor
        let rs_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(tty_read_stream_ctor),
            1,
            JSFUN_CONSTRUCTOR,
            c"ReadStream".as_ptr(),
        );
        if !rs_fn.is_null() {
            let rs_obj = JS_GetFunctionObject(rs_fn);
            rooted!(&in(cx) let rs_val = ObjectValue(rs_obj));
            JS_DefineProperty(
                cx.raw_cx(),
                tty_obj.handle().into(),
                c"ReadStream".as_ptr(),
                rs_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // WriteStream constructor
        let ws_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(tty_write_stream_ctor),
            1,
            JSFUN_CONSTRUCTOR,
            c"WriteStream".as_ptr(),
        );
        if !ws_fn.is_null() {
            let ws_obj = JS_GetFunctionObject(ws_fn);
            rooted!(&in(cx) let ws_val = ObjectValue(ws_obj));
            JS_DefineProperty(
                cx.raw_cx(),
                tty_obj.handle().into(),
                c"WriteStream".as_ptr(),
                ws_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    cache_builtin(cx, "tty", tty_obj.get());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tty_isatty(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() } else { -1 }
    } else {
        -1
    };
    let result = if fd >= 0 { libc::isatty(fd) } else { 0 };
    args.rval().set(BooleanValue(result == 1));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tty_read_stream_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() } else { 0 }
    } else {
        0
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // fd
    rooted!(&in(cx_ref) let fv = Int32Value(fd));
    JS_DefineProperty(cx, obj.handle().into(), c"fd".as_ptr(), fv.handle().into(), JSPROP_ENUMERATE as u32);

    // isTTY
    rooted!(&in(cx_ref) let tv = BooleanValue(libc::isatty(fd) == 1));
    JS_DefineProperty(cx, obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);

    // isRaw
    rooted!(&in(cx_ref) let rv = BooleanValue(false));
    JS_DefineProperty(cx, obj.handle().into(), c"isRaw".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);

    // readable
    rooted!(&in(cx_ref) let readable_v = BooleanValue(true));
    JS_DefineProperty(cx, obj.handle().into(), c"readable".as_ptr(), readable_v.handle().into(), JSPROP_ENUMERATE as u32);

    w2::JS_DefineFunction(cx_ref, obj.handle(), c"setRawMode".as_ptr(), Some(tty_set_raw_mode), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"ref".as_ptr(), Some(tty_noop), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"unref".as_ptr(), Some(tty_noop), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"on".as_ptr(), Some(tty_noop), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"emit".as_ptr(), Some(tty_noop), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"resume".as_ptr(), Some(tty_noop), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"pause".as_ptr(), Some(tty_noop), 0, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"destroy".as_ptr(), Some(tty_noop), 0, 0);

    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tty_write_stream_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() } else { 1 }
    } else {
        1
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // fd
    rooted!(&in(cx_ref) let fv = Int32Value(fd));
    JS_DefineProperty(cx, obj.handle().into(), c"fd".as_ptr(), fv.handle().into(), JSPROP_ENUMERATE as u32);

    let is_tty = libc::isatty(fd) == 1;

    // isTTY
    rooted!(&in(cx_ref) let tv = BooleanValue(is_tty));
    JS_DefineProperty(cx, obj.handle().into(), c"isTTY".as_ptr(), tv.handle().into(), JSPROP_ENUMERATE as u32);

    // columns / rows from ioctl TIOCGWINSZ
    let mut ws: libc::winsize = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    let has_size = is_tty && libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0;
    if has_size {
        rooted!(&in(cx_ref) let cols = Int32Value(ws.ws_col as i32));
        JS_DefineProperty(cx, obj.handle().into(), c"columns".as_ptr(), cols.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(cx_ref) let rows = Int32Value(ws.ws_row as i32));
        JS_DefineProperty(cx, obj.handle().into(), c"rows".as_ptr(), rows.handle().into(), JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx_ref, obj.handle(), c"getWindowSize".as_ptr(), Some(tty_get_window_size), 0, JSPROP_ENUMERATE as u32);
    } else {
        let undef = UndefinedValue();
        rooted!(&in(cx_ref) let cols = undef);
        JS_DefineProperty(cx, obj.handle().into(), c"columns".as_ptr(), cols.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(cx_ref) let rows = undef);
        JS_DefineProperty(cx, obj.handle().into(), c"rows".as_ptr(), rows.handle().into(), JSPROP_ENUMERATE as u32);
    }

    w2::JS_DefineFunction(cx_ref, obj.handle(), c"clearLine".as_ptr(), Some(tty_noop), 2, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"clearScreenDown".as_ptr(), Some(tty_noop), 1, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"cursorTo".as_ptr(), Some(tty_noop), 3, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"moveCursor".as_ptr(), Some(tty_noop), 3, 0);
    w2::JS_DefineFunction(cx_ref, obj.handle(), c"write".as_ptr(), Some(tty_write_stream_write), 1, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tty_set_raw_mode(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Get fd from this object
    let mut fd_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"fd".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fd_val },
    );
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    let raw_flag = if argc > 0 {
        let v = *args.get(0).ptr;
        v.to_boolean()
    } else {
        true
    };

    if fd < 0 {
        JS_ReportErrorUTF8(cx, b"setRawMode: invalid fd\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    let mut term: libc::termios = ::std::mem::zeroed();
    if libc::tcgetattr(fd, &mut term) != 0 {
        JS_ReportErrorUTF8(cx, b"setRawMode: tcgetattr failed\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    if raw_flag {
        libc::cfmakeraw(&mut term);
    } else {
        libc::cfmakeraw(&mut term);
        term.c_iflag |= libc::ICRNL;
        term.c_oflag |= libc::OPOST | libc::ONLCR;
        term.c_lflag |= libc::ICANON | libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ISIG;
    }

    if libc::tcsetattr(fd, libc::TCSANOW, &term) != 0 {
        JS_ReportErrorUTF8(cx, b"setRawMode: tcsetattr failed\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }

    // Update isRaw on the object
    rooted!(&in(cx_ref) let rv = BooleanValue(raw_flag));
    JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this }, c"isRaw".as_ptr(), rv.handle().into(), JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(this));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tty_get_window_size(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv().to_object();

    let mut fd_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"fd".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fd_val },
    );
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { -1 };

    let mut ws: libc::winsize = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    if fd >= 0 && libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0 {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, 2));
        if !arr.get().is_null() {
            rooted!(&in(cx_ref) let cv = Int32Value(ws.ws_col as i32));
            JS_DefineElement(cx, arr.handle().into(), 0, cv.handle().into(), JSPROP_ENUMERATE as u32);
            rooted!(&in(cx_ref) let rv = Int32Value(ws.ws_row as i32));
            JS_DefineElement(cx, arr.handle().into(), 1, rv.handle().into(), JSPROP_ENUMERATE as u32);
            args.rval().set(ObjectValue(arr.get()));
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn tty_write_stream_write(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();

    let mut fd_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"fd".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fd_val },
    );
    let fd = if fd_val.is_int32() { fd_val.to_int32() } else { 1 };

    if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            let s = jsstr_to_string(cx, NonNull::new_unchecked(v.to_string()));
            let bytes = s.as_bytes();
            libc::write(fd, bytes.as_ptr() as *const ::std::os::raw::c_void, bytes.len());
        }
    }
    args.rval().set(BooleanValue(true));
    true
}

unsafe extern "C" fn tty_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}
