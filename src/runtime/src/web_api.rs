// @trace REQ-ENG-006
// WebSocket + Performance + TextEncoder/TextDecoder + atob/btoa + queueMicrotask
// D5根治: WebSocket客户端 std::net::TcpStream → bun_uws_sys::SocketGroup::connect
// 两阶段架构: WsClientUpgrade(HTTP握手) → adopt → WsClient(帧解析+事件分发)
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ptr::NonNull;
use ::std::sync::OnceLock;

use ::std::ffi::c_int;
use core::ffi::c_void;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject, NewArrayObject1, CallOriginalPromiseResolve, CallOriginalPromiseThen};
use mozjs::conversions::jsstr_to_string;

use bun_uws_sys::{
    SocketGroup, SocketKind, Loop as UwsLoop, us_socket_t,
    vtable, socket_group::VTable, ConnectResult,
};
use bun_uws_sys::socket::SocketTcp as Socket;
use bun_socket_dispatch::register_kind;

// ── WebSocket client: two-phase uSockets architecture ──
//
// Phase A (WsClientUpgrade): SocketGroup::connect sends HTTP upgrade request.
//   on_open: write "GET / HTTP/1.1\r\nUpgrade: websocket\r\n..."
//   on_data: parse "HTTP/1.1 101" response, then adopt to WsClient group
//
// Phase B (WsClient): after adopt, on_data receives raw TCP bytes.
//   Rust frame parser extracts opcode/payload, dispatches JS events.

// ──────────────────── Per-socket extension data ────────────────────

/// Extension for WsClientUpgrade sockets (HTTP handshake phase).
#[repr(C)]
struct WsUpgradeSocketExt {
    js_obj: *mut JSObject,
    cx: *mut JSContext,
    is_tls: bool,
    /// HTTP response buffer (may arrive in multiple on_data calls).
    http_buf_len: usize,
    http_buf_cap: usize,
    http_buf: *mut u8,
    /// Whether we already sent the upgrade request.
    upgrade_sent: bool,
    /// Whether upgrade completed (101 received).
    upgrade_complete: bool,
}

/// Extension for WsClient sockets (WebSocket data phase).
#[repr(C)]
struct WsClientSocketExt {
    js_obj: *mut JSObject,
    cx: *mut JSContext,
    ready_state: u8, // 0=CONNECTING, 1=OPEN, 2=CLOSING, 3=CLOSED
    is_tls: bool,
    /// Frame parse state.
    recv_state: WsRecvState,
    /// Incomplete frame buffer (for TCP segmentation).
    recv_buf_len: usize,
    recv_buf_cap: usize,
    recv_buf: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
enum WsRecvState {
    #[default]
    NeedHeader,
    NeedExtLen16,
    NeedExtLen64,
    NeedBody,
}

// ──────────────────── WsUpgradeHandler ────────────────────

struct WsUpgradeHandler;

impl vtable::Handler for WsUpgradeHandler {
    type Ext = WsUpgradeSocketExt;

    const HAS_ON_OPEN: bool = true;
    const HAS_ON_DATA: bool = true;
    const HAS_ON_CLOSE: bool = true;
    const HAS_ON_CONNECT_ERROR: bool = true;
    const HAS_ON_WRITABLE: bool = true;
    const HAS_ON_HANDSHAKE: bool = true;

    fn on_open(ext: &mut WsUpgradeSocketExt, s: *mut us_socket_t, _is_client: bool, _ip: &[u8]) {
        if ext.upgrade_sent {
            return;
        }
        ext.upgrade_sent = true;

        // Read URL info stored on the JS object to construct the upgrade request.
        let cx = ext.cx;
        let js_obj = ext.js_obj;

        let (host, path, key) = unsafe {
            let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &js_obj };

            // Read _wsHost, _wsPort, _wsPath from JS object
            let mut host_val = UndefinedValue();
            JS_GetProperty(cx, obj_h, c"_wsHost".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut host_val });
            let host = if host_val.is_string() {
                crate::js_to_rust_string(cx, host_val)
            } else {
                return;
            };

            let mut path_val = UndefinedValue();
            JS_GetProperty(cx, obj_h, c"_wsPath".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut path_val });
            let path = if path_val.is_string() {
                crate::js_to_rust_string(cx, path_val)
            } else {
                "/".to_string()
            };

