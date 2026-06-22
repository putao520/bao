// @trace REQ-ENG-006 [api:node:http2]
use bun_core::ZBox;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, ObjectValue, Int32Value, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let http2_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if http2_obj.get().is_null() {
        return;
    }

    unsafe {
        // Server creation
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"createServer".as_ptr(), Some(http2_create_server), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"createSecureServer".as_ptr(), Some(http2_create_secure_server), 1, JSPROP_ENUMERATE as u32);

        // Client connection
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"connect".as_ptr(), Some(http2_connect), 1, JSPROP_ENUMERATE as u32);

        // Constants
        define_int_prop(cx, http2_obj.get(), "HEADER_STATUS", 0x01);
        define_int_prop(cx, http2_obj.get(), "HEADER_METHOD", 0x02);
        define_int_prop(cx, http2_obj.get(), "HEADER_PATH", 0x04);
        define_int_prop(cx, http2_obj.get(), "HEADER_AUTHORITY", 0x08);
        define_int_prop(cx, http2_obj.get(), "HEADER_SCHEME", 0x10);

        define_int_prop(cx, http2_obj.get(), "NGHTTP2_NO_ERROR", 0);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_PROTOCOL_ERROR", 1);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_INTERNAL_ERROR", 2);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FLOW_CONTROL_ERROR", 3);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_SETTINGS_TIMEOUT", 4);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_STREAM_CLOSED", 5);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_FRAME_SIZE_ERROR", 6);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_REFUSED_STREAM", 7);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_CANCEL", 8);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_COMPRESSION_ERROR", 9);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_CONNECT_ERROR", 10);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_ENHANCE_YOUR_CALM", 11);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_INADEQUATE_SECURITY", 12);
        define_int_prop(cx, http2_obj.get(), "NGHTTP2_HTTP_1_1_REQUIRED", 13);

        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_STATUS", 0x01);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_METHOD", 0x02);
        define_int_prop(cx, http2_obj.get(), "HTTP2_HEADER_PATH", 0x04);

        // Default settings
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_HEADER_TABLE_SIZE", 4096);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_ENABLE_PUSH", 1);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_INITIAL_WINDOW_SIZE", 65535);
        define_int_prop(cx, http2_obj.get(), "DEFAULT_SETTINGS_MAX_FRAME_SIZE", 16384);

        // Http2Session class
        let session_fn = JS_NewFunction(cx.raw_cx(), Some(http2_session_constructor), 0, 0x400, c"Http2Session".as_ptr());
        if !session_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(session_fn);
            rooted!(&in(cx) let fn_root = fn_obj);
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(cx, proto.handle(), c"request".as_ptr(), Some(http2_session_request), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"respond".as_ptr(), Some(http2_session_respond), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"close".as_ptr(), Some(http2_session_close), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"destroy".as_ptr(), Some(http2_session_destroy), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"ping".as_ptr(), Some(http2_session_ping), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"settings".as_ptr(), Some(http2_session_settings), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"goaway".as_ptr(), Some(http2_session_goaway), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"ref".as_ptr(), Some(http2_session_noop), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"unref".as_ptr(), Some(http2_session_noop), 0, JSPROP_ENUMERATE as u32);

                // Session properties
                w2::JS_DefineFunction(cx, proto.handle(), c"state".as_ptr(), Some(http2_session_state), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"remoteSettings".as_ptr(), Some(http2_session_remote_settings), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"localSettings".as_ptr(), Some(http2_session_local_settings), 0, JSPROP_ENUMERATE as u32);

                rooted!(&in(cx) let proto_val = ObjectValue(proto.get()));
                JS_DefineProperty(cx.raw_cx(), fn_root.handle().into(), c"prototype".as_ptr(), proto_val.handle().into(), 0u32);
            }
            rooted!(&in(cx) let fn_val = ObjectValue(fn_root.get()));
            JS_DefineProperty(cx.raw_cx(), http2_obj.handle().into(), c"Http2Session".as_ptr(), fn_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // Http2Stream class
        let stream_fn = JS_NewFunction(cx.raw_cx(), Some(http2_stream_constructor), 0, 0x400, c"Http2Stream".as_ptr());
        if !stream_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(stream_fn);
            rooted!(&in(cx) let fn_root = fn_obj);
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(cx, proto.handle(), c"respond".as_ptr(), Some(http2_session_respond), 1, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"end".as_ptr(), Some(http2_stream_end), 0, JSPROP_ENUMERATE as u32);
                w2::JS_DefineFunction(cx, proto.handle(), c"close".as_ptr(), Some(http2_session_close), 0, JSPROP_ENUMERATE as u32);

                rooted!(&in(cx) let proto_val = ObjectValue(proto.get()));
                JS_DefineProperty(cx.raw_cx(), fn_root.handle().into(), c"prototype".as_ptr(), proto_val.handle().into(), 0u32);
            }
            rooted!(&in(cx) let fn_val = ObjectValue(fn_root.get()));
            JS_DefineProperty(cx.raw_cx(), http2_obj.handle().into(), c"Http2Stream".as_ptr(), fn_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // getPackedSettings / getUnpackedSettings
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"getPackedSettings".as_ptr(), Some(http2_get_packed_settings), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, http2_obj.handle(), c"getUnpackedSettings".as_ptr(), Some(http2_get_unpacked_settings), 1, JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "http2", http2_obj.get());
}