            let mut key_val = UndefinedValue();
            JS_GetProperty(cx, obj_h, c"_wsKey".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut key_val });
            let key = if key_val.is_string() {
                crate::js_to_rust_string(cx, key_val)
            } else {
                return;
            };
            (host, path, key)
        };

        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            path, host, key
        );

        let socket = Socket::from(s);
        socket.write(request.as_bytes());
    }

    fn on_data(ext: &mut WsUpgradeSocketExt, s: *mut us_socket_t, data: &[u8]) {
        if ext.upgrade_complete {
            return;
        }

        // Buffer the HTTP response (may arrive in chunks).
        append_to_buf(&mut ext.http_buf, &mut ext.http_buf_len, &mut ext.http_buf_cap, data);

        // Parse HTTP response using bun_picohttp (replaces hand-written find_header_end + starts_with).
        let buf = unsafe { ::std::slice::from_raw_parts(ext.http_buf, ext.http_buf_len) };
        let mut headers = [bun_picohttp::Header::ZERO; 16];
        match bun_picohttp::Response::parse(buf, &mut headers) {
            Ok(response) => {
                if response.status_code != 101 {
                    // Upgrade failed — server did not return Switching Protocols.
                    unsafe { ws_trigger_event(ext.cx, ext.js_obj, "onerror", None); }
                    let socket = Socket::from(s);
                    socket.close(bun_uws_sys::CloseCode::Normal);
                    return;
                }
                ext.upgrade_complete = true;

                // Set readyState = OPEN on JS object.
                let cx = ext.cx;
                let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ext.js_obj };
                let open_val = Int32Value(1);
                let open_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &open_val };
                unsafe { JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), open_h); }

                // Adopt the socket to the WsClient group.
                let kind = if ext.is_tls { SocketKind::WsClientTls } else { SocketKind::WsClient };
                let client_group = get_or_create_ws_client_group(kind);
                let ext_size = ::std::mem::size_of::<WsClientSocketExt>() as c_int;

                let new_ext = WsClientSocketExt {
                    js_obj: ext.js_obj,
                    cx: ext.cx,
                    ready_state: 1, // OPEN
                    is_tls: ext.is_tls,
                    recv_state: WsRecvState::NeedHeader,
                    recv_buf_len: 0,
                    recv_buf_cap: 0,
                    recv_buf: ::std::ptr::null_mut(),
                };

                let new_ext_box = Box::new(new_ext);
                let new_ext_ptr = Box::into_raw(new_ext_box);

                let adopted = unsafe {
                    bun_uws_sys::us_socket_t::opaque_mut(s);
                    sock_adopt(
                        s,
                        client_group,
                        kind as u8,
                        ext_size,
                        ext_size,
                    )
                };

                if !adopted.is_null() {
                    let ext_ptr = bun_uws_sys::us_socket_t::opaque_mut(adopted).ext::<*mut c_void>();
                    *ext_ptr = new_ext_ptr as *mut c_void;

                    let ptr_as_f64 = adopted as usize as f64;
                    let ptr_val2 = mozjs::jsval::DoubleValue(ptr_as_f64);
                    let ptr_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ptr_val2 };
                    unsafe { JS_DefineProperty(cx, obj_h, c"_wsPtr".as_ptr(), ptr_h, 0); }

                    WS_SOCKETS.with(|m| m.borrow_mut().insert(adopted as usize, adopted));
                    unsafe { ws_trigger_event(cx, ext.js_obj, "onopen", None); }
                } else {
                    unsafe { ws_trigger_event(cx, ext.js_obj, "onerror", None); }
                    unsafe { drop(Box::from_raw(new_ext_ptr)); }
                }

                free_buf(&mut ext.http_buf, &mut ext.http_buf_cap);
            }
            Err(bun_picohttp::ParseResponseError::ShortRead) => {
                // Need more data — wait for next on_data callback.
            }
            Err(_) => {
                // Malformed HTTP response.
                unsafe { ws_trigger_event(ext.cx, ext.js_obj, "onerror", None); }
                let socket = Socket::from(s);
                socket.close(bun_uws_sys::CloseCode::Normal);
            }
        }
    }

    fn on_close(ext: &mut WsUpgradeSocketExt, _s: *mut us_socket_t, _code: i32, _reason: Option<*mut c_void>) {
        if !ext.upgrade_complete {
            unsafe { ws_trigger_event(ext.cx, ext.js_obj, "onerror", None); }
        }
        free_buf(&mut ext.http_buf, &mut ext.http_buf_cap);
    }

    fn on_connect_error(ext: &mut WsUpgradeSocketExt, _s: *mut us_socket_t, _code: i32) {
        unsafe { ws_trigger_event(ext.cx, ext.js_obj, "onerror", None); }
        free_buf(&mut ext.http_buf, &mut ext.http_buf_cap);
    }

    fn on_writable(_ext: &mut WsUpgradeSocketExt, _s: *mut us_socket_t) {}
    fn on_handshake(_ext: &mut WsUpgradeSocketExt, _s: *mut us_socket_t, _ok: bool, _err: bun_uws_sys::us_bun_verify_error_t) {}
}

// ──────────────────── WsClientHandler ────────────────────

struct WsClientHandler;

impl vtable::Handler for WsClientHandler {
    type Ext = WsClientSocketExt;

    const HAS_ON_DATA: bool = true;
    const HAS_ON_CLOSE: bool = true;
    const HAS_ON_WRITABLE: bool = true;

    fn on_data(ext: &mut WsClientSocketExt, s: *mut us_socket_t, data: &[u8]) {
        if ext.ready_state != 1 {
            return; // Not OPEN, ignore data.
        }

        // Append new data to recv buffer.
        append_to_buf(&mut ext.recv_buf, &mut ext.recv_buf_len, &mut ext.recv_buf_cap, data);

        // Try to parse complete frames from the buffer using WebsocketHeader API.
        loop {
            let buf = unsafe { ::std::slice::from_raw_parts(ext.recv_buf, ext.recv_buf_len) };
            if buf.len() < 2 {
                break; // Need at least 2 bytes for header.
            }

            let header = bun_http::websocket::WebsocketHeader::from_slice([buf[0], buf[1]]);
            let opcode = header.opcode() as u8;
            let masked = header.mask();
            let payload_len_byte = header.len();

            // Decode extended payload length.
            let (payload_len, header_size) = match payload_len_byte {
                126 => {
                    if buf.len() < 4 { break; }
                    (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4)
                },
                127 => {
                    if buf.len() < 10 { break; }
                    (u64::from_be_bytes([buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8], buf[9]]), 10)
                },
                _ => (payload_len_byte as u64, 2),
            };

            let mask_size = if masked { 4usize } else { 0usize };
            let total_frame = header_size + mask_size + payload_len as usize;

            if buf.len() < total_frame {
                break; // Incomplete frame, wait for more data.
            }

            let payload_start = header_size + mask_size;
            let payload_end = payload_start + payload_len as usize;
            let mut payload = buf[payload_start..payload_end].to_vec();

            // Unmask if needed (server→client frames shouldn't be masked per RFC 6455,
            // but handle it defensively).
            if masked {
                let mask_key = &buf[header_size..header_size + 4];
                for (i, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask_key[i % 4];
                }
            }

            // Consume bytes from recv buffer.
            let remaining = ext.recv_buf_len - total_frame;
            if remaining > 0 {
                unsafe {
                    ::std::ptr::copy(ext.recv_buf.add(total_frame), ext.recv_buf, remaining);
                }
            }
            ext.recv_buf_len = remaining;

            // Dispatch based on opcode (using bun_http::websocket::Opcode for clarity).
            let cx = ext.cx;
            match opcode {
                0x1 => {
                    // Text frame.
                    let text = String::from_utf8_lossy(&payload);
                    let c_text = bun_core::ZBox::from_bytes(text.as_bytes());
                    unsafe {
                        let js_str = JS_NewStringCopyZ(cx, c_text.as_ptr());
                        if !js_str.is_null() {
                            let dv = StringValue(&*js_str);
                            ws_trigger_event(cx, ext.js_obj, "onmessage", Some(dv));
                        }
                    }
                }
                0x2 => {
                    // Binary frame — wrap in Uint8Array.
                    let data_val = unsafe { make_array_buffer_value(cx, &payload) };
                    unsafe { ws_trigger_event(cx, ext.js_obj, "onmessage", Some(data_val)); }
                }
                0x8 => {
                    // Close frame.
                    ext.ready_state = 3;
                    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ext.js_obj };
                    let closed_val = Int32Value(3);
                    let closed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closed_val };
                    unsafe { JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), closed_h); }
                    unsafe { ws_trigger_event(cx, ext.js_obj, "onclose", None); }
                    WS_SOCKETS.with(|m| m.borrow_mut().remove(&(s as usize)));
                    let socket = Socket::from(s);
                    socket.close(bun_uws_sys::CloseCode::Normal);
                    free_buf(&mut ext.recv_buf, &mut ext.recv_buf_cap);
                    return;
                }
                0x9 => {
                    // Ping — auto-reply with Pong.
                    let pong_frame = build_pong_frame(&payload);
                    let socket = Socket::from(s);
                    socket.write(&pong_frame);
                }
                0xA => {
                    // Pong — ignore.
                }
                _ => {} // Reserved/continuation — ignore.
            }
        }
    }

    fn on_close(ext: &mut WsClientSocketExt, s: *mut us_socket_t, _code: i32, _reason: Option<*mut c_void>) {
        ext.ready_state = 3;
        let cx = ext.cx;
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ext.js_obj };
        let closed_val = Int32Value(3);
        let closed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closed_val };
        unsafe { JS_SetProperty(cx, obj_h, c"readyState".as_ptr(), closed_h); }
        unsafe { ws_trigger_event(cx, ext.js_obj, "onclose", None); }
        WS_SOCKETS.with(|m| m.borrow_mut().remove(&(s as usize)));
        free_buf(&mut ext.recv_buf, &mut ext.recv_buf_cap);
    }

    fn on_writable(_ext: &mut WsClientSocketExt, _s: *mut us_socket_t) {}
}

// ──────────────────── VTable registration ────────────────────

static WS_UPGRADE_VTABLE: OnceLock<&'static VTable> = OnceLock::new();
static WS_CLIENT_VTABLE: OnceLock<&'static VTable> = OnceLock::new();

fn ws_upgrade_vtable() -> &'static VTable {
    WS_UPGRADE_VTABLE.get_or_init(vtable::make::<WsUpgradeHandler>)
}
fn ws_client_vtable() -> &'static VTable {
    WS_CLIENT_VTABLE.get_or_init(vtable::make::<WsClientHandler>)
}

// ──────────────────── Thread-local state ────────────────────

thread_local! {
    static WS_SOCKETS: RefCell<HashMap<usize, *mut us_socket_t>> = RefCell::new(HashMap::new());
    static WS_UPGRADE_GROUPS: RefCell<HashMap<u8, Box<SocketGroup>>> = RefCell::new(HashMap::new());
    static WS_CLIENT_GROUPS: RefCell<HashMap<u8, Box<SocketGroup>>> = RefCell::new(HashMap::new());
}

fn get_or_create_ws_upgrade_group(kind: SocketKind) -> *mut SocketGroup {
    let key = kind as u8;
    WS_UPGRADE_GROUPS.with(|g| {
        let mut groups = g.borrow_mut();
        groups.entry(key).or_insert_with(|| {
            let mut group = Box::new(SocketGroup::default());
            let loop_ptr = get_uws_loop();
            group.init(loop_ptr, Some(ws_upgrade_vtable()), ::std::ptr::null_mut());
            group
        }).as_mut() as *mut SocketGroup
    })
}

fn get_or_create_ws_client_group(kind: SocketKind) -> *mut SocketGroup {
    let key = kind as u8;
    WS_CLIENT_GROUPS.with(|g| {
        let mut groups = g.borrow_mut();
        groups.entry(key).or_insert_with(|| {
            let mut group = Box::new(SocketGroup::default());
            let loop_ptr = get_uws_loop();
            group.init(loop_ptr, Some(ws_client_vtable()), ::std::ptr::null_mut());
            group
        }).as_mut() as *mut SocketGroup
    })
}

fn get_uws_loop() -> *mut UwsLoop {
    bao_uloop::uws_get_loop()
}

// ──────────────────── Helper functions ────────────────────