// --- Server creation ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_create_server(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let server_obj = w2::JS_NewPlainObject(cx_ref));
    if server_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Copy EE methods from node_events
    let ee_on: JSNative = Some(crate::node_events::ee_on);
    let ee_emit: JSNative = Some(crate::node_events::ee_emit);
    let ee_once: JSNative = Some(crate::node_events::ee_once);
    let ee_off: JSNative = Some(crate::node_events::ee_off);

    for (name, op) in [("on", ee_on), ("once", ee_once), ("emit", ee_emit), ("off", ee_off), ("addListener", ee_on), ("removeListener", ee_off)] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        mozjs_sys::jsapi::JS_DefineFunction(cx, server_obj.handle().into(), c_name.as_ptr(), op, 2, JSPROP_ENUMERATE as u32);
    }

    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"listen".as_ptr(), Some(http2_server_listen), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"close".as_ptr(), Some(http2_session_close), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, server_obj.handle(), c"setTimeout".as_ptr(), Some(http2_session_noop), 1, JSPROP_ENUMERATE as u32);

    // If a callback was provided, call it with 'session' event
    if argc > 0 {
        let cb_val = *args.get(0).ptr;
        if cb_val.is_object() {
            rooted!(&in(cx_ref) let cb = cb_val.to_object());
            rooted!(&in(cx_ref) let cb_val2 = ObjectValue(cb.get()));
            JS_DefineProperty(cx, server_obj.handle().into(), c"_onRequest".as_ptr(), cb_val2.handle().into(), 0u32);
        }
    }

    args.rval().set(ObjectValue(server_obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_create_secure_server(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    http2_create_server(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_connect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let session = w2::JS_NewPlainObject(cx_ref));
    if session.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
    rooted!(&in(cx_ref) let id_val = Int32Value(session_id as i32));
    JS_DefineProperty(cx, session.handle().into(), c"_sessionId".as_ptr(), id_val.handle().into(), 0u32);

    // Copy session methods
    w2::JS_DefineFunction(cx_ref, session.handle(), c"request".as_ptr(), Some(http2_session_request), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"close".as_ptr(), Some(http2_session_close), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"destroy".as_ptr(), Some(http2_session_destroy), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"ping".as_ptr(), Some(http2_session_ping), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"settings".as_ptr(), Some(http2_session_settings), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"goaway".as_ptr(), Some(http2_session_goaway), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"ref".as_ptr(), Some(http2_session_noop), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, session.handle(), c"unref".as_ptr(), Some(http2_session_noop), 0, JSPROP_ENUMERATE as u32);

    // EE methods
    let ee_on: JSNative = Some(crate::node_events::ee_on);
    let ee_emit: JSNative = Some(crate::node_events::ee_emit);
    let ee_off: JSNative = Some(crate::node_events::ee_off);
    for (name, op) in [("on", ee_on), ("emit", ee_emit), ("off", ee_off)] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        mozjs_sys::jsapi::JS_DefineFunction(cx, session.handle().into(), c_name.as_ptr(), op, 2, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(ObjectValue(session.get()));
    true
}

// --- Session/Stream constructors ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_constructor(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
    rooted!(&in(cx_ref) let id_val = Int32Value(session_id as i32));
    JS_DefineProperty(cx, obj.handle().into(), c"_sessionId".as_ptr(), id_val.handle().into(), 0u32);
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_stream_constructor(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
    if obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

// --- Session methods ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_request(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let stream = w2::JS_NewPlainObject(cx_ref));
    if stream.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    w2::JS_DefineFunction(cx_ref, stream.handle(), c"respond".as_ptr(), Some(http2_session_respond), 1, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, stream.handle(), c"end".as_ptr(), Some(http2_stream_end), 0, JSPROP_ENUMERATE as u32);
    w2::JS_DefineFunction(cx_ref, stream.handle(), c"close".as_ptr(), Some(http2_session_close), 0, JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(stream.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_respond(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_close(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_destroy(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_ping(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_settings(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_goaway(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_state(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let state = w2::JS_NewPlainObject(cx_ref));
    if state.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    // Root all property values before passing to JS_DefineProperty
    rooted!(&in(cx_ref) let eff_loc_w = Int32Value(65535));
    rooted!(&in(cx_ref) let eff_recv = Int32Value(0));
    rooted!(&in(cx_ref) let next_stream = Int32Value(1));
    rooted!(&in(cx_ref) let last_proc = Int32Value(0));
    rooted!(&in(cx_ref) let outbound_q = Int32Value(0));
    rooted!(&in(cx_ref) let deflate_sz = Int32Value(0));
    rooted!(&in(cx_ref) let inflate_sz = Int32Value(0));

    JS_DefineProperty(cx, state.handle().into(), c"effectiveLocalWindowSize".as_ptr(), eff_loc_w.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"effectiveRecvDataLength".as_ptr(), eff_recv.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"nextStreamID".as_ptr(), next_stream.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"localWindowSize".as_ptr(), eff_loc_w.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"lastProcStreamID".as_ptr(), last_proc.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"remoteWindowSize".as_ptr(), eff_loc_w.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"outboundQueueSize".as_ptr(), outbound_q.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"deflateDynamicTableSize".as_ptr(), deflate_sz.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineProperty(cx, state.handle().into(), c"inflateDynamicTableSize".as_ptr(), inflate_sz.handle().into(), JSPROP_ENUMERATE as u32);
    args.rval().set(ObjectValue(state.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_remote_settings(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let settings = w2::JS_NewPlainObject(cx_ref));
    if settings.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    define_int_prop(cx_ref, settings.get(), "headerTableSize", 4096);
    define_int_prop(cx_ref, settings.get(), "enablePush", 0);
    define_int_prop(cx_ref, settings.get(), "initialWindowSize", 65535);
    define_int_prop(cx_ref, settings.get(), "maxFrameSize", 16384);
    define_int_prop(cx_ref, settings.get(), "maxConcurrentStreams", 100);
    define_int_prop(cx_ref, settings.get(), "maxHeaderListSize", 65535);
    args.rval().set(ObjectValue(settings.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_session_local_settings(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    http2_session_remote_settings(cx, _argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_server_listen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this_val = args.thisv();
    if this_val.is_object() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let this_obj = this_val.to_object());
        rooted!(&in(cx_ref) let listening = BooleanValue(true));
        JS_DefineProperty(cx, this_obj.handle().into(), c"_listening".as_ptr(), listening.handle().into(), 0u32);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_stream_end(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// --- Utility ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_get_packed_settings(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let buf = crate::globals::create_buffer_object(_cx, &[]);
    if buf.is_null() {
        args.rval().set(UndefinedValue());
    } else {
        args.rval().set(ObjectValue(buf));
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn http2_get_unpacked_settings(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let settings = w2::JS_NewPlainObject(cx_ref));
    if settings.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    define_int_prop(cx_ref, settings.get(), "headerTableSize", 4096);
    define_int_prop(cx_ref, settings.get(), "enablePush", 0);
    define_int_prop(cx_ref, settings.get(), "initialWindowSize", 65535);
    define_int_prop(cx_ref, settings.get(), "maxFrameSize", 16384);
    args.rval().set(ObjectValue(settings.get()));
    true
}

// --- Property helpers ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_int_prop(cx: &mut mozjs::context::JSContext, obj_ptr: *mut JSObject, name: &str, val: i32) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let raw_cx = cx.raw_cx();
    rooted!(&in(cx) let obj = obj_ptr);
    rooted!(&in(cx) let v = Int32Value(val));
    JS_DefineProperty(raw_cx, obj.handle().into(), c_name.as_ptr(), v.handle().into(), JSPROP_ENUMERATE as u32);
}