fn append_to_buf(buf: &mut *mut u8, len: &mut usize, cap: &mut usize, data: &[u8]) {
    let new_len = *len + data.len();
    if new_len > *cap {
        let new_cap = (new_len * 2).max(256);
        let new_buf = if *cap > 0 && !buf.is_null() {
            unsafe { ::std::alloc::realloc(*buf, ::std::alloc::Layout::from_size_align(*cap, 1).unwrap(), new_cap) }
        } else {
            unsafe { ::std::alloc::alloc(::std::alloc::Layout::from_size_align(new_cap, 1).unwrap()) }
        };
        *buf = new_buf;
        *cap = new_cap;
    }
    if !buf.is_null() && !data.is_empty() {
        unsafe { ::std::ptr::copy_nonoverlapping(data.as_ptr(), buf.add(*len), data.len()); }
    }
    *len = new_len;
}

fn free_buf(buf: &mut *mut u8, cap: &mut usize) {
    if *cap > 0 && !buf.is_null() {
        unsafe { ::std::alloc::dealloc(*buf, ::std::alloc::Layout::from_size_align(*cap, 1).unwrap()); }
    }
    *buf = ::std::ptr::null_mut();
    *cap = 0;
}

fn build_pong_frame(ping_data: &[u8]) -> Vec<u8> {
    use bun_http::websocket::{Opcode, WebsocketHeader};

    let mut header = WebsocketHeader::new(WebsocketHeader::pack_length(ping_data.len()), false, Opcode::Pong);
    header.set_final(true);

    let cap = WebsocketHeader::frame_size(ping_data.len());
    let mut frame = Vec::with_capacity(cap);
    header.write_header(&mut frame, ping_data.len()).unwrap();
    frame.extend_from_slice(ping_data);
    frame
}

unsafe fn make_array_buffer_value(cx: *mut JSContext, data: &[u8]) -> JSVal {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr = NewArrayObject1(&mut wrapped_cx, data.len()));
    for (i, &byte) in data.iter().enumerate() {
        let val = Int32Value(byte as i32);
        rooted!(&in(wrapped_cx) let v = val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }
    ObjectValue(arr.get())
}

/// Low-level socket adopt FFI.
unsafe fn sock_adopt(
    s: *mut us_socket_t,
    group: *mut SocketGroup,
    kind: u8,
    old_ext_size: c_int,
    new_ext_size: c_int,
) -> *mut us_socket_t {
    unsafe extern "C" {
        fn us_socket_adopt(
            s: *mut us_socket_t,
            group: *mut SocketGroup,
            kind: u8,
            old_ext_size: c_int,
            new_ext_size: c_int,
        ) -> *mut us_socket_t;
    }
    us_socket_adopt(s, group, kind, old_ext_size, new_ext_size)
}

// ── URL parsing ──

fn parse_ws_url(url: &str) -> ::std::result::Result<(String, u16, String, bool), String> {
    let url_ptr = bun_url::whatwg::URL::from_utf8(url.as_bytes())
        .ok_or_else(|| format!("invalid WebSocket URL: {}", url))?;
    let url_ref = unsafe { url_ptr.as_ref() };

    let proto = url_ref.protocol();
    let proto_utf8 = proto.to_utf8();
    let proto_bytes = if proto.is_dead() { &b""[..] } else { proto_utf8.slice() };

    let is_tls;
    if proto_bytes == b"wss:" {
        is_tls = true;
    } else if proto_bytes == b"ws:" {
        is_tls = false;
    } else {
        let mut p = url_ptr;
        unsafe { p.as_mut().deinit(); }
        return Err(format!("WebSocket URL must use ws:// or wss://, got: {}", url));
    }

    let host_str = url_ref.host();
    let host = if host_str.is_dead() {
        let mut p = url_ptr;
        unsafe { p.as_mut().deinit(); }
        return Err(format!("WebSocket URL has no host: {}", url));
    } else {
        String::from_utf8_lossy(host_str.to_utf8().slice()).into_owned()
    };

    let port_u32 = url_ref.port();
    let default_port = if is_tls { 443u16 } else { 80u16 };
    let port = if port_u32 != u32::MAX { port_u32 as u16 } else { default_port };

    let pathname = url_ref.pathname();
    let path = if pathname.is_dead() {
        "/".to_string()
    } else {
        let s = String::from_utf8_lossy(pathname.to_utf8().slice()).into_owned();
        if s.is_empty() { "/".to_string() } else { s }
    };

    let mut p = url_ptr;
    unsafe { p.as_mut().deinit(); }
    Ok((host, port, path, is_tls))
}

// ── JS bridge ──

pub fn install_websocket_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    // Register WsClient vtables with socket_dispatch.
    register_kind(SocketKind::WsClientUpgrade, ws_upgrade_vtable());
    register_kind(SocketKind::WsClientUpgradeTls, ws_upgrade_vtable());
    register_kind(SocketKind::WsClient, ws_client_vtable());
    register_kind(SocketKind::WsClientTls, ws_client_vtable());

    unsafe {
        let ws_fun = JS_NewFunction(cx.raw_cx(), Some(websocket_constructor), 1, JSFUN_CONSTRUCTOR, c"WebSocket".as_ptr());
        if !ws_fun.is_null() {
            let ctor_obj = JS_GetFunctionObject(ws_fun);
            if !ctor_obj.is_null() {
                let val = mozjs::jsval::ObjectValue(ctor_obj);
                rooted!(&in(cx) let v = val);
                JS_DefineProperty(cx.raw_cx(), global.into(), c"WebSocket".as_ptr(), v.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);

                let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor_obj };
                for (name, value) in &[("CONNECTING", 0i32), ("OPEN", 1), ("CLOSING", 2), ("CLOSED", 3)] {
                    let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
                    let v = Int32Value(*value);
                    let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                    JS_DefineProperty(cx.raw_cx(), ctor_h, c_name.as_ptr(), v_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
                }
            }
        }
    }
}

unsafe fn ws_trigger_event(cx: *mut JSContext, ws_obj: *mut JSObject, event_name: &str, data_val: Option<JSVal>) {
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj };
    let mut handler_val = UndefinedValue();
    let c_name = bun_core::ZBox::from_bytes(event_name.as_bytes());
    JS_GetProperty(cx, obj_h, c_name.as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut handler_val });
    if handler_val.is_object() {
        let handler_obj = handler_val.to_object();
        if JS_ObjectIsFunction(handler_obj) {
            let global = CurrentGlobalOrNull(cx);
            if !global.is_null() {
                let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
                let handler_jsval = ObjectValue(handler_obj);
                let handler_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &handler_jsval };

                let event_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
                if !event_obj.is_null() {
                    let ev_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &event_obj };
                    if let Some(dv) = data_val {
                        let dv_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &dv };
                        JS_DefineProperty(cx, ev_h, c"data".as_ptr(), dv_h, JSPROP_ENUMERATE as u32);
                    }
                    let ev_val = ObjectValue(event_obj);
                    let call_args = HandleValueArray { length_: 1, elements_: &ev_val };
                    let mut rval = UndefinedValue();
                    let _ = JS_CallFunctionValue(cx, global_h, handler_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
                }
            }
        }
    }
}

unsafe extern "C" fn ws_send(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"WebSocket.send() requires a message argument".as_ptr());
        return false;
    }
    let msg_val = *args.get(0).ptr;

    let this_obj = args.thisv().to_object();
    let this_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };

    // Read _wsPtr (socket pointer stored as double).
    let mut ptr_val = UndefinedValue();
    JS_GetProperty(cx, this_h, c"_wsPtr".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ptr_val });
    if !ptr_val.is_double() {
        let c_msg = bun_core::ZBox::from_bytes(b"WebSocket not connected");
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let sock_ptr = ptr_val.to_double() as usize as *mut us_socket_t;
    if sock_ptr.is_null() {
        let c_msg = bun_core::ZBox::from_bytes(b"WebSocket not connected");
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    // Build WebSocket frame based on message type.
    let frame = if msg_val.is_string() {
        let s = jsstr_to_string(cx, NonNull::new_unchecked(msg_val.to_string()));
        build_ws_frame(s.as_bytes(), 0x1) // text opcode
    } else {
        // Treat as binary (could be ArrayBuffer, but for simplicity handle as string fallback).
        let s = unsafe { crate::js_to_rust_string(cx, msg_val) };
        build_ws_frame(s.as_bytes(), 0x2) // binary opcode
    };

    let socket = Socket::from(sock_ptr);
    socket.write(&frame);
    args.rval().set(UndefinedValue());
    true
}

fn build_ws_frame(payload: &[u8], opcode: u8) -> Vec<u8> {
    use bun_http::websocket::{Opcode, WebsocketHeader};

    let op = Opcode::from_raw(opcode);
    let mut header = WebsocketHeader::new(WebsocketHeader::pack_length(payload.len()), true, op);
    header.set_final(true);

    let cap = WebsocketHeader::frame_size_including_mask(payload.len());
    let mut frame = Vec::with_capacity(cap);
    header.write_header(&mut frame, payload.len()).unwrap();

    // Client frames MUST be masked (RFC 6455).
    let mut mask_key = [0u8; 4];
    let _ = getrandom::fill(&mut mask_key);
    frame.extend_from_slice(&mask_key);
    for (i, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask_key[i % 4]);
    }
    frame
}

unsafe extern "C" fn ws_close_fn(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this_obj = args.thisv().to_object();
    let this_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj };

    // Set readyState = CLOSING.
    let closing_val = Int32Value(2);
    let closing_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closing_val };
    JS_SetProperty(cx, this_h, c"readyState".as_ptr(), closing_h);

    // Read _wsPtr.
    let mut ptr_val = UndefinedValue();
    JS_GetProperty(cx, this_h, c"_wsPtr".as_ptr(),
        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ptr_val });
    if ptr_val.is_double() {
        let sock_ptr = ptr_val.to_double() as usize as *mut us_socket_t;
        if !sock_ptr.is_null() {
            // Send close frame.
            let close_frame = build_ws_frame(&[], 0x8);
            let socket = Socket::from(sock_ptr);
            socket.write(&close_frame);
            socket.close(bun_uws_sys::CloseCode::Normal);
            WS_SOCKETS.with(|m| m.borrow_mut().remove(&(sock_ptr as usize)));
        }
    }

    // Set readyState = CLOSED.
    let closed_val = Int32Value(3);
    let closed_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &closed_val };
    JS_SetProperty(cx, this_h, c"readyState".as_ptr(), closed_h);
    ws_trigger_event(cx, this_obj, "onclose", None);

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn websocket_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"WebSocket requires a URL argument".as_ptr());
        return false;
    }
    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, c"WebSocket URL must be a string".as_ptr());
        return false;
    }
    let url = jsstr_to_string(cx, NonNull::new_unchecked(url_val.to_string()));

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let ws_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if ws_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ws_obj.get() };

    // Set url property.
    let c_url = bun_core::ZBox::from_bytes(url.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_url.as_ptr());
    if !js_str.is_null() {
        let v = StringValue(&*js_str);
        let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
        JS_DefineProperty(cx, obj_h, c"url".as_ptr(), v_h, JSPROP_ENUMERATE as u32);
    }

    // readyState = CONNECTING.
    let state_val = Int32Value(0);
    let state_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &state_val };
    JS_DefineProperty(cx, obj_h, c"readyState".as_ptr(), state_h, JSPROP_ENUMERATE as u32);

    let ba_val = Int32Value(0);
    let ba_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ba_val };
    JS_DefineProperty(cx, obj_h, c"bufferedAmount".as_ptr(), ba_h, JSPROP_ENUMERATE as u32);

    for name in &["onopen", "onmessage", "onerror", "onclose"] {
        let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
        let ud = UndefinedValue();
        let ud_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ud };
        JS_DefineProperty(cx, obj_h, c_name.as_ptr(), ud_h, JSPROP_ENUMERATE as u32);
    }

    mozjs_sys::jsapi::JS_DefineFunction(
        cx, obj_h, c"send".as_ptr(), Some(ws_send), 1, JSPROP_ENUMERATE as u32,
    );
    mozjs_sys::jsapi::JS_DefineFunction(
        cx, obj_h, c"close".as_ptr(), Some(ws_close_fn), 0, JSPROP_ENUMERATE as u32,
    );

    // Parse URL and initiate async connect.
    match parse_ws_url(&url) {
        Ok((host, port, path, is_tls)) => {
            // Generate Sec-WebSocket-Key.
            let mut key_base = [0u8; 16];
            let _ = getrandom::fill(&mut key_base);
            let key_bytes = bun_base64::encode_alloc(&key_base);
            let key_str = ::std::str::from_utf8(&key_bytes).unwrap_or("");

            // Store connect params on JS object for on_open to use.
            let c_host = bun_core::ZBox::from_bytes(host.as_bytes());
            let host_js = JS_NewStringCopyZ(cx, c_host.as_ptr());
            if !host_js.is_null() {
                let v = StringValue(&*host_js);
                let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                JS_DefineProperty(cx, obj_h, c"_wsHost".as_ptr(), v_h, 0);
            }

            let c_path = bun_core::ZBox::from_bytes(path.as_bytes());
            let path_js = JS_NewStringCopyZ(cx, c_path.as_ptr());
            if !path_js.is_null() {
                let v = StringValue(&*path_js);
                let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                JS_DefineProperty(cx, obj_h, c"_wsPath".as_ptr(), v_h, 0);
            }

            let c_key = bun_core::ZBox::from_bytes(key_str.as_bytes());
            let key_js = JS_NewStringCopyZ(cx, c_key.as_ptr());
            if !key_js.is_null() {
                let v = StringValue(&*key_js);
                let v_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
                JS_DefineProperty(cx, obj_h, c"_wsKey".as_ptr(), v_h, 0);
            }

            let is_tls_val = BooleanValue(is_tls);
            let is_tls_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &is_tls_val };
            JS_DefineProperty(cx, obj_h, c"_wsTls".as_ptr(), is_tls_h, 0);

            // Initiate async connect via SocketGroup.
            let kind = if is_tls { SocketKind::WsClientUpgradeTls } else { SocketKind::WsClientUpgrade };
            let group = get_or_create_ws_upgrade_group(kind);
            let ext_size = ::std::mem::size_of::<WsUpgradeSocketExt>() as c_int;

            let host_zbox = bun_core::ZBox::from_bytes(host.as_bytes());
            let host_cstr = host_zbox.as_zstr().as_cstr();
            let result = unsafe {
                (*group).connect(kind, None, host_cstr, port as c_int, 0, ext_size)
            };

            match result {
                ConnectResult::Socket(s) => {
                    // Synchronous connect — init ext and trigger on_open.
                    let ext_ptr = bun_uws_sys::us_socket_t::opaque_mut(s).ext::<WsUpgradeSocketExt>();
                    unsafe {
                        ::std::ptr::write(ext_ptr, WsUpgradeSocketExt {
                            js_obj: ws_obj.get(),
                            cx,
                            is_tls,
                            http_buf_len: 0,
                            http_buf_cap: 0,
                            http_buf: ::std::ptr::null_mut(),
                            upgrade_sent: false,
                            upgrade_complete: false,
                        });
                    }
                    // on_open will be called automatically by uSockets.
                }
                ConnectResult::Connecting(_c) => {
                    // Async connect — ext will be set up when on_open fires.
                    // For connecting sockets, we need to set ext on the connecting socket.
                    // The on_open callback will handle the rest.
                    // We store the js_obj/cx in a thread-local pending map keyed by connecting socket.
                    WS_PENDING_CONNECT.with(|p| {
                        p.borrow_mut().insert(_c as usize, (ws_obj.get(), cx, is_tls));
                    });
                }
                ConnectResult::Failed => {
                    ws_trigger_event(cx, ws_obj.get(), "onerror", None);
                }
            }
        }
        Err(e) => {
            let c_msg = bun_core::ZBox::from_bytes(format!("WebSocket connection failed: {}", e).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    args.rval().set(mozjs::jsval::ObjectValue(ws_obj.get()));
    true
}

thread_local! {
    static WS_PENDING_CONNECT: RefCell<HashMap<usize, (*mut JSObject, *mut JSContext, bool)>> = RefCell::new(HashMap::new());
}

// ── Performance ──

pub fn install_performance(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        rooted!(&in(cx) let perf_obj = JS_NewPlainObject(cx));
        if perf_obj.get().is_null() {
            return;
        }
        JS_DefineFunction(cx, perf_obj.handle(), c"now".as_ptr(), Some(performance_now), 0, JSPROP_ENUMERATE as u32);
        JS_DefineProperty3(cx, global, c"performance".as_ptr(), perf_obj.handle(), JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn performance_now(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let ms = bun_core::time::milli_timestamp() as f64;
    args.rval().set(mozjs::jsval::DoubleValue(ms));
    true
}

// ── TextEncoder / TextDecoder ──

pub fn install_web_encodings(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let te_fun = JS_NewFunction(cx.raw_cx(), Some(text_encoder_constructor), 0, JSFUN_CONSTRUCTOR, c"TextEncoder".as_ptr());
        if !te_fun.is_null() {
            let te_obj = JS_GetFunctionObject(te_fun);
            if !te_obj.is_null() {
                rooted!(&in(cx) let te_obj_r = te_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(cx, proto.handle(), c"encode".as_ptr(), Some(text_encoder_encode), 1, JSPROP_ENUMERATE as u32);
                    JS_DefineFunction(cx, proto.handle(), c"encodeInto".as_ptr(), Some(text_encoder_encode_into), 2, JSPROP_ENUMERATE as u32);
                    JS_DefineProperty3(cx, te_obj_r.handle(), c"prototype".as_ptr(), proto.handle(), JSPROP_PERMANENT as u32);
                }
                JS_DefineProperty3(cx, global, c"TextEncoder".as_ptr(), te_obj_r.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }

        let td_fun = JS_NewFunction(cx.raw_cx(), Some(text_decoder_constructor), 1, JSFUN_CONSTRUCTOR, c"TextDecoder".as_ptr());
        if !td_fun.is_null() {
            let td_obj = JS_GetFunctionObject(td_fun);
            if !td_obj.is_null() {
                rooted!(&in(cx) let td_obj_r = td_obj);
                rooted!(&in(cx) let proto = JS_NewPlainObject(cx));
                if !proto.get().is_null() {
                    JS_DefineFunction(cx, proto.handle(), c"decode".as_ptr(), Some(text_decoder_decode), 1, JSPROP_ENUMERATE as u32);
                    JS_DefineProperty3(cx, td_obj_r.handle(), c"prototype".as_ptr(), proto.handle(), JSPROP_PERMANENT as u32);
                }
                JS_DefineProperty3(cx, global, c"TextDecoder".as_ptr(), td_obj_r.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

pub fn install_atob_btoa(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(cx, global, c"atob".as_ptr(), Some(atob_fn), 1, JSPROP_ENUMERATE as u32);
        JS_DefineFunction(cx, global, c"btoa".as_ptr(), Some(btoa_fn), 1, JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn atob_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let s = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()));
    match bun_base64::decode_alloc(s.as_bytes()) {
        Ok(bytes) => {
            let decoded = String::from_utf8_lossy(&bytes);
            let c_str = bun_core::ZBox::from_bytes(decoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() { args.rval().set(UndefinedValue()); }
            else { args.rval().set(StringValue(&*js_str)); }
        }
        Err(_) => {
            JS_ReportErrorUTF8(cx, c"Failed to decode base64".as_ptr());
            return false;
        }
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn btoa_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let s = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked((*args.get(0).ptr).to_string()));
    let encoded_bytes = bun_base64::encode_alloc(s.as_bytes());
    let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
    let c_str = bun_core::ZBox::from_bytes(encoded.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
    if js_str.is_null() { args.rval().set(UndefinedValue()); }
    else { args.rval().set(StringValue(&*js_str)); }
    true
}

pub fn install_queue_microtask(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(cx, global, c"queueMicrotask".as_ptr(), Some(queue_microtask_fn), 1, JSPROP_ENUMERATE as u32);
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding_str = JS_NewStringCopyZ(cx, c"utf-8".as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c"encoding".as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"encode".as_ptr(), Some(text_encoder_encode), 1, JSPROP_ENUMERATE as u32);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"encodeInto".as_ptr(), Some(text_encoder_encode_into), 2, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let input = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { crate::js_to_rust_string(cx, v) } else { String::new() }
    } else {
        String::new()
    };

    let bytes = input.as_bytes();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let arr = NewArrayObject1(&mut wrapped_cx, bytes.len()));

    for (i, &byte) in bytes.iter().enumerate() {
        let val = Int32Value(byte as i32);
        rooted!(&in(wrapped_cx) let v = val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }

    let global = CurrentGlobalOrNull(cx);
    if !global.is_null() {
        let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
        let mut buf_ctor = UndefinedValue();
        JS_GetProperty(cx, global_h, c"Uint8Array".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut buf_ctor });
        if buf_ctor.is_object() {
            let arr_val = ObjectValue(arr.get());
            rooted!(&in(wrapped_cx) let av = arr_val);
            let call_args = HandleValueArray { length_: 1, elements_: &av.get() };
            let ctor_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &buf_ctor };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_h, ctor_h, &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
            if rval.is_object() {
                args.rval().set(rval);
                return true;
            }
        }
    }

    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_encoder_encode_into(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let encoding = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { crate::js_to_rust_string(cx, v) } else { "utf-8".to_string() }
    } else {
        "utf-8".to_string()
    };
    let encoding_lower = encoding.to_lowercase();
    let encoding_str = JS_NewStringCopyZ(cx, bun_core::ZBox::from_bytes(encoding_lower.as_bytes()).as_ptr());
    if !encoding_str.is_null() {
        let val = StringValue(&*encoding_str);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, c"encoding".as_ptr(), val_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    }
    let fatal_val = BooleanValue(false);
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let fatal_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fatal_val };
    JS_DefineProperty(cx, obj_h, c"fatal".as_ptr(), fatal_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);
    let bom_val = BooleanValue(false);
    let bom_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bom_val };
    JS_DefineProperty(cx, obj_h, c"ignoreBOM".as_ptr(), bom_h, (JSPROP_ENUMERATE | JSPROP_READONLY) as u32);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_DefineFunction(&mut wrapped_cx, obj_r.handle(), c"decode".as_ptr(), Some(text_decoder_decode), 1, JSPROP_ENUMERATE as u32);

    args.rval().set(ObjectValue(obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn text_decoder_decode(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let empty = JS_NewStringCopyZ(cx, c"".as_ptr());
        args.rval().set(if empty.is_null() { UndefinedValue() } else { StringValue(&*empty) });
        return true;
    }

    let input = *args.get(0).ptr;

    let bytes = if input.is_object() {
        let obj = input.to_object();
        let mut len_val = UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        JS_GetProperty(cx, obj_h, c"length".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut len_val });
        let len = if len_val.is_int32() { len_val.to_int32() as u32 } else { 0 };
        let mut result = Vec::new();
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(cx, obj_h, i, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
            if elem.is_int32() {
                result.push(elem.to_int32() as u8);
            }
        }
        result
    } else {
        Vec::new()
    };

    let decoded = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => {
            JS_ReportErrorUTF8(cx, c"The encoded data was not valid".as_ptr());
            return false;
        }
    };

    let utf16: Vec<u16> = decoded.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn queue_microtask_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        return true;
    }
    let callback = (*args.get(0).ptr).to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx = &mut wrapped_cx;

    rooted!(&in(cx) let undef_val = UndefinedValue());
    let resolved = CallOriginalPromiseResolve(cx, undef_val.handle());
    if resolved.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx) let promise = resolved);
    rooted!(&in(cx) let on_fulfilled = callback);
    rooted!(&in(cx) let null_reject = ::std::ptr::null_mut::<JSObject>());
    CallOriginalPromiseThen(cx, promise.handle(), on_fulfilled.handle(), null_reject.handle());
    args.rval().set(UndefinedValue());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ws_url_ws() {
        let (host, port, path, is_tls) = parse_ws_url("ws://example.com/chat").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/chat");
        assert!(!is_tls);
    }

    #[test]
    fn parse_ws_url_wss() {
        let (host, port, path, is_tls) = parse_ws_url("wss://example.com/secure").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/secure");
        assert!(is_tls);
    }

    #[test]
    fn parse_ws_url_with_port() {
        let (host, port, path, is_tls) = parse_ws_url("ws://localhost:8080/ws").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/ws");
        assert!(!is_tls);
    }

    #[test]
    fn parse_ws_url_default_path() {
        let (_, _, path, _) = parse_ws_url("ws://host/").unwrap();
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_ws_url_no_path_defaults_to_slash() {
        let (_, _, path, _) = parse_ws_url("ws://host").unwrap();
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_ws_url_bare_host_no_scheme() {
        let result = parse_ws_url("example.com/chat");
        // Without ws:// or wss:// scheme, URL parsing should fail.
        assert!(result.is_err());
    }

    #[test]
    fn parse_ws_url_empty_string() {
        let result = parse_ws_url("");
        // Empty string is not a valid WebSocket URL.
        assert!(result.is_err());
    }

    #[test]
    fn parse_ws_url_ipv4_with_port() {
        let (host, port, path, _) = parse_ws_url("ws://127.0.0.1:9222/json").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9222);
        assert_eq!(path, "/json");
    }

    #[test]
    fn parse_ws_url_query_string() {
        let (host, port, path, _) = parse_ws_url("ws://example.com/ws?token=abc").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert!(path.starts_with("/ws"));
    }

    #[test]
    fn parse_ws_url_deep_path() {
        let (host, _, path, _) = parse_ws_url("ws://host/a/b/c/d").unwrap();
        assert_eq!(host, "host");
        assert_eq!(path, "/a/b/c/d");
    }

    #[test]
    fn build_ws_frame_empty_payload() {
        let frame = build_ws_frame(b"", 0x1);
        // FIN+opcode(1) + mask+length(1) + mask_key(4) = 6 bytes minimum
        assert!(frame.len() >= 6);
        assert_eq!(frame[0], 0x81); // FIN + text opcode
    }

    #[test]
    fn build_ws_frame_short_payload() {
        let frame = build_ws_frame(b"hello", 0x2);
        // 1 opcode + 1 mask+len + 4 mask + 5 payload = 11
        assert_eq!(frame.len(), 11);
        assert_eq!(frame[0], 0x82); // FIN + binary opcode
        assert_eq!(frame[1] & 0x7F, 5); // length = 5
    }

    #[test]
    fn build_ws_frame_medium_payload() {
        let payload = vec![0u8; 200];
        let frame = build_ws_frame(&payload, 0x1);
        // 1 + 1 + 2 extended + 4 mask + 200 = 208
        assert_eq!(frame.len(), 208);
        assert_eq!(frame[1] & 0x7F, 126); // 126 signals 16-bit length
        let ext_len = u16::from_be_bytes([frame[2], frame[3]]);
        assert_eq!(ext_len, 200);
    }

    #[test]
    fn build_ws_frame_large_payload() {
        let payload = vec![0u8; 70000];
        let frame = build_ws_frame(&payload, 0x2);
        // 1 + 1 + 8 extended + 4 mask + 70000 = 70014
        assert_eq!(frame.len(), 70014);
        assert_eq!(frame[1] & 0x7F, 127); // 127 signals 64-bit length
    }
}
